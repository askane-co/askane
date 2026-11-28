use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::budget::BudgetCore;
use crate::media::audio::mixer::{
    MixerCommand, MixerState, OverflowPolicy, VoicePlayParams, VoiceSource,
};
use crate::media::audio::sound::{
    AudioEvent, BoundedCmdQueue, BoundedEventQueue, MusicShared, MusicStatus, StreamRing,
    VoiceShared,
};
use crate::media::audio::wav::WavStreamDecoder;
use crate::media::audio::{
    CMD_QUEUE_CAP, ENGINE_JOIN_TIMEOUT_MS, ENGINE_RATE, EVT_QUEUE_CAP, MAX_BUSES, MAX_MUSICS,
    MAX_VOICES, QUANTUM, RING_FRAMES, err_audio_nondet,
};
use crate::media::{err_bad_opt, err_limit};

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SinkKind {
    Null,
    Virtual,
    WaveOut,
    Alsa,
}

impl SinkKind {
    pub fn name(self) -> &'static str {
        match self {
            SinkKind::Null => "null",
            SinkKind::Virtual => "virtual",
            SinkKind::WaveOut => "waveout",
            SinkKind::Alsa => "alsa",
        }
    }
}

#[derive(Clone, Debug)]
pub struct InitOpts {
    pub sink: Option<String>,
    pub sample_rate: u32,
    pub max_voices: usize,
    pub buffer_ms: u32,
}

impl Default for InitOpts {
    fn default() -> Self {
        Self {
            sink: None,
            sample_rate: ENGINE_RATE,
            max_voices: MAX_VOICES,
            buffer_ms: 40,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AudioStateSnapshot {
    pub initialized: bool,
    pub backend: String,
    pub sample_rate: u32,
    pub voices: usize,
    pub max_voices: usize,
    pub buses: usize,
    pub musics: usize,
    pub frames_played: u64,
    pub clock_ms: u64,
    pub underruns: u64,
    pub dropped_events: u64,
    pub pending_cmds: usize,
    pub leaked_threads: u64,
    pub fatal: bool,
}

pub struct MusicSlot {
    pub shared: Arc<MusicShared>,
    pub ring: Arc<Mutex<StreamRing>>,
    pub total_frames: u64,
    pub active_voice_id: Option<u64>,
    pub loop_mode: bool,
    pub loop_points: Option<(u64, u64)>,
    pub volume: f32,
    pub stop_flag: Arc<AtomicBool>,
    pub done_flag: Arc<AtomicBool>,
    pub thread: Option<JoinHandle<()>>,
}

struct EngineInstance {
    sink: SinkKind,
    max_voices: usize,
    mixer: Arc<Mutex<MixerState>>,
    cmds: Arc<BoundedCmdQueue<MixerCommand>>,
    events: Arc<BoundedEventQueue>,
    stop_flag: Arc<AtomicBool>,
    done_flag: Arc<AtomicBool>,
    deadline_epoch_ms: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
    buses_by_name: HashMap<String, u16>,
    bus_names_by_id: HashMap<u16, String>,
    next_bus_id: u16,
    next_voice_id: u64,
    next_music_id: u64,
    voices: HashMap<u64, Arc<VoiceShared>>,
    musics: HashMap<u64, MusicSlot>,
    budget: Option<Arc<BudgetCore>>,
}

thread_local! {
    static AUDIO_ENGINE: RefCell<Option<EngineInstance>> = const { RefCell::new(None) };
    static LEAKED_THREADS: AtomicU64 = const { AtomicU64::new(0) };
}

fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(windows)]
mod win32_waveout {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};

    const WAVE_MAPPER: u32 = 0xFFFF_FFFF;
    const CALLBACK_EVENT: u32 = 0x0005_0000;
    const WHDR_DONE: u32 = 0x0000_0001;
    const INFINITE: u32 = 0xFFFF_FFFF;

    #[repr(C)]
    struct WAVEFORMATEX {
        w_format_tag: u16,
        n_channels: u16,
        n_samples_per_sec: u32,
        n_avg_bytes_per_sec: u32,
        n_block_align: u16,
        w_bits_per_sample: u16,
        cb_size: u16,
    }

    #[repr(C)]
    struct WAVEHDR {
        lp_data: *mut u8,
        dw_buffer_length: u32,
        dw_bytes_recorded: u32,
        dw_user: usize,
        dw_flags: u32,
        dw_loops: u32,
        lp_next: *mut WAVEHDR,
        reserved: usize,
    }

    #[link(name = "winmm")]
    unsafe extern "system" {
        fn waveOutOpen(
            phwo: *mut *mut c_void,
            u_device_id: u32,
            pwfx: *const WAVEFORMATEX,
            dw_callback: usize,
            dw_instance: usize,
            fdw_open: u32,
        ) -> u32;
        fn waveOutPrepareHeader(hwo: *mut c_void, pwh: *mut WAVEHDR, cbwh: u32) -> u32;
        fn waveOutUnprepareHeader(hwo: *mut c_void, pwh: *mut WAVEHDR, cbwh: u32) -> u32;
        fn waveOutWrite(hwo: *mut c_void, pwh: *mut WAVEHDR, cbwh: u32) -> u32;
        fn waveOutReset(hwo: *mut c_void) -> u32;
        fn waveOutClose(hwo: *mut c_void) -> u32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateEventW(
            lp_event_attributes: *const c_void,
            b_manual_reset: i32,
            b_initial_state: i32,
            lp_name: *const u16,
        ) -> *mut c_void;
        fn WaitForSingleObject(h_handle: *mut c_void, dw_milliseconds: u32) -> u32;
        fn CloseHandle(h_object: *mut c_void) -> i32;
    }

    pub struct WaveOutDevice {
        hwo: *mut c_void,
        event: *mut c_void,
        buffers: Vec<Vec<i16>>,
        headers: Vec<WAVEHDR>,
        chunk_frames: usize,
    }

    unsafe impl Send for WaveOutDevice {}

    impl WaveOutDevice {
        pub fn open(sample_rate: u32, buffer_ms: u32) -> Result<Self, String> {
            let chunk_frames = ((sample_rate as usize * (buffer_ms.clamp(15, 200) as usize)) / 1000 / 4).max(240);
            let wfx = WAVEFORMATEX {
                w_format_tag: 1,
                n_channels: 2,
                n_samples_per_sec: sample_rate,
                n_avg_bytes_per_sec: sample_rate * 4,
                n_block_align: 4,
                w_bits_per_sample: 16,
                cb_size: 0,
            };
            let event = unsafe { CreateEventW(null(), 0, 1, null()) };
            if event.is_null() {
                return Err("CreateEventW failed for waveOut".to_string());
            }
            let mut hwo: *mut c_void = null_mut();
            let res = unsafe {
                waveOutOpen(
                    &mut hwo,
                    WAVE_MAPPER,
                    &wfx,
                    event as usize,
                    0,
                    CALLBACK_EVENT,
                )
            };
            if res != 0 || hwo.is_null() {
                unsafe {
                    CloseHandle(event);
                }
                return Err(format!("waveOutOpen failed with code {}", res));
            }

            let num_bufs = 4usize;
            let mut buffers = Vec::with_capacity(num_bufs);
            let mut headers = Vec::with_capacity(num_bufs);
            for i in 0..num_bufs {
                let mut pcm = vec![0i16; chunk_frames * 2];
                let hdr = WAVEHDR {
                    lp_data: pcm.as_mut_ptr() as *mut u8,
                    dw_buffer_length: (chunk_frames * 4) as u32,
                    dw_bytes_recorded: 0,
                    dw_user: i,
                    dw_flags: WHDR_DONE,
                    dw_loops: 0,
                    lp_next: null_mut(),
                    reserved: 0,
                };
                buffers.push(pcm);
                headers.push(hdr);
            }

            // Prepare headers with pinned heap buffer pointers.
            for i in 0..num_bufs {
                headers[i].lp_data = buffers[i].as_mut_ptr() as *mut u8;
                let pr = unsafe {
                    waveOutPrepareHeader(
                        hwo,
                        &mut headers[i],
                        std::mem::size_of::<WAVEHDR>() as u32,
                    )
                };
                if pr != 0 {
                    for j in 0..i {
                        unsafe {
                            waveOutUnprepareHeader(
                                hwo,
                                &mut headers[j],
                                std::mem::size_of::<WAVEHDR>() as u32,
                            );
                        }
                    }
                    unsafe {
                        waveOutClose(hwo);
                        CloseHandle(event);
                    }
                    return Err(format!("waveOutPrepareHeader failed with code {}", pr));
                }
                headers[i].dw_flags |= WHDR_DONE;
            }

            Ok(Self {
                hwo,
                event,
                buffers,
                headers,
                chunk_frames,
            })
        }

        pub fn chunk_frames(&self) -> usize {
            self.chunk_frames
        }

        pub fn pump_available(&mut self, mut fill: impl FnMut(&mut [i16])) -> Result<bool, String> {
            let mut wrote_any = false;
            for i in 0..self.headers.len() {
                if (self.headers[i].dw_flags & WHDR_DONE) != 0 {
                    fill(&mut self.buffers[i]);
                    self.headers[i].dw_flags &= !WHDR_DONE;
                    let wr = unsafe {
                        waveOutWrite(
                            self.hwo,
                            &mut self.headers[i],
                            std::mem::size_of::<WAVEHDR>() as u32,
                        )
                    };
                    if wr != 0 {
                        return Err(format!("waveOutWrite failed with code {}", wr));
                    }
                    wrote_any = true;
                }
            }
            Ok(wrote_any)
        }

        pub fn wait_slice(&self, timeout_ms: u32) {
            unsafe {
                WaitForSingleObject(self.event, timeout_ms.min(INFINITE));
            }
        }
    }

    impl Drop for WaveOutDevice {
        fn drop(&mut self) {
            unsafe {
                waveOutReset(self.hwo);
                for hdr in &mut self.headers {
                    waveOutUnprepareHeader(
                        self.hwo,
                        hdr,
                        std::mem::size_of::<WAVEHDR>() as u32,
                    );
                }
                waveOutClose(self.hwo);
                CloseHandle(self.event);
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod linux_alsa {
    use std::ffi::{c_char, c_int, c_uint, c_void};
    use std::ptr::null_mut;

    const RTLD_NOW: c_int = 2;
    const SND_PCM_STREAM_PLAYBACK: c_int = 0;
    const SND_PCM_FORMAT_S16_LE: c_int = 2;
    const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;

    #[cfg_attr(target_env = "gnu", link(name = "dl"))]
    unsafe extern "C" {
        fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn dlclose(handle: *mut c_void) -> c_int;
    }

    type FnOpen = unsafe extern "C" fn(*mut *mut c_void, *const c_char, c_int, c_int) -> c_int;
    type FnSetParams =
        unsafe extern "C" fn(*mut c_void, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int;
    type FnWritei = unsafe extern "C" fn(*mut c_void, *const c_void, usize) -> isize;
    type FnRecover = unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int;
    type FnDrop = unsafe extern "C" fn(*mut c_void) -> c_int;
    type FnClose = unsafe extern "C" fn(*mut c_void) -> c_int;

    pub struct AlsaDevice {
        lib: *mut c_void,
        pcm: *mut c_void,
        writei: FnWritei,
        recover: FnRecover,
        drop_fn: FnDrop,
        close_fn: FnClose,
    }

    unsafe impl Send for AlsaDevice {}

    impl AlsaDevice {
        pub fn open(sample_rate: u32, buffer_ms: u32) -> Result<Self, String> {
            let lib = unsafe { dlopen(b"libasound.so.2\0".as_ptr() as *const c_char, RTLD_NOW) };
            if lib.is_null() {
                return Err("libasound.so.2 not available".to_string());
            }

            let load_sym = |name: &[u8]| -> Option<*mut c_void> {
                let p = unsafe { dlsym(lib, name.as_ptr() as *const c_char) };
                if p.is_null() { None } else { Some(p) }
            };

            let (Some(p_open), Some(p_set), Some(p_writei), Some(p_rec), Some(p_drop), Some(p_close)) = (
                load_sym(b"snd_pcm_open\0"),
                load_sym(b"snd_pcm_set_params\0"),
                load_sym(b"snd_pcm_writei\0"),
                load_sym(b"snd_pcm_recover\0"),
                load_sym(b"snd_pcm_drop\0"),
                load_sym(b"snd_pcm_close\0"),
            ) else {
                unsafe {
                    dlclose(lib);
                }
                return Err("incomplete libasound.so.2 symbols".to_string());
            };

            let open_fn: FnOpen = unsafe { std::mem::transmute(p_open) };
            let set_fn: FnSetParams = unsafe { std::mem::transmute(p_set) };
            let writei: FnWritei = unsafe { std::mem::transmute(p_writei) };
            let recover: FnRecover = unsafe { std::mem::transmute(p_rec) };
            let drop_fn: FnDrop = unsafe { std::mem::transmute(p_drop) };
            let close_fn: FnClose = unsafe { std::mem::transmute(p_close) };

            let mut pcm: *mut c_void = null_mut();
            let rc = unsafe {
                open_fn(
                    &mut pcm,
                    b"default\0".as_ptr() as *const c_char,
                    SND_PCM_STREAM_PLAYBACK,
                    0,
                )
            };
            if rc < 0 || pcm.is_null() {
                unsafe {
                    dlclose(lib);
                }
                return Err(format!("snd_pcm_open failed with code {}", rc));
            }

            let latency_us = buffer_ms.clamp(15, 250).saturating_mul(1000);
            let prc = unsafe {
                set_fn(
                    pcm,
                    SND_PCM_FORMAT_S16_LE,
                    SND_PCM_ACCESS_RW_INTERLEAVED,
                    2,
                    sample_rate,
                    1,
                    latency_us,
                )
            };
            if prc < 0 {
                unsafe {
                    close_fn(pcm);
                    dlclose(lib);
                }
                return Err(format!("snd_pcm_set_params failed with code {}", prc));
            }

            Ok(Self {
                lib,
                pcm,
                writei,
                recover,
                drop_fn,
                close_fn,
            })
        }

        pub fn write_s16(&self, samples: &[i16]) -> Result<(), String> {
            let mut offset = 0usize;
            let total_frames = samples.len() / 2;
            while offset < total_frames {
                let ptr = unsafe { samples.as_ptr().add(offset * 2) as *const c_void };
                let written = unsafe { (self.writei)(self.pcm, ptr, total_frames - offset) };
                if written < 0 {
                    let rec = unsafe { (self.recover)(self.pcm, written as c_int, 1) };
                    if rec < 0 {
                        return Err(format!("ALSA snd_pcm_writei error {}", written));
                    }
                } else if written == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                } else {
                    offset += written as usize;
                }
            }
            Ok(())
        }
    }

    impl Drop for AlsaDevice {
        fn drop(&mut self) {
            unsafe {
                (self.drop_fn)(self.pcm);
                (self.close_fn)(self.pcm);
                dlclose(self.lib);
            }
        }
    }
}

fn f32_to_s16_slice(src: &[f32], dst: &mut [i16]) {
    let n = src.len().min(dst.len());
    for i in 0..n {
        let s = if src[i].is_finite() {
            src[i].clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let v = if s >= 0.0 {
            (s * 32767.0 + 0.5) as i32
        } else {
            (s * 32768.0 - 0.5) as i32
        };
        dst[i] = v.clamp(-32768, 32767) as i16;
    }
}

enum DeviceHandle {
    None,
    #[cfg(windows)]
    WaveOut(win32_waveout::WaveOutDevice),
    #[cfg(target_os = "linux")]
    Alsa(linux_alsa::AlsaDevice),
}

fn spawn_engine_thread(
    sink: SinkKind,
    mixer: Arc<Mutex<MixerState>>,
    cmds: Arc<BoundedCmdQueue<MixerCommand>>,
    events: Arc<BoundedEventQueue>,
    stop_flag: Arc<AtomicBool>,
    done_flag: Arc<AtomicBool>,
    deadline_epoch_ms: Arc<AtomicU64>,
    dev: DeviceHandle,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut device = dev;
        let mut cmd_batch: Vec<MixerCommand> = Vec::with_capacity(64);
        let mut f_buf = vec![0.0f32; QUANTUM * 2];

        loop {
            if stop_flag.load(Ordering::Acquire) {
                break;
            }
            let dl = deadline_epoch_ms.load(Ordering::Acquire);
            if dl > 0 && current_unix_ms() > dl {
                break;
            }

            cmds.drain_into(&mut cmd_batch);
            if let Ok(mut mx) = mixer.lock() {
                for c in cmd_batch.drain(..) {
                    mx.apply_command(c);
                }
                if mx.shutdown_requested {
                    break;
                }
            } else {
                break;
            }

            match &mut device {
                DeviceHandle::None => {
                    if sink == SinkKind::Null {
                        if let Ok(mut mx) = mixer.lock() {
                            let ur = mx.render_quantum(&mut f_buf);
                            if ur > 0 {
                                events.push_drop_oldest(AudioEvent::Underrun { frames: ur });
                            }
                        }
                        thread::sleep(Duration::from_millis(5));
                    } else {
                        thread::sleep(Duration::from_millis(10));
                    }
                }
                #[cfg(windows)]
                DeviceHandle::WaveOut(wo) => {
                    let chunk_frames = wo.chunk_frames();
                    let res = wo.pump_available(|out_s16| {
                        let mut pos = 0usize;
                        while pos < chunk_frames {
                            let n = (chunk_frames - pos).min(QUANTUM);
                            let slice = &mut f_buf[..n * 2];
                            if let Ok(mut mx) = mixer.lock() {
                                let ur = mx.render_quantum(slice);
                                if ur > 0 {
                                    events.push_drop_oldest(AudioEvent::Underrun { frames: ur });
                                }
                            } else {
                                slice.fill(0.0);
                            }
                            f32_to_s16_slice(slice, &mut out_s16[pos * 2..(pos + n) * 2]);
                            pos += n;
                        }
                    });
                    match res {
                        Ok(true) => {}
                        Ok(false) => wo.wait_slice(10),
                        Err(e) => {
                            events.push_drop_oldest(AudioEvent::DeviceStopped { reason: e });
                            break;
                        }
                    }
                }
                #[cfg(target_os = "linux")]
                DeviceHandle::Alsa(alsa) => {
                    let mut out_s16 = vec![0i16; QUANTUM * 2];
                    if let Ok(mut mx) = mixer.lock() {
                        let ur = mx.render_quantum(&mut f_buf);
                        if ur > 0 {
                            events.push_drop_oldest(AudioEvent::Underrun { frames: ur });
                        }
                    } else {
                        break;
                    }
                    f32_to_s16_slice(&f_buf, &mut out_s16);
                    if let Err(e) = alsa.write_s16(&out_s16) {
                        events.push_drop_oldest(AudioEvent::DeviceStopped { reason: e });
                        break;
                    }
                }
            }
        }
        drop(device);
        done_flag.store(true, Ordering::Release);
    })
}

fn select_backend(
    opts: &InitOpts,
    deterministic: bool,
    events: &BoundedEventQueue,
) -> Result<(SinkKind, DeviceHandle), String> {
    let env_sink = std::env::var("ASKANE_AUDIO_SINK").ok();
    let requested = opts
        .sink
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "auto")
        .map(|s| s.to_ascii_lowercase())
        .or_else(|| {
            env_sink
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != "auto")
                .map(|s| s.to_ascii_lowercase())
        });

    if deterministic {
        match requested.as_deref() {
            None => return Ok((SinkKind::Null, DeviceHandle::None)),
            Some("null") => return Ok((SinkKind::Null, DeviceHandle::None)),
            Some("virtual") => return Ok((SinkKind::Virtual, DeviceHandle::None)),
            Some(other) => {
                return Err(err_audio_nondet(format!(
                    "sink '{}' is forbidden under --deterministic (only 'null' or 'virtual')",
                    other
                )));
            }
        }
    }

    match requested.as_deref() {
        Some("null") => Ok((SinkKind::Null, DeviceHandle::None)),
        Some("virtual") => Ok((SinkKind::Virtual, DeviceHandle::None)),
        Some("waveout") => {
            #[cfg(windows)]
            {
                match win32_waveout::WaveOutDevice::open(opts.sample_rate, opts.buffer_ms) {
                    Ok(dev) => Ok((SinkKind::WaveOut, DeviceHandle::WaveOut(dev))),
                    Err(reason) => {
                        events.push_drop_oldest(AudioEvent::Fallback {
                            from: "waveout".to_string(),
                            to: "null".to_string(),
                            reason,
                        });
                        Ok((SinkKind::Null, DeviceHandle::None))
                    }
                }
            }
            #[cfg(not(windows))]
            {
                events.push_drop_oldest(AudioEvent::Fallback {
                    from: "waveout".to_string(),
                    to: "null".to_string(),
                    reason: "waveout only available on Windows".to_string(),
                });
                Ok((SinkKind::Null, DeviceHandle::None))
            }
        }
        Some("alsa") => {
            #[cfg(target_os = "linux")]
            {
                match linux_alsa::AlsaDevice::open(opts.sample_rate, opts.buffer_ms) {
                    Ok(dev) => Ok((SinkKind::Alsa, DeviceHandle::Alsa(dev))),
                    Err(reason) => {
                        events.push_drop_oldest(AudioEvent::Fallback {
                            from: "alsa".to_string(),
                            to: "null".to_string(),
                            reason,
                        });
                        Ok((SinkKind::Null, DeviceHandle::None))
                    }
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                events.push_drop_oldest(AudioEvent::Fallback {
                    from: "alsa".to_string(),
                    to: "null".to_string(),
                    reason: "alsa only available on Linux".to_string(),
                });
                Ok((SinkKind::Null, DeviceHandle::None))
            }
        }
        Some(other) => Err(err_bad_opt(format!(
            "unknown audio sink '{}' (expected auto, null, virtual, waveout, alsa)",
            other
        ))),
        None => {
            #[cfg(windows)]
            {
                match win32_waveout::WaveOutDevice::open(opts.sample_rate, opts.buffer_ms) {
                    Ok(dev) => Ok((SinkKind::WaveOut, DeviceHandle::WaveOut(dev))),
                    Err(reason) => {
                        events.push_drop_oldest(AudioEvent::Fallback {
                            from: "waveout".to_string(),
                            to: "null".to_string(),
                            reason,
                        });
                        Ok((SinkKind::Null, DeviceHandle::None))
                    }
                }
            }
            #[cfg(target_os = "linux")]
            {
                match linux_alsa::AlsaDevice::open(opts.sample_rate, opts.buffer_ms) {
                    Ok(dev) => Ok((SinkKind::Alsa, DeviceHandle::Alsa(dev))),
                    Err(reason) => {
                        events.push_drop_oldest(AudioEvent::Fallback {
                            from: "alsa".to_string(),
                            to: "null".to_string(),
                            reason,
                        });
                        Ok((SinkKind::Null, DeviceHandle::None))
                    }
                }
            }
            #[cfg(not(any(windows, target_os = "linux")))]
            {
                events.push_drop_oldest(AudioEvent::Fallback {
                    from: "auto".to_string(),
                    to: "null".to_string(),
                    reason: "platform uses null audio sink".to_string(),
                });
                Ok((SinkKind::Null, DeviceHandle::None))
            }
        }
    }
}

fn join_flag_with_deadline(
    stop: &AtomicBool,
    done: &AtomicBool,
    handle: Option<JoinHandle<()>>,
    deadline: Instant,
) {
    stop.store(true, Ordering::Release);
    while !done.load(Ordering::Acquire) {
        if Instant::now() >= deadline {
            LEAKED_THREADS.with(|c| {
                c.fetch_add(1, Ordering::AcqRel);
            });
            // Detach cleanly so VM/runtime timeout stays bounded <= 250ms.
            drop(handle);
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    if let Some(h) = handle {
        let _ = h.join();
    }
}

fn shutdown_engine_instance(mut eng: EngineInstance) {
    let deadline = Instant::now() + Duration::from_millis(ENGINE_JOIN_TIMEOUT_MS);
    for (_, mut ms) in eng.musics.drain() {
        join_flag_with_deadline(&ms.stop_flag, &ms.done_flag, ms.thread.take(), deadline);
    }
    let _ = eng.cmds.try_send(MixerCommand::Shutdown);
    eng.stop_flag.store(true, Ordering::Release);
    if eng.sink == SinkKind::Virtual {
        if let Ok(mut mx) = eng.mixer.lock() {
            mx.apply_command(MixerCommand::Shutdown);
        }
        eng.done_flag.store(true, Ordering::Release);
    } else {
        join_flag_with_deadline(&eng.stop_flag, &eng.done_flag, eng.thread.take(), deadline);
    }
}

pub fn init_engine(
    opts: &InitOpts,
    deterministic: bool,
    budget_core: Option<Arc<BudgetCore>>,
    timeout_ms_left: Option<u64>,
) -> Result<AudioStateSnapshot, String> {
    if opts.sample_rate != ENGINE_RATE {
        return Err(err_bad_opt(format!(
            "engine sample_rate must be {} Hz in 0.20, got {}",
            ENGINE_RATE, opts.sample_rate
        )));
    }
    if !(1..=crate::media::audio::HARD_MAX_VOICES).contains(&opts.max_voices) {
        return Err(err_limit(format!(
            "max_voices {} must be in 1..={}",
            opts.max_voices,
            crate::media::audio::HARD_MAX_VOICES
        )));
    }

    // Re-init cleanly shuts down any previous instance first.
    shutdown_engine();

    let events = Arc::new(BoundedEventQueue::new(EVT_QUEUE_CAP));
    let (sink, dev) = select_backend(opts, deterministic, &events)?;
    let mixer = Arc::new(Mutex::new(MixerState::new(opts.max_voices)));
    let cmds = Arc::new(BoundedCmdQueue::new(CMD_QUEUE_CAP));
    let stop_flag = Arc::new(AtomicBool::new(false));
    let done_flag = Arc::new(AtomicBool::new(sink == SinkKind::Virtual));
    let dl_ms = timeout_ms_left
        .map(|left| current_unix_ms().saturating_add(left))
        .unwrap_or(0);
    let deadline_epoch_ms = Arc::new(AtomicU64::new(dl_ms));

    let thread = if sink == SinkKind::Virtual {
        None
    } else {
        Some(spawn_engine_thread(
            sink,
            Arc::clone(&mixer),
            Arc::clone(&cmds),
            Arc::clone(&events),
            Arc::clone(&stop_flag),
            Arc::clone(&done_flag),
            Arc::clone(&deadline_epoch_ms),
            dev,
        ))
    };

    let mut buses_by_name = HashMap::new();
    let mut bus_names_by_id = HashMap::new();
    buses_by_name.insert("master".to_string(), 0u16);
    bus_names_by_id.insert(0u16, "master".to_string());

    let inst = EngineInstance {
        sink,
        max_voices: opts.max_voices,
        mixer,
        cmds,
        events,
        stop_flag,
        done_flag,
        deadline_epoch_ms,
        thread,
        buses_by_name,
        bus_names_by_id,
        next_bus_id: 1,
        next_voice_id: 1,
        next_music_id: 1,
        voices: HashMap::new(),
        musics: HashMap::new(),
        budget: budget_core,
    };

    AUDIO_ENGINE.with(|slot| {
        *slot.borrow_mut() = Some(inst);
    });
    Ok(get_state())
}

pub fn shutdown_engine() {
    let old = AUDIO_ENGINE.with(|slot| slot.borrow_mut().take());
    if let Some(eng) = old {
        shutdown_engine_instance(eng);
    }
}

pub fn reset_audio_state() {
    shutdown_engine();
}

pub fn update_deadline(timeout_ms_left: Option<u64>) {
    AUDIO_ENGINE.with(|slot| {
        if let Some(eng) = slot.borrow().as_ref() {
            let dl = timeout_ms_left
                .map(|left| current_unix_ms().saturating_add(left))
                .unwrap_or(0);
            eng.deadline_epoch_ms.store(dl, Ordering::Release);
        }
    });
}

pub fn is_initialized() -> bool {
    AUDIO_ENGINE.with(|slot| slot.borrow().is_some())
}

pub fn current_sink_name() -> String {
    AUDIO_ENGINE.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|e| e.sink.name().to_string())
            .unwrap_or_else(|| "uninitialized".to_string())
    })
}

pub fn get_state() -> AudioStateSnapshot {
    let leaked = LEAKED_THREADS.with(|c| c.load(Ordering::Acquire));
    AUDIO_ENGINE.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let Some(eng) = borrow.as_mut() else {
            return AudioStateSnapshot {
                leaked_threads: leaked,
                ..AudioStateSnapshot::default()
            };
        };
        eng.voices.retain(|_, v| v.playing.load(Ordering::Acquire));
        let (frames_played, underruns, buses, mx_voices) = eng
            .mixer
            .lock()
            .map(|m| (m.frames_played, m.underrun_events, m.buses.len(), m.voices.len()))
            .unwrap_or((0, 0, 1, eng.voices.len()));
        let dropped = eng.events.peek_dropped();
        let fatal = eng
            .budget
            .as_ref()
            .map(|b| b.fatal.load(Ordering::Acquire))
            .unwrap_or(false);
        AudioStateSnapshot {
            initialized: true,
            backend: eng.sink.name().to_string(),
            sample_rate: ENGINE_RATE,
            voices: mx_voices.max(eng.voices.len()),
            max_voices: eng.max_voices,
            buses,
            musics: eng.musics.len(),
            frames_played,
            clock_ms: (frames_played.saturating_mul(1000)) / (ENGINE_RATE as u64),
            underruns,
            dropped_events: dropped,
            pending_cmds: eng.cmds.len(),
            leaked_threads: leaked,
            fatal,
        }
    })
}

pub fn poll_events() -> Vec<AudioEvent> {
    AUDIO_ENGINE.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|e| e.events.drain().0)
            .unwrap_or_default()
    })
}

pub fn ensure_initialized_auto(
    deterministic: bool,
    budget_core: Option<Arc<BudgetCore>>,
    timeout_ms_left: Option<u64>,
) -> Result<(), String> {
    if !is_initialized() {
        init_engine(&InitOpts::default(), deterministic, budget_core, timeout_ms_left)?;
    } else {
        update_deadline(timeout_ms_left);
    }
    Ok(())
}

pub fn get_or_create_bus(name: &str) -> Result<u16, String> {
    let norm = name.trim().to_string();
    if norm.is_empty() {
        return Err(err_bad_opt("bus name cannot be empty"));
    }
    AUDIO_ENGINE.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let eng = borrow
            .as_mut()
            .ok_or_else(|| "audio engine not initialized".to_string())?;
        if let Some(&id) = eng.buses_by_name.get(&norm) {
            return Ok(id);
        }
        if eng.buses_by_name.len() > MAX_BUSES {
            return Err(err_limit(format!(
                "maximum audio buses ({} + master) exceeded",
                MAX_BUSES
            )));
        }
        let id = eng.next_bus_id;
        eng.next_bus_id += 1;
        eng.buses_by_name.insert(norm.clone(), id);
        eng.bus_names_by_id.insert(id, norm.clone());
        eng.cmds.try_send(MixerCommand::CreateBus {
            bus_id: id,
            name: norm,
        })?;
        Ok(id)
    })
}

pub fn send_command(cmd: MixerCommand) -> Result<(), String> {
    AUDIO_ENGINE.with(|slot| {
        let borrow = slot.borrow();
        let eng = borrow
            .as_ref()
            .ok_or_else(|| "audio engine not initialized".to_string())?;
        eng.cmds.try_send(cmd)
    })
}

pub fn play_pcm_voice(
    pcm: Arc<[f32]>,
    total_frames: u64,
    bus_id: u16,
    volume: f32,
    pan: f32,
    pitch: f32,
    loop_mode: bool,
    loop_points: Option<(u64, u64)>,
    fade_in_ms: u32,
    priority: i32,
    on_full: OverflowPolicy,
    pos3: Option<[f32; 3]>,
) -> Result<u64, String> {
    AUDIO_ENGINE.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let eng = borrow
            .as_mut()
            .ok_or_else(|| "audio engine not initialized".to_string())?;
        eng.voices.retain(|_, v| v.playing.load(Ordering::Acquire));
        if eng.voices.len() >= eng.max_voices && on_full == OverflowPolicy::Error {
            return Err(err_limit(format!(
                "maximum active audio voices ({}) reached",
                eng.max_voices
            )));
        }
        let vid = eng.next_voice_id;
        eng.next_voice_id += 1;
        let shared = Arc::new(VoiceShared::default());
        eng.voices.insert(vid, Arc::clone(&shared));
        eng.cmds.try_send(MixerCommand::PlayVoice(VoicePlayParams {
            voice_id: vid,
            source: VoiceSource::Pcm { pcm, total_frames },
            bus_id,
            volume,
            pan,
            pitch,
            loop_mode,
            loop_points,
            fade_in_ms,
            priority,
            on_full,
            pos3,
            shared,
        }))?;
        Ok(vid)
    })
}

pub fn get_voice_shared(vid: u64) -> Option<Arc<VoiceShared>> {
    AUDIO_ENGINE.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|e| e.voices.get(&vid).cloned())
    })
}

pub fn get_bus_state(bus_id: u16) -> Option<(String, f32, bool)> {
    AUDIO_ENGINE.with(|slot| {
        let borrow = slot.borrow();
        let eng = borrow.as_ref()?;
        let mut cmds = Vec::new();
        eng.cmds.drain_into(&mut cmds);
        let mut mx = eng.mixer.lock().ok()?;
        for c in cmds {
            mx.apply_command(c);
        }
        mx.buses
            .iter()
            .find(|b| b.id == bus_id)
            .map(|b| (b.name.clone(), b.volume, b.muted))
    })
}

pub fn bus_exists(bus_id: u16) -> bool {
    AUDIO_ENGINE.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|e| e.bus_names_by_id.contains_key(&bus_id))
    })
}

pub fn tick_virtual(ms: u32, deterministic: bool) -> Result<(u64, u64), String> {
    if ms > 60_000 {
        return Err(err_limit("audio.tick() step cannot exceed 60000 ms"));
    }
    AUDIO_ENGINE.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let eng = borrow
            .as_mut()
            .ok_or_else(|| "audio engine not initialized".to_string())?;
        if matches!(eng.sink, SinkKind::WaveOut | SinkKind::Alsa) {
            return Err(err_audio_nondet(format!(
                "audio.tick() cannot be used with realtime sink '{}'",
                eng.sink.name()
            )));
        }
        if deterministic && eng.sink != SinkKind::Virtual && eng.sink != SinkKind::Null {
            return Err(err_audio_nondet("audio.tick() non-deterministic sink"));
        }

        let mut cmd_batch = Vec::new();
        eng.cmds.drain_into(&mut cmd_batch);
        let mut mx = eng
            .mixer
            .lock()
            .map_err(|_| "mixer lock poisoned".to_string())?;
        for c in cmd_batch {
            mx.apply_command(c);
        }

        let total_frames = ((ms as u64).saturating_mul(ENGINE_RATE as u64)) / 1000;
        let mut buf = vec![0.0f32; QUANTUM * 2];
        let mut rem = total_frames as usize;
        while rem > 0 {
            let n = rem.min(QUANTUM);
            let ur = mx.render_quantum(&mut buf[..n * 2]);
            if ur > 0 {
                eng.events.push_drop_oldest(AudioEvent::Underrun { frames: ur });
            }
            rem -= n;
        }
        let clock_ms = (mx.frames_played.saturating_mul(1000)) / (ENGINE_RATE as u64);
        Ok((mx.frames_played, clock_ms))
    })
}

pub fn create_music_stream(
    wav_bytes: Vec<u8>,
    chunk_delay_ms: u32,
    budget_core: Arc<BudgetCore>,
    timeout_ms_left: Option<u64>,
) -> Result<(u64, Arc<MusicShared>, u64), String> {
    let mut decoder = WavStreamDecoder::from_bytes(wav_bytes, Some(&budget_core))?;
    let total_frames = decoder.estimated_total_engine_frames();
    let dur_ms = (total_frames.saturating_mul(1000)) / (ENGINE_RATE as u64);

    AUDIO_ENGINE.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let eng = borrow
            .as_mut()
            .ok_or_else(|| "audio engine not initialized".to_string())?;
        if eng.musics.len() >= MAX_MUSICS {
            return Err(err_limit(format!(
                "maximum active music streams ({}) exceeded",
                MAX_MUSICS
            )));
        }

        let mid = eng.next_music_id;
        eng.next_music_id += 1;
        let shared = Arc::new(MusicShared::default());
        shared.dur_ms.store(dur_ms, Ordering::Release);
        let ring = Arc::new(Mutex::new(StreamRing::new(RING_FRAMES)));
        let stop_flag = Arc::new(AtomicBool::new(false));
        let done_flag = Arc::new(AtomicBool::new(false));

        let dl_ms = timeout_ms_left
            .map(|l| current_unix_ms().saturating_add(l))
            .unwrap_or_else(|| eng.deadline_epoch_ms.load(Ordering::Acquire));

        let t_ring = Arc::clone(&ring);
        let t_stop = Arc::clone(&stop_flag);
        let t_done = Arc::clone(&done_flag);
        let t_events = Arc::clone(&eng.events);
        let t_budget = Arc::clone(&budget_core);
        let t_shared = Arc::clone(&shared);
        let dl_atomic = Arc::clone(&eng.deadline_epoch_ms);
        if dl_ms > 0 {
            dl_atomic.store(dl_ms, Ordering::Release);
        }

        let handle = thread::spawn(move || {
            loop {
                if t_stop.load(Ordering::Acquire) || t_budget.fatal.load(Ordering::Acquire) {
                    break;
                }
                let dl = dl_atomic.load(Ordering::Acquire);
                if dl > 0 && current_unix_ms() > dl {
                    break;
                }

                let (free, seek_req, is_eof) = if let Ok(mut rg) = t_ring.lock() {
                    let s = rg.seek_request_frame.take();
                    (rg.free_frames(), s, rg.eof)
                } else {
                    break;
                };

                if let Some(target_frame) = seek_req {
                    decoder.seek_engine_frame(target_frame);
                    continue;
                }

                if is_eof || free < 64 {
                    thread::sleep(Duration::from_millis(3));
                    continue;
                }

                if chunk_delay_ms > 0 {
                    let mut rem = chunk_delay_ms;
                    while rem > 0 {
                        if t_stop.load(Ordering::Acquire) {
                            t_done.store(true, Ordering::Release);
                            return;
                        }
                        let dl2 = dl_atomic.load(Ordering::Acquire);
                        if dl2 > 0 && current_unix_ms() > dl2 {
                            t_done.store(true, Ordering::Release);
                            return;
                        }
                        let s = rem.min(5);
                        thread::sleep(Duration::from_millis(s as u64));
                        rem -= s;
                    }
                }

                let take = free.min(QUANTUM * 2);
                match decoder.read_next_stereo(take) {
                    Ok((samples, reached_eof)) => {
                        if let Ok(mut rg) = t_ring.lock() {
                            if rg.seek_request_frame.is_none() {
                                if !samples.is_empty() {
                                    rg.push_stereo(&samples);
                                }
                                if reached_eof {
                                    rg.eof = true;
                                }
                            }
                        }
                    }
                    Err(msg) => {
                        if let Ok(mut rg) = t_ring.lock() {
                            rg.failed = Some(msg.clone());
                        }
                        t_shared.state.store(MusicStatus::Failed as u8, Ordering::Release);
                        t_events.push_drop_oldest(AudioEvent::StreamError {
                            music_id: mid,
                            message: msg,
                        });
                        break;
                    }
                }
            }
            t_done.store(true, Ordering::Release);
        });

        eng.musics.insert(
            mid,
            MusicSlot {
                shared: Arc::clone(&shared),
                ring,
                total_frames,
                active_voice_id: None,
                loop_mode: false,
                loop_points: None,
                volume: 1.0,
                stop_flag,
                done_flag,
                thread: Some(handle),
            },
        );
        Ok((mid, shared, dur_ms))
    })
}

pub fn play_music_stream(
    music_id: u64,
    bus_id: u16,
    volume_override: Option<f32>,
    fade_in_ms: u32,
) -> Result<u64, String> {
    AUDIO_ENGINE.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let eng = borrow
            .as_mut()
            .ok_or_else(|| "audio engine not initialized".to_string())?;
        let m = eng
            .musics
            .get_mut(&music_id)
            .ok_or_else(|| err_bad_opt(format!("invalid music handle {}", music_id)))?;
        if let Some(old_vid) = m.active_voice_id.take() {
            let _ = eng.cmds.try_send(MixerCommand::StopVoice { voice_id: old_vid });
        }
        let vol = volume_override.unwrap_or(m.volume).clamp(0.0, 4.0);
        m.volume = vol;
        m.shared.vol_milli.store((vol * 1000.0) as u32, Ordering::Release);
        let generation = m.ring.lock().map(|r| r.generation).unwrap_or(1);
        let ring_arc = Arc::clone(&m.ring);
        let m_shared = Arc::clone(&m.shared);
        let loop_mode = m.loop_mode;
        let loop_points = m.loop_points;

        let vid = eng.next_voice_id;
        eng.next_voice_id += 1;
        let v_shared = Arc::new(VoiceShared::default());
        eng.voices.insert(vid, Arc::clone(&v_shared));
        m.active_voice_id = Some(vid);

        eng.cmds.try_send(MixerCommand::PlayVoice(VoicePlayParams {
            voice_id: vid,
            source: VoiceSource::Stream {
                ring: ring_arc,
                music_shared: m_shared,
                stream_last_pair: [0.0, 0.0],
                expected_generation: generation,
            },
            bus_id,
            volume: vol,
            pan: 0.0,
            pitch: 1.0,
            loop_mode,
            loop_points,
            fade_in_ms,
            priority: 10,
            on_full: OverflowPolicy::DropOldest,
            pos3: None,
            shared: v_shared,
        }))?;
        Ok(vid)
    })
}

pub fn control_music<R>(
    music_id: u64,
    f: impl FnOnce(&mut MusicSlot, &Arc<BoundedCmdQueue<MixerCommand>>) -> Result<R, String>,
) -> Result<R, String> {
    AUDIO_ENGINE.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let eng = borrow
            .as_mut()
            .ok_or_else(|| "audio engine not initialized".to_string())?;
        let cmds = Arc::clone(&eng.cmds);
        let m = eng
            .musics
            .get_mut(&music_id)
            .ok_or_else(|| err_bad_opt(format!("invalid or closed music handle {}", music_id)))?;
        f(m, &cmds)
    })
}

pub fn close_music_stream(music_id: u64) {
    let removed = AUDIO_ENGINE.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let Some(eng) = borrow.as_mut() else {
            return None;
        };
        let m = eng.musics.remove(&music_id)?;
        if let Some(vid) = m.active_voice_id {
            let _ = eng.cmds.try_send(MixerCommand::StopVoice { voice_id: vid });
        }
        Some(m)
    });
    if let Some(mut ms) = removed {
        let deadline = Instant::now() + Duration::from_millis(ENGINE_JOIN_TIMEOUT_MS);
        join_flag_with_deadline(&ms.stop_flag, &ms.done_flag, ms.thread.take(), deadline);
    }
}

pub fn get_music_shared(music_id: u64) -> Option<(Arc<MusicShared>, u64)> {
    AUDIO_ENGINE.with(|slot| {
        let borrow = slot.borrow();
        let eng = borrow.as_ref()?;
        eng.musics
            .get(&music_id)
            .map(|m| (Arc::clone(&m.shared), m.total_frames))
    })
}

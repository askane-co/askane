use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use crate::media::audio::{ENGINE_RATE, err_backpressure};
use crate::media::{Digest, MediaBudgetGuard};

#[derive(Debug)]
struct SoundInner {
    pcm: Arc<[f32]>,
    _budget: MediaBudgetGuard,
}

#[derive(Debug)]
pub struct SoundData {
    pub digest: Digest,
    pub frames: u64,
    pub loop_points: Option<(u64, u64)>,
    pub closed: AtomicBool,
    inner: Mutex<Option<SoundInner>>,
}

impl SoundData {
    pub fn from_pcm(
        pcm_vec: Vec<f32>,
        loop_points: Option<(u64, u64)>,
        digest: Option<Digest>,
        budget_core: Option<&Arc<crate::budget::BudgetCore>>,
    ) -> Result<Self, String> {
        let byte_len = pcm_vec.len().saturating_mul(std::mem::size_of::<f32>());
        let guard = MediaBudgetGuard::charge(budget_core, byte_len)?;
        let frames = (pcm_vec.len() / 2) as u64;
        let d = digest.unwrap_or_else(|| {
            let mut raw = Vec::with_capacity(16 + (byte_len.min(1024 * 1024)));
            raw.extend_from_slice(b"askane.snd.v1|");
            raw.extend_from_slice(&frames.to_le_bytes());
            for &s in &pcm_vec {
                let norm = if s == 0.0 { 0.0f32 } else { s };
                raw.extend_from_slice(&norm.to_bits().to_le_bytes());
            }
            Digest::from_parts(&[&raw])
        });
        let pcm: Arc<[f32]> = Arc::from(pcm_vec);
        Ok(Self {
            digest: d,
            frames,
            loop_points,
            closed: AtomicBool::new(false),
            inner: Mutex::new(Some(SoundInner {
                pcm,
                _budget: guard,
            })),
        })
    }

    pub fn pcm_clone(&self) -> Result<Arc<[f32]>, String> {
        if self.closed.load(Ordering::Acquire) {
            return Err("audio: sound resource was closed".to_string());
        }
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()
            .map(|inner| Arc::clone(&inner.pcm))
            .ok_or_else(|| "audio: sound resource was closed".to_string())
    }

    #[inline]
    pub fn duration_ms(&self) -> u64 {
        (self.frames.saturating_mul(1000)) / (ENGINE_RATE as u64)
    }

    #[allow(dead_code)]
    #[inline]
    pub fn byte_len(&self) -> usize {
        if self.closed.load(Ordering::Acquire) {
            0
        } else {
            (self.frames as usize).saturating_mul(8)
        }
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        if let Ok(mut guard) = self.inner.lock() {
            *guard = None;
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MusicStatus {
    Idle = 0,
    Playing = 1,
    Paused = 2,
    Ended = 3,
    Failed = 4,
}

impl MusicStatus {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Playing,
            2 => Self::Paused,
            3 => Self::Ended,
            4 => Self::Failed,
            _ => Self::Idle,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Playing => "playing",
            Self::Paused => "paused",
            Self::Ended => "ended",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug)]
pub struct MusicShared {
    pub state: AtomicU8,
    pub pos_ms: AtomicU64,
    pub dur_ms: AtomicU64,
    pub vol_milli: AtomicU32,
}

impl Default for MusicShared {
    fn default() -> Self {
        Self {
            state: AtomicU8::new(MusicStatus::Idle as u8),
            pos_ms: AtomicU64::new(0),
            dur_ms: AtomicU64::new(0),
            vol_milli: AtomicU32::new(1000),
        }
    }
}

#[derive(Debug)]
pub struct VoiceShared {
    pub playing: AtomicBool,
    pub paused: AtomicBool,
    pub pos_ms: AtomicU64,
    pub bus_id: AtomicU16,
}

impl Default for VoiceShared {
    fn default() -> Self {
        Self {
            playing: AtomicBool::new(true),
            paused: AtomicBool::new(false),
            pos_ms: AtomicU64::new(0),
            bus_id: AtomicU16::new(0),
        }
    }
}

#[derive(Clone, Debug)]
pub enum AudioEvent {
    Fallback {
        from: String,
        to: String,
        reason: String,
    },
    Underrun {
        frames: u64,
    },
    StreamError {
        music_id: u64,
        message: String,
    },
    DeviceStopped {
        reason: String,
    },
}

/// Bounded audio stream PCM ring buffer holding interleaved f32 stereo frames (ENGINE_RATE).
pub struct StreamRing {
    pub buf: Vec<f32>,
    pub cap_frames: usize,
    pub read_frame: usize,
    pub write_frame: usize,
    pub len_frames: usize,
    pub total_decoded_frames: u64,
    pub eof: bool,
    pub failed: Option<String>,
    pub seek_request_frame: Option<u64>,
    pub generation: u64,
}

impl StreamRing {
    pub fn new(cap_frames: usize) -> Self {
        let cap = cap_frames.max(240);
        Self {
            buf: vec![0.0; cap * 2],
            cap_frames: cap,
            read_frame: 0,
            write_frame: 0,
            len_frames: 0,
            total_decoded_frames: 0,
            eof: false,
            failed: None,
            seek_request_frame: None,
            generation: 1,
        }
    }

    pub fn free_frames(&self) -> usize {
        self.cap_frames.saturating_sub(self.len_frames)
    }

    pub fn push_stereo(&mut self, stereo: &[f32]) -> usize {
        let avail_in = stereo.len() / 2;
        let to_write = avail_in.min(self.free_frames());
        for i in 0..to_write {
            let di = self.write_frame * 2;
            let si = i * 2;
            self.buf[di] = stereo[si];
            self.buf[di + 1] = stereo[si + 1];
            self.write_frame += 1;
            if self.write_frame >= self.cap_frames {
                self.write_frame = 0;
            }
        }
        self.len_frames += to_write;
        self.total_decoded_frames += to_write as u64;
        to_write
    }

    pub fn pop_frame(&mut self) -> Option<[f32; 2]> {
        if self.len_frames == 0 {
            return None;
        }
        let ri = self.read_frame * 2;
        let pair = [self.buf[ri], self.buf[ri + 1]];
        self.read_frame += 1;
        if self.read_frame >= self.cap_frames {
            self.read_frame = 0;
        }
        self.len_frames -= 1;
        Some(pair)
    }

    pub fn clear_for_seek(&mut self, frame: u64) {
        self.read_frame = 0;
        self.write_frame = 0;
        self.len_frames = 0;
        self.total_decoded_frames = frame;
        self.eof = false;
        self.seek_request_frame = Some(frame);
        self.generation = self.generation.wrapping_add(1);
    }
}

pub struct BoundedEventQueue {
    inner: Mutex<(VecDeque<AudioEvent>, u64)>,
    cap: usize,
}

impl BoundedEventQueue {
    pub fn new(cap: usize) -> Self {
        Self {
            inner: Mutex::new((VecDeque::with_capacity(cap.min(256)), 0)),
            cap,
        }
    }

    pub fn push_drop_oldest(&self, evt: AudioEvent) {
        if let Ok(mut guard) = self.inner.lock() {
            if guard.0.len() >= self.cap {
                guard.0.pop_front();
                guard.1 = guard.1.saturating_add(1);
            }
            guard.0.push_back(evt);
        }
    }

    pub fn drain(&self) -> (Vec<AudioEvent>, u64) {
        if let Ok(mut guard) = self.inner.lock() {
            let items: Vec<AudioEvent> = guard.0.drain(..).collect();
            let dropped = guard.1;
            (items, dropped)
        } else {
            (Vec::new(), 0)
        }
    }

    pub fn peek_dropped(&self) -> u64 {
        self.inner.lock().map(|g| g.1).unwrap_or(0)
    }
}

pub struct BoundedCmdQueue<T> {
    inner: Mutex<VecDeque<T>>,
    cap: usize,
}

impl<T> BoundedCmdQueue<T> {
    pub fn new(cap: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(cap.min(256))),
            cap,
        }
    }

    pub fn try_send(&self, cmd: T) -> Result<(), String> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| err_backpressure("audio command queue lock poisoned"))?;
        if guard.len() >= self.cap {
            return Err(err_backpressure(format!(
                "audio command queue full ({} pending commands)",
                self.cap
            )));
        }
        guard.push_back(cmd);
        Ok(())
    }

    pub fn drain_into(&self, out: &mut Vec<T>) {
        if let Ok(mut guard) = self.inner.lock() {
            while let Some(c) = guard.pop_front() {
                out.push(c);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
    }
}

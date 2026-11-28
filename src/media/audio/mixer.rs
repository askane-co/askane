use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crate::media::audio::sound::{MusicShared, MusicStatus, StreamRing, VoiceShared};
use crate::media::audio::synth::{lut_cos, lut_sin, pan_gains};
use crate::media::audio::{ENGINE_RATE, HARD_MAX_VOICES, MAX_BUSES, MAX_FADE_MS, QUANTUM};
use crate::media::err_bad_opt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverflowPolicy {
    Error,
    DropOldest,
}

#[derive(Clone)]
pub enum VoiceSource {
    Pcm {
        pcm: Arc<[f32]>,
        total_frames: u64,
    },
    Stream {
        ring: Arc<Mutex<StreamRing>>,
        music_shared: Arc<MusicShared>,
        stream_last_pair: [f32; 2],
        expected_generation: u64,
    },
}

#[derive(Clone, Debug)]
pub struct FadeRamp {
    pub start_vol: f32,
    pub target_vol: f32,
    pub elapsed_frames: u64,
    pub total_frames: u64,
    pub stop_on_finish: bool,
}

pub struct VoicePlayParams {
    pub voice_id: u64,
    pub source: VoiceSource,
    pub bus_id: u16,
    pub volume: f32,
    pub pan: f32,
    pub pitch: f32,
    pub loop_mode: bool,
    pub loop_points: Option<(u64, u64)>,
    pub fade_in_ms: u32,
    pub priority: i32,
    pub on_full: OverflowPolicy,
    pub pos3: Option<[f32; 3]>,
    pub shared: Arc<VoiceShared>,
}

pub struct MixerVoice {
    pub source: VoiceSource,
    pub bus_id: u16,
    pub volume: f32,
    pub pan: f32,
    pub pitch: f32,
    pub loop_mode: bool,
    pub loop_points: Option<(u64, u64)>,
    pub paused: bool,
    pub pos_q32: u64,
    pub stream_frames_consumed: u64,
    pub priority: i32,
    pub pos3: Option<[f32; 3]>,
    pub fade: Option<FadeRamp>,
    pub shared: Arc<VoiceShared>,
}

pub struct AudioBus {
    pub id: u16,
    pub name: String,
    pub volume: f32,
    pub muted: bool,
    pub lowpass_cutoff_hz: Option<f32>,
    lp_alpha: f32,
    lp_prev_l: f32,
    lp_prev_r: f32,
}

impl AudioBus {
    pub fn new(id: u16, name: String) -> Self {
        Self {
            id,
            name,
            volume: 1.0,
            muted: false,
            lowpass_cutoff_hz: None,
            lp_alpha: 1.0,
            lp_prev_l: 0.0,
            lp_prev_r: 0.0,
        }
    }

    pub fn set_lowpass(&mut self, cutoff_hz: Option<f32>) {
        self.lowpass_cutoff_hz = cutoff_hz;
        if let Some(hz) = cutoff_hz {
            let clamped = hz.clamp(40.0, 20_000.0);
            // Exponential smoothing coefficient alpha = 2*pi*fc / (2*pi*fc + fs), pure deterministic arithmetic
            let two_pi_fc = 6.2831853f32 * clamped;
            self.lp_alpha = (two_pi_fc / (two_pi_fc + ENGINE_RATE as f32)).clamp(0.001, 1.0);
        } else {
            self.lp_alpha = 1.0;
        }
    }
}

pub struct Listener3D {
    pub pos: [f32; 3],
    pub right: [f32; 3],
    pub rolloff: f32,
    pub ref_dist: f32,
    pub max_dist: f32,
}

impl Default for Listener3D {
    fn default() -> Self {
        Self {
            pos: [0.0, 0.0, 0.0],
            right: [1.0, 0.0, 0.0],
            rolloff: 1.0,
            ref_dist: 1.0,
            max_dist: 64.0,
        }
    }
}

impl Listener3D {
    pub fn compute_spatial(&self, src: [f32; 3]) -> (f32, f32) {
        let dx = src[0] - self.pos[0];
        let dy = src[1] - self.pos[1];
        let dz = src[2] - self.pos[2];
        let dist_sq = dx * dx + dy * dy + dz * dz;
        let dist = dist_sq.sqrt();
        let d = dist.clamp(self.ref_dist, self.max_dist.max(self.ref_dist + 0.01));
        let atten = self.ref_dist / (self.ref_dist + self.rolloff * (d - self.ref_dist));
        let pan = if dist > 1e-4 {
            let dot_r = (dx * self.right[0] + dy * self.right[1] + dz * self.right[2]) / dist;
            dot_r.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        (atten.clamp(0.0, 1.0), pan)
    }
}

pub enum MixerCommand {
    PlayVoice(VoicePlayParams),
    StopVoice { voice_id: u64 },
    PauseVoice { voice_id: u64 },
    ResumeVoice { voice_id: u64 },
    SetVoiceVolume { voice_id: u64, volume: f32 },
    SetVoicePan { voice_id: u64, pan: f32 },
    SetVoicePitch { voice_id: u64, pitch: f32 },
    SetVoiceLoop { voice_id: u64, loop_mode: bool },
    SetVoiceLoopPoints { voice_id: u64, start_frame: u64, end_frame: u64 },
    FadeVoice { voice_id: u64, target_vol: f32, ms: u32, stop_on_finish: bool },
    SetVoicePos3 { voice_id: u64, pos3: [f32; 3] },
    StopBusVoices { bus_id: Option<u16> },
    PauseBusVoices { bus_id: Option<u16>, paused: bool },
    CreateBus { bus_id: u16, name: String },
    SetBusVolume { bus_id: u16, volume: f32 },
    SetBusMute { bus_id: u16, muted: bool },
    SetBusEffect { bus_id: u16, lowpass_hz: Option<f32> },
    SetListener { pos: [f32; 3], forward: [f32; 3], rolloff: f32, max_dist: f32 },
    Shutdown,
}

pub struct MixerState {
    pub max_voices: usize,
    pub buses: Vec<AudioBus>,
    pub voices: BTreeMap<u64, MixerVoice>,
    pub listener: Listener3D,
    pub frames_played: u64,
    pub global_paused: bool,
    pub shutdown_requested: bool,
    pub underrun_events: u64,
    scratch_bus: Vec<Vec<f32>>,
}

impl MixerState {
    pub fn new(max_voices: usize) -> Self {
        let mut buses = Vec::with_capacity(MAX_BUSES + 1);
        buses.push(AudioBus::new(0, "master".to_string()));
        Self {
            max_voices: max_voices.clamp(1, HARD_MAX_VOICES),
            buses,
            voices: BTreeMap::new(),
            listener: Listener3D::default(),
            frames_played: 0,
            global_paused: false,
            shutdown_requested: false,
            underrun_events: 0,
            scratch_bus: vec![vec![0.0f32; QUANTUM * 2]; MAX_BUSES + 1],
        }
    }

    pub fn apply_command(&mut self, cmd: MixerCommand) {
        match cmd {
            MixerCommand::Shutdown => {
                self.shutdown_requested = true;
                for (_, v) in &mut self.voices {
                    v.shared.playing.store(false, Ordering::Release);
                    if let VoiceSource::Stream { music_shared, .. } = &v.source {
                        music_shared.state.store(MusicStatus::Idle as u8, Ordering::Release);
                    }
                }
                self.voices.clear();
            }
            MixerCommand::CreateBus { bus_id, name } => {
                let idx = bus_id as usize;
                if idx <= MAX_BUSES && self.buses.iter().all(|b| b.id != bus_id) {
                    self.buses.push(AudioBus::new(bus_id, name));
                    self.buses.sort_by_key(|b| b.id);
                }
            }
            MixerCommand::SetBusVolume { bus_id, volume } => {
                if let Some(b) = self.buses.iter_mut().find(|b| b.id == bus_id) {
                    b.volume = volume.clamp(0.0, 4.0);
                }
            }
            MixerCommand::SetBusMute { bus_id, muted } => {
                if let Some(b) = self.buses.iter_mut().find(|b| b.id == bus_id) {
                    b.muted = muted;
                }
            }
            MixerCommand::SetBusEffect { bus_id, lowpass_hz } => {
                if let Some(b) = self.buses.iter_mut().find(|b| b.id == bus_id) {
                    b.set_lowpass(lowpass_hz);
                }
            }
            MixerCommand::SetListener {
                pos,
                forward,
                rolloff,
                max_dist,
            } => {
                self.listener.pos = pos;
                // Compute right vector = forward x (0, 1, 0) in RH Y-up -Z forward coordinates
                let rx = forward[1] * 0.0 - forward[2] * 1.0;
                let ry = forward[2] * 0.0 - forward[0] * 0.0;
                let rz = forward[0] * 1.0 - forward[1] * 0.0;
                let norm = (rx * rx + ry * ry + rz * rz).sqrt();
                self.listener.right = if norm > 1e-5 {
                    [rx / norm, ry / norm, rz / norm]
                } else {
                    [1.0, 0.0, 0.0]
                };
                self.listener.rolloff = rolloff.clamp(0.0, 16.0);
                self.listener.max_dist = max_dist.clamp(1.0, 4096.0);
            }
            MixerCommand::PlayVoice(p) => {
                if self.voices.len() >= self.max_voices {
                    match p.on_full {
                        OverflowPolicy::Error => {
                            p.shared.playing.store(false, Ordering::Release);
                            return;
                        }
                        OverflowPolicy::DropOldest => {
                            if let Some((&victim_id, _)) = self
                                .voices
                                .iter()
                                .min_by(|(ida, va), (idb, vb)| {
                                    va.priority.cmp(&vb.priority).then_with(|| ida.cmp(idb))
                                })
                            {
                                if let Some(victim) = self.voices.remove(&victim_id) {
                                    victim.shared.playing.store(false, Ordering::Release);
                                }
                            }
                        }
                    }
                }
                let fade = if p.fade_in_ms > 0 {
                    let f_frames = ((p.fade_in_ms.min(MAX_FADE_MS) as u64) * (ENGINE_RATE as u64)) / 1000;
                    if f_frames > 0 {
                        Some(FadeRamp {
                            start_vol: 0.0,
                            target_vol: p.volume.clamp(0.0, 4.0),
                            elapsed_frames: 0,
                            total_frames: f_frames,
                            stop_on_finish: false,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                };
                let init_vol = if fade.is_some() { 0.0 } else { p.volume.clamp(0.0, 4.0) };
                let init_gen = match &p.source {
                    VoiceSource::Stream { expected_generation, .. } => *expected_generation,
                    _ => 0,
                };
                let mut src = p.source;
                if let VoiceSource::Stream { expected_generation, music_shared, .. } = &mut src {
                    *expected_generation = init_gen;
                    music_shared.state.store(MusicStatus::Playing as u8, Ordering::Release);
                }
                p.shared.playing.store(true, Ordering::Release);
                p.shared.paused.store(false, Ordering::Release);
                p.shared.bus_id.store(p.bus_id, Ordering::Release);

                self.voices.insert(
                    p.voice_id,
                    MixerVoice {
                        source: src,
                        bus_id: p.bus_id,
                        volume: init_vol,
                        pan: p.pan.clamp(-1.0, 1.0),
                        pitch: p.pitch.clamp(0.25, 4.0),
                        loop_mode: p.loop_mode,
                        loop_points: p.loop_points,
                        paused: false,
                        pos_q32: 0,
                        stream_frames_consumed: 0,
                        priority: p.priority,
                        pos3: p.pos3,
                        fade,
                        shared: p.shared,
                    },
                );
            }
            MixerCommand::StopVoice { voice_id } => {
                if let Some(v) = self.voices.remove(&voice_id) {
                    v.shared.playing.store(false, Ordering::Release);
                    if let VoiceSource::Stream { music_shared, .. } = v.source {
                        music_shared.state.store(MusicStatus::Idle as u8, Ordering::Release);
                    }
                }
            }
            MixerCommand::PauseVoice { voice_id } => {
                if let Some(v) = self.voices.get_mut(&voice_id) {
                    v.paused = true;
                    v.shared.paused.store(true, Ordering::Release);
                    if let VoiceSource::Stream { music_shared, .. } = &v.source {
                        music_shared.state.store(MusicStatus::Paused as u8, Ordering::Release);
                    }
                }
            }
            MixerCommand::ResumeVoice { voice_id } => {
                if let Some(v) = self.voices.get_mut(&voice_id) {
                    v.paused = false;
                    v.shared.paused.store(false, Ordering::Release);
                    if let VoiceSource::Stream { music_shared, .. } = &v.source {
                        music_shared.state.store(MusicStatus::Playing as u8, Ordering::Release);
                    }
                }
            }
            MixerCommand::SetVoiceVolume { voice_id, volume } => {
                if let Some(v) = self.voices.get_mut(&voice_id) {
                    v.volume = volume.clamp(0.0, 4.0);
                    v.fade = None;
                }
            }
            MixerCommand::SetVoicePan { voice_id, pan } => {
                if let Some(v) = self.voices.get_mut(&voice_id) {
                    v.pan = pan.clamp(-1.0, 1.0);
                }
            }
            MixerCommand::SetVoicePitch { voice_id, pitch } => {
                if let Some(v) = self.voices.get_mut(&voice_id) {
                    v.pitch = pitch.clamp(0.25, 4.0);
                }
            }
            MixerCommand::SetVoiceLoop { voice_id, loop_mode } => {
                if let Some(v) = self.voices.get_mut(&voice_id) {
                    v.loop_mode = loop_mode;
                }
            }
            MixerCommand::SetVoiceLoopPoints {
                voice_id,
                start_frame,
                end_frame,
            } => {
                if let Some(v) = self.voices.get_mut(&voice_id) {
                    if end_frame > start_frame {
                        v.loop_points = Some((start_frame, end_frame));
                        v.loop_mode = true;
                    }
                }
            }
            MixerCommand::FadeVoice {
                voice_id,
                target_vol,
                ms,
                stop_on_finish,
            } => {
                if let Some(v) = self.voices.get_mut(&voice_id) {
                    let f_frames = ((ms.min(MAX_FADE_MS) as u64) * (ENGINE_RATE as u64)) / 1000;
                    if f_frames == 0 {
                        v.volume = target_vol.clamp(0.0, 4.0);
                        v.fade = None;
                        if stop_on_finish {
                            v.shared.playing.store(false, Ordering::Release);
                            self.voices.remove(&voice_id);
                        }
                    } else {
                        v.fade = Some(FadeRamp {
                            start_vol: v.volume,
                            target_vol: target_vol.clamp(0.0, 4.0),
                            elapsed_frames: 0,
                            total_frames: f_frames,
                            stop_on_finish,
                        });
                    }
                }
            }
            MixerCommand::SetVoicePos3 { voice_id, pos3 } => {
                if let Some(v) = self.voices.get_mut(&voice_id) {
                    v.pos3 = Some(pos3);
                }
            }
            MixerCommand::StopBusVoices { bus_id } => match bus_id {
                None => {
                    for (_, v) in &self.voices {
                        v.shared.playing.store(false, Ordering::Release);
                        if let VoiceSource::Stream { music_shared, .. } = &v.source {
                            music_shared.state.store(MusicStatus::Idle as u8, Ordering::Release);
                        }
                    }
                    self.voices.clear();
                }
                Some(bid) => {
                    let ids: Vec<u64> = self
                        .voices
                        .iter()
                        .filter(|(_, v)| v.bus_id == bid)
                        .map(|(&id, _)| id)
                        .collect();
                    for id in ids {
                        if let Some(v) = self.voices.remove(&id) {
                            v.shared.playing.store(false, Ordering::Release);
                            if let VoiceSource::Stream { music_shared, .. } = v.source {
                                music_shared.state.store(MusicStatus::Idle as u8, Ordering::Release);
                            }
                        }
                    }
                }
            },
            MixerCommand::PauseBusVoices { bus_id, paused } => {
                if bus_id.is_none() {
                    self.global_paused = paused;
                }
                for (_, v) in &mut self.voices {
                    if bus_id.is_none() || Some(v.bus_id) == bus_id {
                        v.paused = paused;
                        v.shared.paused.store(paused, Ordering::Release);
                        if let VoiceSource::Stream { music_shared, .. } = &v.source {
                            let st = if paused {
                                MusicStatus::Paused
                            } else {
                                MusicStatus::Playing
                            };
                            music_shared.state.store(st as u8, Ordering::Release);
                        }
                    }
                }
            }
        }
    }

    /// Pure deterministic quantum renderer: renders exactly `QUANTUM` stereo frames (or slice len / 2 <= QUANTUM)
    pub fn render_quantum(&mut self, out: &mut [f32]) -> u64 {
        let frames = (out.len() / 2).min(QUANTUM);
        for s in &mut out[..frames * 2] {
            *s = 0.0;
        }
        if frames == 0 || self.global_paused {
            return 0;
        }

        for bus_buf in &mut self.scratch_bus {
            for s in &mut bus_buf[..frames * 2] {
                *s = 0.0;
            }
        }

        let mut finished_ids = Vec::new();
        let mut underruns_this_quantum = 0u64;

        // Strict deterministic order by voice_id ascending (BTreeMap).
        for (&vid, v) in &mut self.voices {
            if v.paused {
                continue;
            }
            let b_idx = if (v.bus_id as usize) < self.scratch_bus.len() {
                v.bus_id as usize
            } else {
                0
            };
            let bus_out = &mut self.scratch_bus[b_idx];

            let (spatial_atten, spatial_pan) = if let Some(pos3) = v.pos3 {
                self.listener.compute_spatial(pos3)
            } else {
                (1.0, 0.0)
            };
            let eff_pan = (v.pan + spatial_pan).clamp(-1.0, 1.0);
            let (pan_l, pan_r) = pan_gains(eff_pan);
            let pitch_step_q32: u64 = ((v.pitch as f64) * 4_294_967_296.0).round() as u64;
            let mut voice_ended = false;

            for fi in 0..frames {
                if let Some(ref mut ramp) = v.fade {
                    let t = (ramp.elapsed_frames as f32) / (ramp.total_frames as f32);
                    v.volume = ramp.start_vol + (ramp.target_vol - ramp.start_vol) * t;
                    ramp.elapsed_frames += 1;
                    if ramp.elapsed_frames >= ramp.total_frames {
                        v.volume = ramp.target_vol;
                        let stop = ramp.stop_on_finish;
                        v.fade = None;
                        if stop {
                            voice_ended = true;
                            break;
                        }
                    }
                }

                let gain = v.volume * spatial_atten;
                let (sl, sr) = match &mut v.source {
                    VoiceSource::Pcm { pcm, total_frames } => {
                        let tf = *total_frames;
                        if tf == 0 {
                            voice_ended = true;
                            break;
                        }
                        let cur_idx = v.pos_q32 >> 32;
                        let (loop_a, loop_b) = v.loop_points.unwrap_or((0, tf));
                        let limit_end = if v.loop_mode { loop_b.min(tf) } else { tf };
                        if cur_idx >= limit_end {
                            if v.loop_mode && loop_b > loop_a {
                                let loop_len = loop_b - loop_a;
                                let wrapped = loop_a + ((cur_idx - loop_a) % loop_len);
                                v.pos_q32 = (wrapped << 32) | (v.pos_q32 & 0xFFFF_FFFF);
                            } else {
                                voice_ended = true;
                                break;
                            }
                        }

                        let i0 = ((v.pos_q32 >> 32) as usize).min(tf as usize - 1);
                        let i1 = if v.loop_mode && (i0 as u64 + 1) >= limit_end {
                            loop_a.min(tf - 1) as usize
                        } else {
                            (i0 + 1).min(tf as usize - 1)
                        };
                        let frac = ((v.pos_q32 & 0xFFFF_FFFF) as f32) * (1.0 / 4_294_967_296.0);
                        let l0 = pcm[i0 * 2];
                        let r0 = pcm[i0 * 2 + 1];
                        let l1 = pcm[i1 * 2];
                        let r1 = pcm[i1 * 2 + 1];
                        let sl = l0 + (l1 - l0) * frac;
                        let sr = r0 + (r1 - r0) * frac;

                        v.pos_q32 = v.pos_q32.saturating_add(pitch_step_q32);
                        (sl, sr)
                    }
                    VoiceSource::Stream {
                        ring,
                        stream_last_pair,
                        expected_generation,
                        music_shared,
                        ..
                    } => {
                        let mut popped = None;
                        if let Ok(mut rg) = ring.lock() {
                            if rg.generation != *expected_generation {
                                *expected_generation = rg.generation;
                                v.stream_frames_consumed = rg.total_decoded_frames.saturating_sub(rg.len_frames as u64);
                            }
                            if let Some(pair) = rg.pop_frame() {
                                *stream_last_pair = pair;
                                v.stream_frames_consumed += 1;
                                popped = Some(pair);
                            } else if rg.failed.is_some() {
                                music_shared.state.store(MusicStatus::Failed as u8, Ordering::Release);
                                voice_ended = true;
                                break;
                            } else if rg.eof {
                                if !v.loop_mode {
                                    music_shared.state.store(MusicStatus::Ended as u8, Ordering::Release);
                                    voice_ended = true;
                                    break;
                                } else {
                                    let loop_start = v.loop_points.map(|(a, _)| a).unwrap_or(0);
                                    rg.clear_for_seek(loop_start);
                                    popped = Some([0.0, 0.0]);
                                }
                            } else {
                                underruns_this_quantum += 1;
                                popped = Some([0.0, 0.0]);
                            }
                        }
                        let pair = popped.unwrap_or([0.0, 0.0]);
                        (pair[0], pair[1])
                    }
                };

                // Preserve natural stereo balance while allowing pan modulation.
                let (wl, wr) = if eff_pan.abs() < 1e-4 {
                    (sl * gain, sr * gain)
                } else {
                    let mono = (sl + sr) * 0.5;
                    let mix_l = sl + (mono - sl) * eff_pan.abs();
                    let mix_r = sr + (mono - sr) * eff_pan.abs();
                    (mix_l * gain * pan_l * 1.4142135, mix_r * gain * pan_r * 1.4142135)
                };
                bus_out[fi * 2] += wl;
                bus_out[fi * 2 + 1] += wr;
            }

            let cur_pos_ms = match &v.source {
                VoiceSource::Pcm { .. } => {
                    let frame_idx = v.pos_q32 >> 32;
                    (frame_idx.saturating_mul(1000)) / (ENGINE_RATE as u64)
                }
                VoiceSource::Stream { music_shared, .. } => {
                    let ms = (v.stream_frames_consumed.saturating_mul(1000)) / (ENGINE_RATE as u64);
                    music_shared.pos_ms.store(ms, Ordering::Release);
                    ms
                }
            };
            v.shared.pos_ms.store(cur_pos_ms, Ordering::Release);

            if voice_ended {
                v.shared.playing.store(false, Ordering::Release);
                finished_ids.push(vid);
            }
        }

        for id in finished_ids {
            self.voices.remove(&id);
        }

        // Submix custom buses + apply per-bus volume/mute/lowpass, accumulate onto master.
        let mut master_vol = 1.0f32;
        let mut master_muted = false;
        let mut master_alpha = 1.0f32;
        let mut master_prev_l = 0.0f32;
        let mut master_prev_r = 0.0f32;

        for bus in &mut self.buses {
            if bus.id == 0 {
                master_vol = bus.volume;
                master_muted = bus.muted;
                master_alpha = bus.lp_alpha;
                master_prev_l = bus.lp_prev_l;
                master_prev_r = bus.lp_prev_r;
                continue;
            }
            if bus.muted || (bus.id as usize) >= self.scratch_bus.len() {
                continue;
            }
            let b_idx = bus.id as usize;
            let v_bus = bus.volume;
            let alpha = bus.lp_alpha;
            let mut pl = bus.lp_prev_l;
            let mut pr = bus.lp_prev_r;
            let (s_master, s_other) = self.scratch_bus.split_at_mut(b_idx);
            let master_buf = &mut s_master[0];
            let bus_buf = &s_other[0];
            for fi in 0..frames {
                let mut l = bus_buf[fi * 2] * v_bus;
                let mut r = bus_buf[fi * 2 + 1] * v_bus;
                if alpha < 0.9999 {
                    pl += alpha * (l - pl);
                    pr += alpha * (r - pr);
                    l = pl;
                    r = pr;
                }
                master_buf[fi * 2] += l;
                master_buf[fi * 2 + 1] += r;
            }
            bus.lp_prev_l = pl;
            bus.lp_prev_r = pr;
        }

        if !master_muted {
            let master_buf = &self.scratch_bus[0];
            for fi in 0..frames {
                let mut l = master_buf[fi * 2] * master_vol;
                let mut r = master_buf[fi * 2 + 1] * master_vol;
                if master_alpha < 0.9999 {
                    master_prev_l += master_alpha * (l - master_prev_l);
                    master_prev_r += master_alpha * (r - master_prev_r);
                    l = master_prev_l;
                    r = master_prev_r;
                }
                // Soft deterministic limiter for headroom > 1.0 + hard clip safety
                out[fi * 2] = soft_limit(l);
                out[fi * 2 + 1] = soft_limit(r);
            }
            if let Some(m) = self.buses.iter_mut().find(|b| b.id == 0) {
                m.lp_prev_l = master_prev_l;
                m.lp_prev_r = master_prev_r;
            }
        }

        self.frames_played = self.frames_played.saturating_add(frames as u64);
        self.underrun_events = self.underrun_events.saturating_add(underruns_this_quantum);
        underruns_this_quantum
    }
}

#[inline]
fn soft_limit(x: f32) -> f32 {
    let ax = x.abs();
    if ax <= 0.95 {
        x
    } else {
        let excess = ax - 0.95;
        let compressed = 0.95 + excess / (1.0 + excess * 20.0);
        if x >= 0.0 {
            compressed.min(1.0)
        } else {
            -compressed.min(1.0)
        }
    }
}

pub fn render_sound_offline(
    pcm: &[f32],
    total_frames: u64,
    volume: f32,
    pan: f32,
    pitch: f32,
    loop_mode: bool,
    loop_points: Option<(u64, u64)>,
    duration_ms: Option<u32>,
) -> Result<Vec<f32>, String> {
    if total_frames == 0 {
        return Ok(Vec::new());
    }
    let out_frames = if let Some(ms) = duration_ms {
        ((ms as u64) * (ENGINE_RATE as u64)) / 1000
    } else if loop_mode {
        return Err(err_bad_opt(
            "render_offline() on looping sound requires explicit 'duration_ms'",
        ));
    } else {
        ((total_frames as f64) / (pitch.clamp(0.25, 4.0) as f64)).ceil() as u64
    };
    let out_bytes = (out_frames as usize).saturating_mul(8);
    if out_bytes > crate::media::audio::MAX_SOUND_BYTES {
        return Err(crate::media::err_limit("offline audio render exceeds 64 MiB limit"));
    }

    let mut mixer = MixerState::new(4);
    let shared = Arc::new(VoiceShared::default());
    let arc_pcm: Arc<[f32]> = Arc::from(pcm.to_vec());
    mixer.apply_command(MixerCommand::PlayVoice(VoicePlayParams {
        voice_id: 1,
        source: VoiceSource::Pcm {
            pcm: arc_pcm,
            total_frames,
        },
        bus_id: 0,
        volume,
        pan,
        pitch,
        loop_mode,
        loop_points,
        fade_in_ms: 0,
        priority: 0,
        on_full: OverflowPolicy::Error,
        pos3: None,
        shared,
    }));

    let total_usize = out_frames as usize;
    let mut result = vec![0.0f32; total_usize * 2];
    let mut pos = 0usize;
    while pos < total_usize {
        let n = (total_usize - pos).min(QUANTUM);
        mixer.render_quantum(&mut result[pos * 2..(pos + n) * 2]);
        pos += n;
    }
    let _ = (lut_sin(0.0), lut_cos(0.0));
    Ok(result)
}

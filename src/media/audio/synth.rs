use std::sync::{Arc, OnceLock};

use crate::budget::BudgetCore;
use crate::media::audio::sound::SoundData;
use crate::media::audio::wav::resample_stereo_linear;
use crate::media::audio::{ENGINE_RATE, MAX_AUDIO_DURATION_MS, MAX_SOUND_BYTES};
use crate::media::{Digest, check_deadline, err_bad_opt, err_limit, temp_budget_guard};

const SIN_LUT_LEN: usize = 4096;
static SIN_LUT: OnceLock<[f32; SIN_LUT_LEN]> = OnceLock::new();

/// Pure polynomial Taylor series for sin(pi/2 * t) on t in [0, 1], free of `libm`/`f32::sin`/`mul_add`.
fn sin_quarter_poly(t: f64) -> f64 {
    let u = t * t;
    let p = 0.00016044118478735975 + u * (-0.000003598843235212085);
    let p = -0.004681754135318688 + u * p;
    let p = 0.07969262624616704 + u * p;
    let p = -0.6459640975062463 + u * p;
    t * (1.5707963267948966 + u * p)
}

pub fn sin_lut_table() -> &'static [f32; SIN_LUT_LEN] {
    SIN_LUT.get_or_init(|| {
        let mut table = [0.0f32; SIN_LUT_LEN];
        let q = SIN_LUT_LEN / 4; // 1024
        for i in 0..SIN_LUT_LEN {
            let quad = i / q;
            let rem = i % q;
            let val = match quad {
                0 => sin_quarter_poly((rem as f64) / (q as f64)),
                1 => sin_quarter_poly(((q - rem) as f64) / (q as f64)),
                2 => -sin_quarter_poly((rem as f64) / (q as f64)),
                _ => -sin_quarter_poly(((q - rem) as f64) / (q as f64)),
            };
            table[i] = val as f32;
        }
        table
    })
}

/// Table-based deterministic sine in [-1.0, 1.0] for normalized phase in [0.0, 1.0).
#[inline]
pub fn lut_sin(phase_norm: f32) -> f32 {
    let lut = sin_lut_table();
    let wrapped = phase_norm - phase_norm.floor();
    let scaled = wrapped * (SIN_LUT_LEN as f32);
    let idx0 = (scaled as usize) & (SIN_LUT_LEN - 1);
    let idx1 = (idx0 + 1) & (SIN_LUT_LEN - 1);
    let frac = scaled - (idx0 as f32);
    let a = lut[idx0];
    let b = lut[idx1];
    a + (b - a) * frac
}

#[inline]
pub fn lut_cos(phase_norm: f32) -> f32 {
    lut_sin(phase_norm + 0.25)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Waveform {
    Sine,
    Square,
    Triangle,
    Saw,
}

impl Waveform {
    pub fn from_name(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "sine" | "sin" => Ok(Self::Sine),
            "square" | "sq" => Ok(Self::Square),
            "triangle" | "tri" => Ok(Self::Triangle),
            "saw" | "sawtooth" => Ok(Self::Saw),
            other => Err(err_bad_opt(format!("unknown audio waveform '{}'", other))),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToneOpts {
    pub waveform: Waveform,
    pub volume: f32,
    pub pan: f32,
    pub attack_ms: u32,
    pub decay_ms: u32,
    pub sustain: f32,
    pub release_ms: u32,
}

impl Default for ToneOpts {
    fn default() -> Self {
        Self {
            waveform: Waveform::Sine,
            volume: 1.0,
            pan: 0.0,
            attack_ms: 4,
            decay_ms: 0,
            sustain: 1.0,
            release_ms: 8,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoiseColor {
    White,
    Pink,
}

impl NoiseColor {
    pub fn from_name(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "white" => Ok(Self::White),
            "pink" => Ok(Self::Pink),
            other => Err(err_bad_opt(format!("unknown noise color '{}'", other))),
        }
    }
}

#[derive(Clone, Debug)]
pub struct NoiseOpts {
    pub color: NoiseColor,
    pub seed: u64,
    pub volume: f32,
    pub pan: f32,
    pub attack_ms: u32,
    pub release_ms: u32,
}

impl Default for NoiseOpts {
    fn default() -> Self {
        Self {
            color: NoiseColor::White,
            seed: 0x5EED_A0D1_0000_0001,
            volume: 1.0,
            pan: 0.0,
            attack_ms: 2,
            release_ms: 4,
        }
    }
}

#[inline]
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let p = pan.clamp(-1.0, 1.0);
    // Constant-power curve using LUT: angle = (p + 1) * 0.125 turns (0..0.25 turn)
    let turn = (p + 1.0) * 0.125;
    (lut_cos(turn), lut_sin(turn))
}

#[inline]
fn envelope_gain(
    frame: u64,
    total_frames: u64,
    attack_f: u64,
    decay_f: u64,
    sustain: f32,
    release_f: u64,
) -> f32 {
    let rel_start = total_frames.saturating_sub(release_f);
    let adsr = if frame < attack_f && attack_f > 0 {
        (frame as f32) / (attack_f as f32)
    } else if frame < attack_f + decay_f && decay_f > 0 {
        let t = ((frame - attack_f) as f32) / (decay_f as f32);
        1.0 + (sustain - 1.0) * t
    } else {
        sustain
    };
    if frame >= rel_start && release_f > 0 {
        let rt = ((total_frames - frame) as f32) / (release_f as f32);
        adsr * rt.clamp(0.0, 1.0)
    } else {
        adsr
    }
}

pub fn synth_tone(
    freq_hz: f64,
    ms: u32,
    opts: &ToneOpts,
    budget_core: Option<&Arc<BudgetCore>>,
    deadline: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<SoundData, String> {
    if !freq_hz.is_finite() || freq_hz <= 0.0 || freq_hz > 24_000.0 {
        return Err(err_bad_opt("tone frequency must be finite and in (0, 24000] Hz"));
    }
    if ms == 0 || ms > MAX_AUDIO_DURATION_MS {
        return Err(err_limit(format!(
            "tone duration {} ms must be in 1..={}",
            ms, MAX_AUDIO_DURATION_MS
        )));
    }
    let total_frames = ((ms as u64).saturating_mul(ENGINE_RATE as u64)) / 1000;
    if total_frames == 0 {
        return Err(err_bad_opt("tone duration produced 0 frames"));
    }
    let byte_len = (total_frames as usize).saturating_mul(8);
    if byte_len > MAX_SOUND_BYTES {
        return Err(err_limit("tone buffer exceeds 64 MiB limit"));
    }

    let _tmp = temp_budget_guard(budget_core, byte_len)?;
    let attack_f = ((opts.attack_ms as u64) * (ENGINE_RATE as u64)) / 1000;
    let decay_f = ((opts.decay_ms as u64) * (ENGINE_RATE as u64)) / 1000;
    let release_f = ((opts.release_ms as u64) * (ENGINE_RATE as u64)) / 1000;
    let sustain = opts.sustain.clamp(0.0, 1.0);
    let vol = opts.volume.clamp(0.0, 4.0);
    let (gl, gr) = pan_gains(opts.pan);

    let mut out = Vec::with_capacity((total_frames as usize) * 2);
    let mut phase_q32: u64 = 0;
    let step_q32: u64 = ((freq_hz / (ENGINE_RATE as f64)) * 4_294_967_296.0).round() as u64;

    for i in 0..total_frames {
        if (i & 4095) == 0 {
            check_deadline(deadline)?;
        }
        let p = (phase_q32 as f32) * (1.0 / 4_294_967_296.0);
        let raw = match opts.waveform {
            Waveform::Sine => lut_sin(p),
            Waveform::Square => {
                if p < 0.5 { 1.0 } else { -1.0 }
            }
            Waveform::Triangle => {
                if p < 0.25 {
                    p * 4.0
                } else if p < 0.75 {
                    2.0 - p * 4.0
                } else {
                    p * 4.0 - 4.0
                }
            }
            Waveform::Saw => 2.0 * p - 1.0,
        };
        let env = envelope_gain(i, total_frames, attack_f, decay_f, sustain, release_f);
        let s = raw * env * vol;
        out.push(s * gl);
        out.push(s * gr);
        phase_q32 = phase_q32.wrapping_add(step_q32);
    }
    drop(_tmp);
    let digest = Digest::from_parts(&[
        b"synth:tone.v1|",
        &freq_hz.to_bits().to_le_bytes(),
        &ms.to_le_bytes(),
        &[opts.waveform as u8],
        &vol.to_bits().to_le_bytes(),
        &opts.pan.to_bits().to_le_bytes(),
        &opts.attack_ms.to_le_bytes(),
        &opts.decay_ms.to_le_bytes(),
        &sustain.to_bits().to_le_bytes(),
        &opts.release_ms.to_le_bytes(),
    ]);
    SoundData::from_pcm(out, None, Some(digest), budget_core)
}

struct Pcg32 {
    state: u64,
    inc: u64,
}

impl Pcg32 {
    fn new(seed: u64) -> Self {
        let mut p = Self {
            state: 0,
            inc: (seed << 1) | 1,
        };
        let _ = p.next_u32();
        p.state = p.state.wrapping_add(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let _ = p.next_u32();
        p
    }

    #[inline]
    fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(6364136223846793005).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    #[inline]
    fn next_f32_signed(&mut self) -> f32 {
        let u = self.next_u32();
        ((u as f32) * (1.0 / 2_147_483_648.0)) - 1.0
    }
}

pub fn synth_noise(
    ms: u32,
    opts: &NoiseOpts,
    budget_core: Option<&Arc<BudgetCore>>,
    deadline: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<SoundData, String> {
    if ms == 0 || ms > MAX_AUDIO_DURATION_MS {
        return Err(err_limit(format!(
            "noise duration {} ms must be in 1..={}",
            ms, MAX_AUDIO_DURATION_MS
        )));
    }
    let total_frames = ((ms as u64).saturating_mul(ENGINE_RATE as u64)) / 1000;
    if total_frames == 0 {
        return Err(err_bad_opt("noise duration produced 0 frames"));
    }
    let byte_len = (total_frames as usize).saturating_mul(8);
    if byte_len > MAX_SOUND_BYTES {
        return Err(err_limit("noise buffer exceeds 64 MiB limit"));
    }
    let _tmp = temp_budget_guard(budget_core, byte_len)?;
    let attack_f = ((opts.attack_ms as u64) * (ENGINE_RATE as u64)) / 1000;
    let release_f = ((opts.release_ms as u64) * (ENGINE_RATE as u64)) / 1000;
    let vol = opts.volume.clamp(0.0, 4.0);
    let (gl, gr) = pan_gains(opts.pan);

    let mut rng = Pcg32::new(opts.seed);
    let mut b0 = 0.0f32;
    let mut b1 = 0.0f32;
    let mut b2 = 0.0f32;
    let mut b3 = 0.0f32;
    let mut b4 = 0.0f32;
    let mut b5 = 0.0f32;
    let mut b6 = 0.0f32;

    let mut out = Vec::with_capacity((total_frames as usize) * 2);
    for i in 0..total_frames {
        if (i & 4095) == 0 {
            check_deadline(deadline)?;
        }
        let white = rng.next_f32_signed();
        let raw = match opts.color {
            NoiseColor::White => white,
            NoiseColor::Pink => {
                // Paul Kellet deterministic 7-pole approximation
                b0 = 0.99886 * b0 + white * 0.0555179;
                b1 = 0.99332 * b1 + white * 0.0750759;
                b2 = 0.96900 * b2 + white * 0.1538520;
                b3 = 0.86650 * b3 + white * 0.3104856;
                b4 = 0.55000 * b4 + white * 0.5329522;
                b5 = -0.7616 * b5 - white * 0.0168980;
                let pink = (b0 + b1 + b2 + b3 + b4 + b5 + b6 + white * 0.5362) * 0.11;
                b6 = white * 0.115926;
                pink.clamp(-1.0, 1.0)
            }
        };
        let env = envelope_gain(i, total_frames, attack_f, 0, 1.0, release_f);
        let s = raw * env * vol;
        out.push(s * gl);
        out.push(s * gr);
    }
    drop(_tmp);
    let digest = Digest::from_parts(&[
        b"synth:noise.v1|",
        &ms.to_le_bytes(),
        &[opts.color as u8],
        &opts.seed.to_le_bytes(),
        &vol.to_bits().to_le_bytes(),
        &opts.pan.to_bits().to_le_bytes(),
    ]);
    SoundData::from_pcm(out, None, Some(digest), budget_core)
}

pub fn from_samples_f32(
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
    budget_core: Option<&Arc<BudgetCore>>,
    deadline: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<SoundData, String> {
    if sample_rate == 0 || sample_rate > 384_000 {
        return Err(err_bad_opt("sample_rate must be in 1..=384000 Hz"));
    }
    if channels != 1 && channels != 2 {
        return Err(err_bad_opt("channels must be 1 or 2"));
    }
    let ch = channels as usize;
    if samples.len() < ch || (samples.len() % ch) != 0 {
        return Err(err_bad_opt("sample array length does not match channel count"));
    }
    let frames = samples.len() / ch;
    let mut stereo = Vec::with_capacity(frames * 2);
    if ch == 1 {
        for &s in samples {
            let v = if s.is_finite() { s.clamp(-1.0, 1.0) } else { 0.0 };
            stereo.push(v);
            stereo.push(v);
        }
    } else {
        for pair in samples.chunks_exact(2) {
            let l = if pair[0].is_finite() { pair[0].clamp(-1.0, 1.0) } else { 0.0 };
            let r = if pair[1].is_finite() { pair[1].clamp(-1.0, 1.0) } else { 0.0 };
            stereo.push(l);
            stereo.push(r);
        }
    }
    let resampled = resample_stereo_linear(&stereo, sample_rate, ENGINE_RATE, deadline)?;
    SoundData::from_pcm(resampled, None, None, budget_core)
}

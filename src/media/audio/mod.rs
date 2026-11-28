pub mod mixer;
pub mod sound;
pub mod synth;
pub mod wav;

pub const ENGINE_RATE: u32 = 48_000;
pub const QUANTUM: usize = 240;
pub const RING_FRAMES: usize = 4 * QUANTUM;
pub const MAX_VOICES: usize = 64;
pub const HARD_MAX_VOICES: usize = 256;
pub const MAX_MUSICS: usize = 8;
pub const MAX_BUSES: usize = 16;
pub const MAX_SOUND_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_FADE_MS: u32 = 60_000;
pub const MAX_AUDIO_DURATION_MS: u32 = 600_000;
pub const CMD_QUEUE_CAP: usize = 256;
pub const EVT_QUEUE_CAP: usize = 256;
pub const ENGINE_JOIN_TIMEOUT_MS: u64 = 250;

#[inline]
pub fn err_backpressure(msg: impl AsRef<str>) -> String {
    format!("E_AUDIO_BACKPRESSURE: {}", msg.as_ref())
}

#[inline]
pub fn err_stream_unsupported(msg: impl AsRef<str>) -> String {
    format!("E_AUDIO_STREAM_UNSUPPORTED: {}", msg.as_ref())
}

#[inline]
pub fn err_audio_nondet(msg: impl AsRef<str>) -> String {
    format!("E_AUDIO_NONDET: {}", msg.as_ref())
}

#[inline]
pub fn err_cap_audio(msg: impl AsRef<str>) -> String {
    format!("E_CAP_AUDIO: {}", msg.as_ref())
}

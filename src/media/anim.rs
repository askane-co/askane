use std::cell::RefCell;
use std::rc::Rc;

use crate::media::image::MediaImage;
use crate::media::{MAX_ASSET_HANDLES, err_bad_opt, err_limit};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimMode {
    Loop,
    Once,
    PingPong,
}

impl AnimMode {
    pub fn from_name(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "loop" => Ok(AnimMode::Loop),
            "once" | "clamp" => Ok(AnimMode::Once),
            "pingpong" | "ping_pong" => Ok(AnimMode::PingPong),
            other => Err(err_bad_opt(format!("unknown animation mode '{}'", other))),
        }
    }
}

#[derive(Clone)]
pub enum AnimSource {
    Frames(Vec<Rc<MediaImage>>),
    Sheet {
        image: Rc<MediaImage>,
        cols: u32,
        rows: u32,
        indices: Vec<u32>,
    },
    Atlas {
        image: Rc<MediaImage>,
        rects: Vec<[u32; 4]>,
    },
}

impl AnimSource {
    #[inline]
    pub fn len(&self) -> usize {
        match self {
            AnimSource::Frames(v) => v.len(),
            AnimSource::Sheet { indices, .. } => indices.len(),
            AnimSource::Atlas { rects, .. } => rects.len(),
        }
    }

    pub fn resolve_frame(&self, frame_idx: usize) -> Option<(Rc<MediaImage>, [i32; 4])> {
        match self {
            AnimSource::Frames(v) => {
                let im = v.get(frame_idx)?;
                Some((
                    Rc::clone(im),
                    [0, 0, im.width as i32, im.height as i32],
                ))
            }
            AnimSource::Sheet {
                image,
                cols,
                rows,
                indices,
            } => {
                let cell = *indices.get(frame_idx)?;
                let fw = (image.width / *cols) as i32;
                let fh = (image.height / *rows) as i32;
                let col = (cell % *cols) as i32;
                let row = (cell / *cols) as i32;
                Some((Rc::clone(image), [col * fw, row * fh, fw, fh]))
            }
            AnimSource::Atlas { image, rects } => {
                let r = *rects.get(frame_idx)?;
                Some((
                    Rc::clone(image),
                    [r[0] as i32, r[1] as i32, r[2] as i32, r[3] as i32],
                ))
            }
        }
    }
}

#[derive(Clone)]
pub struct SpriteAnim {
    pub source: AnimSource,
    pub durations_ms: Vec<u32>,
    pub mode: AnimMode,
    pub current: usize,
    pub accum_ms: u64,
    pub ping_dir: i8,
    pub finished: bool,
}

impl SpriteAnim {
    pub fn new(
        source: AnimSource,
        durations_ms: Vec<u32>,
        mode: AnimMode,
    ) -> Result<Self, String> {
        let count = source.len();
        if count == 0 {
            return Err(err_bad_opt("animation must contain at least 1 frame"));
        }
        if durations_ms.len() != count || durations_ms.iter().any(|&d| d == 0) {
            return Err(err_bad_opt(
                "animation frame durations must match frame count and be > 0 ms",
            ));
        }
        Ok(Self {
            source,
            durations_ms,
            mode,
            current: 0,
            accum_ms: 0,
            ping_dir: 1,
            finished: false,
        })
    }

    pub fn advance(&mut self, dt_ms: u64) -> usize {
        if self.finished || self.durations_ms.len() <= 1 || dt_ms == 0 {
            return self.current;
        }
        let n = self.durations_ms.len();
        let total_cycle: u64 = match self.mode {
            AnimMode::Loop | AnimMode::Once => {
                self.durations_ms.iter().map(|&d| d as u64).sum::<u64>()
            }
            AnimMode::PingPong => {
                let fwd: u64 = self.durations_ms[..n - 1].iter().map(|&d| d as u64).sum();
                let bwd: u64 = self.durations_ms[1..].iter().map(|&d| d as u64).sum();
                (fwd + bwd).max(1)
            }
        };

        let mut rem = self.accum_ms.saturating_add(dt_ms);
        if self.mode != AnimMode::Once && total_cycle > 0 && rem >= total_cycle * 2 {
            rem %= total_cycle;
        }

        self.accum_ms = rem;
        while !self.finished {
            let dur = self.durations_ms[self.current] as u64;
            if self.accum_ms < dur {
                break;
            }
            self.accum_ms -= dur;
            match self.mode {
                AnimMode::Loop => {
                    self.current = (self.current + 1) % n;
                }
                AnimMode::Once => {
                    if self.current + 1 < n {
                        self.current += 1;
                    } else {
                        self.accum_ms = 0;
                        self.finished = true;
                        break;
                    }
                }
                AnimMode::PingPong => {
                    if self.ping_dir >= 0 {
                        if self.current + 1 < n {
                            self.current += 1;
                        } else {
                            self.ping_dir = -1;
                            if self.current > 0 {
                                self.current -= 1;
                            }
                        }
                    } else if self.current > 0 {
                        self.current -= 1;
                    } else {
                        self.ping_dir = 1;
                        if n > 1 {
                            self.current = 1;
                        }
                    }
                }
            }
        }
        self.current
    }
}

const ANIM_HANDLE_BASE: i64 = 30_000;

thread_local! {
    static ANIM_BANK: RefCell<Vec<Option<SpriteAnim>>> = const { RefCell::new(Vec::new()) };
}

pub fn reset_anims() {
    ANIM_BANK.with(|b| b.borrow_mut().clear());
}

pub fn insert_anim(anim: SpriteAnim) -> Result<i64, String> {
    ANIM_BANK.with(|bank| {
        let mut list = bank.borrow_mut();
        let active = list.iter().filter(|s| s.is_some()).count();
        if active >= MAX_ASSET_HANDLES {
            return Err(err_limit(format!(
                "maximum sprite animations ({}) exceeded",
                MAX_ASSET_HANDLES
            )));
        }
        let slot = if let Some(pos) = list.iter().position(|s| s.is_none()) {
            list[pos] = Some(anim);
            pos
        } else {
            list.push(Some(anim));
            list.len() - 1
        };
        Ok(ANIM_HANDLE_BASE + slot as i64)
    })
}

pub fn update_anim(handle: i64, dt_ms: u64) -> Result<i64, String> {
    let slot = (handle - ANIM_HANDLE_BASE) as usize;
    if handle < ANIM_HANDLE_BASE {
        return Err(err_bad_opt(format!("invalid animation handle {}", handle)));
    }
    ANIM_BANK.with(|bank| {
        let mut list = bank.borrow_mut();
        let anim = list
            .get_mut(slot)
            .and_then(|s| s.as_mut())
            .ok_or_else(|| err_bad_opt(format!("invalid or freed animation handle {}", handle)))?;
        Ok(anim.advance(dt_ms) as i64)
    })
}

pub fn get_anim_frame_idx(handle: i64) -> Result<i64, String> {
    let slot = (handle - ANIM_HANDLE_BASE) as usize;
    if handle < ANIM_HANDLE_BASE {
        return Err(err_bad_opt(format!("invalid animation handle {}", handle)));
    }
    ANIM_BANK.with(|bank| {
        let list = bank.borrow();
        let anim = list
            .get(slot)
            .and_then(|s| s.as_ref())
            .ok_or_else(|| err_bad_opt(format!("invalid or freed animation handle {}", handle)))?;
        Ok(anim.current as i64)
    })
}

pub fn get_anim_current_draw(handle: i64) -> Result<(Rc<MediaImage>, [i32; 4]), String> {
    let slot = (handle - ANIM_HANDLE_BASE) as usize;
    if handle < ANIM_HANDLE_BASE {
        return Err(err_bad_opt(format!("invalid animation handle {}", handle)));
    }
    ANIM_BANK.with(|bank| {
        let list = bank.borrow();
        let anim = list
            .get(slot)
            .and_then(|s| s.as_ref())
            .ok_or_else(|| err_bad_opt(format!("invalid or freed animation handle {}", handle)))?;
        anim.source
            .resolve_frame(anim.current)
            .ok_or_else(|| err_bad_opt("animation frame index out of range"))
    })
}

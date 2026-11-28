use std::sync::Arc;

use crate::budget::BudgetCore;
use crate::media::audio::synth::{lut_cos, lut_sin};
use crate::media::{MediaBudgetGuard, err_bad_opt, err_limit};
use crate::surface::{ClipRect, fill_circle_clipped, put_pixel_clipped};

pub const MAX_PARTICLES: usize = 65_536;
pub const DEFAULT_PARTICLE_SEED: u64 = 0x5EED_5EED_CAFE_BABE;

/// Deterministic PCG32 PRNG (identical output across all OS/CPU architectures).
#[derive(Clone, Debug)]
pub struct Pcg32 {
    state: u64,
    inc: u64,
}

impl Pcg32 {
    pub fn from_seed(seed: u64) -> Self {
        let mut rng = Self {
            state: 0,
            inc: 0xda3e_39cb_94b9_5bdb | 1,
        };
        let _ = rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        let _ = rng.next_u32();
        rng
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old
            .wrapping_mul(6364_1362_2384_6793_005)
            .wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        let v = self.next_u32() >> 8;
        (v as f32) * (1.0 / 16_777_216.0)
    }

    #[inline]
    pub fn range_f32(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Particle {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub life: f32,
    pub max_life: f32,
    pub size: f32,
    pub color: u32,
    pub fade: bool,
}

#[derive(Debug)]
pub struct ParticleSystem {
    pub rng: Pcg32,
    pub max_particles: usize,
    pub gravity_x: f32,
    pub gravity_y: f32,
    pub particles: Vec<Particle>,
    #[allow(dead_code)]
    pub budget: Option<MediaBudgetGuard>,
}

impl ParticleSystem {
    pub fn new(
        max_particles: usize,
        seed: Option<u64>,
        budget_core: Option<&Arc<BudgetCore>>,
    ) -> Result<Self, String> {
        if max_particles == 0 || max_particles > MAX_PARTICLES {
            return Err(err_limit(format!(
                "particle pool size {} out of 1..={}",
                max_particles, MAX_PARTICLES
            )));
        }
        let bytes = max_particles.saturating_mul(std::mem::size_of::<Particle>());
        let guard = MediaBudgetGuard::charge(budget_core, bytes)?;
        Ok(Self {
            rng: Pcg32::from_seed(seed.unwrap_or(DEFAULT_PARTICLE_SEED)),
            max_particles,
            gravity_x: 0.0,
            gravity_y: 0.0,
            particles: Vec::with_capacity(max_particles.min(1024)),
            budget: Some(guard),
        })
    }

    pub fn set_seed(&mut self, seed: u64) {
        self.rng = Pcg32::from_seed(seed);
    }

    pub fn set_gravity(&mut self, gx: f32, gy: f32) -> Result<(), String> {
        if !gx.is_finite() || !gy.is_finite() {
            return Err(err_bad_opt("particle gravity must be finite"));
        }
        self.gravity_x = gx;
        self.gravity_y = gy;
        Ok(())
    }

    pub fn clear(&mut self) {
        self.particles.clear();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        &mut self,
        x: f32,
        y: f32,
        vx: f32,
        vy: f32,
        life: f32,
        size: f32,
        color: u32,
        fade: bool,
    ) -> Result<bool, String> {
        if !x.is_finite()
            || !y.is_finite()
            || !vx.is_finite()
            || !vy.is_finite()
            || !life.is_finite()
            || life <= 0.0
            || !size.is_finite()
            || size <= 0.0
        {
            return Err(err_bad_opt("particle spawn parameters must be positive/finite"));
        }
        if self.particles.len() >= self.max_particles {
            return Ok(false);
        }
        self.particles.push(Particle {
            x,
            y,
            vx,
            vy,
            life,
            max_life: life,
            size,
            color,
            fade,
        });
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn emit_burst(
        &mut self,
        x: f32,
        y: f32,
        count: usize,
        speed_min: f32,
        speed_max: f32,
        angle_rad: f32,
        spread_rad: f32,
        life_min: f32,
        life_max: f32,
        size: f32,
        color: u32,
        fade: bool,
    ) -> Result<usize, String> {
        if !x.is_finite()
            || !y.is_finite()
            || !speed_min.is_finite()
            || !speed_max.is_finite()
            || !angle_rad.is_finite()
            || !spread_rad.is_finite()
            || !life_min.is_finite()
            || !life_max.is_finite()
            || life_max <= 0.0
            || !size.is_finite()
            || size <= 0.0
        {
            return Err(err_bad_opt("particle emit parameters must be valid finite values"));
        }
        let s_lo = speed_min.min(speed_max);
        let s_hi = speed_min.max(speed_max);
        let l_lo = life_min.max(0.001).min(life_max.max(0.001));
        let l_hi = life_min.max(life_max).max(0.001);
        let two_pi = 2.0 * std::f32::consts::PI;

        let mut spawned = 0usize;
        let to_spawn = count.min(MAX_PARTICLES);
        for _ in 0..to_spawn {
            if self.particles.len() >= self.max_particles {
                break;
            }
            let a = angle_rad + self.rng.range_f32(-spread_rad * 0.5, spread_rad * 0.5);
            let turn = a / two_pi;
            let spd = self.rng.range_f32(s_lo, s_hi);
            let life = self.rng.range_f32(l_lo, l_hi);
            self.particles.push(Particle {
                x,
                y,
                vx: lut_cos(turn) * spd,
                vy: lut_sin(turn) * spd,
                life,
                max_life: life,
                size,
                color,
                fade,
            });
            spawned += 1;
        }
        Ok(spawned)
    }

    pub fn update(&mut self, dt: f32) -> Result<usize, String> {
        if !dt.is_finite() || dt < 0.0 {
            return Err(err_bad_opt("particles.update(dt) requires finite dt >= 0"));
        }
        if dt == 0.0 {
            return Ok(self.particles.len());
        }
        for p in &mut self.particles {
            p.vx += self.gravity_x * dt;
            p.vy += self.gravity_y * dt;
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            p.life -= dt;
        }
        self.particles.retain(|p| p.life > 0.0);
        Ok(self.particles.len())
    }

    pub fn draw(&self, buf: &mut [u32], bw: i32, _bh: i32, clip: ClipRect) {
        if bw <= 0 {
            return;
        }
        for p in &self.particles {
            let base_a = ((p.color >> 24) & 0xFF) as f32;
            let a_factor = if p.fade {
                (p.life / p.max_life.max(1e-6)).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let alpha = ((if base_a == 0.0 { 255.0 } else { base_a }) * a_factor)
                .round()
                .clamp(0.0, 255.0) as u32;
            if alpha == 0 {
                continue;
            }
            let cx = p.x.round() as i32;
            let cy = p.y.round() as i32;
            let r = (p.size * 0.5).round() as i32;
            let rgb = p.color & 0x00FF_FFFF;
            if r <= 0 {
                if alpha >= 255 {
                    put_pixel_clipped(buf, bw, clip, cx, cy, 0xFF00_0000 | rgb);
                } else {
                    fill_circle_clipped(buf, bw, clip, cx, cy, 1, rgb, alpha, false);
                }
            } else {
                fill_circle_clipped(buf, bw, clip, cx, cy, r, rgb, alpha, false);
            }
        }
    }
}

/// 2D Camera with zoom, rotation, viewport center pivot, and deterministic linear-decay shake.
#[derive(Clone, Copy, Debug)]
pub struct Camera2D {
    pub enabled: bool,
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
    pub angle: f64,
    pub shake_intensity: f64,
    pub shake_duration: f64,
    pub shake_remaining: f64,
    pub shake_offset_x: f64,
    pub shake_offset_y: f64,
    pub shake_rng: u64,
}

const DEFAULT_SHAKE_SEED: u64 = 0xA5A4_E202_0502_0001;

impl Default for Camera2D {
    fn default() -> Self {
        Self {
            enabled: false,
            x: 0.0,
            y: 0.0,
            zoom: 1.0,
            angle: 0.0,
            shake_intensity: 0.0,
            shake_duration: 0.0,
            shake_remaining: 0.0,
            shake_offset_x: 0.0,
            shake_offset_y: 0.0,
            shake_rng: DEFAULT_SHAKE_SEED,
        }
    }
}

impl Camera2D {
    #[inline]
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    #[inline]
    pub fn is_identity_rect(&self) -> bool {
        !self.enabled
            && self.shake_offset_x == 0.0
            && self.shake_offset_y == 0.0
            && self.angle == 0.0
            && self.zoom == 1.0
    }

    pub fn set(&mut self, x: f64, y: f64, zoom: f64, angle: f64) -> Result<(), String> {
        if !x.is_finite() || !y.is_finite() || !zoom.is_finite() || !angle.is_finite() {
            return Err("gfx.set_camera() expects finite numeric arguments".to_string());
        }
        if zoom <= 1e-6 || zoom > 64.0 {
            return Err("gfx.set_camera() zoom must be in (1e-6, 64.0]".to_string());
        }
        self.enabled = true;
        self.x = x;
        self.y = y;
        self.zoom = zoom;
        self.angle = angle;
        Ok(())
    }

    pub fn set_shake(&mut self, intensity: f64, duration: f64, seed: Option<u64>) -> Result<(), String> {
        if !intensity.is_finite() || !duration.is_finite() || intensity < 0.0 || duration < 0.0 {
            return Err("gfx.set_camera_shake() intensity and duration must be >= 0".to_string());
        }
        if let Some(s) = seed {
            self.shake_rng = s;
        }
        if intensity == 0.0 || duration == 0.0 {
            self.shake_intensity = 0.0;
            self.shake_duration = 0.0;
            self.shake_remaining = 0.0;
            self.shake_offset_x = 0.0;
            self.shake_offset_y = 0.0;
            return Ok(());
        }
        self.shake_intensity = intensity;
        self.shake_duration = duration;
        self.shake_remaining = duration;
        self.sample_shake_offset();
        Ok(())
    }

    /// Advances shake with deterministic linear decay. Returns current shake offset.
    pub fn tick_shake(&mut self, dt: f64) -> (f64, f64) {
        if self.shake_remaining <= 0.0 || self.shake_duration <= 0.0 {
            self.shake_remaining = 0.0;
            self.shake_offset_x = 0.0;
            self.shake_offset_y = 0.0;
            return (0.0, 0.0);
        }
        let step = dt.clamp(0.0, 1.0);
        self.shake_remaining = (self.shake_remaining - step).max(0.0);
        if self.shake_remaining <= 0.0 {
            self.shake_offset_x = 0.0;
            self.shake_offset_y = 0.0;
        } else {
            self.sample_shake_offset();
        }
        (self.shake_offset_x, self.shake_offset_y)
    }

    #[inline]
    pub fn current_shake_amp(&self) -> f64 {
        if self.shake_duration <= 0.0 || self.shake_remaining <= 0.0 {
            0.0
        } else {
            self.shake_intensity * (self.shake_remaining / self.shake_duration)
        }
    }

    fn next_unit_signed(&mut self) -> f64 {
        self.shake_rng = self
            .shake_rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let u = ((self.shake_rng >> 11) as f64) / ((1u64 << 53) as f64);
        u * 2.0 - 1.0
    }

    fn sample_shake_offset(&mut self) {
        let amp = self.current_shake_amp();
        self.shake_offset_x = self.next_unit_signed() * amp;
        self.shake_offset_y = self.next_unit_signed() * amp;
    }

    /// World `(wx, wy)` -> Screen `(sx, sy)`:
    /// `center + R(-angle) * ((world - cam_pos) * zoom) + shake_offset`.
    #[inline]
    pub fn world_to_screen(&self, wx: f64, wy: f64, view_w: i32, view_h: i32) -> (f64, f64) {
        if !self.enabled {
            return (wx + self.shake_offset_x, wy + self.shake_offset_y);
        }
        let cx = (view_w as f64) * 0.5;
        let cy = (view_h as f64) * 0.5;
        let dx = (wx - self.x) * self.zoom;
        let dy = (wy - self.y) * self.zoom;
        let (s, c) = (-self.angle).sin_cos();
        let rx = dx * c - dy * s;
        let ry = dx * s + dy * c;
        (cx + rx + self.shake_offset_x, cy + ry + self.shake_offset_y)
    }

    /// Screen `(sx, sy)` -> World `(wx, wy)` exact inverse of `world_to_screen`.
    #[inline]
    pub fn screen_to_world(&self, sx: f64, sy: f64, view_w: i32, view_h: i32) -> (f64, f64) {
        if !self.enabled {
            return (sx - self.shake_offset_x, sy - self.shake_offset_y);
        }
        let cx = (view_w as f64) * 0.5;
        let cy = (view_h as f64) * 0.5;
        let rx = sx - cx - self.shake_offset_x;
        let ry = sy - cy - self.shake_offset_y;
        let (s, c) = self.angle.sin_cos();
        let dx = (rx * c - ry * s) / self.zoom;
        let dy = (rx * s + ry * c) / self.zoom;
        (self.x + dx, self.y + dy)
    }
}

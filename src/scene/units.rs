#[allow(dead_code)]
pub const PIXEL_FORMAT_ARGB8888: &str = "argb8888";
pub const DEPTH_CLEAR_VALUE: f32 = 1.0;
pub const EPSILON_F32: f32 = 1e-6;

pub const DEG_TO_RAD_F64: f64 = std::f64::consts::PI / 180.0;
pub const RAD_TO_DEG_F64: f64 = 180.0 / std::f64::consts::PI;

#[inline]
pub fn deg_to_rad(deg: f64) -> f64 {
    deg * DEG_TO_RAD_F64
}

#[inline]
pub fn rad_to_deg(rad: f64) -> f64 {
    rad * RAD_TO_DEG_F64
}

#[inline]
pub fn ensure_finite_f32(v: f32, label: &str) -> Result<f32, String> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(format!("{} must be a finite number", label))
    }
}

#[inline]
pub fn ensure_finite_f64(v: f64, label: &str) -> Result<f64, String> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(format!("{} must be a finite number", label))
    }
}

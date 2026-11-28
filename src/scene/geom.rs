use crate::scene::units::EPSILON_F32;

/// 2D vector in screen/surface or 2D world space (+X right, +Y down in 2D raster targets).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    #[inline]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    #[inline]
    pub const fn zero() -> Self {
        Self { x: 0.0, y: 0.0 }
    }

    #[inline]
    pub fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y)
    }

    #[inline]
    pub fn scale(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s)
    }

    #[inline]
    pub fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y
    }

    #[inline]
    pub fn cross(self, o: Self) -> f32 {
        self.x * o.y - self.y * o.x
    }

    #[inline]
    pub fn len_sq(self) -> f32 {
        self.dot(self)
    }

    #[inline]
    pub fn len(self) -> f32 {
        self.len_sq().sqrt()
    }

    #[inline]
    pub fn dist(self, o: Self) -> f32 {
        self.sub(o).len()
    }

    #[inline]
    pub fn normalize(self) -> Self {
        let l = self.len();
        if l <= EPSILON_F32 {
            Self::zero()
        } else {
            self.scale(1.0 / l)
        }
    }

    #[inline]
    pub fn lerp(self, o: Self, t: f32) -> Self {
        Self::new(self.x + (o.x - self.x) * t, self.y + (o.y - self.y) * t)
    }

    #[inline]
    pub fn rotate(self, rad: f32) -> Self {
        let (s, c) = (rad as f64).sin_cos();
        let x = (self.x as f64) * c - (self.y as f64) * s;
        let y = (self.x as f64) * s + (self.y as f64) * c;
        Self::new(x as f32, y as f32)
    }

    #[inline]
    pub fn rot90(self, steps: i32) -> Self {
        match steps.rem_euclid(4) {
            0 => self,
            1 => Self::new(-self.y, self.x),
            2 => Self::new(-self.x, -self.y),
            3 => Self::new(self.y, -self.x),
            _ => unreachable!(),
        }
    }

    #[inline]
    pub fn reflect(self, n: Self) -> Self {
        self.sub(n.scale(2.0 * self.dot(n)))
    }

    #[inline]
    pub fn project(self, onto: Self) -> Self {
        let d = onto.len_sq();
        if d <= EPSILON_F32 {
            Self::zero()
        } else {
            onto.scale(self.dot(onto) / d)
        }
    }
}

/// 3D vector in right-handed Y-up, -Z forward coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    #[inline]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    #[inline]
    pub const fn zero() -> Self {
        Self::new(0.0, 0.0, 0.0)
    }

    #[inline]
    pub const fn one() -> Self {
        Self::new(1.0, 1.0, 1.0)
    }

    #[inline]
    pub const fn unit_x() -> Self {
        Self::new(1.0, 0.0, 0.0)
    }

    #[inline]
    pub const fn unit_y() -> Self {
        Self::new(0.0, 1.0, 0.0)
    }

    #[inline]
    pub const fn unit_z() -> Self {
        Self::new(0.0, 0.0, 1.0)
    }

    #[inline]
    pub const fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }

    #[inline]
    pub const fn from_array(a: [f32; 3]) -> Self {
        Self::new(a[0], a[1], a[2])
    }

    #[inline]
    pub fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }

    #[inline]
    pub fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }

    #[inline]
    pub fn mul_comp(self, o: Self) -> Self {
        Self::new(self.x * o.x, self.y * o.y, self.z * o.z)
    }

    #[inline]
    pub fn scale(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }

    #[inline]
    pub fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    #[inline]
    pub fn cross(self, o: Self) -> Self {
        Self::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    #[inline]
    pub fn len_sq(self) -> f32 {
        self.dot(self)
    }

    #[inline]
    pub fn len(self) -> f32 {
        self.len_sq().sqrt()
    }

    #[inline]
    pub fn dist(self, o: Self) -> f32 {
        self.sub(o).len()
    }

    #[inline]
    pub fn normalize(self) -> Self {
        let l = self.len();
        if l <= EPSILON_F32 {
            Self::zero()
        } else {
            self.scale(1.0 / l)
        }
    }

    #[inline]
    pub fn lerp(self, o: Self, t: f32) -> Self {
        Self::new(
            self.x + (o.x - self.x) * t,
            self.y + (o.y - self.y) * t,
            self.z + (o.z - self.z) * t,
        )
    }

    #[inline]
    pub fn reflect(self, n: Self) -> Self {
        self.sub(n.scale(2.0 * self.dot(n)))
    }

    #[inline]
    pub fn project(self, onto: Self) -> Self {
        let d = onto.len_sq();
        if d <= EPSILON_F32 {
            Self::zero()
        } else {
            onto.scale(self.dot(onto) / d)
        }
    }

    #[inline]
    pub fn min(self, o: Self) -> Self {
        Self::new(self.x.min(o.x), self.y.min(o.y), self.z.min(o.z))
    }

    #[inline]
    pub fn max(self, o: Self) -> Self {
        Self::new(self.x.max(o.x), self.y.max(o.y), self.z.max(o.z))
    }

    #[inline]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

/// 4D homogeneous vector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Vec4 {
    #[inline]
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    #[inline]
    pub const fn from_array(a: [f32; 4]) -> Self {
        Self::new(a[0], a[1], a[2], a[3])
    }

    #[inline]
    pub const fn to_array(self) -> [f32; 4] {
        [self.x, self.y, self.z, self.w]
    }

    #[inline]
    pub fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z, self.w + o.w)
    }

    #[inline]
    pub fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z, self.w - o.w)
    }

    #[inline]
    pub fn scale(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s, self.w * s)
    }

    #[inline]
    pub fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z + self.w * o.w
    }

    #[inline]
    pub fn len_sq(self) -> f32 {
        self.dot(self)
    }

    #[inline]
    pub fn len(self) -> f32 {
        self.len_sq().sqrt()
    }

    #[inline]
    pub fn normalize(self) -> Self {
        let l = self.len();
        if l <= EPSILON_F32 {
            Self::new(0.0, 0.0, 0.0, 0.0)
        } else {
            self.scale(1.0 / l)
        }
    }

    #[inline]
    pub fn lerp(self, o: Self, t: f32) -> Self {
        Self::new(
            self.x + (o.x - self.x) * t,
            self.y + (o.y - self.y) * t,
            self.z + (o.z - self.z) * t,
            self.w + (o.w - self.w) * t,
        )
    }
}

/// Unit quaternion `(x, y, z, w)` for 3D rotations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    #[inline]
    pub const fn identity() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        }
    }

    #[inline]
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    #[inline]
    pub const fn from_array(a: [f32; 4]) -> Self {
        Self::new(a[0], a[1], a[2], a[3])
    }

    #[inline]
    pub const fn to_array(self) -> [f32; 4] {
        [self.x, self.y, self.z, self.w]
    }

    pub fn from_axis_angle(axis: Vec3, radians: f32) -> Self {
        let n = axis.normalize();
        if n.len_sq() <= EPSILON_F32 {
            return Self::identity();
        }
        let half = (radians as f64) * 0.5;
        let (s, c) = half.sin_cos();
        Self::new(
            (n.x as f64 * s) as f32,
            (n.y as f64 * s) as f32,
            (n.z as f64 * s) as f32,
            c as f32,
        )
        .normalize()
    }

    /// YXZ intrinsic Euler angles (yaw around Y, pitch around X, roll around Z) in radians.
    pub fn from_euler_yxz(yaw: f32, pitch: f32, roll: f32) -> Self {
        let qy = Self::from_axis_angle(Vec3::unit_y(), yaw);
        let qx = Self::from_axis_angle(Vec3::unit_x(), pitch);
        let qz = Self::from_axis_angle(Vec3::unit_z(), roll);
        qy.mul(qx).mul(qz).normalize()
    }

    #[inline]
    pub fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z + self.w * o.w
    }

    #[inline]
    pub fn normalize(self) -> Self {
        let l = self.dot(self).sqrt();
        if l <= EPSILON_F32 {
            Self::identity()
        } else {
            let inv = 1.0 / l;
            Self::new(self.x * inv, self.y * inv, self.z * inv, self.w * inv)
        }
    }

    #[inline]
    pub fn conjugate(self) -> Self {
        Self::new(-self.x, -self.y, -self.z, self.w)
    }

    #[inline]
    pub fn mul(self, o: Self) -> Self {
        Self::new(
            self.w * o.x + self.x * o.w + self.y * o.z - self.z * o.y,
            self.w * o.y - self.x * o.z + self.y * o.w + self.z * o.x,
            self.w * o.z + self.x * o.y - self.y * o.x + self.z * o.w,
            self.w * o.w - self.x * o.x - self.y * o.y - self.z * o.z,
        )
    }

    #[inline]
    pub fn rotate_vec3(self, v: Vec3) -> Vec3 {
        let qv = Vec3::new(self.x, self.y, self.z);
        let t = qv.cross(v).scale(2.0);
        v.add(t.scale(self.w)).add(qv.cross(t))
    }

    pub fn slerp(self, mut o: Self, t: f32) -> Self {
        let mut dot = self.dot(o) as f64;
        if dot < 0.0 {
            o = Self::new(-o.x, -o.y, -o.z, -o.w);
            dot = -dot;
        }
        if dot > 0.9995 {
            let out = Self::new(
                self.x + (o.x - self.x) * t,
                self.y + (o.y - self.y) * t,
                self.z + (o.z - self.z) * t,
                self.w + (o.w - self.w) * t,
            );
            return out.normalize();
        }
        let theta0 = dot.clamp(-1.0, 1.0).acos();
        let theta = theta0 * (t as f64);
        let sin_theta = theta.sin();
        let sin_theta0 = theta0.sin();
        let s0 = ((theta0 - theta).sin() / sin_theta0) as f32;
        let s1 = (sin_theta / sin_theta0) as f32;
        Self::new(
            self.x * s0 + o.x * s1,
            self.y * s0 + o.y * s1,
            self.z * s0 + o.z * s1,
            self.w * s0 + o.w * s1,
        )
        .normalize()
    }
}

/// Column-major 4x4 matrix (`m[col * 4 + row]`), targeting right-handed NDC `[0, 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4 {
    pub m: [f32; 16],
}

impl Mat4 {
    #[inline]
    pub const fn identity() -> Self {
        Self {
            m: [
                1.0, 0.0, 0.0, 0.0,
                0.0, 1.0, 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                0.0, 0.0, 0.0, 1.0,
            ],
        }
    }

    #[inline]
    pub const fn from_array(m: [f32; 16]) -> Self {
        Self { m }
    }

    #[inline]
    pub const fn to_array(self) -> [f32; 16] {
        self.m
    }

    #[inline]
    pub fn from_cols(c0: Vec4, c1: Vec4, c2: Vec4, c3: Vec4) -> Self {
        Self {
            m: [
                c0.x, c0.y, c0.z, c0.w,
                c1.x, c1.y, c1.z, c1.w,
                c2.x, c2.y, c2.z, c2.w,
                c3.x, c3.y, c3.z, c3.w,
            ],
        }
    }

    #[inline]
    pub fn row(&self, r: usize) -> Vec4 {
        Vec4::new(self.m[r], self.m[4 + r], self.m[8 + r], self.m[12 + r])
    }

    #[inline]
    pub fn col(&self, c: usize) -> Vec4 {
        let base = c * 4;
        Vec4::new(self.m[base], self.m[base + 1], self.m[base + 2], self.m[base + 3])
    }

    #[inline]
    pub fn translate(t: Vec3) -> Self {
        let mut m = Self::identity().m;
        m[12] = t.x;
        m[13] = t.y;
        m[14] = t.z;
        Self { m }
    }

    #[inline]
    pub fn scale(s: Vec3) -> Self {
        let mut m = [0.0f32; 16];
        m[0] = s.x;
        m[5] = s.y;
        m[10] = s.z;
        m[15] = 1.0;
        Self { m }
    }

    pub fn rotate_x(rad: f32) -> Self {
        let (s, c) = (rad as f64).sin_cos();
        let (s, c) = (s as f32, c as f32);
        Self::from_cols(
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, c, s, 0.0),
            Vec4::new(0.0, -s, c, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        )
    }

    pub fn rotate_y(rad: f32) -> Self {
        let (s, c) = (rad as f64).sin_cos();
        let (s, c) = (s as f32, c as f32);
        Self::from_cols(
            Vec4::new(c, 0.0, -s, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(s, 0.0, c, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        )
    }

    pub fn rotate_z(rad: f32) -> Self {
        let (s, c) = (rad as f64).sin_cos();
        let (s, c) = (s as f32, c as f32);
        Self::from_cols(
            Vec4::new(c, s, 0.0, 0.0),
            Vec4::new(-s, c, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        )
    }

    pub fn from_quat(q: Quat) -> Self {
        let q = q.normalize();
        let xx = q.x * q.x;
        let yy = q.y * q.y;
        let zz = q.z * q.z;
        let xy = q.x * q.y;
        let xz = q.x * q.z;
        let yz = q.y * q.z;
        let wx = q.w * q.x;
        let wy = q.w * q.y;
        let wz = q.w * q.z;
        Self::from_cols(
            Vec4::new(1.0 - 2.0 * (yy + zz), 2.0 * (xy + wz), 2.0 * (xz - wy), 0.0),
            Vec4::new(2.0 * (xy - wz), 1.0 - 2.0 * (xx + zz), 2.0 * (yz + wx), 0.0),
            Vec4::new(2.0 * (xz + wy), 2.0 * (yz - wx), 1.0 - 2.0 * (xx + yy), 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        )
    }

    pub fn from_trs(t: Vec3, r: Quat, s: Vec3) -> Self {
        let rot = Self::from_quat(r);
        let mut out = Self::from_cols(
            rot.col(0).scale(s.x),
            rot.col(1).scale(s.y),
            rot.col(2).scale(s.z),
            Vec4::new(t.x, t.y, t.z, 1.0),
        );
        out.m[3] = 0.0;
        out.m[7] = 0.0;
        out.m[11] = 0.0;
        out.m[15] = 1.0;
        out
    }

    pub fn mul_mat4(&self, o: &Self) -> Self {
        let mut out = [0.0f32; 16];
        for col in 0..4 {
            for row in 0..4 {
                out[col * 4 + row] = self.m[row] * o.m[col * 4]
                    + self.m[4 + row] * o.m[col * 4 + 1]
                    + self.m[8 + row] * o.m[col * 4 + 2]
                    + self.m[12 + row] * o.m[col * 4 + 3];
            }
        }
        Self { m: out }
    }

    #[inline]
    pub fn mul_vec4(&self, v: Vec4) -> Vec4 {
        Vec4::new(
            self.m[0] * v.x + self.m[4] * v.y + self.m[8] * v.z + self.m[12] * v.w,
            self.m[1] * v.x + self.m[5] * v.y + self.m[9] * v.z + self.m[13] * v.w,
            self.m[2] * v.x + self.m[6] * v.y + self.m[10] * v.z + self.m[14] * v.w,
            self.m[3] * v.x + self.m[7] * v.y + self.m[11] * v.z + self.m[15] * v.w,
        )
    }

    #[inline]
    pub fn transform_point3(&self, p: Vec3) -> Vec3 {
        let h = self.mul_vec4(Vec4::new(p.x, p.y, p.z, 1.0));
        if h.w.abs() > EPSILON_F32 && (h.w - 1.0).abs() > EPSILON_F32 {
            let inv_w = 1.0 / h.w;
            Vec3::new(h.x * inv_w, h.y * inv_w, h.z * inv_w)
        } else {
            Vec3::new(h.x, h.y, h.z)
        }
    }

    #[inline]
    pub fn transform_vector3(&self, v: Vec3) -> Vec3 {
        let h = self.mul_vec4(Vec4::new(v.x, v.y, v.z, 0.0));
        Vec3::new(h.x, h.y, h.z)
    }

    pub fn transpose(&self) -> Self {
        Self::from_cols(self.row(0), self.row(1), self.row(2), self.row(3))
    }

    pub fn determinant(&self) -> f32 {
        let m = &self.m;
        let a = m[0] * m[5] - m[1] * m[4];
        let b = m[0] * m[6] - m[2] * m[4];
        let c = m[0] * m[7] - m[3] * m[4];
        let d = m[1] * m[6] - m[2] * m[5];
        let e = m[1] * m[7] - m[3] * m[5];
        let f = m[2] * m[7] - m[3] * m[6];
        let g = m[8] * m[13] - m[9] * m[12];
        let h = m[8] * m[14] - m[10] * m[12];
        let i = m[8] * m[15] - m[11] * m[12];
        let j = m[9] * m[14] - m[10] * m[13];
        let k = m[9] * m[15] - m[11] * m[13];
        let l = m[10] * m[15] - m[11] * m[14];
        a * l - b * k + c * j + d * i - e * h + f * g
    }

    pub fn inverse(&self) -> Option<Self> {
        let m = &self.m;
        let a = m[0] * m[5] - m[1] * m[4];
        let b = m[0] * m[6] - m[2] * m[4];
        let c = m[0] * m[7] - m[3] * m[4];
        let d = m[1] * m[6] - m[2] * m[5];
        let e = m[1] * m[7] - m[3] * m[5];
        let f = m[2] * m[7] - m[3] * m[6];
        let g = m[8] * m[13] - m[9] * m[12];
        let h = m[8] * m[14] - m[10] * m[12];
        let i = m[8] * m[15] - m[11] * m[12];
        let j = m[9] * m[14] - m[10] * m[13];
        let k = m[9] * m[15] - m[11] * m[13];
        let l = m[10] * m[15] - m[11] * m[14];
        let det = a * l - b * k + c * j + d * i - e * h + f * g;
        if det.abs() <= 1e-12 || !det.is_finite() {
            return None;
        }
        let inv = 1.0 / det;
        Some(Self {
            m: [
                (m[5] * l - m[6] * k + m[7] * j) * inv,
                (-m[1] * l + m[2] * k - m[3] * j) * inv,
                (m[13] * f - m[14] * e + m[15] * d) * inv,
                (-m[9] * f + m[10] * e - m[11] * d) * inv,
                (-m[4] * l + m[6] * i - m[7] * h) * inv,
                (m[0] * l - m[2] * i + m[3] * h) * inv,
                (-m[12] * f + m[14] * c - m[15] * b) * inv,
                (m[8] * f - m[10] * c + m[11] * b) * inv,
                (m[4] * k - m[5] * i + m[7] * g) * inv,
                (-m[0] * k + m[1] * i - m[3] * g) * inv,
                (m[12] * e - m[13] * c + m[15] * a) * inv,
                (-m[8] * e + m[9] * c - m[11] * a) * inv,
                (-m[4] * j + m[5] * h - m[6] * g) * inv,
                (m[0] * j - m[1] * h + m[2] * g) * inv,
                (-m[12] * d + m[13] * b - m[14] * a) * inv,
                (m[8] * d - m[9] * b + m[10] * a) * inv,
            ],
        })
    }

    /// Right-handed look-at view matrix (+X right, +Y up, -Z forward).
    pub fn look_at_rh(eye: Vec3, target: Vec3, up: Vec3) -> Self {
        let f = target.sub(eye).normalize();
        let s = f.cross(up).normalize();
        let u = s.cross(f);
        Self::from_cols(
            Vec4::new(s.x, u.x, -f.x, 0.0),
            Vec4::new(s.y, u.y, -f.y, 0.0),
            Vec4::new(s.z, u.z, -f.z, 0.0),
            Vec4::new(-s.dot(eye), -u.dot(eye), f.dot(eye), 1.0),
        )
    }

    /// Right-handed perspective matrix mapping view `-z_near -> 0.0`, `-z_far -> 1.0`.
    pub fn perspective_rh_zo(fov_y_rad: f32, aspect: f32, z_near: f32, z_far: f32) -> Result<Self, String> {
        if !fov_y_rad.is_finite() || fov_y_rad <= 0.0 || fov_y_rad >= std::f32::consts::PI {
            return Err("perspective fov_y must be in (0, PI)".to_string());
        }
        if !aspect.is_finite() || aspect <= 0.0 {
            return Err("perspective aspect must be positive".to_string());
        }
        if !z_near.is_finite() || !z_far.is_finite() || z_near <= 0.0 || z_far <= z_near {
            return Err("perspective requires 0 < z_near < z_far".to_string());
        }
        let f = 1.0 / ((fov_y_rad as f64) * 0.5).tan() as f32;
        let denom = z_near - z_far;
        Ok(Self::from_cols(
            Vec4::new(f / aspect, 0.0, 0.0, 0.0),
            Vec4::new(0.0, f, 0.0, 0.0),
            Vec4::new(0.0, 0.0, z_far / denom, -1.0),
            Vec4::new(0.0, 0.0, (z_near * z_far) / denom, 0.0),
        ))
    }

    /// Right-handed orthographic matrix mapping view `-z_near -> 0.0`, `-z_far -> 1.0`.
    pub fn orthographic_rh_zo(
        left: f32,
        right: f32,
        bottom: f32,
        top: f32,
        z_near: f32,
        z_far: f32,
    ) -> Result<Self, String> {
        if (right - left).abs() <= EPSILON_F32
            || (top - bottom).abs() <= EPSILON_F32
            || (z_far - z_near).abs() <= EPSILON_F32
        {
            return Err("orthographic bounds cannot be degenerate".to_string());
        }
        let rl = 1.0 / (right - left);
        let tb = 1.0 / (top - bottom);
        let fn_inv = 1.0 / (z_far - z_near);
        Ok(Self::from_cols(
            Vec4::new(2.0 * rl, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 2.0 * tb, 0.0, 0.0),
            Vec4::new(0.0, 0.0, -fn_inv, 0.0),
            Vec4::new(
                -(right + left) * rl,
                -(top + bottom) * tb,
                -z_near * fn_inv,
                1.0,
            ),
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Transform {
    #[inline]
    pub const fn identity() -> Self {
        Self {
            position: Vec3::zero(),
            rotation: Quat::identity(),
            scale: Vec3::one(),
        }
    }

    #[inline]
    pub fn to_mat4(self) -> Mat4 {
        Mat4::from_trs(self.position, self.rotation, self.scale)
    }

    #[inline]
    pub fn transform_point(self, p: Vec3) -> Vec3 {
        self.rotation.rotate_vec3(p.mul_comp(self.scale)).add(self.position)
    }

    pub fn combine(self, child: Self) -> Self {
        Self {
            position: self.transform_point(child.position),
            rotation: self.rotation.mul(child.rotation).normalize(),
            scale: self.scale.mul_comp(child.scale),
        }
    }

    pub fn from_mat4(mat: &Mat4) -> Result<Self, String> {
        let m = &mat.m;
        if m.iter().any(|x| !x.is_finite()) {
            return Err("E_MESH_CORRUPT: transform matrix contains non-finite value".to_string());
        }
        let pos = Vec3::new(m[12], m[13], m[14]);
        let mut sx = Vec3::new(m[0], m[1], m[2]).len();
        let sy = Vec3::new(m[4], m[5], m[6]).len();
        let sz = Vec3::new(m[8], m[9], m[10]).len();
        if sx <= 1e-7 || sy <= 1e-7 || sz <= 1e-7 {
            return Err("E_MESH_CORRUPT: singular transform matrix scale".to_string());
        }
        let det3 = m[0] * (m[5] * m[10] - m[6] * m[9])
            - m[4] * (m[1] * m[10] - m[2] * m[9])
            + m[8] * (m[1] * m[6] - m[2] * m[5]);
        if det3 < 0.0 {
            sx = -sx;
        }
        let r00 = m[0] / sx;
        let r10 = m[1] / sx;
        let r20 = m[2] / sx;
        let r01 = m[4] / sy;
        let r11 = m[5] / sy;
        let r21 = m[6] / sy;
        let r02 = m[8] / sz;
        let r12 = m[9] / sz;
        let r22 = m[10] / sz;
        let trace = r00 + r11 + r22;
        let quat = if trace > 0.0 {
            let s = (trace + 1.0).sqrt() * 2.0;
            Quat::new((r21 - r12) / s, (r02 - r20) / s, (r10 - r01) / s, 0.25 * s)
        } else if r00 > r11 && r00 > r22 {
            let s = (1.0 + r00 - r11 - r22).sqrt() * 2.0;
            Quat::new(0.25 * s, (r01 + r10) / s, (r02 + r20) / s, (r21 - r12) / s)
        } else if r11 > r22 {
            let s = (1.0 + r11 - r00 - r22).sqrt() * 2.0;
            Quat::new((r01 + r10) / s, 0.25 * s, (r12 + r21) / s, (r02 - r20) / s)
        } else {
            let s = (1.0 + r22 - r00 - r11).sqrt() * 2.0;
            Quat::new((r02 + r20) / s, (r12 + r21) / s, 0.25 * s, (r10 - r01) / s)
        };
        Ok(Self {
            position: pos,
            rotation: quat.normalize(),
            scale: Vec3::new(sx, sy, sz),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    #[inline]
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self {
            min: min.min(max),
            max: min.max(max),
        }
    }

    #[inline]
    pub fn contains_point(self, p: Vec3) -> bool {
        p.x >= self.min.x
            && p.x <= self.max.x
            && p.y >= self.min.y
            && p.y <= self.max.y
            && p.z >= self.min.z
            && p.z <= self.max.z
    }

    #[inline]
    pub fn intersects(self, o: Self) -> bool {
        self.min.x <= o.max.x
            && self.max.x >= o.min.x
            && self.min.y <= o.max.y
            && self.max.y >= o.min.y
            && self.min.z <= o.max.z
            && self.max.z >= o.min.z
    }

    #[inline]
    pub fn center(self) -> Vec3 {
        self.min.add(self.max).scale(0.5)
    }

    #[inline]
    pub fn size(self) -> Vec3 {
        self.max.sub(self.min)
    }

    #[inline]
    pub fn expand_point(self, p: Vec3) -> Self {
        Self {
            min: self.min.min(p),
            max: self.max.max(p),
        }
    }

    pub fn transform(self, mat: &Mat4) -> Self {
        let xs = [self.min.x, self.max.x];
        let ys = [self.min.y, self.max.y];
        let zs = [self.min.z, self.max.z];
        let first = mat.transform_point3(Vec3::new(xs[0], ys[0], zs[0]));
        let mut out = Self { min: first, max: first };
        for &x in &xs {
            for &y in &ys {
                for &z in &zs {
                    out = out.expand_point(mat.transform_point3(Vec3::new(x, y, z)));
                }
            }
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

impl Ray {
    #[inline]
    pub fn new(origin: Vec3, dir: Vec3) -> Self {
        Self {
            origin,
            dir: dir.normalize(),
        }
    }

    #[inline]
    pub fn at(self, t: f32) -> Vec3 {
        self.origin.add(self.dir.scale(t))
    }

    pub fn intersect_aabb(self, b: Aabb) -> Option<f32> {
        let mut tmin = 0.0f32;
        let mut tmax = f32::INFINITY;
        let axes = [
            (self.origin.x, self.dir.x, b.min.x, b.max.x),
            (self.origin.y, self.dir.y, b.min.y, b.max.y),
            (self.origin.z, self.dir.z, b.min.z, b.max.z),
        ];
        for (orig, d, min_v, max_v) in axes {
            if d.abs() <= EPSILON_F32 {
                if orig < min_v || orig > max_v {
                    return None;
                }
            } else {
                let inv = 1.0 / d;
                let mut t1 = (min_v - orig) * inv;
                let mut t2 = (max_v - orig) * inv;
                if t1 > t2 {
                    std::mem::swap(&mut t1, &mut t2);
                }
                tmin = tmin.max(t1);
                tmax = tmax.min(t2);
                if tmin > tmax {
                    return None;
                }
            }
        }
        Some(tmin)
    }

    pub fn intersect_plane(self, plane: Plane) -> Option<f32> {
        let denom = plane.normal.dot(self.dir);
        if denom.abs() <= EPSILON_F32 {
            return None;
        }
        let t = -(plane.normal.dot(self.origin) + plane.d) / denom;
        if t >= 0.0 {
            Some(t)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub normal: Vec3,
    pub d: f32,
}

impl Plane {
    #[inline]
    pub fn from_vec4(v: Vec4) -> Self {
        let len = Vec3::new(v.x, v.y, v.z).len();
        if len <= EPSILON_F32 {
            Self {
                normal: Vec3::unit_y(),
                d: 0.0,
            }
        } else {
            let inv = 1.0 / len;
            Self {
                normal: Vec3::new(v.x * inv, v.y * inv, v.z * inv),
                d: v.w * inv,
            }
        }
    }

    #[inline]
    pub fn from_normal_point(normal: Vec3, point: Vec3) -> Self {
        let n = normal.normalize();
        Self {
            normal: n,
            d: -n.dot(point),
        }
    }

    #[inline]
    pub fn signed_distance(self, p: Vec3) -> f32 {
        self.normal.dot(p) + self.d
    }
}

/// Six inward-pointing planes extracted from a VP matrix targeting NDC `[0, 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frustum {
    pub planes: [Plane; 6],
}

impl Frustum {
    pub fn from_mat4_zo(vp: &Mat4) -> Self {
        let r0 = vp.row(0);
        let r1 = vp.row(1);
        let r2 = vp.row(2);
        let r3 = vp.row(3);
        Self {
            planes: [
                Plane::from_vec4(r3.add(r0)), // Left
                Plane::from_vec4(r3.sub(r0)), // Right
                Plane::from_vec4(r3.add(r1)), // Bottom
                Plane::from_vec4(r3.sub(r1)), // Top
                Plane::from_vec4(r2),         // Near (NDC z >= 0)
                Plane::from_vec4(r3.sub(r2)), // Far (NDC z <= 1)
            ],
        }
    }

    pub fn contains_point(&self, p: Vec3) -> bool {
        self.planes
            .iter()
            .all(|pl| pl.signed_distance(p) >= -EPSILON_F32)
    }

    pub fn intersects_aabb(&self, b: Aabb) -> bool {
        for pl in &self.planes {
            let px = if pl.normal.x >= 0.0 { b.max.x } else { b.min.x };
            let py = if pl.normal.y >= 0.0 { b.max.y } else { b.min.y };
            let pz = if pl.normal.z >= 0.0 { b.max.z } else { b.min.z };
            if pl.signed_distance(Vec3::new(px, py, pz)) < -EPSILON_F32 {
                return false;
            }
        }
        true
    }
}

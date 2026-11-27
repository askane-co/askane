use std::sync::OnceLock;

/// Exact IEC 61966-2-1 sRGB `[0..=255]` -> linear `u16` `[0..=65535]` lookup table.
pub const SRGB_TO_LINEAR_U16: [u16; 256] = [
    0, 20, 40, 60, 80, 99, 119, 139, 159, 179, 199, 219, 241, 264, 288, 313, 340, 367, 396, 427,
    458, 491, 526, 562, 599, 637, 677, 718, 761, 805, 851, 898, 947, 997, 1048, 1101, 1156, 1212,
    1270, 1330, 1391, 1453, 1517, 1583, 1651, 1720, 1790, 1863, 1937, 2013, 2090, 2170, 2250,
    2333, 2418, 2504, 2592, 2681, 2773, 2866, 2961, 3058, 3157, 3258, 3360, 3464, 3570, 3678,
    3788, 3900, 4014, 4129, 4247, 4366, 4488, 4611, 4736, 4864, 4993, 5124, 5257, 5392, 5530,
    5669, 5810, 5953, 6099, 6246, 6395, 6547, 6700, 6856, 7014, 7174, 7335, 7500, 7666, 7834,
    8004, 8177, 8352, 8528, 8708, 8889, 9072, 9258, 9445, 9635, 9828, 10022, 10219, 10417, 10619,
    10822, 11028, 11235, 11446, 11658, 11873, 12090, 12309, 12530, 12754, 12980, 13209, 13440,
    13673, 13909, 14146, 14387, 14629, 14874, 15122, 15371, 15623, 15878, 16135, 16394, 16656,
    16920, 17187, 17456, 17727, 18001, 18277, 18556, 18837, 19121, 19407, 19696, 19987, 20281,
    20577, 20876, 21177, 21481, 21787, 22096, 22407, 22721, 23038, 23357, 23678, 24002, 24329,
    24658, 24990, 25325, 25662, 26001, 26344, 26688, 27036, 27386, 27739, 28094, 28452, 28813,
    29176, 29542, 29911, 30282, 30656, 31033, 31412, 31794, 32179, 32567, 32957, 33350, 33745,
    34143, 34544, 34948, 35355, 35764, 36176, 36591, 37008, 37429, 37852, 38278, 38706, 39138,
    39572, 40009, 40449, 40891, 41337, 41785, 42236, 42690, 43147, 43606, 44069, 44534, 45002,
    45473, 45947, 46423, 46903, 47385, 47871, 48359, 48850, 49344, 49841, 50341, 50844, 51349,
    51858, 52369, 52884, 53401, 53921, 54445, 54971, 55500, 56032, 56567, 57105, 57646, 58190,
    58737, 59287, 59840, 60396, 60955, 61517, 62082, 62650, 63221, 63795, 64372, 64952, 65535,
];

static LINEAR_TO_SRGB_LUT: OnceLock<[u8; 4096]> = OnceLock::new();

/// Deterministic 12-bit linear `[0..4095]` -> sRGB `u8` `[0..=255]` LUT derived solely by integer inversion of `SRGB_TO_LINEAR_U16` (no `powf`).
pub fn linear_to_srgb_lut() -> &'static [u8; 4096] {
    LINEAR_TO_SRGB_LUT.get_or_init(|| {
        let mut table = [0u8; 4096];
        let mut s = 0usize;
        for (i, slot) in table.iter_mut().enumerate() {
            let target = ((i * 65535) + 2047) / 4095;
            while s + 1 < 256
                && ((SRGB_TO_LINEAR_U16[s + 1] as usize).abs_diff(target)
                    <= (SRGB_TO_LINEAR_U16[s] as usize).abs_diff(target))
            {
                s += 1;
            }
            *slot = s as u8;
        }
        table
    })
}

#[inline]
pub fn linear16_to_srgb8(v16: u16) -> u8 {
    let idx = ((v16 as usize) * 4095 + 32767) / 65535;
    linear_to_srgb_lut()[idx.min(4095)]
}

#[inline]
pub fn srgb8_to_linear8(v: u8) -> u8 {
    ((SRGB_TO_LINEAR_U16[v as usize] as u32 + 128) >> 8).min(255) as u8
}

#[inline]
pub fn linear8_to_srgb8(v: u8) -> u8 {
    let idx = ((v as usize) * 4095 + 127) / 255;
    linear_to_srgb_lut()[idx.min(4095)]
}

/// Premultiplies straight RGBA8 pixel buffer cleanly using pure integer rounding.
pub fn premultiply_rgba8(straight: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(straight.len());
    for px in straight.chunks_exact(4) {
        let a = px[3] as u32;
        if a == 255 {
            out.extend_from_slice(px);
        } else if a == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let r = ((px[0] as u32 * a + 127) / 255) as u8;
            let g = ((px[1] as u32 * a + 127) / 255) as u8;
            let b = ((px[2] as u32 * a + 127) / 255) as u8;
            out.extend_from_slice(&[r, g, b, px[3]]);
        }
    }
    out
}

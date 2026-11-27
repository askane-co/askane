use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::budget::BudgetCore;
use crate::media::formats::{self, RawDecodedImage};
use crate::media::ops::{convert_buffer, generate_mipmaps};
use crate::media::{
    Digest, MAX_DIM, MAX_GIF_FRAMES, MAX_SOURCE_BYTES, MediaBudgetGuard, check_deadline,
    err_bad_opt, err_limit, err_unsupported, validate_dims,
};
use crate::value::Value;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    R8 = 1,
    Rg8 = 2,
    Rgb8 = 3,
    Rgba8 = 4,
}

impl PixelFormat {
    #[inline]
    pub fn channels(self) -> usize {
        self as usize
    }

    #[inline]
    pub fn bpp(self) -> usize {
        self as usize
    }

    #[inline]
    pub fn has_alpha(self) -> bool {
        matches!(self, PixelFormat::Rgba8)
    }

    #[inline]
    pub fn name(self) -> &'static str {
        match self {
            PixelFormat::R8 => "r8",
            PixelFormat::Rg8 => "rg8",
            PixelFormat::Rgb8 => "rgb8",
            PixelFormat::Rgba8 => "rgba8",
        }
    }

    pub fn from_name(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "r8" | "gray" | "l8" => Ok(PixelFormat::R8),
            "rg8" => Ok(PixelFormat::Rg8),
            "rgb8" | "rgb" => Ok(PixelFormat::Rgb8),
            "rgba8" | "rgba" => Ok(PixelFormat::Rgba8),
            other => Err(err_bad_opt(format!("unknown pixel format '{}'", other))),
        }
    }

    pub fn byte_len(self, width: u32, height: u32) -> Result<usize, String> {
        let pixels = validate_dims(width, height, MAX_DIM)?;
        pixels
            .checked_mul(self.channels())
            .ok_or_else(|| err_limit("image byte size overflow"))
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorSpace {
    Srgb = 0,
    Linear = 1,
}

impl ColorSpace {
    #[inline]
    pub fn name(self) -> &'static str {
        match self {
            ColorSpace::Srgb => "srgb",
            ColorSpace::Linear => "linear",
        }
    }

    pub fn from_name(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "srgb" => Ok(ColorSpace::Srgb),
            "linear" => Ok(ColorSpace::Linear),
            other => Err(err_bad_opt(format!("unknown color space '{}'", other))),
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceFormat {
    Raw = 0,
    Png = 1,
    Bmp = 2,
    Tga = 3,
    Qoi = 4,
    Gif = 5,
    Jpeg = 6,
    Derived = 7,
    Procgen = 8,
}

impl SourceFormat {
    #[inline]
    pub fn name(self) -> &'static str {
        match self {
            SourceFormat::Raw => "raw",
            SourceFormat::Png => "png",
            SourceFormat::Bmp => "bmp",
            SourceFormat::Tga => "tga",
            SourceFormat::Qoi => "qoi",
            SourceFormat::Gif => "gif",
            SourceFormat::Jpeg => "jpeg",
            SourceFormat::Derived => "derived",
            SourceFormat::Procgen => "procgen",
        }
    }

    pub fn from_ext(ext: &str) -> Option<Self> {
        match ext.trim().to_ascii_lowercase().as_str() {
            "png" => Some(SourceFormat::Png),
            "bmp" => Some(SourceFormat::Bmp),
            "tga" => Some(SourceFormat::Tga),
            "qoi" => Some(SourceFormat::Qoi),
            "gif" => Some(SourceFormat::Gif),
            "jpg" | "jpeg" => Some(SourceFormat::Jpeg),
            _ => None,
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WrapMode {
    Clamp = 0,
    Repeat = 1,
    Mirror = 2,
}

impl WrapMode {
    #[inline]
    pub fn name(self) -> &'static str {
        match self {
            WrapMode::Clamp => "clamp",
            WrapMode::Repeat => "repeat",
            WrapMode::Mirror => "mirror",
        }
    }

    pub fn from_name(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "clamp" | "clamp_to_edge" => Ok(WrapMode::Clamp),
            "repeat" | "wrap" => Ok(WrapMode::Repeat),
            "mirror" | "mirrored_repeat" => Ok(WrapMode::Mirror),
            other => Err(err_bad_opt(format!("unknown wrap mode '{}'", other))),
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Filter {
    Nearest = 0,
    Linear = 1,
    MipNearest = 2,
    MipLinear = 3,
}

impl Filter {
    #[inline]
    pub fn name(self) -> &'static str {
        match self {
            Filter::Nearest => "nearest",
            Filter::Linear => "linear",
            Filter::MipNearest => "mip_nearest",
            Filter::MipLinear => "mip_linear",
        }
    }

    pub fn from_name(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "nearest" | "point" => Ok(Filter::Nearest),
            "linear" | "bilinear" => Ok(Filter::Linear),
            "mip_nearest" | "mipmap_nearest" => Ok(Filter::MipNearest),
            "mip_linear" | "trilinear" | "mipmap_linear" => Ok(Filter::MipLinear),
            other => Err(err_bad_opt(format!("unknown texture filter '{}'", other))),
        }
    }

    #[inline]
    pub fn uses_mipmaps(self) -> bool {
        matches!(self, Filter::MipNearest | Filter::MipLinear)
    }
}

#[derive(Clone, Debug)]
pub struct MipLevel {
    pub width: u32,
    pub height: u32,
    pub offset: usize,
    pub len: usize,
}

#[derive(Debug)]
pub struct MipChain {
    pub levels: Vec<MipLevel>,
    pub data: Rc<[u8]>,
    pub _budget: MediaBudgetGuard,
}

impl MipChain {
    pub fn level_slice(&self, idx: usize) -> Option<(u32, u32, &[u8])> {
        let lvl = self.levels.get(idx)?;
        let end = lvl.offset.saturating_add(lvl.len);
        let slice = self.data.get(lvl.offset..end)?;
        Some((lvl.width, lvl.height, slice))
    }

    #[inline]
    pub fn level_count(&self) -> usize {
        self.levels.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageMeta {
    pub source_format: SourceFormat,
    pub color_space: ColorSpace,
    pub has_alpha: bool,
    pub is_animated: bool,
    pub frame_count: u32,
    pub loop_count: u16,
    pub mip_count: u8,
}

#[derive(Clone, Debug, Default)]
pub struct DecodeOpts {
    pub format: Option<PixelFormat>,
    pub color_space: Option<ColorSpace>,
    pub mipmaps: Option<bool>,
    pub strict_crc: bool,
    pub max_frames: Option<usize>,
    pub max_dim: Option<u32>,
}

impl DecodeOpts {
    pub fn strict_default() -> Self {
        Self {
            format: None,
            color_space: None,
            mipmaps: None,
            strict_crc: true,
            max_frames: None,
            max_dim: None,
        }
    }
}

pub fn parse_decode_opts(val: Option<&Value>, ctx: &str) -> Result<DecodeOpts, String> {
    let mut opts = DecodeOpts::strict_default();
    let Some(v) = val else {
        return Ok(opts);
    };
    if matches!(v, Value::Unit) {
        return Ok(opts);
    }
    let Value::Dict(dict) = v else {
        return Err(err_bad_opt(format!(
            "{} options must be a dict, got '{}'",
            ctx,
            v.type_name()
        )));
    };
    let map = dict.borrow();
    for (k, fv) in map.iter() {
        match k.as_str() {
            "format" => {
                let s = fv
                    .as_str()
                    .ok_or_else(|| err_bad_opt(format!("{} option 'format' must be str", ctx)))?;
                opts.format = Some(PixelFormat::from_name(s)?);
            }
            "color_space" => {
                let s = fv.as_str().ok_or_else(|| {
                    err_bad_opt(format!("{} option 'color_space' must be str", ctx))
                })?;
                opts.color_space = Some(ColorSpace::from_name(s)?);
            }
            "mipmaps" => match fv {
                Value::Bool(b) => opts.mipmaps = Some(*b),
                _ => {
                    return Err(err_bad_opt(format!(
                        "{} option 'mipmaps' must be bool",
                        ctx
                    )));
                }
            },
            "strict_crc" => match fv {
                Value::Bool(b) => opts.strict_crc = *b,
                _ => {
                    return Err(err_bad_opt(format!(
                        "{} option 'strict_crc' must be bool",
                        ctx
                    )));
                }
            },
            "max_frames" => match fv {
                Value::Int(n) if *n >= 1 && (*n as usize) <= MAX_GIF_FRAMES => {
                    opts.max_frames = Some(*n as usize);
                }
                _ => {
                    return Err(err_bad_opt(format!(
                        "{} option 'max_frames' must be int in 1..={}",
                        ctx, MAX_GIF_FRAMES
                    )));
                }
            },
            "max_dim" => match fv {
                Value::Int(n) if *n >= 1 && (*n as u64) <= MAX_DIM as u64 => {
                    opts.max_dim = Some(*n as u32);
                }
                _ => {
                    return Err(err_bad_opt(format!(
                        "{} option 'max_dim' must be int in 1..={}",
                        ctx, MAX_DIM
                    )));
                }
            },
            unknown => {
                return Err(err_bad_opt(format!(
                    "{}: unknown option key '{}'",
                    ctx, unknown
                )));
            }
        }
    }
    Ok(opts)
}

#[derive(Debug)]
pub struct MediaImage {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub color_space: ColorSpace,
    pub source_format: SourceFormat,
    pub has_alpha: bool,
    pub data: Rc<[u8]>,
    pub frames: Option<Vec<Rc<[u8]>>>,
    pub delays_ms: Vec<u16>,
    pub loop_count: u16,
    pub digest: Digest,
    pub mips: RefCell<Option<Rc<MipChain>>>,
    pub budget: MediaBudgetGuard,
}

impl MediaImage {
    pub fn compute_raw_digest(
        width: u32,
        height: u32,
        format: PixelFormat,
        color_space: ColorSpace,
        pixels: &[u8],
    ) -> Digest {
        Digest::from_parts(&[
            b"askane.img.v1|",
            &width.to_le_bytes(),
            &height.to_le_bytes(),
            &[format as u8, color_space as u8],
            pixels,
        ])
    }

    pub fn from_vec(
        width: u32,
        height: u32,
        format: PixelFormat,
        color_space: ColorSpace,
        source_format: SourceFormat,
        pixels: Vec<u8>,
        explicit_alpha: Option<bool>,
        digest: Option<Digest>,
        budget_core: Option<&Arc<BudgetCore>>,
    ) -> Result<Rc<MediaImage>, String> {
        let expected_len = format.byte_len(width, height)?;
        if pixels.len() != expected_len {
            return Err(err_bad_opt(format!(
                "pixel buffer length {} does not match {}x{} {}",
                pixels.len(),
                width,
                height,
                format.name()
            )));
        }
        let guard = MediaBudgetGuard::charge(budget_core, pixels.len())?;
        let d = digest.unwrap_or_else(|| {
            Self::compute_raw_digest(width, height, format, color_space, &pixels)
        });
        let has_alpha = explicit_alpha.unwrap_or_else(|| format.has_alpha());
        let rc_data: Rc<[u8]> = Rc::from(pixels);
        Ok(Rc::new(MediaImage {
            width,
            height,
            format,
            color_space,
            source_format,
            has_alpha,
            data: rc_data,
            frames: None,
            delays_ms: Vec::new(),
            loop_count: 0,
            digest: d,
            mips: RefCell::new(None),
            budget: guard,
        }))
    }

    pub fn from_raw(
        raw: RawDecodedImage,
        opts: &DecodeOpts,
        budget_core: Option<&Arc<BudgetCore>>,
        deadline: Option<&dyn Fn() -> Result<(), String>>,
    ) -> Result<Rc<MediaImage>, String> {
        let max_d = opts.max_dim.unwrap_or(MAX_DIM).min(MAX_DIM);
        validate_dims(raw.width, raw.height, max_d)?;

        let color_space = opts.color_space.unwrap_or(ColorSpace::Srgb);
        let target_format = opts.format.unwrap_or(raw.format);
        let expected_raw_len = raw.format.byte_len(raw.width, raw.height)?;
        if raw.data.len() != expected_raw_len {
            return Err(crate::media::err_corrupt(
                "decoded frame byte length mismatch",
            ));
        }

        let (primary_rc, frames_opt, total_bytes) = if let Some(raw_frames) = raw.frames {
            if raw_frames.is_empty() {
                return Err(crate::media::err_corrupt("animated image has no frames"));
            }
            let target_frame_bytes = target_format.byte_len(raw.width, raw.height)?;
            let total = target_frame_bytes
                .checked_mul(raw_frames.len())
                .ok_or_else(|| err_limit("animated frames size overflow"))?;
            let mut converted_frames: Vec<Rc<[u8]>> = Vec::with_capacity(raw_frames.len());
            for frame in raw_frames {
                if frame.len() != expected_raw_len {
                    return Err(crate::media::err_corrupt("frame byte length mismatch"));
                }
                let buf = if target_format == raw.format {
                    frame
                } else {
                    convert_buffer(&frame, raw.width, raw.height, raw.format, target_format)?
                };
                converted_frames.push(Rc::from(buf));
            }
            let first = Rc::clone(&converted_frames[0]);
            (first, Some(converted_frames), total)
        } else {
            let buf = if target_format == raw.format {
                raw.data
            } else {
                convert_buffer(&raw.data, raw.width, raw.height, raw.format, target_format)?
            };
            let total = buf.len();
            (Rc::from(buf), None, total)
        };

        let guard = MediaBudgetGuard::charge(budget_core, total_bytes)?;
        let digest = if let Some(ref flist) = frames_opt {
            let mut parts: Vec<&[u8]> = Vec::with_capacity(4 + flist.len());
            let w_b = raw.width.to_le_bytes();
            let h_b = raw.height.to_le_bytes();
            let meta_b = [target_format as u8, color_space as u8, raw.source as u8];
            parts.push(b"askane.animimg.v1|");
            parts.push(&w_b);
            parts.push(&h_b);
            parts.push(&meta_b);
            for f in flist {
                parts.push(f.as_ref());
            }
            Digest::from_parts(&parts)
        } else {
            Self::compute_raw_digest(raw.width, raw.height, target_format, color_space, &primary_rc)
        };

        let img = Rc::new(MediaImage {
            width: raw.width,
            height: raw.height,
            format: target_format,
            color_space,
            source_format: raw.source,
            has_alpha: target_format.has_alpha(),
            data: primary_rc,
            frames: frames_opt,
            delays_ms: raw.delays_ms,
            loop_count: raw.loop_count,
            digest,
            mips: RefCell::new(None),
            budget: guard,
        });

        if opts.mipmaps == Some(true) {
            img.ensure_mipmaps(budget_core, deadline)?;
        }
        Ok(img)
    }

    pub fn ensure_mipmaps(
        &self,
        budget_core: Option<&Arc<BudgetCore>>,
        deadline: Option<&dyn Fn() -> Result<(), String>>,
    ) -> Result<Rc<MipChain>, String> {
        if let Some(existing) = self.mips.borrow().as_ref() {
            return Ok(Rc::clone(existing));
        }
        let chain = generate_mipmaps(self, budget_core, deadline)?;
        *self.mips.borrow_mut() = Some(Rc::clone(&chain));
        Ok(chain)
    }

    pub fn to_argb_u32_vec(&self) -> Vec<u32> {
        let count = (self.width as usize).saturating_mul(self.height as usize);
        let mut out = Vec::with_capacity(count);
        match self.format {
            PixelFormat::R8 => {
                for &g in self.data.iter() {
                    let v = g as u32;
                    out.push(0xFF00_0000 | (v << 16) | (v << 8) | v);
                }
            }
            PixelFormat::Rg8 => {
                for px in self.data.chunks_exact(2) {
                    out.push(0xFF00_0000 | ((px[0] as u32) << 16) | ((px[1] as u32) << 8));
                }
            }
            PixelFormat::Rgb8 => {
                for px in self.data.chunks_exact(3) {
                    out.push(
                        0xFF00_0000
                            | ((px[0] as u32) << 16)
                            | ((px[1] as u32) << 8)
                            | (px[2] as u32),
                    );
                }
            }
            PixelFormat::Rgba8 => {
                for px in self.data.chunks_exact(4) {
                    out.push(
                        ((px[3] as u32) << 24)
                            | ((px[0] as u32) << 16)
                            | ((px[1] as u32) << 8)
                            | (px[2] as u32),
                    );
                }
            }
        }
        out
    }

    #[inline]
    pub fn frame_count(&self) -> usize {
        self.frames.as_ref().map(|f| f.len()).unwrap_or(1)
    }

    #[inline]
    pub fn frame_data(&self, idx: usize) -> Option<&[u8]> {
        if let Some(ref frames) = self.frames {
            frames.get(idx).map(|f| f.as_ref())
        } else if idx == 0 {
            Some(self.data.as_ref())
        } else {
            None
        }
    }

    #[inline]
    pub fn mip_count(&self) -> u8 {
        self.mips
            .borrow()
            .as_ref()
            .map(|m| m.levels.len() as u8)
            .unwrap_or(1)
    }

    #[inline]
    pub fn byte_len(&self) -> usize {
        self.budget.bytes()
    }

    pub fn meta(&self) -> ImageMeta {
        let fc = self.frame_count();
        ImageMeta {
            source_format: self.source_format,
            color_space: self.color_space,
            has_alpha: self.has_alpha,
            is_animated: fc > 1,
            frame_count: fc as u32,
            loop_count: self.loop_count,
            mip_count: self.mip_count(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TextureData {
    pub image: Rc<MediaImage>,
    pub wrap_u: WrapMode,
    pub wrap_v: WrapMode,
    pub filter: Filter,
    pub srgb: bool,
    pub digest: Digest,
}

impl TextureData {
    pub fn new(
        image: Rc<MediaImage>,
        wrap_u: WrapMode,
        wrap_v: WrapMode,
        filter: Filter,
        srgb: bool,
        budget_core: Option<&Arc<BudgetCore>>,
        deadline: Option<&dyn Fn() -> Result<(), String>>,
    ) -> Result<Rc<TextureData>, String> {
        if filter.uses_mipmaps() {
            image.ensure_mipmaps(budget_core, deadline)?;
        }
        let flags = [wrap_u as u8, wrap_v as u8, filter as u8, u8::from(srgb)];
        let digest = Digest::from_parts(&[b"askane.tex.v1|", &image.digest.0, &flags]);
        Ok(Rc::new(TextureData {
            image,
            wrap_u,
            wrap_v,
            filter,
            srgb,
            digest,
        }))
    }
}

pub fn looks_like_tga(bytes: &[u8]) -> bool {
    if bytes.len() >= 26 && &bytes[bytes.len() - 18..] == b"TRUEVISION-XFILE.\0" {
        return true;
    }
    if bytes.len() < 18 {
        return false;
    }
    let id_len = bytes[0] as usize;
    let cmap_type = bytes[1];
    let img_type = bytes[2];
    if cmap_type != 0 || !matches!(img_type, 2 | 3 | 10 | 11) {
        return false;
    }
    let w = u16::from_le_bytes([bytes[12], bytes[13]]);
    let h = u16::from_le_bytes([bytes[14], bytes[15]]);
    let bpp = bytes[16];
    if w == 0 || h == 0 || w as u32 > MAX_DIM || h as u32 > MAX_DIM {
        return false;
    }
    if !matches!(bpp, 8 | 15 | 16 | 24 | 32) {
        return false;
    }
    bytes.len() >= 18 + id_len
}

pub fn sniff_source_format(bytes: &[u8]) -> Option<SourceFormat> {
    if bytes.len() >= 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" {
        Some(SourceFormat::Png)
    } else if bytes.len() >= 2 && &bytes[..2] == b"BM" {
        Some(SourceFormat::Bmp)
    } else if bytes.len() >= 6 && (&bytes[..6] == b"GIF87a" || &bytes[..6] == b"GIF89a") {
        Some(SourceFormat::Gif)
    } else if bytes.len() >= 4 && &bytes[..4] == b"qoif" {
        Some(SourceFormat::Qoi)
    } else if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        Some(SourceFormat::Jpeg)
    } else if looks_like_tga(bytes) {
        Some(SourceFormat::Tga)
    } else {
        None
    }
}

pub fn decode_image_bytes(
    bytes: &[u8],
    opts: &DecodeOpts,
    budget_core: Option<&Arc<BudgetCore>>,
    deadline: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<Rc<MediaImage>, String> {
    decode_image_bytes_with_hint(bytes, None, opts, budget_core, deadline)
}

pub fn decode_image_bytes_with_hint(
    bytes: &[u8],
    hint: Option<SourceFormat>,
    opts: &DecodeOpts,
    budget_core: Option<&Arc<BudgetCore>>,
    deadline: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<Rc<MediaImage>, String> {
    if bytes.is_empty() {
        return Err(crate::media::err_corrupt("empty image input"));
    }
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(err_limit(format!(
            "encoded image size {} bytes exceeds {} byte limit",
            bytes.len(),
            MAX_SOURCE_BYTES
        )));
    }
    check_deadline(deadline)?;

    let detected = sniff_source_format(bytes).or_else(|| match hint {
        Some(SourceFormat::Tga) => Some(SourceFormat::Tga),
        _ => None,
    });

    let fmt = detected.ok_or_else(|| {
        err_unsupported("unrecognized or unsupported image format (supported: png, bmp, tga, qoi, gif, jpeg)")
    })?;

    let raw = match fmt {
        SourceFormat::Png => formats::png::decode_png(bytes, opts.strict_crc, budget_core, deadline)?,
        SourceFormat::Bmp => formats::bmp::decode_bmp(bytes, budget_core, deadline)?,
        SourceFormat::Gif => formats::gif::decode_gif(
            bytes,
            opts.max_frames.unwrap_or(MAX_GIF_FRAMES),
            budget_core,
            deadline,
        )?,
        SourceFormat::Qoi => formats::qoi::decode_qoi(bytes, budget_core, deadline)?,
        SourceFormat::Jpeg => formats::jpeg::decode_jpeg(bytes, budget_core, deadline)?,
        SourceFormat::Tga => formats::tga::decode_tga(bytes, budget_core, deadline)?,
        _ => return Err(err_unsupported("unsupported source image format")),
    };

    MediaImage::from_raw(raw, opts, budget_core, deadline)
}

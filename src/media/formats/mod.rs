pub mod bmp;
pub mod gif;
pub mod jpeg;
pub mod png;
pub mod qoi;
pub mod tga;

use crate::media::image::{PixelFormat, SourceFormat};

pub struct RawDecodedImage {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub source: SourceFormat,
    pub data: Vec<u8>,
    pub frames: Option<Vec<Vec<u8>>>,
    pub delays_ms: Vec<u16>,
    pub loop_count: u16,
}

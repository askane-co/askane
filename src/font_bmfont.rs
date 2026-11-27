use std::collections::HashMap;
use std::path::Path;

use crate::budget::BudgetCore;
use crate::modules::gfx::{decode_bmp_argb, decode_png};

pub const MAX_FNT_FILE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_BMFONT_PAGES: usize = 8;
pub const MAX_BMFONT_CHARS: usize = 16_384;
pub const MAX_BMFONT_KERNINGS: usize = 32_768;

#[derive(Clone, Copy, Debug)]
pub struct BmGlyph {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub xoffset: i32,
    pub yoffset: i32,
    pub xadvance: i32,
    pub page: usize,
}

#[derive(Clone, Debug)]
pub struct BmPage {
    pub width: i32,
    pub height: i32,
    pub coverage: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct BmFont {
    pub size: i32,
    pub line_height: i32,
    pub glyphs: HashMap<u32, BmGlyph>,
    pub kernings: HashMap<(u32, u32), i32>,
    pub pages: Vec<BmPage>,
    pub charged_bytes: u64,
}

impl BmFont {
    pub fn refund_to(&mut self, budget: Option<&BudgetCore>) {
        let bytes = std::mem::take(&mut self.charged_bytes);
        if bytes > 0 {
            if let Some(b) = budget {
                b.refund_alloc(bytes);
            }
        }
    }
}

fn tokenize_fnt_line(line: &str) -> Vec<(&str, &str)> {
    let mut pairs = Vec::new();
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut i = 0usize;
    while i < n {
        while i < n && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= n {
            break;
        }
        let k_start = i;
        while i < n && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' {
            i += 1;
        }
        if i >= n || bytes[i] != b'=' {
            // Skip tag keyword itself
            continue;
        }
        let key = &line[k_start..i];
        i += 1; // skip '='
        if i < n && bytes[i] == b'"' {
            i += 1;
            let v_start = i;
            while i < n && bytes[i] != b'"' {
                i += 1;
            }
            let val = &line[v_start..i];
            if i < n && bytes[i] == b'"' {
                i += 1;
            }
            pairs.push((key, val));
        } else {
            let v_start = i;
            while i < n && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let val = &line[v_start..i];
            pairs.push((key, val));
        }
    }
    pairs
}

fn parse_i32_attr(pairs: &[(&str, &str)], key: &str, default: Option<i32>) -> Result<i32, String> {
    if let Some((_, v)) = pairs.iter().find(|&&(k, _)| k == key) {
        v.parse::<i32>()
            .map_err(|_| format!("Invalid integer for '{}' in .fnt: '{}'", key, v))
    } else if let Some(d) = default {
        Ok(d)
    } else {
        Err(format!("Missing required attribute '{}' in .fnt", key))
    }
}

/// Validates and converts an RGBA (`0xAARRGGBB`) BMFont texture into a single-channel R8 mask.
/// Rejects chromatic RGB pages so colored sprite fonts fail loudly instead of silently losing hues.
pub fn extract_r8_mask_from_argb(
    w: i32,
    h: i32,
    pixels: &[u32],
    has_explicit_alpha: bool,
) -> Result<Vec<u8>, String> {
    if w <= 0 || h <= 0 || pixels.len() != (w as usize) * (h as usize) {
        return Err("BMFont page dimension/buffer mismatch".to_string());
    }
    let mut has_varying_alpha = false;
    for &px in pixels {
        let a = (px >> 24) & 0xFF;
        let r = (px >> 16) & 0xFF;
        let g = (px >> 8) & 0xFF;
        let b = px & 0xFF;
        if a < 255 {
            has_varying_alpha = true;
        }
        if a > 0 && (r != g || g != b) {
            return Err(
                "BMFont page must be single-channel grayscale/alpha mask (chromatic RGB pages are not supported)"
                    .to_string(),
            );
        }
    }
    let use_alpha = has_explicit_alpha && has_varying_alpha;
    let mut out = Vec::with_capacity(pixels.len());
    for &px in pixels {
        let cov = if use_alpha {
            ((px >> 24) & 0xFF) as u8
        } else {
            ((px >> 16) & 0xFF) as u8
        };
        out.push(cov);
    }
    Ok(out)
}

pub fn decode_page_bytes_r8(data: &[u8], path: &Path) -> Result<BmPage, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (w, h, pixels, has_alpha) = if ext == "png" || (data.len() >= 8 && &data[..8] == b"\x89PNG\r\n\x1a\n") {
        let (w, h, px) = decode_png(data)?;
        (w, h, px, true)
    } else if ext == "bmp" || (data.len() >= 2 && &data[..2] == b"BM") {
        let (w, h, px) = decode_bmp_argb(data)?;
        (w, h, px, false)
    } else {
        return Err("BMFont page must be PNG or BMP image".to_string());
    };
    let coverage = extract_r8_mask_from_argb(w, h, &pixels, has_alpha)?;
    Ok(BmPage {
        width: w,
        height: h,
        coverage,
    })
}

pub fn parse_bmfont_ascii<F>(
    fnt_text: &str,
    budget: Option<&BudgetCore>,
    mut load_page: F,
) -> Result<BmFont, String>
where
    F: FnMut(usize, &str) -> Result<BmPage, String>,
{
    let mut size: i32 = 16;
    let mut line_height: i32 = 0;
    let mut expected_pages: usize = 0;
    let mut page_files: Vec<(usize, String)> = Vec::new();
    let mut glyphs: HashMap<u32, BmGlyph> = HashMap::new();
    let mut kernings: HashMap<(u32, u32), i32> = HashMap::new();

    for raw_line in fnt_text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let tag = line.split_whitespace().next().unwrap_or("");
        let pairs = tokenize_fnt_line(line);
        match tag {
            "info" => {
                size = parse_i32_attr(&pairs, "size", Some(16))?.abs();
                if !(8..=128).contains(&size) {
                    return Err(format!(
                        "BMFont size {} out of supported 8..=128 range",
                        size
                    ));
                }
            }
            "common" => {
                line_height = parse_i32_attr(&pairs, "lineHeight", None)?;
                if !(1..=512).contains(&line_height) {
                    return Err(format!("BMFont invalid lineHeight {}", line_height));
                }
                let p = parse_i32_attr(&pairs, "pages", Some(1))?;
                if p < 1 || (p as usize) > MAX_BMFONT_PAGES {
                    return Err(format!(
                        "BMFont pages {} out of 1..={} range",
                        p, MAX_BMFONT_PAGES
                    ));
                }
                expected_pages = p as usize;
            }
            "page" => {
                if page_files.len() >= MAX_BMFONT_PAGES {
                    return Err(format!(
                        "BMFont page count exceeds max {}",
                        MAX_BMFONT_PAGES
                    ));
                }
                let id = parse_i32_attr(&pairs, "id", None)?;
                if id < 0 || (id as usize) >= MAX_BMFONT_PAGES {
                    return Err(format!("BMFont invalid page id {}", id));
                }
                let file = pairs
                    .iter()
                    .find(|&&(k, _)| k == "file")
                    .map(|&(_, v)| v.to_string())
                    .ok_or_else(|| "BMFont page missing file attribute".to_string())?;
                page_files.push((id as usize, file));
            }
            "char" => {
                if glyphs.len() >= MAX_BMFONT_CHARS {
                    return Err(format!(
                        "BMFont glyph count exceeds max {}",
                        MAX_BMFONT_CHARS
                    ));
                }
                let id = parse_i32_attr(&pairs, "id", None)?;
                if id < 0 {
                    continue;
                }
                let g = BmGlyph {
                    x: parse_i32_attr(&pairs, "x", None)?,
                    y: parse_i32_attr(&pairs, "y", None)?,
                    width: parse_i32_attr(&pairs, "width", None)?,
                    height: parse_i32_attr(&pairs, "height", None)?,
                    xoffset: parse_i32_attr(&pairs, "xoffset", Some(0))?,
                    yoffset: parse_i32_attr(&pairs, "yoffset", Some(0))?,
                    xadvance: parse_i32_attr(&pairs, "xadvance", Some(0))?,
                    page: parse_i32_attr(&pairs, "page", Some(0))? as usize,
                };
                if g.width < 0 || g.height < 0 || g.width > 512 || g.height > 512 {
                    return Err(format!("BMFont glyph {} invalid size {}x{}", id, g.width, g.height));
                }
                glyphs.insert(id as u32, g);
            }
            "kerning" => {
                if kernings.len() < MAX_BMFONT_KERNINGS {
                    let first = parse_i32_attr(&pairs, "first", None)?;
                    let second = parse_i32_attr(&pairs, "second", None)?;
                    let amount = parse_i32_attr(&pairs, "amount", None)?;
                    if first >= 0 && second >= 0 {
                        kernings.insert((first as u32, second as u32), amount);
                    }
                }
            }
            _ => {}
        }
    }

    if line_height <= 0 {
        return Err("Invalid BMFont: missing 'common lineHeight'".to_string());
    }
    if page_files.is_empty() {
        return Err("Invalid BMFont: missing page definitions".to_string());
    }
    if expected_pages > 0 && page_files.len() < expected_pages {
        return Err("Invalid BMFont: missing declared texture pages".to_string());
    }
    page_files.sort_by_key(|(id, _)| *id);

    let mut pages = Vec::with_capacity(page_files.len());
    let mut total_bytes: u64 =
        (glyphs.len() * std::mem::size_of::<BmGlyph>() + kernings.len() * 16) as u64;
    for (idx, (id, file)) in page_files.iter().enumerate() {
        if *id != idx {
            return Err("BMFont page IDs must be contiguous 0..N".to_string());
        }
        let page = load_page(*id, file)?;
        total_bytes = total_bytes.saturating_add(page.coverage.len() as u64);
        pages.push(page);
    }

    for (&cp, g) in &glyphs {
        let Some(page) = pages.get(g.page) else {
            return Err(format!("BMFont glyph {} references missing page {}", cp, g.page));
        };
        if g.x < 0
            || g.y < 0
            || g.x.saturating_add(g.width) > page.width
            || g.y.saturating_add(g.height) > page.height
        {
            return Err(format!("BMFont glyph {} rect out of page bounds", cp));
        }
    }

    if let Some(b) = budget {
        b.charge_alloc(total_bytes)?;
    }
    Ok(BmFont {
        size,
        line_height,
        glyphs,
        kernings,
        pages,
        charged_bytes: if budget.is_some() { total_bytes } else { 0 },
    })
}

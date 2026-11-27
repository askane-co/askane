use std::collections::HashMap;

use crate::budget::BudgetCore;
use crate::camera::Camera2D;
use crate::font_bitmap::{bitmap_scaled_dims, rasterize_bitmap_glyph, BITMAP_CELL_H};
use crate::font_bmfont::BmFont;
use crate::surface::{blend_rgb_alpha_on, ClipRect, PixelFormat, Surface};

pub const ATLAS_PAGE_DIM: i32 = 1024;
pub const MAX_ATLAS_PAGES: usize = 8;
pub const MAX_FONT_HANDLES: usize = 32;
pub const MIN_FONT_SIZE: i32 = 8;
pub const MAX_FONT_SIZE: i32 = 128;
pub const MAX_TEXT_CHARS: usize = 16_384;

#[derive(Clone, Copy, Debug)]
pub struct CachedGlyph {
    pub page_idx: usize,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub xoffset: i32,
    pub yoffset: i32,
    pub xadvance: i32,
}

#[derive(Debug)]
struct ShelfPage {
    surface: Surface,
    cursor_x: i32,
    cursor_y: i32,
    row_h: i32,
    used_pixels: usize,
}

pub struct FontSystem {
    pub fonts: Vec<Option<BmFont>>,
    entries: HashMap<u64, CachedGlyph>,
    pages: Vec<ShelfPage>,
    pub flush_count: u64,
}

impl Default for FontSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl FontSystem {
    pub fn new() -> Self {
        Self {
            fonts: Vec::new(),
            entries: HashMap::new(),
            pages: Vec::new(),
            flush_count: 0,
        }
    }

    pub fn clear_all(&mut self, budget: Option<&BudgetCore>) {
        self.flush_atlas(budget);
        for slot in self.fonts.iter_mut() {
            if let Some(mut font) = slot.take() {
                font.refund_to(budget);
            }
        }
        self.fonts.clear();
        self.flush_count = 0;
    }

    pub fn flush_atlas(&mut self, budget: Option<&BudgetCore>) {
        for p in self.pages.iter_mut() {
            p.surface.refund_to(budget);
        }
        self.pages.clear();
        self.entries.clear();
        self.flush_count = self.flush_count.saturating_add(1);
    }

    pub fn active_bmfont_count(&self) -> usize {
        self.fonts.iter().filter(|f| f.is_some()).count()
    }

    pub fn register_bmfont(&mut self, font: BmFont) -> Result<usize, String> {
        if self.active_bmfont_count() >= MAX_FONT_HANDLES {
            return Err(format!("Maximum font handle limit ({}) reached", MAX_FONT_HANDLES));
        }
        if let Some(idx) = self.fonts.iter().position(|s| s.is_none()) {
            self.fonts[idx] = Some(font);
            Ok(idx + 1)
        } else {
            self.fonts.push(Some(font));
            Ok(self.fonts.len())
        }
    }

    pub fn free_font(&mut self, font_id: i64, budget: Option<&BudgetCore>) -> Result<(), String> {
        if font_id == 0 {
            return Err("Cannot free built-in bitmap font (id 0)".to_string());
        }
        if font_id < 1 {
            return Err(format!("Invalid font handle {}", font_id));
        }
        let idx = (font_id - 1) as usize;
        let Some(slot) = self.fonts.get_mut(idx) else {
            return Err(format!("Invalid or freed font handle {}", font_id));
        };
        let Some(mut font) = slot.take() else {
            return Err(format!("Invalid or freed font handle {}", font_id));
        };
        font.refund_to(budget);
        // Drop cached glyph entries belonging to freed font_id cleanly
        let prefix = (font_id as u64) << 48;
        let mask = 0xFFFFu64 << 48;
        self.entries.retain(|k, _| (k & mask) != prefix);
        Ok(())
    }

    pub fn validate_font_and_size(&self, font_id: i64, size: i32) -> Result<i32, String> {
        if !(MIN_FONT_SIZE..=MAX_FONT_SIZE).contains(&size) {
            return Err(format!(
                "Font size {} out of {}..={} bounds",
                size, MIN_FONT_SIZE, MAX_FONT_SIZE
            ));
        }
        if font_id != 0 {
            let ok = font_id > 0
                && self
                    .fonts
                    .get((font_id - 1) as usize)
                    .is_some_and(|s| s.is_some());
            if !ok {
                return Err(format!("Invalid or freed font handle {}", font_id));
            }
        }
        Ok(size)
    }

    pub fn line_height(&self, font_id: i64, size: i32) -> Result<i32, String> {
        let size = self.validate_font_and_size(font_id, size)?;
        if font_id == 0 {
            return Ok(size);
        }
        let font = self.fonts[(font_id - 1) as usize].as_ref().unwrap();
        let lh = ((font.line_height as f64 * size as f64) / (font.size.max(1) as f64))
            .round()
            .max(1.0) as i32;
        Ok(lh)
    }

    fn key(font_id: i64, size: i32, cp: u32) -> u64 {
        ((font_id as u64 & 0xFFFF) << 48)
            | ((size as u64 & 0xFFFF) << 32)
            | (cp as u64 & 0xFFFF_FFFF)
    }

    fn total_capacity_pixels() -> usize {
        (ATLAS_PAGE_DIM as usize) * (ATLAS_PAGE_DIM as usize) * MAX_ATLAS_PAGES
    }

    fn used_pixels(&self) -> usize {
        self.pages.iter().map(|p| p.used_pixels).sum()
    }

    fn alloc_glyph_slot(
        &mut self,
        gw: i32,
        gh: i32,
        coverage: &[u8],
        budget: Option<&BudgetCore>,
    ) -> Result<(usize, i32, i32), String> {
        let pad_w = (gw + 1).max(1);
        let pad_h = (gh + 1).max(1);
        let add_px = (pad_w as usize) * (pad_h as usize);
        let max_fill = (Self::total_capacity_pixels() * 90) / 100;
        if self.used_pixels() + add_px > max_fill {
            self.flush_atlas(budget);
        }

        for attempt in 0..2 {
            let mut placed: Option<(usize, i32, i32)> = None;
            for (idx, p) in self.pages.iter_mut().enumerate() {
                if p.cursor_x + pad_w <= ATLAS_PAGE_DIM && p.cursor_y + pad_h <= ATLAS_PAGE_DIM {
                    placed = Some((idx, p.cursor_x, p.cursor_y));
                    p.cursor_x += pad_w;
                    p.row_h = p.row_h.max(pad_h);
                    break;
                }
                let next_y = p.cursor_y + p.row_h;
                if next_y + pad_h <= ATLAS_PAGE_DIM && pad_w <= ATLAS_PAGE_DIM {
                    p.cursor_x = pad_w;
                    p.cursor_y = next_y;
                    p.row_h = pad_h;
                    placed = Some((idx, 0, next_y));
                    break;
                }
            }
            if placed.is_none() && self.pages.len() < MAX_ATLAS_PAGES {
                let surface = Surface::new_r8(ATLAS_PAGE_DIM, ATLAS_PAGE_DIM, 0, budget)?;
                self.pages.push(ShelfPage {
                    surface,
                    cursor_x: pad_w,
                    cursor_y: 0,
                    row_h: pad_h,
                    used_pixels: 0,
                });
                placed = Some((self.pages.len() - 1, 0, 0));
            }
            if let Some((idx, ax, ay)) = placed {
                let p = &mut self.pages[idx];
                debug_assert_eq!(p.surface.format, PixelFormat::R8);
                p.used_pixels = p.used_pixels.saturating_add(add_px);
                for row in 0..gh {
                    let srow = (row as usize) * (gw as usize);
                    let drow = ((ay + row) as usize) * (ATLAS_PAGE_DIM as usize) + (ax as usize);
                    p.surface.r8[drow..drow + gw as usize]
                        .copy_from_slice(&coverage[srow..srow + gw as usize]);
                }
                return Ok((idx, ax, ay));
            }
            if attempt == 0 {
                self.flush_atlas(budget);
            }
        }
        Err("Glyph dimensions exceed FontAtlas page size (1024x1024)".to_string())
    }

    fn get_or_cache_glyph(
        &mut self,
        font_id: i64,
        size: i32,
        cp: u32,
        budget: Option<&BudgetCore>,
    ) -> Result<CachedGlyph, String> {
        let k = Self::key(font_id, size, cp);
        if let Some(&cached) = self.entries.get(&k) {
            return Ok(cached);
        }

        if font_id == 0 {
            let (gw, gh, cov) = rasterize_bitmap_glyph(cp, size);
            let (page_idx, ax, ay) = self.alloc_glyph_slot(gw, gh, &cov, budget)?;
            let g = CachedGlyph {
                page_idx,
                x: ax,
                y: ay,
                w: gw,
                h: gh,
                xoffset: 0,
                yoffset: 0,
                xadvance: gw,
            };
            self.entries.insert(k, g);
            return Ok(g);
        }

        let font = self.fonts[(font_id - 1) as usize]
            .as_ref()
            .ok_or_else(|| "Invalid font handle".to_string())?;
        let scale = (size as f64) / (font.size.max(1) as f64);

        if let Some(bg) = font.glyphs.get(&cp) {
            let dw = if bg.width == 0 {
                0
            } else {
                ((bg.width as f64) * scale).round().max(1.0) as i32
            };
            let dh = if bg.height == 0 {
                0
            } else {
                ((bg.height as f64) * scale).round().max(1.0) as i32
            };
            let xoff = ((bg.xoffset as f64) * scale).round() as i32;
            let yoff = ((bg.yoffset as f64) * scale).round() as i32;
            let xadv = ((bg.xadvance as f64) * scale).round().max(0.0) as i32;

            let mut cov = Vec::new();
            let (page_idx, ax, ay) = if dw > 0 && dh > 0 {
                let page = &font.pages[bg.page];
                cov.resize((dw as usize) * (dh as usize), 0u8);
                for py in 0..dh {
                    let sy = bg.y + ((py * bg.height) / dh).clamp(0, bg.height - 1);
                    let srow = (sy as usize) * (page.width as usize);
                    let drow = (py as usize) * (dw as usize);
                    for px in 0..dw {
                        let sx = bg.x + ((px * bg.width) / dw).clamp(0, bg.width - 1);
                        cov[drow + (px as usize)] = page.coverage[srow + (sx as usize)];
                    }
                }
                self.alloc_glyph_slot(dw, dh, &cov, budget)?
            } else {
                (0, 0, 0)
            };
            let g = CachedGlyph {
                page_idx,
                x: ax,
                y: ay,
                w: dw,
                h: dh,
                xoffset: xoff,
                yoffset: yoff,
                xadvance: xadv,
            };
            self.entries.insert(k, g);
            return Ok(g);
        }

        // Unknown BMFont codepoint: fall back cleanly to deterministic bitmap box
        let (gw, gh, cov) = rasterize_bitmap_glyph(0xFFFF_FFFF, size);
        let (page_idx, ax, ay) = self.alloc_glyph_slot(gw, gh, &cov, budget)?;
        let g = CachedGlyph {
            page_idx,
            x: ax,
            y: ay,
            w: gw,
            h: gh,
            xoffset: 0,
            yoffset: 0,
            xadvance: gw,
        };
        self.entries.insert(k, g);
        Ok(g)
    }

    fn glyph_advance_and_kerning(
        &self,
        font_id: i64,
        size: i32,
        prev_cp: Option<u32>,
        cp: u32,
    ) -> i32 {
        if font_id == 0 {
            let (w, _) = bitmap_scaled_dims(size);
            return w;
        }
        let font = self.fonts[(font_id - 1) as usize].as_ref().unwrap();
        let scale = (size as f64) / (font.size.max(1) as f64);
        let adv = font
            .glyphs
            .get(&cp)
            .map(|g| ((g.xadvance as f64) * scale).round().max(0.0) as i32)
            .unwrap_or_else(|| bitmap_scaled_dims(size).0);
        let kern = prev_cp
            .and_then(|p| font.kernings.get(&(p, cp)).copied())
            .map(|k| ((k as f64) * scale).round() as i32)
            .unwrap_or(0);
        adv + kern
    }

    pub fn measure_text(&self, font_id: i64, size: i32, text: &str) -> Result<(i32, i32, i32, usize), String> {
        let size = self.validate_font_and_size(font_id, size)?;
        if text.chars().count() > MAX_TEXT_CHARS {
            return Err(format!("Text length exceeds limit of {} characters", MAX_TEXT_CHARS));
        }
        let lh = self.line_height(font_id, size)?;
        if text.is_empty() {
            return Ok((0, lh, lh, 1));
        }
        let mut max_w = 0i32;
        let mut lines = 0usize;
        for line in text.split('\n') {
            lines += 1;
            let mut cur_w = 0i32;
            let mut prev = None;
            for ch in line.chars() {
                let cp = ch as u32;
                if ch != '\r' {
                    cur_w = cur_w.saturating_add(self.glyph_advance_and_kerning(font_id, size, prev, cp));
                    prev = Some(cp);
                }
            }
            max_w = max_w.max(cur_w);
        }
        Ok((max_w.max(0), lh.saturating_mul(lines.max(1) as i32), lh, lines.max(1)))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(
        &mut self,
        dst: &mut [u32],
        dst_w: i32,
        dst_h: i32,
        clip: ClipRect,
        cam: &Camera2D,
        font_id: i64,
        size: i32,
        x: i32,
        y: i32,
        text: &str,
        color_argb: u32,
        align: &str,
        preserve_dst_alpha: bool,
        budget: Option<&BudgetCore>,
    ) -> Result<(), String> {
        let size = self.validate_font_and_size(font_id, size)?;
        if text.chars().count() > MAX_TEXT_CHARS {
            return Err(format!("Text length exceeds limit of {} characters", MAX_TEXT_CHARS));
        }
        let fg_a = (color_argb >> 24) & 0xFF;
        let fg_rgb = color_argb & 0x00FF_FFFF;
        if fg_a == 0 {
            return Ok(());
        }
        let lh = self.line_height(font_id, size)?;
        let (max_w, _, _, _) = self.measure_text(font_id, size, text)?;
        let zoom = if cam.enabled { cam.zoom } else { 1.0 };

        for (line_idx, line) in text.split('\n').enumerate() {
            let line_w = self.measure_text(font_id, size, line)?.0;
            let align_dx = match align {
                "center" => (max_w - line_w) / 2,
                "right" => max_w - line_w,
                _ => 0,
            };
            let mut pen_x = x + align_dx;
            let pen_y = y + (line_idx as i32) * lh;
            let mut prev: Option<u32> = None;

            for ch in line.chars() {
                if ch == '\r' {
                    continue;
                }
                let cp = ch as u32;
                let g = self.get_or_cache_glyph(font_id, size, cp, budget)?;
                let kern = if font_id != 0 {
                    let font = self.fonts[(font_id - 1) as usize].as_ref().unwrap();
                    let scale = (size as f64) / (font.size.max(1) as f64);
                    prev.and_then(|p| font.kernings.get(&(p, cp)).copied())
                        .map(|k| ((k as f64) * scale).round() as i32)
                        .unwrap_or(0)
                } else {
                    0
                };
                pen_x += kern;

                if g.w > 0 && g.h > 0 {
                    let gx = pen_x + g.xoffset;
                    let gy = pen_y + g.yoffset;
                    if cam.is_identity_rect() {
                        let page = &self.pages[g.page_idx].surface;
                        let x0 = gx.max(clip.x0);
                        let y0 = gy.max(clip.y0);
                        let x1 = gx.saturating_add(g.w).min(clip.x1);
                        let y1 = gy.saturating_add(g.h).min(clip.y1);
                        if x0 < x1 && y0 < y1 {
                            let dst_wu = dst_w as usize;
                            let atlas_wu = ATLAS_PAGE_DIM as usize;
                            for py in y0..y1 {
                                let arow = ((g.y + (py - gy)) as usize) * atlas_wu + (g.x as usize);
                                let drow = (py as usize) * dst_wu;
                                for px in x0..x1 {
                                    let cov = page.r8[arow + (px - gx) as usize] as u32;
                                    if cov > 0 {
                                        let alpha = (cov * fg_a + 127) / 255;
                                        let slot = &mut dst[drow + px as usize];
                                        *slot = blend_rgb_alpha_on(*slot, fg_rgb, alpha, preserve_dst_alpha);
                                    }
                                }
                            }
                        }
                    } else {
                        let page = &self.pages[g.page_idx].surface;
                        let atlas_wu = ATLAS_PAGE_DIM as usize;
                        let dst_wu = dst_w as usize;
                        let dw = ((g.w as f64) * zoom).round().max(1.0) as i32;
                        let dh = ((g.h as f64) * zoom).round().max(1.0) as i32;
                        let (sx_f, sy_f) = cam.world_to_screen(gx as f64, gy as f64, dst_w, dst_h);
                        let dx = sx_f.round() as i32;
                        let dy = sy_f.round() as i32;
                        let x0 = dx.max(clip.x0);
                        let y0 = dy.max(clip.y0);
                        let x1 = dx.saturating_add(dw).min(clip.x1);
                        let y1 = dy.saturating_add(dh).min(clip.y1);
                        if x0 < x1 && y0 < y1 {
                            for py in y0..y1 {
                                let sy = (((py - dy) as i64 * g.h as i64) / dh as i64)
                                    .clamp(0, g.h as i64 - 1) as i32;
                                let arow = ((g.y + sy) as usize) * atlas_wu + (g.x as usize);
                                let drow = (py as usize) * dst_wu;
                                for px in x0..x1 {
                                    let sx = (((px - dx) as i64 * g.w as i64) / dw as i64)
                                        .clamp(0, g.w as i64 - 1) as i32;
                                    let cov = page.r8[arow + sx as usize] as u32;
                                    if cov > 0 {
                                        let alpha = (cov * fg_a + 127) / 255;
                                        let slot = &mut dst[drow + px as usize];
                                        *slot = blend_rgb_alpha_on(*slot, fg_rgb, alpha, preserve_dst_alpha);
                                    }
                                }
                            }
                        }
                    }
                }
                pen_x += g.xadvance;
                prev = Some(cp);
            }
        }
        let _ = BITMAP_CELL_H;
        Ok(())
    }
}

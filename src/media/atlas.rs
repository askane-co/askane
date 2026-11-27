use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::budget::BudgetCore;
use crate::media::image::{ColorSpace, MediaImage, PixelFormat, SourceFormat};
use crate::media::ops::convert_buffer;
use crate::media::{
    Digest, MAX_ATLAS_CELLS, MAX_DIM, check_deadline, err_bad_opt, err_limit, temp_budget_guard,
    validate_dims,
};
use crate::value::{Value, new_dict_with_capacity};

#[derive(Clone, Debug)]
pub struct AtlasFrame {
    pub name: String,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub index: usize,
}

#[derive(Clone, Debug)]
pub struct SpriteAtlas {
    pub image: Rc<MediaImage>,
    pub frames: Vec<AtlasFrame>,
}

pub fn pack_atlas(
    items: Vec<(String, Rc<MediaImage>)>,
    padding: u32,
    max_size: u32,
    budget_core: Option<&Arc<BudgetCore>>,
    deadline: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<SpriteAtlas, String> {
    if items.is_empty() {
        return Err(err_bad_opt("image.atlas() requires at least 1 input frame"));
    }
    if items.len() > MAX_ATLAS_CELLS {
        return Err(err_limit(format!(
            "image.atlas() item count {} exceeds {} limit",
            items.len(),
            MAX_ATLAS_CELLS
        )));
    }
    if max_size == 0 || max_size > MAX_DIM {
        return Err(err_bad_opt(format!(
            "atlas max_size {} must be in 1..={}",
            max_size, MAX_DIM
        )));
    }
    if padding > 128 {
        return Err(err_bad_opt("atlas padding must be in 0..=128"));
    }

    let mut seen_names = HashMap::with_capacity(items.len());
    let mut color_space = items[0].1.color_space;
    let mut all_opaque_rgb = true;
    for (idx, (name, img)) in items.iter().enumerate() {
        if name.is_empty() {
            return Err(err_bad_opt("atlas frame name cannot be empty"));
        }
        if seen_names.insert(name.as_str(), idx).is_some() {
            return Err(err_bad_opt(format!("duplicate atlas frame '{}'", name)));
        }
        if img.width + padding > max_size || img.height + padding > max_size {
            return Err(err_limit(format!(
                "atlas frame '{}' ({}x{}) does not fit within max_size {}",
                name, img.width, img.height, max_size
            )));
        }
        if img.color_space != ColorSpace::Srgb {
            color_space = img.color_space;
        }
        if img.format == PixelFormat::Rgba8 || img.has_alpha {
            all_opaque_rgb = false;
        }
    }

    // Sort deterministically for packing: tallest first, widest next, name ascending.
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| {
        items[b]
            .1
            .height
            .cmp(&items[a].1.height)
            .then_with(|| items[b].1.width.cmp(&items[a].1.width))
            .then_with(|| items[a].0.cmp(&items[b].0))
    });

    // Pick initial candidate power-of-two width >= widest item.
    let widest = items.iter().map(|(_, im)| im.width + padding).max().unwrap_or(1);
    let total_area: u64 = items
        .iter()
        .map(|(_, im)| (im.width as u64 + padding as u64) * (im.height as u64 + padding as u64))
        .sum();
    let mut target_w = 64u32.max(widest.next_power_of_two()).min(max_size);
    while (target_w as u64) * (target_w as u64) < total_area && target_w < max_size {
        let next = target_w.saturating_mul(2).min(max_size);
        if next == target_w {
            break;
        }
        target_w = next;
    }

    let mut placements = vec![(0u32, 0u32); items.len()];
    let packed_w: u32;
    let mut packed_h = 0u32;

    loop {
        let mut cur_x = 0u32;
        let mut cur_y = 0u32;
        let mut shelf_h = 0u32;
        let mut max_x = 0u32;
        let mut failed = false;

        for &orig_idx in &order {
            let img = &items[orig_idx].1;
            let cw = img.width + padding;
            let ch = img.height + padding;
            if cur_x > 0 && cur_x + cw > target_w {
                cur_x = 0;
                cur_y = cur_y.saturating_add(shelf_h);
                shelf_h = 0;
            }
            if cur_x + img.width > target_w || cur_y + img.height > max_size {
                failed = true;
                break;
            }
            placements[orig_idx] = (cur_x, cur_y);
            max_x = max_x.max(cur_x + img.width);
            cur_x = cur_x.saturating_add(cw);
            shelf_h = shelf_h.max(ch);
            packed_h = packed_h.max(cur_y + img.height);
        }

        if !failed {
            packed_w = max_x.max(1);
            packed_h = packed_h.max(1);
            break;
        }
        if target_w >= max_size {
            return Err(err_limit(format!(
                "atlas frames do not fit within max_size {}x{}",
                max_size, max_size
            )));
        }
        target_w = target_w.saturating_mul(2).min(max_size);
        packed_h = 0;
    }

    let out_fmt = if all_opaque_rgb {
        PixelFormat::Rgb8
    } else {
        PixelFormat::Rgba8
    };
    validate_dims(packed_w, packed_h, MAX_DIM)?;
    let out_bytes = out_fmt.byte_len(packed_w, packed_h)?;
    let _tmp = temp_budget_guard(budget_core, out_bytes)?;
    let mut canvas = vec![0u8; out_bytes];
    let dst_bpp = out_fmt.channels();
    let dst_stride = (packed_w as usize) * dst_bpp;

    let mut frames = Vec::with_capacity(items.len());
    let mut digest_parts: Vec<Vec<u8>> = Vec::with_capacity(items.len() + 2);
    digest_parts.push(b"op:atlas.v1|".to_vec());
    let mut header_meta = Vec::new();
    header_meta.extend_from_slice(&packed_w.to_le_bytes());
    header_meta.extend_from_slice(&packed_h.to_le_bytes());
    header_meta.extend_from_slice(&padding.to_le_bytes());
    digest_parts.push(header_meta);

    for (idx, (name, img)) in items.iter().enumerate() {
        if (idx & 15) == 0 {
            check_deadline(deadline)?;
        }
        let (px, py) = placements[idx];
        let src_bytes_owned;
        let src_slice: &[u8] = if img.format == out_fmt {
            &img.data
        } else {
            src_bytes_owned = convert_buffer(&img.data, img.width, img.height, img.format, out_fmt)?;
            &src_bytes_owned
        };
        let row_bytes = (img.width as usize) * dst_bpp;
        for row in 0..(img.height as usize) {
            let s_off = row * row_bytes;
            let d_off = ((py as usize) + row) * dst_stride + (px as usize) * dst_bpp;
            canvas[d_off..d_off + row_bytes].copy_from_slice(&src_slice[s_off..s_off + row_bytes]);
        }

        let mut fpart = Vec::with_capacity(name.len() + 48);
        fpart.extend_from_slice(name.as_bytes());
        fpart.extend_from_slice(&px.to_le_bytes());
        fpart.extend_from_slice(&py.to_le_bytes());
        fpart.extend_from_slice(&img.digest.0);
        digest_parts.push(fpart);

        frames.push(AtlasFrame {
            name: name.clone(),
            x: px,
            y: py,
            w: img.width,
            h: img.height,
            index: idx,
        });
    }

    drop(_tmp);
    let refs: Vec<&[u8]> = digest_parts.iter().map(|v| v.as_slice()).collect();
    let digest = Digest::from_parts(&refs);
    let atlas_img = MediaImage::from_vec(
        packed_w,
        packed_h,
        out_fmt,
        color_space,
        SourceFormat::Derived,
        canvas,
        Some(out_fmt.has_alpha()),
        Some(digest),
        budget_core,
    )?;

    Ok(SpriteAtlas {
        image: atlas_img,
        frames,
    })
}

pub fn atlas_to_value(atlas: &SpriteAtlas) -> Value {
    let mut frames_dict = new_dict_with_capacity(atlas.frames.len());
    let mut order_list = Vec::with_capacity(atlas.frames.len());
    for f in &atlas.frames {
        let mut fd = new_dict_with_capacity(6);
        fd.insert("name".to_string(), Value::from_str(f.name.clone()));
        fd.insert("x".to_string(), Value::Int(f.x as i64));
        fd.insert("y".to_string(), Value::Int(f.y as i64));
        fd.insert("w".to_string(), Value::Int(f.w as i64));
        fd.insert("h".to_string(), Value::Int(f.h as i64));
        fd.insert("index".to_string(), Value::Int(f.index as i64));
        frames_dict.insert(f.name.clone(), Value::dict(fd));
        order_list.push(Value::from_str(f.name.clone()));
    }
    let mut root = new_dict_with_capacity(6);
    root.insert("image".to_string(), Value::Image(Rc::clone(&atlas.image)));
    root.insert("width".to_string(), Value::Int(atlas.image.width as i64));
    root.insert("height".to_string(), Value::Int(atlas.image.height as i64));
    root.insert("count".to_string(), Value::Int(atlas.frames.len() as i64));
    root.insert("frames".to_string(), Value::dict(frames_dict));
    root.insert("order".to_string(), Value::List(Rc::new(RefCell::new(order_list))));
    Value::dict(root)
}

pub fn resolve_atlas_frame(
    atlas_val: &Value,
    key_val: &Value,
) -> Result<(Rc<MediaImage>, AtlasFrame), String> {
    let Value::Dict(root_rc) = atlas_val else {
        return Err(err_bad_opt(format!(
            "atlas must be a dict returned by image.atlas(), got '{}'",
            atlas_val.type_name()
        )));
    };
    let root = root_rc.borrow();
    let img = match root.get("image") {
        Some(Value::Image(im)) => Rc::clone(im),
        Some(Value::Texture(t)) => Rc::clone(&t.image),
        _ => return Err(err_bad_opt("atlas dict missing valid 'image'")),
    };
    let Some(Value::Dict(frames_rc)) = root.get("frames") else {
        return Err(err_bad_opt("atlas dict missing 'frames' dict"));
    };

    let frame_name: String = match key_val {
        Value::Str(s) => s.to_string(),
        Value::Int(idx) => {
            if *idx < 0 {
                return Err(err_bad_opt(format!("atlas frame index {} is negative", idx)));
            }
            if let Some(Value::List(ord)) = root.get("order") {
                let items = ord.borrow();
                let Some(Value::Str(name)) = items.get(*idx as usize) else {
                    return Err(err_bad_opt(format!(
                        "atlas frame index {} out of bounds 0..{}",
                        idx,
                        items.len()
                    )));
                };
                name.to_string()
            } else {
                return Err(err_bad_opt("atlas dict missing frame order index"));
            }
        }
        other => {
            return Err(err_bad_opt(format!(
                "atlas key must be frame name (str) or index (int), got '{}'",
                other.type_name()
            )));
        }
    };

    let frames = frames_rc.borrow();
    let Some(Value::Dict(fd_rc)) = frames.get(&frame_name) else {
        return Err(err_bad_opt(format!("atlas frame '{}' not found", frame_name)));
    };
    let fd = fd_rc.borrow();
    let get_u32 = |k: &str| -> Result<u32, String> {
        match fd.get(k) {
            Some(Value::Int(v)) if *v >= 0 && *v <= i64::from(u32::MAX) => Ok(*v as u32),
            _ => Err(err_bad_opt(format!("atlas frame missing '{}'", k))),
        }
    };
    let x = get_u32("x")?;
    let y = get_u32("y")?;
    let w = get_u32("w")?;
    let h = get_u32("h")?;
    let index = match fd.get("index") {
        Some(Value::Int(v)) if *v >= 0 => *v as usize,
        _ => 0,
    };
    Ok((
        img,
        AtlasFrame {
            name: frame_name,
            x,
            y,
            w,
            h,
            index,
        },
    ))
}

#![forbid(unsafe_code)]
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use crate::modules::input;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum X11Target {
    UnixSocket { path: String, screen: u16 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct X11VisualInfo {
    pub root_window: u32,
    pub white_pixel: u32,
    pub black_pixel: u32,
    pub root_visual: u32,
    pub root_depth: u8,
    pub bits_per_pixel: u8,
    pub max_request_quads: usize,
    pub resource_id_base: u32,
    pub resource_id_mask: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum X11Event {
    None,
    KeyPress { keycode: u8, x: i16, y: i16 },
    KeyRelease { keycode: u8, x: i16, y: i16 },
    ButtonPress { button: u8, x: i16, y: i16 },
    ButtonRelease { button: u8, x: i16, y: i16 },
    Motion { x: i16, y: i16 },
    Expose,
    Configure { width: u16, height: u16 },
    ClientDelete,
}

pub fn parse_display(display: &str) -> Result<(X11Target, String), String> {
    let trimmed = display.trim();
    if trimmed.is_empty() {
        return Err("E_BACKEND_UNAVAILABLE: DISPLAY environment variable is empty".to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("unix:") {
        let (sock, scr) = match rest.rsplit_once('.') {
            Some((p, s)) if !p.is_empty() => (p, s.parse::<u16>().unwrap_or(0)),
            _ => (rest, 0),
        };
        let dnum = sock
            .rsplit('X')
            .next()
            .filter(|s| s.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or("0")
            .to_string();
        return Ok((
            X11Target::UnixSocket {
                path: sock.to_string(),
                screen: scr,
            },
            dnum,
        ));
    }
    if let Some(colon) = trimmed.find(':') {
        let host = &trimmed[..colon];
        if !host.is_empty() && host != "unix" && host != "localhost" {
            return Err(format!(
                "E_BACKEND_UNAVAILABLE: remote X11 host '{}' is unsupported in safe raw mode",
                host
            ));
        }
        let after = &trimmed[colon + 1..];
        let (num_s, scr) = match after.split_once('.') {
            Some((n, s)) => (n, s.parse::<u16>().unwrap_or(0)),
            None => (after, 0),
        };
        let dnum: u16 = num_s.parse().map_err(|_| {
            format!("E_BACKEND_UNAVAILABLE: invalid X11 DISPLAY '{}'", display)
        })?;
        return Ok((
            X11Target::UnixSocket {
                path: format!("/tmp/.X11-unix/X{}", dnum),
                screen: scr,
            },
            dnum.to_string(),
        ));
    }
    if trimmed.starts_with('/') {
        return Ok((
            X11Target::UnixSocket {
                path: trimmed.to_string(),
                screen: 0,
            },
            "0".to_string(),
        ));
    }
    Err(format!(
        "E_BACKEND_UNAVAILABLE: unsupported X11 DISPLAY '{}'",
        display
    ))
}

/// Parses `.Xauthority` binary records looking for `MIT-MAGIC-COOKIE-1` on Local (`256`) or Wild (`65535`).
pub fn parse_xauthority(data: &[u8], display_num: &str) -> Option<(String, Vec<u8>)> {
    let mut pos = 0usize;
    let read_u16_be = |buf: &[u8], p: &mut usize| -> Option<u16> {
        let s = buf.get(*p..*p + 2)?;
        *p += 2;
        Some(u16::from_be_bytes([s[0], s[1]]))
    };
    let read_counted = |buf: &[u8], p: &mut usize| -> Option<Vec<u8>> {
        let len = read_u16_be(buf, p)? as usize;
        let slice = buf.get(*p..*p + len)?;
        *p += len;
        Some(slice.to_vec())
    };
    while pos + 10 <= data.len() {
        let family = read_u16_be(data, &mut pos)?;
        let _addr = read_counted(data, &mut pos)?;
        let number = read_counted(data, &mut pos)?;
        let name = read_counted(data, &mut pos)?;
        let auth_data = read_counted(data, &mut pos)?;
        let num_str = String::from_utf8_lossy(&number);
        let name_str = String::from_utf8_lossy(&name);
        if (family == 256 || family == 65535)
            && (num_str.is_empty() || num_str == display_num)
            && name_str == "MIT-MAGIC-COOKIE-1"
        {
            return Some((name_str.into_owned(), auth_data));
        }
    }
    None
}

#[inline]
fn pad4(n: usize) -> usize {
    (4 - (n & 3)) & 3
}

pub fn encode_setup_request(auth_name: &str, auth_data: &[u8]) -> Vec<u8> {
    let npad = pad4(auth_name.len());
    let dpad = pad4(auth_data.len());
    let mut out = Vec::with_capacity(12 + auth_name.len() + npad + auth_data.len() + dpad);
    out.push(b'l'); // Little endian byte order
    out.push(0);
    out.extend_from_slice(&11u16.to_le_bytes()); // major 11
    out.extend_from_slice(&0u16.to_le_bytes()); // minor 0
    out.extend_from_slice(&(auth_name.len() as u16).to_le_bytes());
    out.extend_from_slice(&(auth_data.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(auth_name.as_bytes());
    out.resize(out.len() + npad, 0);
    out.extend_from_slice(auth_data);
    out.resize(out.len() + dpad, 0);
    out
}

pub fn parse_setup_reply_body(body: &[u8], screen_idx: u16) -> Result<X11VisualInfo, String> {
    if body.len() < 32 {
        return Err("E_BACKEND_UNAVAILABLE: X11 setup reply too short".to_string());
    }
    let resource_id_base = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
    let resource_id_mask = u32::from_le_bytes([body[8], body[9], body[10], body[11]]);
    let vendor_len = u16::from_le_bytes([body[16], body[17]]) as usize;
    let max_request_quads = (u16::from_le_bytes([body[18], body[19]]) as usize).max(256);
    let num_screens = body[20];
    let num_formats = body[21] as usize;
    let mut pos = 32 + vendor_len + pad4(vendor_len);

    let mut bpp = 32u8;
    for _ in 0..num_formats {
        if pos + 8 > body.len() {
            return Err("E_BACKEND_UNAVAILABLE: truncated X11 pixmap formats".to_string());
        }
        if body[pos] == 24 || body[pos] == 32 {
            bpp = body[pos + 1];
        }
        pos += 8;
    }

    for si in 0..num_screens {
        if pos + 40 > body.len() {
            return Err("E_BACKEND_UNAVAILABLE: truncated X11 screen record".to_string());
        }
        let root_window = u32::from_le_bytes([body[pos], body[pos + 1], body[pos + 2], body[pos + 3]]);
        let white_pixel =
            u32::from_le_bytes([body[pos + 8], body[pos + 9], body[pos + 10], body[pos + 11]]);
        let black_pixel =
            u32::from_le_bytes([body[pos + 12], body[pos + 13], body[pos + 14], body[pos + 15]]);
        let root_visual =
            u32::from_le_bytes([body[pos + 32], body[pos + 33], body[pos + 34], body[pos + 35]]);
        let root_depth = body[pos + 38];
        let num_depths = body[pos + 39] as usize;
        pos += 40;

        let mut found_truecolor = false;
        for _ in 0..num_depths {
            if pos + 8 > body.len() {
                break;
            }
            let d = body[pos];
            let num_vis = u16::from_le_bytes([body[pos + 2], body[pos + 3]]) as usize;
            pos += 8;
            for _ in 0..num_vis {
                if pos + 24 > body.len() {
                    break;
                }
                let vid = u32::from_le_bytes([body[pos], body[pos + 1], body[pos + 2], body[pos + 3]]);
                let class = body[pos + 4]; // 4 = TrueColor
                if vid == root_visual && class == 4 && (d == 24 || d == 32) {
                    found_truecolor = true;
                }
                pos += 24;
            }
        }

        if u16::from(si) == screen_idx {
            if (root_depth != 24 && root_depth != 32) || !found_truecolor {
                return Err(format!(
                    "E_BACKEND_UNAVAILABLE: X11 visual must be TrueColor depth 24/32 (got depth {})",
                    root_depth
                ));
            }
            return Ok(X11VisualInfo {
                root_window,
                white_pixel,
                black_pixel,
                root_visual,
                root_depth,
                bits_per_pixel: bpp,
                max_request_quads,
                resource_id_base,
                resource_id_mask,
            });
        }
    }
    Err("E_BACKEND_UNAVAILABLE: X11 screen index not found".to_string())
}

pub fn encode_put_image_rows(
    drawable: u32,
    gc: u32,
    width: u16,
    height: u16,
    dst_y: i16,
    depth: u8,
    argb_pixels: &[u32],
) -> Vec<u8> {
    let row_bytes = (width as usize) * 4;
    let payload_bytes = row_bytes * (height as usize);
    let pad = pad4(payload_bytes);
    let total_bytes = 24 + payload_bytes + pad;
    let req_quads = (total_bytes / 4) as u16;
    let mut out = Vec::with_capacity(total_bytes);
    out.push(72); // PutImage opcode
    out.push(2); // ZPixmap
    out.extend_from_slice(&req_quads.to_le_bytes());
    out.extend_from_slice(&drawable.to_le_bytes());
    out.extend_from_slice(&gc.to_le_bytes());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&0i16.to_le_bytes()); // dst-x
    out.extend_from_slice(&dst_y.to_le_bytes());
    out.push(0); // left-pad
    out.push(depth);
    out.extend_from_slice(&0u16.to_le_bytes());
    for &px in argb_pixels {
        out.extend_from_slice(&px.to_le_bytes());
    }
    out.resize(out.len() + pad, 0);
    out
}

pub fn decode_x11_event(pkt: &[u8; 32], wm_delete_atom: u32) -> X11Event {
    let code = pkt[0] & 0x7F;
    let x = i16::from_le_bytes([pkt[24], pkt[25]]);
    let y = i16::from_le_bytes([pkt[26], pkt[27]]);
    match code {
        2 => X11Event::KeyPress {
            keycode: pkt[1],
            x,
            y,
        },
        3 => X11Event::KeyRelease {
            keycode: pkt[1],
            x,
            y,
        },
        4 => X11Event::ButtonPress {
            button: pkt[1],
            x,
            y,
        },
        5 => X11Event::ButtonRelease {
            button: pkt[1],
            x,
            y,
        },
        6 => X11Event::Motion { x, y },
        12 => X11Event::Expose,
        22 => X11Event::Configure {
            width: u16::from_le_bytes([pkt[20], pkt[21]]),
            height: u16::from_le_bytes([pkt[22], pkt[23]]),
        },
        33 => {
            let atom = u32::from_le_bytes([pkt[12], pkt[13], pkt[14], pkt[15]]);
            if atom == wm_delete_atom && wm_delete_atom != 0 {
                X11Event::ClientDelete
            } else {
                X11Event::None
            }
        }
        _ => X11Event::None,
    }
}

/// Static keysym mapping aligned with Askane key names / VK codes.
pub fn keysym_to_vk(keysym: u32) -> Option<i32> {
    if (b'a' as u32..=b'z' as u32).contains(&keysym) {
        return Some((keysym as u8).to_ascii_uppercase() as i32);
    }
    if (b'A' as u32..=b'Z' as u32).contains(&keysym) || (b'0' as u32..=b'9' as u32).contains(&keysym) {
        return Some(keysym as i32);
    }
    match keysym {
        0x0020 => input::key_str_to_vk("space").ok(),
        0xFF0D => input::key_str_to_vk("enter").ok(),
        0xFF1B => input::key_str_to_vk("escape").ok(),
        0xFF09 => input::key_str_to_vk("tab").ok(),
        0xFF08 => input::key_str_to_vk("backspace").ok(),
        0xFFE1 | 0xFFE2 => input::key_str_to_vk("shift").ok(),
        0xFFE3 | 0xFFE4 => input::key_str_to_vk("ctrl").ok(),
        0xFFE9 | 0xFFEA => input::key_str_to_vk("alt").ok(),
        0xFF51 => input::key_str_to_vk("left").ok(),
        0xFF52 => input::key_str_to_vk("up").ok(),
        0xFF53 => input::key_str_to_vk("right").ok(),
        0xFF54 => input::key_str_to_vk("down").ok(),
        0xFF50 => input::key_str_to_vk("home").ok(),
        0xFF57 => input::key_str_to_vk("end").ok(),
        0xFFBE..=0xFFC9 => Some(0x70 + (keysym - 0xFFBE) as i32),
        _ => None,
    }
}

/// Best-effort static X11/evdev keycode (scancode + 8) fallback to Askane VK codes.
#[allow(dead_code)]
pub fn x11_keycode_to_vk(keycode: u8) -> Option<i32> {
    match keycode {
        9 => Some(0x1B),  // Escape
        10..=18 => Some((b'1' + (keycode - 10)) as i32),
        19 => Some(b'0' as i32),
        22 => Some(0x08), // Backspace
        23 => Some(0x09), // Tab
        24 => Some(b'Q' as i32),
        25 => Some(b'W' as i32),
        26 => Some(b'E' as i32),
        27 => Some(b'R' as i32),
        28 => Some(b'T' as i32),
        29 => Some(b'Y' as i32),
        30 => Some(b'U' as i32),
        31 => Some(b'I' as i32),
        32 => Some(b'O' as i32),
        33 => Some(b'P' as i32),
        36 => Some(0x0D), // Return
        37 | 105 => Some(0x11), // Ctrl
        38 => Some(b'A' as i32),
        39 => Some(b'S' as i32),
        40 => Some(b'D' as i32),
        41 => Some(b'F' as i32),
        42 => Some(b'G' as i32),
        43 => Some(b'H' as i32),
        44 => Some(b'J' as i32),
        45 => Some(b'K' as i32),
        46 => Some(b'L' as i32),
        50 | 62 => Some(0x10), // Shift
        52 => Some(b'Z' as i32),
        53 => Some(b'X' as i32),
        54 => Some(b'C' as i32),
        55 => Some(b'V' as i32),
        56 => Some(b'B' as i32),
        57 => Some(b'N' as i32),
        58 => Some(b'M' as i32),
        65 => Some(0x20), // Space
        111 => Some(0x26), // Up
        113 => Some(0x25), // Left
        114 => Some(0x27), // Right
        116 => Some(0x28), // Down
        67..=76 => Some(0x70 + (keycode - 67) as i32), // F1..F10
        95..=96 => Some(0x7A + (keycode - 95) as i32), // F11..F12
        _ => None,
    }
}

#[cfg(target_os = "linux")]
mod linux_client {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    use super::*;

    pub struct X11Session {
        stream: UnixStream,
        pub visual: X11VisualInfo,
        pub wid: u32,
        pub gc: u32,
        pub wm_delete_atom: u32,
        next_id: u32,
        next_seq: u16,
    }

    impl X11Session {
        fn alloc_id(&mut self) -> u32 {
            self.next_id += 1;
            self.visual.resource_id_base | (self.next_id & self.visual.resource_id_mask)
        }

        pub fn connect(title: &str, width: u16, height: u16) -> Result<Self, String> {
            let display = std::env::var("DISPLAY")
                .map_err(|_| "E_BACKEND_UNAVAILABLE: DISPLAY is not set (no X11 server)".to_string())?;
            let (X11Target::UnixSocket { path, screen }, dnum) = parse_display(&display)?;
            let mut stream = UnixStream::connect(&path).map_err(|e| {
                format!(
                    "E_BACKEND_UNAVAILABLE: cannot connect X11 socket '{}': {}",
                    path, e
                )
            })?;
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));

            let mut auth_pair = (String::new(), Vec::new());
            if let Ok(xauth_path) = std::env::var("XAUTHORITY").or_else(|_| {
                std::env::var("HOME").map(|h| format!("{}/.Xauthority", h))
            }) {
                if let Ok(bytes) = std::fs::read(&xauth_path) {
                    if let Some(found) = parse_xauthority(&bytes, &dnum) {
                        auth_pair = found;
                    }
                }
            }

            let req = encode_setup_request(&auth_pair.0, &auth_pair.1);
            stream
                .write_all(&req)
                .map_err(|e| format!("E_BACKEND_UNAVAILABLE: X11 setup write: {}", e))?;

            let mut hdr = [0u8; 8];
            stream
                .read_exact(&mut hdr)
                .map_err(|e| format!("E_BACKEND_UNAVAILABLE: X11 setup reply: {}", e))?;
            let extra_quads = u16::from_le_bytes([hdr[6], hdr[7]]) as usize;
            let mut body = vec![0u8; extra_quads.min(65535) * 4];
            stream
                .read_exact(&mut body)
                .map_err(|e| format!("E_BACKEND_UNAVAILABLE: X11 setup body: {}", e))?;
            if hdr[0] != 1 {
                return Err("E_BACKEND_UNAVAILABLE: X11 server rejected connection".to_string());
            }

            let visual = parse_setup_reply_body(&body, screen)?;
            let mut sess = Self {
                stream,
                visual,
                wid: 0,
                gc: 0,
                wm_delete_atom: 0,
                next_id: 1,
                next_seq: 1,
            };
            let wid = sess.alloc_id();
            let gc = sess.alloc_id();
            sess.wid = wid;
            sess.gc = gc;

            // CreateWindow (opcode 1)
            let ev_mask: u32 = 0x0000_0001 // KeyPress
                | 0x0000_0002 // KeyRelease
                | 0x0000_0004 // ButtonPress
                | 0x0000_0008 // ButtonRelease
                | 0x0000_0040 // PointerMotion
                | 0x0000_8000 // Exposure
                | 0x0002_0000; // StructureNotify
            let mut cw = Vec::with_capacity(40);
            cw.push(1);
            cw.push(sess.visual.root_depth);
            cw.extend_from_slice(&10u16.to_le_bytes()); // 10 quads = 40 bytes (2 value items)
            cw.extend_from_slice(&wid.to_le_bytes());
            cw.extend_from_slice(&sess.visual.root_window.to_le_bytes());
            cw.extend_from_slice(&0i16.to_le_bytes());
            cw.extend_from_slice(&0i16.to_le_bytes());
            cw.extend_from_slice(&width.to_le_bytes());
            cw.extend_from_slice(&height.to_le_bytes());
            cw.extend_from_slice(&0u16.to_le_bytes()); // border
            cw.extend_from_slice(&1u16.to_le_bytes()); // InputOutput
            cw.extend_from_slice(&sess.visual.root_visual.to_le_bytes());
            cw.extend_from_slice(&0x0000_0802u32.to_le_bytes()); // BackPixel | EventMask
            cw.extend_from_slice(&sess.visual.black_pixel.to_le_bytes());
            cw.extend_from_slice(&ev_mask.to_le_bytes());
            sess.write_req(&cw)?;

            // CreateGC (opcode 55)
            let mut cgc = Vec::with_capacity(16);
            cgc.push(55);
            cgc.push(0);
            cgc.extend_from_slice(&4u16.to_le_bytes());
            cgc.extend_from_slice(&gc.to_le_bytes());
            cgc.extend_from_slice(&wid.to_le_bytes());
            cgc.extend_from_slice(&0u32.to_le_bytes());
            sess.write_req(&cgc)?;

            // Set title via ChangeProperty(WM_NAME=39, STRING=31)
            sess.set_title(title)?;

            // MapWindow (opcode 8)
            let mut map = Vec::with_capacity(8);
            map.push(8);
            map.push(0);
            map.extend_from_slice(&2u16.to_le_bytes());
            map.extend_from_slice(&wid.to_le_bytes());
            sess.write_req(&map)?;
            let _ = sess.stream.flush();
            let _ = sess.stream.set_read_timeout(Some(Duration::from_millis(8)));
            Ok(sess)
        }

        fn write_req(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.next_seq = self.next_seq.wrapping_add(1);
            self.stream
                .write_all(bytes)
                .map_err(|e| format!("E_BACKEND_UNAVAILABLE: X11 write failed: {}", e))
        }

        pub fn set_title(&mut self, title: &str) -> Result<(), String> {
            let tbytes = title.as_bytes();
            let pad = pad4(tbytes.len());
            let total = 24 + tbytes.len() + pad;
            let mut req = Vec::with_capacity(total);
            req.push(18); // ChangeProperty
            req.push(0); // Replace
            req.extend_from_slice(&((total / 4) as u16).to_le_bytes());
            req.extend_from_slice(&self.wid.to_le_bytes());
            req.extend_from_slice(&39u32.to_le_bytes()); // WM_NAME
            req.extend_from_slice(&31u32.to_le_bytes()); // STRING
            req.push(8);
            req.extend_from_slice(&[0u8; 3]);
            req.extend_from_slice(&(tbytes.len() as u32).to_le_bytes());
            req.extend_from_slice(tbytes);
            req.resize(total, 0);
            self.write_req(&req)
        }

        pub fn present(&mut self, w: i32, h: i32, pixels: &[u32]) -> Result<(), String> {
            if w <= 0 || h <= 0 {
                return Ok(());
            }
            let wu = w as usize;
            let max_bytes = (self.visual.max_request_quads * 4).saturating_sub(64).max(4096);
            let max_rows = (max_bytes / (wu * 4)).max(1).min(h as usize);
            let mut y = 0usize;
            while y < h as usize {
                let rows = (h as usize - y).min(max_rows);
                let slice = &pixels[y * wu..(y + rows) * wu];
                let pkt = encode_put_image_rows(
                    self.wid,
                    self.gc,
                    w as u16,
                    rows as u16,
                    y as i16,
                    self.visual.root_depth,
                    slice,
                );
                self.write_req(&pkt)?;
                y += rows;
            }
            let _ = self.stream.flush();
            Ok(())
        }

        pub fn poll_events(&mut self) -> Vec<X11Event> {
            let mut out = Vec::new();
            let mut pkt = [0u8; 32];
            let _ = self.stream.set_nonblocking(true);
            for _ in 0..64 {
                match self.stream.read_exact(&mut pkt) {
                    Ok(()) => out.push(decode_x11_event(&pkt, self.wm_delete_atom)),
                    Err(_) => break,
                }
            }
            let _ = self.stream.set_nonblocking(false);
            let _ = self.stream.set_read_timeout(Some(Duration::from_millis(8)));
            out
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux_client::X11Session;

use std::cell::RefCell;
use std::collections::HashSet;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

thread_local! {
    static PREV_KEYS: RefCell<HashSet<i32>> = RefCell::new(HashSet::new());
}

pub fn reset_state() {
    PREV_KEYS.with(|s| s.borrow_mut().clear());
}

pub fn module() -> Value {
    build_module(
        "input",
        &[
            ("vk_code", mod_vk_code),
            ("key_name", mod_key_name),
            ("key_down", mod_key_down),
            ("key_pressed", mod_key_pressed),
            ("hotkey", mod_hotkey),
            ("mouse_down", mod_mouse_down),
            ("mouse_pos", mod_mouse_pos),
        ],
        &[],
    )
}

pub(crate) fn parse_key_spec(val: &Value) -> Result<i32, String> {
    match val {
        Value::Int(vk) => {
            if (1..=255).contains(vk) {
                Ok(*vk as i32)
            } else {
                Err(format!("Virtual key code out of range (1..255): {}", vk))
            }
        }
        Value::Str(s) => key_str_to_vk(s),
        other => Err(format!(
            "Expected key name string or VK integer, got '{}'",
            other.type_name()
        )),
    }
}

pub(crate) fn key_str_to_vk(name: &str) -> Result<i32, String> {
    let lower = name.trim().to_ascii_lowercase();
    if lower.len() == 1 {
        let ch = lower.as_bytes()[0];
        if ch.is_ascii_lowercase() {
            return Ok(ch.to_ascii_uppercase() as i32);
        }
        if ch.is_ascii_digit() {
            return Ok(ch as i32);
        }
    }
    match lower.as_str() {
        "space" | " " => Ok(0x20),
        "enter" | "return" => Ok(0x0D),
        "esc" | "escape" => Ok(0x1B),
        "tab" => Ok(0x09),
        "backspace" | "bksp" => Ok(0x08),
        "shift" | "lshift" => Ok(0x10),
        "rshift" => Ok(0xA1),
        "ctrl" | "control" | "lctrl" => Ok(0x11),
        "rctrl" => Ok(0xA3),
        "alt" | "menu" | "lalt" => Ok(0x12),
        "ralt" => Ok(0xA5),
        "left" => Ok(0x25),
        "up" => Ok(0x26),
        "right" => Ok(0x27),
        "down" => Ok(0x28),
        "home" => Ok(0x24),
        "end" => Ok(0x23),
        "pageup" | "pgup" => Ok(0x21),
        "pagedown" | "pgdn" => Ok(0x22),
        "insert" | "ins" => Ok(0x2D),
        "delete" | "del" => Ok(0x2E),
        "capslock" => Ok(0x14),
        "f1" => Ok(0x70),
        "f2" => Ok(0x71),
        "f3" => Ok(0x72),
        "f4" => Ok(0x73),
        "f5" => Ok(0x74),
        "f6" => Ok(0x75),
        "f7" => Ok(0x76),
        "f8" => Ok(0x77),
        "f9" => Ok(0x78),
        "f10" => Ok(0x79),
        "f11" => Ok(0x7A),
        "f12" => Ok(0x7B),
        _ => Err(format!("Unknown key name: '{}'", name)),
    }
}

pub(crate) fn vk_to_key_name(vk: i32) -> Option<String> {
    if (b'A' as i32..=b'Z' as i32).contains(&vk) {
        return Some(((vk as u8).to_ascii_lowercase() as char).to_string());
    }
    if (b'0' as i32..=b'9' as i32).contains(&vk) {
        return Some((vk as u8 as char).to_string());
    }
    let name = match vk {
        0x20 => "space",
        0x0D => "enter",
        0x1B => "escape",
        0x09 => "tab",
        0x08 => "backspace",
        0x10 => "shift",
        0x11 => "ctrl",
        0x12 => "alt",
        0x25 => "left",
        0x26 => "up",
        0x27 => "right",
        0x28 => "down",
        0x24 => "home",
        0x23 => "end",
        0x21 => "pageup",
        0x22 => "pagedown",
        0x2D => "insert",
        0x2E => "delete",
        0x14 => "capslock",
        0x70 => "f1",
        0x71 => "f2",
        0x72 => "f3",
        0x73 => "f4",
        0x74 => "f5",
        0x75 => "f6",
        0x76 => "f7",
        0x77 => "f8",
        0x78 => "f9",
        0x79 => "f10",
        0x7A => "f11",
        0x7B => "f12",
        _ => return None,
    };
    Some(name.to_string())
}

fn mod_vk_code(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("input.vk_code() expects 1 argument (name)".to_string());
    }
    Ok(Value::Int(parse_key_spec(&args[0])? as i64))
}

fn mod_key_name(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("input.key_name() expects 1 integer argument (vk)".to_string());
    }
    let vk = match &args[0] {
        Value::Int(i) => *i as i32,
        other => {
            return Err(format!(
                "input.key_name() expects int, got '{}'",
                other.type_name()
            ));
        }
    };
    Ok(match vk_to_key_name(vk) {
        Some(s) => Value::Str(s),
        None => Value::Unit,
    })
}

#[cfg(windows)]
mod platform {
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetAsyncKeyState(vkey: i32) -> i16;
        fn GetCursorPos(lp_point: *mut Point) -> i32;
    }

    pub fn async_key_down(vk: i32) -> bool {
        unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
    }

    pub fn cursor_pos() -> Result<(i32, i32), String> {
        let mut pt = Point { x: 0, y: 0 };
        let ok = unsafe { GetCursorPos(&mut pt as *mut Point) };
        if ok == 0 {
            return Err("Failed to query global cursor position".to_string());
        }
        Ok((pt.x, pt.y))
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn async_key_down(_vk: i32) -> bool {
        false
    }

    pub fn cursor_pos() -> Result<(i32, i32), String> {
        Err("Global cursor position is only supported on Windows".to_string())
    }
}

fn mod_key_down(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_input()?;
    if args.len() != 1 {
        return Err("input.key_down() expects 1 argument (key)".to_string());
    }
    let vk = parse_key_spec(&args[0])?;
    Ok(Value::Bool(platform::async_key_down(vk)))
}

fn mod_key_pressed(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_input()?;
    if args.len() != 1 {
        return Err("input.key_pressed() expects 1 argument (key)".to_string());
    }
    let vk = parse_key_spec(&args[0])?;
    let is_down = platform::async_key_down(vk);
    let triggered = PREV_KEYS.with(|prev| {
        let mut set = prev.borrow_mut();
        if is_down {
            set.insert(vk)
        } else {
            set.remove(&vk);
            false
        }
    });
    Ok(Value::Bool(triggered))
}

fn mod_hotkey(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_input()?;
    if args.len() != 1 {
        return Err("input.hotkey() expects 1 string argument (e.g. \"ctrl+s\")".to_string());
    }
    let combo = match &args[0] {
        Value::Str(s) => s.as_str(),
        other => {
            return Err(format!(
                "input.hotkey() expects string, got '{}'",
                other.type_name()
            ));
        }
    };
    let parts: Vec<&str> = combo
        .split('+')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        return Err("input.hotkey() combination cannot be empty".to_string());
    }
    let mut all_down = true;
    for part in parts {
        let vk = key_str_to_vk(part)?;
        if !platform::async_key_down(vk) {
            all_down = false;
        }
    }
    Ok(Value::Bool(all_down))
}

fn mod_mouse_down(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_input()?;
    if args.len() > 1 {
        return Err("input.mouse_down() expects 0 or 1 argument (button)".to_string());
    }
    let vk = if args.is_empty() {
        0x01
    } else {
        match &args[0] {
            Value::Str(s) => match s.to_ascii_lowercase().as_str() {
                "left" | "lmb" => 0x01,
                "right" | "rmb" => 0x02,
                "middle" | "mmb" => 0x04,
                other => return Err(format!("Unknown mouse button: '{}'", other)),
            },
            Value::Int(0) => 0x01,
            Value::Int(1) => 0x02,
            Value::Int(2) => 0x04,
            other => {
                return Err(format!(
                    "input.mouse_down() expects button name or 0/1/2, got '{}'",
                    other.type_name()
                ));
            }
        }
    };
    Ok(Value::Bool(platform::async_key_down(vk)))
}

fn mod_mouse_pos(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_input()?;
    if !args.is_empty() {
        return Err("input.mouse_pos() expects 0 arguments".to_string());
    }
    let (x, y) = platform::cursor_pos()?;
    Ok(Value::List(std::rc::Rc::new(RefCell::new(vec![
        Value::Int(x as i64),
        Value::Int(y as i64),
    ]))))
}

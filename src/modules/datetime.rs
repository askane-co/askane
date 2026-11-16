use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "datetime",
        &[
            ("utc_now", dt_utc_now),
            ("from_timestamp", dt_from_timestamp),
            ("format", dt_format),
            ("parse_iso", dt_parse_iso),
        ],
        &[],
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year, m as u32, d as u32)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = y - if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m = m as u64;
    let d = d as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

fn decompose_unix(ts: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    let hour = secs / 3600;
    let minute = (secs % 3600) / 60;
    let second = secs % 60;
    (year, month, day, hour, minute, second)
}

fn format_iso(ts: i64) -> String {
    let (y, m, d, hh, mm, ss) = decompose_unix(ts);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, hh, mm, ss)
}

fn dt_utc_now(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("datetime.utc_now() expects 0 arguments".to_string());
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs() as i64;
    Ok(Value::Str(format_iso(ts)))
}

fn dt_from_timestamp(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("datetime.from_timestamp() expects 1 integer argument (seconds)".to_string());
    }
    let ts = match &args[0] {
        Value::Int(n) => *n,
        _ => return Err("datetime.from_timestamp() expects an integer".to_string()),
    };
    let (y, m, d, hh, mm, ss) = decompose_unix(ts);
    let mut map = HashMap::new();
    map.insert("year".to_string(), Value::Int(y));
    map.insert("month".to_string(), Value::Int(m as i64));
    map.insert("day".to_string(), Value::Int(d as i64));
    map.insert("hour".to_string(), Value::Int(hh as i64));
    map.insert("minute".to_string(), Value::Int(mm as i64));
    map.insert("second".to_string(), Value::Int(ss as i64));
    map.insert("iso".to_string(), Value::Str(format_iso(ts)));
    Ok(Value::Dict(Rc::new(RefCell::new(map))))
}

fn dt_format(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("datetime.format() expects 2 arguments (timestamp_sec, fmt)".to_string());
    }
    let ts = match &args[0] {
        Value::Int(n) => *n,
        _ => return Err("datetime.format() timestamp must be an integer".to_string()),
    };
    let fmt = match &args[1] {
        Value::Str(s) => s.as_str(),
        _ => return Err("datetime.format() fmt must be a string".to_string()),
    };
    let (y, m, d, hh, mm, ss) = decompose_unix(ts);
    let out = fmt
        .replace("%Y", &format!("{:04}", y))
        .replace("%m", &format!("{:02}", m))
        .replace("%d", &format!("{:02}", d))
        .replace("%H", &format!("{:02}", hh))
        .replace("%M", &format!("{:02}", mm))
        .replace("%S", &format!("{:02}", ss));
    Ok(Value::Str(out))
}

fn dt_parse_iso(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("datetime.parse_iso() expects 1 string argument".to_string());
    }
    let raw = match &args[0] {
        Value::Str(s) => s.trim(),
        _ => return Err("datetime.parse_iso() expects a string".to_string()),
    };
    let s = raw.strip_suffix('Z').unwrap_or(raw);
    let (date_part, time_part) = if let Some((d, t)) = s.split_once('T') {
        (d, Some(t))
    } else {
        (s, None)
    };
    let d_parts: Vec<&str> = date_part.split('-').collect();
    if d_parts.len() != 3 {
        return Err("datetime.parse_iso(): invalid date format (expected YYYY-MM-DD)".to_string());
    }
    let y: i64 = d_parts[0].parse().map_err(|_| "invalid year".to_string())?;
    let m: u32 = d_parts[1]
        .parse()
        .map_err(|_| "invalid month".to_string())?;
    let d: u32 = d_parts[2].parse().map_err(|_| "invalid day".to_string())?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err("datetime.parse_iso(): month or day out of range".to_string());
    }
    let (hh, mm, ss) = if let Some(t) = time_part {
        let t_parts: Vec<&str> = t.split(':').collect();
        if t_parts.len() != 3 {
            return Err(
                "datetime.parse_iso(): invalid time format (expected HH:MM:SS)".to_string(),
            );
        }
        let hh: u32 = t_parts[0].parse().map_err(|_| "invalid hour".to_string())?;
        let mm: u32 = t_parts[1]
            .parse()
            .map_err(|_| "invalid minute".to_string())?;
        let ss: u32 = t_parts[2]
            .parse()
            .map_err(|_| "invalid second".to_string())?;
        if hh > 23 || mm > 59 || ss > 59 {
            return Err("datetime.parse_iso(): time out of range".to_string());
        }
        (hh, mm, ss)
    } else {
        (0, 0, 0)
    };

    let days = days_from_civil(y, m, d);
    let ts = days * 86_400 + (hh as i64) * 3600 + (mm as i64) * 60 + (ss as i64);
    Ok(Value::Int(ts))
}

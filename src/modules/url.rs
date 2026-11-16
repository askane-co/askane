use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "url",
        &[
            ("parse", url_parse),
            ("encode", url_encode),
            ("decode", url_decode),
            ("query_parse", url_query_parse),
            ("query_stringify", url_query_stringify),
        ],
        &[],
    )
}

fn expect_str<'a>(v: &'a Value, fn_name: &str) -> Result<&'a str, String> {
    match v {
        Value::Str(s) => Ok(s.as_str()),
        other => Err(format!(
            "url.{}() expects a string, got '{}'",
            fn_name,
            other.type_name()
        )),
    }
}

fn pct_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else if b == b' ' {
            out.push_str("%20");
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn pct_decode(input: &str, plus_as_space: bool) -> Result<String, String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err("url.decode(): incomplete percent encoding".to_string());
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .map_err(|_| "url.decode(): invalid UTF-8".to_string())?;
            let val = u8::from_str_radix(hex, 16)
                .map_err(|_| "url.decode(): invalid hex escape".to_string())?;
            out.push(val);
            i += 3;
        } else if plus_as_space && bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out)
        .map_err(|_| "url.decode(): decoded bytes are not valid UTF-8".to_string())
}

fn url_encode(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("url.encode() expects 1 string argument".to_string());
    }
    Ok(Value::Str(pct_encode(expect_str(&args[0], "encode")?)))
}

fn url_decode(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("url.decode() expects 1 string argument".to_string());
    }
    Ok(Value::Str(pct_decode(
        expect_str(&args[0], "decode")?,
        true,
    )?))
}

fn url_query_parse(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("url.query_parse() expects 1 string argument".to_string());
    }
    let raw = expect_str(&args[0], "query_parse")?.trim_start_matches('?');
    let mut map = HashMap::new();
    if raw.is_empty() {
        return Ok(Value::Dict(Rc::new(RefCell::new(map))));
    }
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(pct_decode(k, true)?, Value::Str(pct_decode(v, true)?));
    }
    Ok(Value::Dict(Rc::new(RefCell::new(map))))
}

fn url_query_stringify(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("url.query_stringify() expects 1 dict argument".to_string());
    }
    let dict = match &args[0] {
        Value::Dict(d) => d,
        other => {
            return Err(format!(
                "url.query_stringify() expects a dict, got '{}'",
                other.type_name()
            ));
        }
    };
    let map = dict.borrow();
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    let mut parts = Vec::with_capacity(keys.len());
    for k in keys {
        let v = format!("{}", map[k]);
        parts.push(format!("{}={}", pct_encode(k), pct_encode(&v)));
    }
    Ok(Value::Str(parts.join("&")))
}

fn url_parse(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("url.parse() expects 1 string argument".to_string());
    }
    let raw = expect_str(&args[0], "parse")?;
    let (scheme, rest) = raw
        .split_once("://")
        .ok_or_else(|| "url.parse(): missing '://' scheme".to_string())?;
    let (before_frag, fragment) = match rest.split_once('#') {
        Some((a, f)) => (a, Value::Str(f.to_string())),
        None => (rest, Value::Str(String::new())),
    };
    let (before_query, query) = match before_frag.split_once('?') {
        Some((a, q)) => (a, Value::Str(q.to_string())),
        None => (before_frag, Value::Str(String::new())),
    };
    let (authority, path) = match before_query.find('/') {
        Some(idx) => (&before_query[..idx], before_query[idx..].to_string()),
        None => (before_query, "/".to_string()),
    };
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = if host_port.starts_with('[') {
        if let Some(end) = host_port.find(']') {
            let h = &host_port[..=end];
            let p = host_port[end + 1..]
                .strip_prefix(':')
                .and_then(|s| s.parse::<i64>().ok());
            (h.to_string(), p)
        } else {
            return Err("url.parse(): invalid IPv6 host".to_string());
        }
    } else if let Some((h, p)) = host_port.rsplit_once(':') {
        let port_num = p
            .parse::<i64>()
            .map_err(|_| "url.parse(): invalid port".to_string())?;
        (h.to_string(), Some(port_num))
    } else {
        (host_port.to_string(), None)
    };

    let mut out = HashMap::new();
    out.insert("scheme".to_string(), Value::Str(scheme.to_string()));
    out.insert("host".to_string(), Value::Str(host));
    out.insert(
        "port".to_string(),
        port.map(Value::Int).unwrap_or(Value::Unit),
    );
    out.insert("path".to_string(), Value::Str(path));
    out.insert("query".to_string(), query);
    out.insert("fragment".to_string(), fragment);
    Ok(Value::Dict(Rc::new(RefCell::new(out))))
}

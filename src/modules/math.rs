use std::f64::consts::{E, PI};

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "math",
        &[
            ("sqrt", m_sqrt),
            ("abs", m_abs),
            ("pow", m_pow),
            ("sin", m_sin),
            ("cos", m_cos),
            ("floor", m_floor),
            ("ceil", m_ceil),
            ("round", m_round),
            ("min", m_min),
            ("max", m_max),
            ("clamp", m_clamp),
            ("lerp", m_lerp),
            ("dist", m_dist),
            ("atan2", m_atan2),
        ],
        &[("pi", Value::Float(PI)), ("e", Value::Float(E))],
    )
}

fn to_f64(v: &Value, fn_name: &str) -> Result<f64, String> {
    match v {
        Value::Int(i) => Ok(*i as f64),
        Value::Float(f) => Ok(*f),
        other => Err(format!(
            "math.{}() expects a number, got '{}'",
            fn_name,
            other.type_name()
        )),
    }
}

fn m_sqrt(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("math.sqrt() expects 1 argument".to_string());
    }
    Ok(Value::Float(to_f64(&args[0], "sqrt")?.sqrt()))
}

fn m_abs(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("math.abs() expects 1 argument".to_string());
    }
    match &args[0] {
        Value::Int(i) => i
            .checked_abs()
            .map(Value::Int)
            .ok_or_else(|| "math.abs() integer overflow".to_string()),
        Value::Float(f) => Ok(Value::Float(f.abs())),
        other => Err(format!(
            "math.abs() expects a number, got '{}'",
            other.type_name()
        )),
    }
}

fn m_pow(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("math.pow() expects 2 arguments".to_string());
    }
    let a = to_f64(&args[0], "pow")?;
    let b = to_f64(&args[1], "pow")?;
    Ok(Value::Float(a.powf(b)))
}

fn m_sin(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("math.sin() expects 1 argument".to_string());
    }
    Ok(Value::Float(to_f64(&args[0], "sin")?.sin()))
}

fn m_cos(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("math.cos() expects 1 argument".to_string());
    }
    Ok(Value::Float(to_f64(&args[0], "cos")?.cos()))
}

fn m_floor(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("math.floor() expects 1 argument".to_string());
    }
    Ok(Value::Int(to_f64(&args[0], "floor")?.floor() as i64))
}

fn m_ceil(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("math.ceil() expects 1 argument".to_string());
    }
    Ok(Value::Int(to_f64(&args[0], "ceil")?.ceil() as i64))
}

fn m_round(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("math.round() expects 1 argument".to_string());
    }
    Ok(Value::Int(to_f64(&args[0], "round")?.round() as i64))
}

fn m_min(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("math.min() expects 2 arguments".to_string());
    }
    match (&args[0], &args[1]) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(*a.min(b))),
        _ => Ok(Value::Float(
            to_f64(&args[0], "min")?.min(to_f64(&args[1], "min")?),
        )),
    }
}

fn m_max(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("math.max() expects 2 arguments".to_string());
    }
    match (&args[0], &args[1]) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(*a.max(b))),
        _ => Ok(Value::Float(
            to_f64(&args[0], "max")?.max(to_f64(&args[1], "max")?),
        )),
    }
}

fn m_clamp(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("math.clamp() expects 3 arguments (val, min, max)".to_string());
    }
    let v = to_f64(&args[0], "clamp")?;
    let lo = to_f64(&args[1], "clamp")?;
    let hi = to_f64(&args[2], "clamp")?;
    if lo > hi {
        return Err("math.clamp() requires min <= max".to_string());
    }
    Ok(Value::Float(v.clamp(lo, hi)))
}

fn m_lerp(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("math.lerp() expects 3 arguments (a, b, t)".to_string());
    }
    let a = to_f64(&args[0], "lerp")?;
    let b = to_f64(&args[1], "lerp")?;
    let t = to_f64(&args[2], "lerp")?;
    Ok(Value::Float(a + (b - a) * t))
}

fn m_dist(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 4 {
        return Err("math.dist() expects 4 arguments (x1, y1, x2, y2)".to_string());
    }
    let x1 = to_f64(&args[0], "dist")?;
    let y1 = to_f64(&args[1], "dist")?;
    let x2 = to_f64(&args[2], "dist")?;
    let y2 = to_f64(&args[3], "dist")?;
    Ok(Value::Float(((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt()))
}

fn m_atan2(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("math.atan2() expects 2 arguments (y, x)".to_string());
    }
    Ok(Value::Float(
        to_f64(&args[0], "atan2")?.atan2(to_f64(&args[1], "atan2")?),
    ))
}

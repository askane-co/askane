mod audio;
mod base64;
mod bytes;
mod clipboard;
mod csv;
mod datetime;
mod dialog;
mod fs;
mod gfx;
mod glob;
mod hash;
mod hex;
mod http;
mod ini;
mod json;
mod log;
mod math;
mod mem;
mod path;
mod process;
mod random;
mod regex;
pub(crate) mod strings;
mod sys;
mod term;
mod test;
mod time;
mod url;
mod uuid;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::value::{NativeFunction, Value};
use crate::vm::VM;

pub type NativeFnPtr = fn(&mut VM, &[Value]) -> Result<Value, String>;

pub fn build_module(
    name: &str,
    fns: &[(&'static str, NativeFnPtr)],
    consts: &[(&str, Value)],
) -> Value {
    let mut exports = HashMap::new();
    for &(fn_name, func) in fns {
        exports.insert(
            fn_name.to_string(),
            Value::NativeFn(NativeFunction {
                name: fn_name,
                func,
            }),
        );
    }
    for (k, v) in consts {
        exports.insert((*k).to_string(), v.clone());
    }
    Value::Module {
        name: name.to_string(),
        exports: Rc::new(RefCell::new(exports)),
    }
}

pub fn reset_runtime_state() {
    gfx::reset_state();
}

pub fn get_builtin_module(name: &str) -> Option<Value> {
    match name {
        "math" => Some(math::module()),
        "fs" => Some(fs::module()),
        "sys" => Some(sys::module()),
        "mem" => Some(mem::module()),
        "json" => Some(json::module()),
        "http" => Some(http::module()),
        "gfx" => Some(gfx::module()),
        "strings" => Some(strings::module()),
        "path" => Some(path::module()),
        "time" => Some(time::module()),
        "random" => Some(random::module()),
        "base64" => Some(base64::module()),
        "bytes" => Some(bytes::module()),
        "csv" => Some(csv::module()),
        "log" => Some(log::module()),
        "test" => Some(test::module()),
        "hash" => Some(hash::module()),
        "ini" => Some(ini::module()),
        "datetime" => Some(datetime::module()),
        "clipboard" => Some(clipboard::module()),
        "audio" => Some(audio::module()),
        "url" => Some(url::module()),
        "hex" => Some(hex::module()),
        "uuid" => Some(uuid::module()),
        "term" => Some(term::module()),
        "glob" => Some(glob::module()),
        "regex" => Some(regex::module()),
        "dialog" => Some(dialog::module()),
        "process" => Some(process::module()),
        _ => None,
    }
}

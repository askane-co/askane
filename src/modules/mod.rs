pub(crate) mod archive;
mod assets;
pub(crate) mod audio;
pub(crate) mod base64;
mod binary;
mod buffer;
mod bytes;
mod cli;
mod clipboard;
mod collections;
pub(crate) mod compress;
mod crypto;
mod csv;
mod datetime;
mod dialog;
pub(crate) mod diff;
mod fs;
pub(crate) mod gfx;
mod glob;
pub(crate) mod hash;
mod hex;
pub(crate) mod html;
mod http;
mod image;
mod ini;
pub(crate) mod input;
pub(crate) mod json;
mod kv;
pub(crate) mod log;
mod geom;
mod math;
mod mem;
mod net;
mod os;
mod path;
mod pprint;
mod process;
mod procgen;
pub(crate) mod random;
mod regex;
mod signal;
pub(crate) mod strings;
mod sys;
mod task;
mod term;
mod test;
mod time;
pub(crate) mod toml;
pub(crate) mod ui;
mod url;
mod uuid;
mod ws;
mod xml;
mod cubemap;
mod lighting;
mod mesh;
mod nav;
mod particles;
mod postfx;
mod scene;
mod scene_util;
mod terrain;
mod tiled;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::value::{FxBuildHasher, ModuleData, NativeFunction, Value};
use crate::vm::VM;

pub type NativeFnPtr = fn(&mut VM, &[Value]) -> Result<Value, String>;

pub fn build_module(
    name: &str,
    fns: &[(&'static str, NativeFnPtr)],
    consts: &[(&str, Value)],
) -> Value {
    let mut exports = HashMap::with_hasher(FxBuildHasher);
    for &(fn_name, func) in fns {
        exports.insert(
            fn_name.to_string(),
            Value::NativeFn(Rc::new(NativeFunction {
                name: fn_name,
                func,
            })),
        );
    }
    for (k, v) in consts {
        exports.insert((*k).to_string(), v.clone());
    }
    Value::Module(Rc::new(ModuleData {
        ic_id: crate::bytecode::next_ic_id(),
        name: name.to_string(),
        exports: Rc::new(RefCell::new(exports)),
    }))
}

pub fn reset_runtime_state() {
    fs::reset_state();
    gfx::reset_state();
    crate::audio_backend::reset_audio_state();
    crate::media::reset_media_state();
    input::reset_state();
    ui::reset_state();
    kv::reset_state();
    net::reset_state();
    ws::reset_state();
    procgen::reset_state();
    mesh::reset_state();
    tiled::reset_state();
    terrain::reset_state();
    nav::reset_state();
    lighting::reset_state();
    postfx::reset_state();
    particles::reset_state();
    cubemap::reset_state();
    scene::reset_state();
}

pub const BUILTIN_MODULE_NAMES: &[&str] = &[
    "geom",
    "math",
    "fs",
    "sys",
    "mem",
    "json",
    "http",
    "gfx",
    "image",
    "assets",
    "procgen",
    "strings",
    "path",
    "time",
    "random",
    "base64",
    "bytes",
    "buffer",
    "csv",
    "log",
    "test",
    "hash",
    "ini",
    "datetime",
    "clipboard",
    "audio",
    "url",
    "hex",
    "uuid",
    "term",
    "glob",
    "regex",
    "dialog",
    "process",
    "toml",
    "binary",
    "collections",
    "compress",
    "archive",
    "os",
    "cli",
    "diff",
    "pprint",
    "html",
    "xml",
    "input",
    "ui",
    "crypto",
    "kv",
    "net",
    "ws",
    "signal",
    "task",
    "channel",
    "mesh",
    "tiled",
    "terrain",
    "nav",
    "lighting",
    "postfx",
    "particles",
    "cubemap",
    "scene",
];

pub fn module_info(name: &str) -> (&'static str, &'static str, &'static str) {
    crate::registry::module_info_tuple(name)
}

pub fn get_builtin_module(name: &str) -> Option<Value> {
    match name {
        "geom" => Some(geom::module()),
        "mesh" => Some(mesh::module()),
        "tiled" => Some(tiled::module()),
        "terrain" => Some(terrain::module()),
        "nav" => Some(nav::module()),
        "lighting" => Some(lighting::module()),
        "postfx" => Some(postfx::module()),
        "particles" => Some(particles::module()),
        "cubemap" => Some(cubemap::module()),
        "scene" => Some(scene::module()),
        "math" => Some(math::module()),
        "fs" => Some(fs::module()),
        "sys" => Some(sys::module()),
        "mem" => Some(mem::module()),
        "json" => Some(json::module()),
        "http" => Some(http::module()),
        "gfx" => Some(gfx::module()),
        "image" => Some(image::module()),
        "assets" => Some(assets::module()),
        "procgen" => Some(procgen::module()),
        "strings" => Some(strings::module()),
        "path" => Some(path::module()),
        "time" => Some(time::module()),
        "random" => Some(random::module()),
        "base64" => Some(base64::module()),
        "bytes" => Some(bytes::module()),
        "buffer" => Some(buffer::module()),
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
        "toml" => Some(toml::module()),
        "binary" => Some(binary::module()),
        "collections" => Some(collections::module()),
        "compress" => Some(compress::module()),
        "archive" => Some(archive::module()),
        "os" => Some(os::module()),
        "cli" => Some(cli::module()),
        "diff" => Some(diff::module()),
        "pprint" => Some(pprint::module()),
        "html" => Some(html::module()),
        "xml" => Some(xml::module()),
        "input" => Some(input::module()),
        "ui" => Some(ui::module()),
        "crypto" => Some(crypto::module()),
        "kv" => Some(kv::module()),
        "net" => Some(net::module()),
        "ws" => Some(ws::module()),
        "signal" => Some(signal::module()),
        "task" => Some(task::task_module()),
        "channel" => Some(task::channel_module()),
        _ => None,
    }
}

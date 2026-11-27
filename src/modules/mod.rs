pub(crate) mod archive;
mod assets;
mod audio;
mod base64;
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
mod html;
mod http;
mod image;
mod ini;
mod input;
mod json;
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
    crate::media::reset_media_state();
    input::reset_state();
    ui::reset_state();
    kv::reset_state();
    net::reset_state();
    ws::reset_state();
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
];

pub fn module_info(name: &str) -> (&'static str, &'static str, &'static str) {
    match name {
        "math" => (
            "stdlib",
            "all",
            "Floating-point & scalar math helpers (sqrt, pow, sin, cos, clamp, lerp, atan2)",
        ),
        "fs" => (
            "io",
            "all",
            "Sandboxed filesystem & streaming I/O (read_lines, walk, copy_dir, open_lines, write_stream)",
        ),
        "sys" => (
            "runtime",
            "all",
            "CLI args, env vars, process exit, budget & sleep (gated by --allow-env/args)",
        ),
        "mem" => (
            "system",
            "windows",
            "Process memory inspection (--unsafe + ASKANE_ENABLE_UNSAFE_MEM=1)",
        ),
        "json" => (
            "data",
            "all",
            "Strict JSON parser and serializer with depth and control-char guards",
        ),
        "http" => (
            "net",
            "windows",
            "WinHTTP TLS HTTPS client (get, post; requires --allow-net)",
        ),
        "gfx" => (
            "gui",
            "all",
            "2D framebuffer graphics (Win32 window & headless mode, sprite sheets, shapes, input & text)",
        ),
        "image" => (
            "media",
            "all",
            "Safe image decoding, pixel formats, color ops, mipmaps, atlases & textures",
        ),
        "assets" => (
            "media",
            "all",
            "Unified budget-aware asset cache, VFS ZIP bundles & manifest hints",
        ),
        "procgen" => (
            "media",
            "all",
            "Deterministic fixed-point 2D noise, FBM heightmaps, normal maps & procedural patterns",
        ),
        "strings" => (
            "stdlib",
            "all",
            "UTF-8 string utilities (split, trim, replace, slice, byte_len, truncate_utf8)",
        ),
        "path" => (
            "stdlib",
            "all",
            "Path manipulation (join, parent, name, stem, extension, normalize)",
        ),
        "time" => (
            "runtime",
            "all",
            "Monotonic & wall clock helpers (now_ms, now_sec, elapsed_ms, sleep_ms)",
        ),
        "random" => (
            "stdlib",
            "all",
            "Deterministic PRNG and OS CSPRNG (crypto_int, crypto_bytes)",
        ),
        "base64" => (
            "data",
            "all",
            "Standard and URL-safe Base64 encoding and decoding",
        ),
        "bytes" => (
            "data",
            "all",
            "Byte buffer conversions and zero-allocation helpers",
        ),
        "buffer" => (
            "data",
            "all",
            "Typed binary buffers (u8/i32/u32/f32) with safe element views and BudgetCore tracking",
        ),
        "csv" => (
            "data",
            "all",
            "RFC-compatible CSV parsing, dict rows, and serialization",
        ),
        "log" => (
            "runtime",
            "all",
            "Structured leveled logger with automatic secret redaction",
        ),
        "test" => (
            "testing",
            "all",
            "Assertions (assert, assert_eq, assert_ne, assert_err, assert_snapshot)",
        ),
        "hash" => ("data", "all", "Non-keyed hashes: SHA-256, CRC32, FNV-1a"),
        "ini" => ("data", "all", "INI configuration parser and writer"),
        "datetime" => (
            "data",
            "all",
            "UTC timestamps, ISO-8601 parsing and formatting",
        ),
        "clipboard" => (
            "gui",
            "windows",
            "Windows clipboard text read/write (requires --allow-clipboard)",
        ),
        "audio" => ("media", "windows", "Sandboxed WAV playback via WinMM"),
        "url" => (
            "data",
            "all",
            "URL parsing, percent-encoding/decoding and query helpers",
        ),
        "hex" => ("data", "all", "Hexadecimal byte encoding and decoding"),
        "uuid" => ("stdlib", "all", "Random UUID v4 generation"),
        "term" => ("stdlib", "all", "ANSI terminal styling helpers"),
        "glob" => ("stdlib", "all", "Linear/DP-bounded glob pattern matching"),
        "regex" => ("stdlib", "all", "Step-bounded regular expression engine"),
        "dialog" => (
            "gui",
            "windows",
            "Native Win32 alert, confirm and open/save dialogs",
        ),
        "process" => (
            "system",
            "all",
            "Subprocess execution in Job Object (requires --allow-exec)",
        ),
        "toml" => ("data", "all", "Bounded TOML parser and serializer"),
        "binary" => (
            "data",
            "all",
            "Binary struct pack/unpack and typed integer/float codecs",
        ),
        "collections" => (
            "stdlib",
            "all",
            "deque, counter, priority_queue, groupby, window & sequence helpers",
        ),
        "compress" => (
            "data",
            "all",
            "Pure-Rust DEFLATE, Zlib, Gzip compression, CRC32 and Adler32",
        ),
        "archive" => (
            "io",
            "all",
            "Sandboxed ZIP pack/create/list/read_entry/extract",
        ),
        "os" => (
            "system",
            "all",
            "Host OS metadata, cwd, home/temp dirs, env & args",
        ),
        "cli" => (
            "stdlib",
            "all",
            "Command-line flags & positional arguments parser",
        ),
        "diff" => ("testing", "all", "LCS line and value diff formatter"),
        "pprint" => ("stdlib", "all", "Pretty formatter for nested structures"),
        "html" => (
            "data",
            "all",
            "HTML parser, selector, text extraction & escaping",
        ),
        "xml" => ("data", "all", "XML tree parser, selector & escaping"),
        "input" => (
            "gui",
            "windows",
            "Global key/mouse polling and hotkeys (requires --allow-input)",
        ),
        "ui" => (
            "gui",
            "windows",
            "Declarative 2D UI layout and interactive widgets",
        ),
        "crypto" => (
            "crypto",
            "all",
            "HMAC-SHA256, constant_time_eq, random_key, ChaCha20-Poly1305 AEAD",
        ),
        "kv" => (
            "io",
            "all",
            "Embedded append-only CRC32 KV store (sandbox-checked)",
        ),
        "net" => (
            "net",
            "all",
            "Sandboxed TCP & UDP sockets (requires --allow-net)",
        ),
        "ws" => (
            "net",
            "all",
            "RFC 6455 WebSocket server & client (requires --allow-net)",
        ),
        "signal" => (
            "runtime",
            "all",
            "Reactive signal state cells with on change observers",
        ),
        "task" => (
            "runtime",
            "all",
            "Cooperative green tasks (spawn, yield_now, sleep, join, select, timeout)",
        ),
        "channel" => ("runtime", "all", "Bounded channels with deadlock detection"),
        "geom" => ("math", "all", "2D/3D vectors, matrices, quaternions, rays, AABBs and frustums"),
        _ => ("other", "all", ""),
    }
}

pub fn get_builtin_module(name: &str) -> Option<Value> {
    match name {
        "geom" => Some(geom::module()),
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

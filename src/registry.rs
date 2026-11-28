#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::diag;
use crate::modules::{BUILTIN_MODULE_NAMES, get_builtin_module};

#[derive(Clone, Copy, Debug)]
pub struct ModuleDoc {
    pub name: &'static str,
    pub category: &'static str,
    pub platform: &'static str,
    pub stability: &'static str,
    pub summary: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub struct FnDoc {
    pub module: &'static str,
    pub name: &'static str,
    pub signature: &'static str,
    pub summary: &'static str,
    pub capability: Option<&'static str>,
}

#[derive(Clone, Copy, Debug)]
pub struct KeywordDoc {
    pub word: &'static str,
    pub kind: &'static str,
    pub summary: &'static str,
}

const fn md(
    name: &'static str,
    category: &'static str,
    platform: &'static str,
    summary: &'static str,
) -> ModuleDoc {
    ModuleDoc {
        name,
        category,
        platform,
        stability: "stable",
        summary,
    }
}

const fn fd(
    module: &'static str,
    name: &'static str,
    signature: &'static str,
    summary: &'static str,
) -> FnDoc {
    FnDoc {
        module,
        name,
        signature,
        summary,
        capability: None,
    }
}

const fn fdc(
    module: &'static str,
    name: &'static str,
    signature: &'static str,
    summary: &'static str,
    capability: &'static str,
) -> FnDoc {
    FnDoc {
        module,
        name,
        signature,
        summary,
        capability: Some(capability),
    }
}

pub const KEYWORDS: &[KeywordDoc] = &[
    KeywordDoc { word: "let", kind: "keyword", summary: "Bind a local or module variable (supports list/dict destructuring)" },
    KeywordDoc { word: "fn", kind: "keyword", summary: "Declare a named function (`pub fn` exports from a module)" },
    KeywordDoc { word: "struct", kind: "keyword", summary: "Declare a struct record type with typed defaults and methods" },
    KeywordDoc { word: "if", kind: "keyword", summary: "Conditional expression (`if cond { ... } else { ... }`)" },
    KeywordDoc { word: "else", kind: "keyword", summary: "Fallback branch for `if` expressions" },
    KeywordDoc { word: "match", kind: "keyword", summary: "Pattern matching expression with ranges, guards (`if`), and `_`" },
    KeywordDoc { word: "while", kind: "keyword", summary: "Pre-condition loop (`while cond { ... }`)" },
    KeywordDoc { word: "loop", kind: "keyword", summary: "Unconditional loop exited via `break`" },
    KeywordDoc { word: "for", kind: "keyword", summary: "Iterate over sequences, ranges, or `(k, v)` pairs" },
    KeywordDoc { word: "in", kind: "keyword", summary: "Membership operator (`x in seq`) and `for` loop binder" },
    KeywordDoc { word: "import", kind: "keyword", summary: "Import a built-in module or relative `.ask` file" },
    KeywordDoc { word: "from", kind: "keyword", summary: "Selective symbol import (`from mod import a, b as c;`)" },
    KeywordDoc { word: "pub", kind: "keyword", summary: "Export function, struct, or binding from a `.ask` module" },
    KeywordDoc { word: "as", kind: "keyword", summary: "Rename an imported module or symbol" },
    KeywordDoc { word: "return", kind: "keyword", summary: "Return early from the enclosing function" },
    KeywordDoc { word: "break", kind: "keyword", summary: "Exit the innermost loop cleanly" },
    KeywordDoc { word: "continue", kind: "keyword", summary: "Jump to the next iteration of the innermost loop" },
    KeywordDoc { word: "try", kind: "keyword", summary: "Guarded expression block paired with `catch`" },
    KeywordDoc { word: "catch", kind: "keyword", summary: "Handle recoverable runtime errors thrown inside `try`" },
    KeywordDoc { word: "throw", kind: "keyword", summary: "Raise a recoverable runtime error value" },
    KeywordDoc { word: "recover", kind: "keyword", summary: "Provide inline error fallback expression" },
    KeywordDoc { word: "true", kind: "keyword", summary: "Boolean literal true" },
    KeywordDoc { word: "false", kind: "keyword", summary: "Boolean literal false" },
    KeywordDoc { word: "unit", kind: "keyword", summary: "Empty sentinel value (`unit` / `()`)" },
    KeywordDoc { word: "needs", kind: "contextual", summary: "Declare static capability requirements (`needs fs, net;`) checked at runtime" },
    KeywordDoc { word: "defer", kind: "contextual", summary: "Schedule cleanup block on scope exit even when errors unwind" },
    KeywordDoc { word: "every", kind: "contextual", summary: "Periodic interval timer block (`every 16ms { ... }`)" },
    KeywordDoc { word: "on", kind: "contextual", summary: "Reactive observer header paired with `change(sig)`" },
    KeywordDoc { word: "change", kind: "contextual", summary: "Observer clause (`on change(signal) { ... }`)" },
    KeywordDoc { word: "spec", kind: "contextual", summary: "Inline unit specification block run by `askane test`" },
    KeywordDoc { word: "sandbox", kind: "contextual", summary: "Scoped nested policy block narrowing permissions and resource budgets" },
    KeywordDoc { word: "with", kind: "contextual", summary: "Scoped RAII resource binding closed deterministically on exit" },
    KeywordDoc { word: "after", kind: "contextual", summary: "Delayed execution block (`after 100ms { ... }`)" },
];

pub const GLOBAL_BUILTINS: &[(&str, &str, &str)] = &[
    ("print", "print(...args) -> unit", "Write space-separated formatted values to stdout without a trailing newline"),
    ("println", "println(...args) -> unit", "Write space-separated formatted values to stdout followed by newline"),
    ("read_line", "read_line() -> str", "Read one bounded line from stdin"),
    ("len", "len(x) -> int", "Return element, char, or byte length of str/bytes/list/dict/set/range"),
    ("push", "push(list, val) -> unit", "Append an item to the end of a mutable list"),
    ("pop", "pop(list) -> any", "Remove and return the last element of a list"),
    ("range", "range(end) | range(start, end) -> range", "Construct a lazy half-open integer range"),
    ("map", "map(seq, fn) -> list", "Transform each element using a function"),
    ("filter", "filter(seq, pred) -> list", "Keep elements where `pred(item)` is truthy"),
    ("reduce", "reduce(seq, init, fn) -> any", "Fold a sequence from left to right starting from `init`"),
    ("str", "str(x) -> str", "Convert any Askane value to its string representation"),
    ("repr", "repr(x) -> str", "Return unambiguous debug representation of a value"),
    ("int", "int(x) -> int", "Convert int/float/bool/numeric str to 64-bit int or throw"),
    ("try_int", "try_int(x) -> int | unit", "Safe integer conversion returning `unit` on failure"),
    ("float", "float(x) -> float", "Convert numeric/str value to IEEE-754 f64"),
    ("try_float", "try_float(x) -> float | unit", "Safe float conversion returning `unit` on failure"),
    ("bytes", "bytes(x) -> bytes", "Convert byte list, hex string, or UTF-8 string to `bytes`"),
    ("set", "set([seq]) -> set", "Create a mutable deduplicated set from an optional sequence"),
    ("dict", "dict([pairs]) -> dict", "Create a mutable dictionary optionally from `[k, v]` pairs"),
    ("type_of", "type_of(x) -> str", "Return dynamic type name (`int`, `vec2`, `list`, `image`, ...)"),
    ("keys", "keys(dict) -> list", "Return keys list from a dict"),
    ("values", "values(dict) -> list", "Return values list from a dict"),
    ("entries", "entries(dict) -> list", "Return list of `[key, value]` pairs from a dict"),
    ("get", "get(dict, key, [default]) -> any", "Lookup key with optional fallback instead of throwing"),
    ("remove", "remove(collection, key_or_idx) -> any", "Remove item from dict, list, or set"),
    ("sort", "sort(list) -> list", "Return sorted copy of a comparable list"),
    ("reverse", "reverse(list_or_str) -> list | str", "Return reversed copy of a list or string"),
    ("enumerate", "enumerate(seq) -> list", "Return `[index, value]` pairs"),
    ("zip", "zip(a, b) -> list", "Pair elements of two sequences up to shorter length"),
    ("take", "take(seq, n) -> list", "Take first `n` items from a sequence"),
    ("skip", "skip(seq, n) -> list", "Skip first `n` items from a sequence"),
    ("any", "any(seq, pred) -> bool", "Return true if any element satisfies `pred`"),
    ("all", "all(seq, pred) -> bool", "Return true if every element satisfies `pred`"),
    ("find", "find(seq, pred) -> any | unit", "Return first element matching `pred`, or `unit`"),
    ("first", "first(seq) -> any | unit", "Return first element of sequence or `unit`"),
    ("last", "last(seq) -> any | unit", "Return last element of sequence or `unit`"),
    ("is_empty", "is_empty(seq) -> bool", "Return true when collection/string has zero elements"),
    ("find_index", "find_index(seq, pred) -> int | unit", "Return 0-based index of first matching element"),
    ("count", "count(seq, pred) -> int", "Count elements satisfying predicate"),
    ("partition", "partition(seq, pred) -> list", "Split sequence into `[matching, non_matching]`"),
    ("flat_map", "flat_map(seq, fn) -> list", "Map and flatten one level"),
    ("take_while", "take_while(seq, pred) -> list", "Take prefix while `pred` holds"),
    ("drop_while", "drop_while(seq, pred) -> list", "Drop prefix while `pred` holds"),
    ("zip_longest", "zip_longest(a, b, [fill]) -> list", "Zip two sequences padding shorter tail with `fill`"),
    ("min_by", "min_by(seq, key_fn) -> any | unit", "Minimum item according to key selector"),
    ("max_by", "max_by(seq, key_fn) -> any | unit", "Maximum item according to key selector"),
    ("checked_add", "checked_add(a, b) -> int", "Overflow-checked 64-bit integer addition"),
    ("checked_sub", "checked_sub(a, b) -> int", "Overflow-checked 64-bit integer subtraction"),
    ("checked_mul", "checked_mul(a, b) -> int", "Overflow-checked 64-bit integer multiplication"),
    ("wrapping_add", "wrapping_add(a, b) -> int", "Two's-complement wrapping integer addition"),
    ("wrapping_sub", "wrapping_sub(a, b) -> int", "Two's-complement wrapping integer subtraction"),
    ("wrapping_mul", "wrapping_mul(a, b) -> int", "Two's-complement wrapping integer multiplication"),
    ("is_same", "is_same(a, b) -> bool", "Compare reference/value identity without coercion"),
    ("is_unit", "is_unit(x) -> bool", "Check if value is `unit`"),
    ("is_int", "is_int(x) -> bool", "Check if value is `int`"),
    ("is_float", "is_float(x) -> bool", "Check if value is `float`"),
    ("is_bool", "is_bool(x) -> bool", "Check if value is `bool`"),
    ("is_str", "is_str(x) -> bool", "Check if value is `str`"),
    ("is_bytes", "is_bytes(x) -> bool", "Check if value is `bytes`"),
    ("is_list", "is_list(x) -> bool", "Check if value is `list`"),
    ("is_dict", "is_dict(x) -> bool", "Check if value is `dict`"),
    ("is_set", "is_set(x) -> bool", "Check if value is `set`"),
    ("is_fn", "is_fn(x) -> bool", "Check if value is callable"),
    ("unique", "unique(list) -> list", "Return order-preserving deduplicated copy"),
    ("flatten", "flatten(list) -> list", "Flatten one level of nested lists"),
    ("chunk", "chunk(list, size) -> list", "Split list into fixed-size sublists"),
    ("sum", "sum(seq) -> int | float", "Sum numeric elements"),
    ("has", "has(dict_or_set, key) -> bool", "Check membership in dict or set"),
    ("is_main", "is_main() -> bool", "Return true only for the top-level entry script"),
    ("clock", "clock() -> float", "Elapsed seconds since VM start (deterministic virtual time under --deterministic)"),
];

pub const REGISTRY: &[ModuleDoc] = &[
    md("archive", "io", "all", "Sandboxed ZIP pack/create/list/read_entry/extract with Zip-Slip & size guards"),
    md("assets", "media", "all", "Unified budget-aware asset cache, VFS ASKANE-PAK/ZIP mounts & manifest hints"),
    md("audio", "media", "all", "Deterministic multi-channel audio mixer, streaming music, synth & offline render"),
    md("base64", "data", "all", "Standard and URL-safe Base64 encoding and decoding"),
    md("binary", "data", "all", "Binary struct pack/unpack and typed integer/float codecs"),
    md("buffer", "data", "all", "Typed binary buffers (u8/i32/u32/f32) with safe element views and BudgetCore tracking"),
    md("bytes", "data", "all", "Byte buffer conversions and zero-allocation helpers"),
    md("channel", "runtime", "all", "Bounded MPSC channels with cooperative deadlock detection"),
    md("cli", "stdlib", "all", "Command-line flags & positional arguments parser"),
    md("clipboard", "gui", "windows, macos", "Desktop clipboard text read/write via Win32 or pbcopy/pbpaste (requires --allow-clipboard)"),
    md("collections", "stdlib", "all", "deque, counter, priority_queue, groupby, window & sequence helpers"),
    md("compress", "data", "all", "Pure-Rust DEFLATE, Zlib, Gzip compression, CRC32 and Adler32"),
    md("crypto", "crypto", "all", "HMAC-SHA256, constant_time_eq, random_key, ChaCha20-Poly1305 AEAD"),
    md("csv", "data", "all", "RFC-compatible CSV parsing, dict rows, and serialization"),
    md("cubemap", "scene", "all", "6-face square power-of-two cubemaps and directional sampling"),
    md("datetime", "data", "all", "UTC timestamps, ISO-8601 parsing and formatting"),
    md("dialog", "gui", "windows", "Native Win32 alert, confirm and open/save dialogs"),
    md("diff", "testing", "all", "LCS line and value diff formatter"),
    md("fs", "io", "all", "Sandboxed filesystem, streaming I/O & ASKANE-PAK VFS reads"),
    md("geom", "math", "all", "2D/3D vectors, matrices, quaternions, rays, AABBs and frustums"),
    md("gfx", "gui", "all", "2D framebuffer graphics (Win32/raw X11 window & headless), sprites, cameras & fonts"),
    md("glob", "stdlib", "all", "Linear/DP-bounded glob pattern matching"),
    md("hash", "data", "all", "Non-keyed hashes: SHA-256, CRC32, FNV-1a"),
    md("hex", "data", "all", "Hexadecimal byte encoding and decoding"),
    md("html", "data", "all", "HTML parser, selector, text extraction & escaping"),
    md("http", "net", "windows, linux", "HTTPS client via WinHTTP (Windows) or OpenSSL dlopen (Linux, requires --allow-net)"),
    md("image", "media", "all", "Safe image decoding, pixel formats, color ops, mipmaps, atlases & textures"),
    md("ini", "data", "all", "INI configuration parser and writer"),
    md("input", "gui", "windows", "Global key/mouse polling and hotkeys (requires --allow-input)"),
    md("json", "data", "all", "Strict JSON parser and serializer with depth and control-char guards"),
    md("kv", "io", "all", "Embedded append-only CRC32 KV store (sandbox-checked)"),
    md("lighting", "scene", "all", "2D/3D/spot lightmaps with shadow DDA & HDR compositing"),
    md("log", "runtime", "all", "Structured leveled logger with automatic secret redaction"),
    md("math", "stdlib", "all", "Floating-point & scalar math helpers (sqrt, pow, sin, cos, clamp, lerp, atan2)"),
    md("mem", "system", "windows", "Process memory inspection (--unsafe + ASKANE_ENABLE_UNSAFE_MEM=1)"),
    md("mesh", "scene", "all", "OBJ/glTF/GLB 3D model loading, stats, verification & ASKAS cache"),
    md("nav", "scene", "all", "Deterministic A* grid pathfinding, string-pulling & flow fields"),
    md("net", "net", "all", "Sandboxed TCP & UDP sockets (requires --allow-net)"),
    md("os", "system", "all", "Host OS metadata, cwd, home/temp dirs, env & args"),
    md("particles", "scene", "all", "Deterministic PCG32 particle pools, emitters & rendering"),
    md("path", "stdlib", "all", "Path manipulation (join, parent, name, stem, extension, normalize)"),
    md("postfx", "scene", "all", "Bloom, 3D LUT color grading, vignette, scanlines & BMP snapshots"),
    md("pprint", "stdlib", "all", "Pretty formatter for nested structures"),
    md("process", "system", "all", "Subprocess execution in bounded sandbox (requires --allow-exec)"),
    md("procgen", "media", "all", "Deterministic fixed-point 2D noise, FBM heightmaps, normal maps & procedural patterns"),
    md("random", "stdlib", "all", "Deterministic PRNG and OS CSPRNG (crypto_int, crypto_bytes)"),
    md("regex", "stdlib", "all", "Step-bounded regular expression engine"),
    md("scene", "scene", "all", "Unified scene assembly, terrain snapping & depth-tested drawing"),
    md("signal", "runtime", "all", "Reactive signal state cells with on change observers"),
    md("strings", "stdlib", "all", "UTF-8 string utilities (split, trim, replace, slice, byte_len, truncate_utf8)"),
    md("sys", "runtime", "all", "CLI args, env vars, backend probes, process exit, budget & sleep"),
    md("task", "runtime", "all", "Cooperative green tasks (spawn, yield_now, sleep, join, select, timeout)"),
    md("term", "stdlib", "all", "ANSI terminal styling helpers"),
    md("terrain", "scene", "all", "Heightmaps (.r16/PNG), normals, slopes, Mode7 & splatmaps"),
    md("test", "testing", "all", "Assertions (assert, assert_eq, assert_ne, assert_err, assert_snapshot)"),
    md("tiled", "scene", "all", "Tiled .tmj/.tmx map loading, solid collision grids & terrain alignment"),
    md("time", "runtime", "all", "Monotonic & wall clock helpers (now_ms, now_sec, elapsed_ms, sleep_ms)"),
    md("toml", "data", "all", "Bounded TOML parser and serializer"),
    md("ui", "gui", "windows", "Declarative 2D UI layout and interactive widgets"),
    md("url", "data", "all", "URL parsing, percent-encoding/decoding and query helpers"),
    md("uuid", "stdlib", "all", "Random UUID v4 generation"),
    md("ws", "net", "all", "RFC 6455 WebSocket server & client (requires --allow-net)"),
    md("xml", "data", "all", "XML tree parser, selector & escaping"),
];

pub const FN_DOCS: &[FnDoc] = &[
    // archive
    fdc("archive", "pack", "archive.pack(entries_dict) -> bytes", "Build an in-memory ZIP archive from filename->data dict", "--allow-read"),
    fdc("archive", "create", "archive.create(zip_path, entries_dict) -> unit", "Write a ZIP archive atomically within write sandbox", "--allow-write"),
    fdc("archive", "list", "archive.list(zip_path) -> list", "List validated entry names inside a ZIP file", "--allow-read"),
    fdc("archive", "read_entry", "archive.read_entry(zip_path, name) -> bytes", "Read single ZIP entry safely with compression ratio guard", "--allow-read"),
    fdc("archive", "extract", "archive.extract(zip_path, out_dir) -> int", "Extract ZIP entries into sandboxed directory blocking Zip-Slip", "--allow-write"),

    // assets
    fdc("assets", "load_image", "assets.load_image(path, [opts]) -> Image", "Decode and cache image from ASKANE-PAK/ZIP bundle or sandbox disk", "--allow-read"),
    fdc("assets", "load_texture", "assets.load_texture(path, [opts]) -> Texture", "Decode and cache GPU/software Texture with wrap & filter hints", "--allow-read"),
    fdc("assets", "load_sound", "assets.load_sound(path, [opts]) -> Sound", "Load and validate WAV/PCM audio clip from asset bundle or disk", "--allow-read"),
    fdc("assets", "sound", "assets.sound(path, [opts]) -> Sound", "Alias for assets.load_sound()", "--allow-read"),
    fdc("assets", "read_bytes", "assets.read_bytes(path) -> bytes", "Read bounded raw bytes from VFS bundle or disk", "--allow-read"),
    fdc("assets", "read_text", "assets.read_text(path) -> str", "Read bounded UTF-8 text asset from VFS bundle or disk", "--allow-read"),
    fd("assets", "exists", "assets.exists(path) -> bool", "Check if logical asset exists in mounted bundles or disk"),
    fdc("assets", "mount", "assets.mount(zip_path, [prefix]) -> int", "Mount a ZIP asset pack into the active VFS overlay", "--allow-read"),
    fd("assets", "mount_bytes", "assets.mount_bytes(zip_bytes, [prefix]) -> int", "Mount raw ZIP archive bytes as an asset bundle"),
    fd("assets", "unmount", "assets.unmount(id_or_prefix) -> bool", "Unmount a previously mounted asset bundle"),
    fd("assets", "entries", "assets.entries() -> list", "List sorted virtual paths across mounted ZIP bundles"),
    fd("assets", "list", "assets.list() -> list", "Alias for assets.entries()"),
    fd("assets", "set_budget", "assets.set_budget(max_bytes) -> unit", "Configure decoded asset LRU cache budget in bytes"),
    fd("assets", "gc", "assets.gc() -> int", "Evict unreferenced cached asset entries"),
    fd("assets", "stats", "assets.stats() -> dict", "Return cache usage, hits, misses, evictions & mount count"),
    fd("assets", "clear", "assets.clear() -> unit", "Clear cached images and mounted ZIP bundles"),
    fd("assets", "reset", "assets.reset() -> unit", "Alias for assets.clear()"),

    // audio
    fdc("audio", "init", "audio.init([opts]) -> dict", "Initialize deterministic mixer thread or headless NullSink/VirtualSink", "--allow-audio"),
    fd("audio", "shutdown", "audio.shutdown() -> unit", "Stop mixer worker thread and release audio backend handles"),
    fd("audio", "sink_name", "audio.sink_name() -> str", "Return active backend sink name (waveout, alsa, null, virtual)"),
    fd("audio", "state", "audio.state() -> dict", "Return active mixer metrics (voices, clock_ms, sample_rate, sink)"),
    fd("audio", "poll_errors", "audio.poll_errors() -> list", "Drain bounded audio underrun/fallback/device-stopped event queue"),
    fdc("audio", "load", "audio.load(path, [opts]) -> Sound", "Load WAV sound asset via asset cache & decode bounds", "--allow-read"),
    fdc("audio", "stream", "audio.stream(path, [opts]) -> Music", "Open bounded streaming music handle", "--allow-read"),
    fdc("audio", "music_load", "audio.music_load(path, [opts]) -> Music", "Alias for audio.stream()", "--allow-read"),
    fd("audio", "music_play", "audio.music_play(music) -> unit", "Start or resume a music stream"),
    fd("audio", "music_stop", "audio.music_stop(music) -> unit", "Stop music stream playback and rewind"),
    fd("audio", "music_pause", "audio.music_pause(music) -> unit", "Pause music stream preserving position"),
    fd("audio", "music_resume", "audio.music_resume(music) -> unit", "Resume paused music stream"),
    fd("audio", "music_seek", "audio.music_seek(music, ms) -> unit", "Seek streaming music position in milliseconds"),
    fd("audio", "music_position_ms", "audio.music_position_ms(music) -> int", "Return current music playback position in milliseconds"),
    fd("audio", "music_duration_ms", "audio.music_duration_ms(music) -> int", "Return total music duration in milliseconds"),
    fd("audio", "music_set_volume", "audio.music_set_volume(music, vol) -> unit", "Set music stream gain (0.0..4.0)"),
    fd("audio", "music_set_loop", "audio.music_set_loop(music, loop) -> unit", "Enable or disable looping on music stream"),
    fd("audio", "music_set_loop_points", "audio.music_set_loop_points(music, start_ms, end_ms) -> unit", "Configure sub-range loop points in milliseconds"),
    fd("audio", "music_fade_to", "audio.music_fade_to(music, vol, ms) -> unit", "Linearly fade music gain over `ms` milliseconds"),
    fd("audio", "music_state", "audio.music_state(music) -> str", "Return music state (stopped, playing, paused)"),
    fd("audio", "music_close", "audio.music_close(music) -> unit", "Close music stream and refund budget"),
    fd("audio", "from_samples", "audio.from_samples(samples, rate, channels) -> Sound", "Construct Sound from normalized f32 sample list or f32 Buffer"),
    fd("audio", "tone", "audio.tone(freq, dur_ms, [wave, amp, rate]) -> Sound", "Synthesize deterministic band-limited waveform (sine/square/saw/tri)"),
    fd("audio", "noise", "audio.noise(dur_ms, [seed, amp, rate]) -> Sound", "Synthesize deterministic seeded white noise Sound"),
    fd("audio", "render_offline", "audio.render_offline(dur_ms, [rate, gain]) -> Sound", "Render current virtual/offline mixer graph deterministically to PCM Sound"),
    fd("audio", "bus", "audio.bus(name) -> Bus", "Lookup or open named mixer bus handle"),
    fd("audio", "bus_new", "audio.bus_new(name, [parent]) -> Bus", "Create custom mixer sub-bus"),
    fd("audio", "bus_set_volume", "audio.bus_set_volume(bus, vol) -> unit", "Set bus gain"),
    fd("audio", "bus_set_mute", "audio.bus_set_mute(bus, muted) -> unit", "Mute or unmute mixer bus"),
    fd("audio", "bus_set_effect", "audio.bus_set_effect(bus, kind, opts) -> unit", "Attach DSP filter (lowpass/echo/compressor) to bus"),
    fd("audio", "bus_stop_all", "audio.bus_stop_all(bus) -> unit", "Stop all active voices routed through bus"),
    fd("audio", "play", "audio.play(sound_or_path, [opts]) -> Voice", "Play Sound handle (mixer) or legacy WAV file (under read sandbox)"),
    fd("audio", "play_at", "audio.play_at(sound, x, y, z, [opts]) -> Voice", "Spawn 3D spatialized audio voice"),
    fd("audio", "stop", "audio.stop() -> unit", "Stop legacy audio playback"),
    fd("audio", "stop_all", "audio.stop_all() -> unit", "Stop all active mixer voices"),
    fd("audio", "pause_all", "audio.pause_all() -> unit", "Pause all active mixer voices"),
    fd("audio", "resume_all", "audio.resume_all() -> unit", "Resume all paused mixer voices"),
    fd("audio", "set_listener", "audio.set_listener(x, y, z, [fx, fy, fz, ux, uy, uz]) -> unit", "Set 3D spatial audio listener pose"),
    fd("audio", "clock_ms", "audio.clock_ms() -> int", "Return mixer clock in milliseconds"),
    fd("audio", "tick", "audio.tick(ms) -> int", "Advance VirtualSink mixer deterministically by `ms` milliseconds"),
    fd("audio", "voice_stop", "audio.voice_stop(voice) -> unit", "Stop active voice handle"),
    fd("audio", "voice_pause", "audio.voice_pause(voice) -> unit", "Pause active voice handle"),
    fd("audio", "voice_resume", "audio.voice_resume(voice) -> unit", "Resume paused voice handle"),
    fd("audio", "voice_set_volume", "audio.voice_set_volume(voice, vol) -> unit", "Adjust active voice gain"),
    fd("audio", "voice_set_pan", "audio.voice_set_pan(voice, pan) -> unit", "Set stereo pan (-1.0..1.0)"),
    fd("audio", "voice_set_pitch", "audio.voice_set_pitch(voice, pitch) -> unit", "Set playback pitch multiplier (0.25..4.0)"),
    fd("audio", "voice_set_loop", "audio.voice_set_loop(voice, loop) -> unit", "Toggle looping on active voice"),
    fd("audio", "voice_set_loop_points", "audio.voice_set_loop_points(voice, start_ms, end_ms) -> unit", "Set voice loop sub-range in ms"),
    fd("audio", "voice_fade_to", "audio.voice_fade_to(voice, vol, ms) -> unit", "Linearly ramp voice gain"),
    fd("audio", "voice_set_pos3", "audio.voice_set_pos3(voice, x, y, z) -> unit", "Update 3D position of playing voice"),
    fd("audio", "voice_is_playing", "audio.voice_is_playing(voice) -> bool", "Return true if voice is currently active"),
    fd("audio", "voice_position_ms", "audio.voice_position_ms(voice) -> int", "Return playback cursor in ms"),
    fd("audio", "ENGINE_RATE", "audio.ENGINE_RATE: int", "Internal stereo mixer sample rate (48000 Hz)"),
    fd("audio", "SAMPLE_RATE", "audio.SAMPLE_RATE: int", "Alias for audio.ENGINE_RATE (48000 Hz)"),
    fd("audio", "CHANNELS", "audio.CHANNELS: int", "Mixer channel count (2 = stereo)"),
    fd("audio", "QUANTUM", "audio.QUANTUM: int", "Fixed DSP render quantum frame count (256)"),
    fd("audio", "MAX_VOICES", "audio.MAX_VOICES: int", "Hard cap on simultaneous voices"),
    fd("audio", "MAX_BUSES", "audio.MAX_BUSES: int", "Hard cap on mixer buses"),
    fd("audio", "MAX_MUSICS", "audio.MAX_MUSICS: int", "Hard cap on concurrent music streams"),

    // base64
    fd("base64", "encode", "base64.encode(data) -> str", "Encode string or bytes using standard RFC 4648 Base64"),
    fd("base64", "decode", "base64.decode(text) -> str", "Decode Base64 string to UTF-8 string"),
    fd("base64", "decode_bytes", "base64.decode_bytes(text) -> bytes", "Decode Base64 string to raw bytes"),
    fd("base64", "encode_url", "base64.encode_url(data) -> str", "Encode bytes using URL-safe Base64 without padding"),
    fd("base64", "decode_url", "base64.decode_url(text) -> str", "Decode URL-safe Base64 to UTF-8 string"),
    fd("base64", "decode_url_bytes", "base64.decode_url_bytes(text) -> bytes", "Decode URL-safe Base64 to raw bytes"),

    // binary
    fd("binary", "pack", "binary.pack(fmt, ...vals) -> bytes", "Pack typed integers/floats according to format string (<, >, u16/i32/f32/...)"),
    fd("binary", "unpack", "binary.unpack(fmt, bytes, [offset]) -> list", "Unpack binary fields according to format string"),
    fd("binary", "read_u16_le", "binary.read_u16_le(bytes, offset) -> int", "Read little-endian u16"),
    fd("binary", "read_u16_be", "binary.read_u16_be(bytes, offset) -> int", "Read big-endian u16"),
    fd("binary", "read_u32_le", "binary.read_u32_le(bytes, offset) -> int", "Read little-endian u32"),
    fd("binary", "read_u32_be", "binary.read_u32_be(bytes, offset) -> int", "Read big-endian u32"),
    fd("binary", "read_i32_le", "binary.read_i32_le(bytes, offset) -> int", "Read little-endian i32"),
    fd("binary", "read_i32_be", "binary.read_i32_be(bytes, offset) -> int", "Read big-endian i32"),
    fd("binary", "read_i64_le", "binary.read_i64_le(bytes, offset) -> int", "Read little-endian i64"),
    fd("binary", "read_i64_be", "binary.read_i64_be(bytes, offset) -> int", "Read big-endian i64"),
    fd("binary", "read_f32_le", "binary.read_f32_le(bytes, offset) -> float", "Read little-endian IEEE f32"),
    fd("binary", "read_f32_be", "binary.read_f32_be(bytes, offset) -> float", "Read big-endian IEEE f32"),
    fd("binary", "read_f64_le", "binary.read_f64_le(bytes, offset) -> float", "Read little-endian IEEE f64"),
    fd("binary", "read_f64_be", "binary.read_f64_be(bytes, offset) -> float", "Read big-endian IEEE f64"),
    fd("binary", "write_u16_le", "binary.write_u16_le(val) -> bytes", "Encode little-endian u16 bytes"),
    fd("binary", "write_u16_be", "binary.write_u16_be(val) -> bytes", "Encode big-endian u16 bytes"),
    fd("binary", "write_u32_le", "binary.write_u32_le(val) -> bytes", "Encode little-endian u32 bytes"),
    fd("binary", "write_u32_be", "binary.write_u32_be(val) -> bytes", "Encode big-endian u32 bytes"),
    fd("binary", "write_i64_le", "binary.write_i64_le(val) -> bytes", "Encode little-endian i64 bytes"),
    fd("binary", "write_i64_be", "binary.write_i64_be(val) -> bytes", "Encode big-endian i64 bytes"),
    fd("binary", "write_f32_le", "binary.write_f32_le(val) -> bytes", "Encode little-endian f32 bytes"),
    fd("binary", "write_f64_le", "binary.write_f64_le(val) -> bytes", "Encode little-endian f64 bytes"),

    // buffer
    fd("buffer", "new", "buffer.new(kind, count) -> Buffer", "Allocate budget-tracked typed binary buffer (\"u8\", \"i32\", \"u32\", \"f32\")"),
    fd("buffer", "len", "buffer.len(buf) -> int", "Return element count in typed buffer"),
    fd("buffer", "byte_len", "buffer.byte_len(buf) -> int", "Return raw byte length of typed buffer"),
    fd("buffer", "elem_type", "buffer.elem_type(buf) -> str", "Return element type tag (\"u8\", \"i32\", \"u32\", \"f32\")"),
    fd("buffer", "get", "buffer.get(buf, idx) -> int | float", "Read element with bounds check"),
    fd("buffer", "set", "buffer.set(buf, idx, val) -> unit", "Write element with bounds check"),
    fd("buffer", "fill", "buffer.fill(buf, val) -> unit", "Fill all elements of buffer with constant"),
    fd("buffer", "fill_rgba", "buffer.fill_rgba(buf, r, g, b, a) -> unit", "Fill u32/u8 pixel buffer with packed RGBA8"),
    fd("buffer", "blit", "buffer.blit(dst, dst_off, src, src_off, count) -> unit", "Bounds-checked element block copy"),
    fd("buffer", "copy_from", "buffer.copy_from(buf, list_or_buf, [offset]) -> unit", "Copy values into typed buffer"),
    fd("buffer", "slice", "buffer.slice(buf, start, end) -> Buffer", "Copy sub-range into a new typed buffer"),
    fd("buffer", "clone", "buffer.clone(buf) -> Buffer", "Duplicate typed buffer respecting memory budget"),
    fd("buffer", "as_bytes", "buffer.as_bytes(buf) -> bytes", "Copy underlying little-endian buffer bytes"),
    fd("buffer", "from_bytes", "buffer.from_bytes(kind, bytes) -> Buffer", "Construct typed buffer from aligned raw bytes"),

    // bytes
    fd("bytes", "from_utf8", "bytes.from_utf8(bytes) -> str", "Decode strict UTF-8 bytes into string"),
    fd("bytes", "from_utf8_lossy", "bytes.from_utf8_lossy(bytes) -> str", "Decode UTF-8 replacing invalid sequences with U+FFFD"),
    fd("bytes", "to_utf8", "bytes.to_utf8(text) -> bytes", "Convert string into UTF-8 bytes"),
    fd("bytes", "from_str", "bytes.from_str(text) -> bytes", "Alias for bytes.to_utf8"),
    fd("bytes", "from_list", "bytes.from_list(byte_list) -> bytes", "Convert list of 0..255 integers into bytes"),
    fd("bytes", "to_list", "bytes.to_list(bytes) -> list", "Convert bytes into list of 0..255 integers"),
    fd("bytes", "zeros", "bytes.zeros(len) -> bytes", "Allocate zero-filled byte buffer up to safety cap"),

    // channel
    fd("channel", "bounded", "channel.bounded(capacity) -> Channel", "Create bounded cooperative channel of capacity >= 1"),
    fd("channel", "send", "channel.send(ch, val) -> unit", "Send value yielding if full until space or closed"),
    fd("channel", "try_send", "channel.try_send(ch, val) -> bool", "Non-blocking send returning false when full"),
    fd("channel", "recv", "channel.recv(ch) -> any", "Receive value yielding if empty"),
    fd("channel", "try_recv", "channel.try_recv(ch) -> any | unit", "Non-blocking receive returning `unit` if empty"),
    fd("channel", "close", "channel.close(ch) -> unit", "Close channel waking waiting tasks"),
    fd("channel", "len", "channel.len(ch) -> int", "Return queued item count"),
    fd("channel", "is_closed", "channel.is_closed(ch) -> bool", "Return true if channel is closed"),

    // cli
    fd("cli", "parse", "cli.parse([argv]) -> dict", "Parse CLI flags (`--k=v`, `-f`) and positional args into `{flags, args}`"),
    fd("cli", "has_flag", "cli.has_flag(parsed, name) -> bool", "Check if boolean or value flag is present"),

    // clipboard
    fdc("clipboard", "get_text", "clipboard.get_text() -> str", "Read plain UTF-8 text from system clipboard (256 KiB cap)", "--allow-clipboard"),
    fdc("clipboard", "set_text", "clipboard.set_text(text) -> unit", "Write plain UTF-8 text to system clipboard (256 KiB cap)", "--allow-clipboard"),

    // collections
    fd("collections", "deque", "collections.deque([items]) -> Deque", "Double-ended queue with O(1) push/pop ends"),
    fd("collections", "counter", "collections.counter([seq]) -> Counter", "Frequency multiset with `most_common` and `total`"),
    fd("collections", "priority_queue", "collections.priority_queue() -> PriorityQueue", "Min-heap priority queue ordering `(priority, value)` entries"),
    fd("collections", "groupby", "collections.groupby(seq, key_fn) -> dict", "Group sequence items by computed key string"),
    fd("collections", "window", "collections.window(seq, size) -> list", "Sliding fixed-size windows over sequence"),
    fd("collections", "first", "collections.first(seq) -> any | unit", "First item or `unit`"),
    fd("collections", "last", "collections.last(seq) -> any | unit", "Last item or `unit`"),
    fd("collections", "is_empty", "collections.is_empty(seq) -> bool", "Return true when sequence is empty"),
    fd("collections", "find_index", "collections.find_index(seq, pred) -> int | unit", "Return first index matching predicate"),
    fd("collections", "count", "collections.count(seq, pred) -> int", "Count items satisfying predicate"),
    fd("collections", "partition", "collections.partition(seq, pred) -> list", "Split items into `[pass, fail]` lists"),
    fd("collections", "flat_map", "collections.flat_map(seq, fn) -> list", "Map items and flatten lists one level"),
    fd("collections", "take_while", "collections.take_while(seq, pred) -> list", "Take leading items matching predicate"),
    fd("collections", "drop_while", "collections.drop_while(seq, pred) -> list", "Drop leading items matching predicate"),
    fd("collections", "zip_longest", "collections.zip_longest(a, b, [fill]) -> list", "Zip sequences padding missing items with `fill`"),
    fd("collections", "min_by", "collections.min_by(seq, key_fn) -> any | unit", "Minimum item by key function"),
    fd("collections", "max_by", "collections.max_by(seq, key_fn) -> any | unit", "Maximum item by key function"),

    // compress
    fd("compress", "deflate", "compress.deflate(data) -> bytes", "Raw DEFLATE compression (RFC 1951)"),
    fd("compress", "inflate", "compress.inflate(data, [max_out]) -> bytes", "Bounded raw DEFLATE decompression"),
    fd("compress", "zlib_compress", "compress.zlib_compress(data) -> bytes", "Compress with Zlib header + Adler32 checksum"),
    fd("compress", "zlib_decompress", "compress.zlib_decompress(data, [max_out]) -> bytes", "Decompress bounded Zlib stream"),
    fd("compress", "gzip_compress", "compress.gzip_compress(data) -> bytes", "Compress with deterministic Gzip container"),
    fd("compress", "gzip_decompress", "compress.gzip_decompress(data, [max_out]) -> bytes", "Decompress bounded Gzip stream verifying CRC32"),
    fd("compress", "crc32", "compress.crc32(data) -> int", "Compute IEEE 802.3 CRC32 checksum"),
    fd("compress", "adler32", "compress.adler32(data) -> int", "Compute Adler-32 checksum"),

    // crypto
    fd("crypto", "hmac_sha256", "crypto.hmac_sha256(key, data) -> bytes", "Compute 32-byte HMAC-SHA256 tag"),
    fd("crypto", "hmac_sha256_hex", "crypto.hmac_sha256_hex(key, data) -> str", "Compute lowercase hex HMAC-SHA256 digest"),
    fd("crypto", "constant_time_eq", "crypto.constant_time_eq(a, b) -> bool", "Fixed-timing byte/string comparison"),
    fd("crypto", "random_key", "crypto.random_key() -> bytes", "Generate 32-byte OS CSPRNG key"),
    fd("crypto", "random_bytes", "crypto.random_bytes(n) -> bytes", "Generate `n` bytes from OS CSPRNG"),
    fd("crypto", "seal", "crypto.seal(key, plaintext, [aad]) -> bytes", "Encrypt with ChaCha20-Poly1305 AEAD and fresh nonce"),
    fd("crypto", "open", "crypto.open(key, ciphertext, [aad]) -> bytes", "Authenticate and decrypt ChaCha20-Poly1305 payload"),

    // csv
    fd("csv", "parse", "csv.parse(text, [delim]) -> list", "Parse RFC-4180 CSV rows into list of lists"),
    fd("csv", "parse_dicts", "csv.parse_dicts(text, [delim]) -> list", "Parse CSV using first row as dictionary keys"),
    fd("csv", "stringify", "csv.stringify(rows, [delim]) -> str", "Serialize rows or dicts back to CSV text"),

    // cubemap
    fd("cubemap", "load", "cubemap.load(px, nx, py, ny, pz, nz) -> Cubemap", "Load 6 equal square power-of-two face images into a Cubemap",),
    fd("cubemap", "from_colors", "cubemap.from_colors(size, faces_colors) -> Cubemap", "Synthesize procedural gradient/solid Cubemap handle"),
    fd("cubemap", "sample", "cubemap.sample(cube, dx, dy, dz) -> list", "Sample normalized RGB directional reflection/skybox color"),
    fd("cubemap", "size", "cubemap.size(cube) -> int", "Return face edge size in pixels"),
    fd("cubemap", "close", "cubemap.close(cube) -> unit", "Release Cubemap handle and refund budget"),
    fd("cubemap", "free", "cubemap.free(cube) -> unit", "Alias for cubemap.close()"),

    // datetime
    fd("datetime", "utc_now", "datetime.utc_now() -> int", "Current Unix epoch milliseconds in UTC"),
    fd("datetime", "from_timestamp", "datetime.from_timestamp(ms) -> dict", "Decompose UTC epoch ms into year/month/day/hour/minute/second"),
    fd("datetime", "format", "datetime.format(ms, [fmt]) -> str", "Format UTC epoch ms as ISO-8601 or strftime pattern"),
    fd("datetime", "parse_iso", "datetime.parse_iso(text) -> int", "Parse ISO-8601 UTC date/time string into epoch ms"),

    // dialog
    fd("dialog", "info", "dialog.info(title, msg) -> unit", "Show native modal informational dialog (Windows)"),
    fd("dialog", "confirm", "dialog.confirm(title, msg) -> bool", "Show Ok/Cancel confirmation dialog (Windows)"),
    fd("dialog", "open_file", "dialog.open_file() -> str | unit", "Show native open-file picker checked against read sandbox"),
    fd("dialog", "save_file", "dialog.save_file() -> str | unit", "Show native save-file picker checked against write sandbox"),

    // diff
    fd("diff", "lines", "diff.lines(a, b) -> list", "Compute LCS line diff operations `[tag, line]`"),
    fd("diff", "format", "diff.format(a, b) -> str", "Render unified human-readable text diff"),
    fd("diff", "values", "diff.values(a, b) -> str", "Format structural diff between two values"),

    // fs
    fdc("fs", "read", "fs.read(path) -> str", "Read bounded UTF-8 file from mounted VFS bundle or sandbox disk", "--allow-read"),
    fdc("fs", "write", "fs.write(path, content) -> unit", "Write UTF-8 text atomically within write sandbox", "--allow-write"),
    fdc("fs", "append", "fs.append(path, content) -> unit", "Append UTF-8 text within write sandbox", "--allow-write"),
    fd("fs", "exists", "fs.exists(path) -> bool", "Check file/dir existence in VFS bundle or read sandbox"),
    fdc("fs", "remove", "fs.remove(path) -> unit", "Remove regular file within write sandbox", "--allow-write"),
    fdc("fs", "remove_dir", "fs.remove_dir(path) -> unit", "Recursively delete sandboxed directory", "--allow-write"),
    fd("fs", "list_dir", "fs.list_dir(path) -> list", "List directory entry names in VFS bundle or sandbox disk"),
    fdc("fs", "walk", "fs.walk(dir, [opts]) -> list", "Recursively traverse directory respecting symlink and depth caps", "--allow-read"),
    fdc("fs", "mkdir", "fs.mkdir(path) -> unit", "Create directory and parents inside write sandbox", "--allow-write"),
    fd("fs", "is_dir", "fs.is_dir(path) -> bool", "Check if path is a directory in bundle or disk"),
    fdc("fs", "read_bytes", "fs.read_bytes(path) -> bytes", "Read bounded binary bytes from VFS bundle or disk", "--allow-read"),
    fdc("fs", "write_bytes", "fs.write_bytes(path, bytes) -> unit", "Write raw bytes within write sandbox", "--allow-write"),
    fdc("fs", "read_lines", "fs.read_lines(path, [max_lines]) -> list", "Read bounded list of lines from VFS bundle or disk", "--allow-read"),
    fdc("fs", "write_lines", "fs.write_lines(path, lines) -> unit", "Write lines separated by LF inside write sandbox", "--allow-write"),
    fdc("fs", "open_lines", "fs.open_lines(path) -> LineReader", "Open streaming line reader handle", "--allow-read"),
    fdc("fs", "open_writer", "fs.open_writer(path) -> FileWriter", "Open buffered streaming file writer inside write sandbox", "--allow-write"),
    fdc("fs", "write_stream", "fs.write_stream(path) -> FileWriter", "Alias for fs.open_writer()", "--allow-write"),
    fdc("fs", "temp_file", "fs.temp_file([prefix]) -> str", "Create unique temp file path inside sandbox", "--allow-write"),
    fdc("fs", "copy", "fs.copy(src, dst) -> unit", "Copy regular file cleanly within sandbox", "--allow-write"),
    fdc("fs", "copy_dir", "fs.copy_dir(src, dst) -> int", "Copy directory tree safely without following symlinks", "--allow-write"),
    fdc("fs", "move", "fs.move(src, dst) -> unit", "Move/rename file within write sandbox", "--allow-write"),
    fd("fs", "size", "fs.size(path) -> int", "Return byte length of file in VFS bundle or disk"),
    fdc("fs", "glob", "fs.glob(pattern) -> list", "Expand sandboxed path glob pattern", "--allow-read"),

    // geom
    fd("geom", "vec2", "geom.vec2(x, y) -> Vec2", "Construct 2D vector"),
    fd("geom", "vec3", "geom.vec3(x, y, z) -> Vec3", "Construct 3D vector list/dict"),
    fd("geom", "vec4", "geom.vec4(x, y, z, w) -> Vec4", "Construct 4D homogeneous vector"),
    fd("geom", "quat", "geom.quat(x, y, z, w) -> Quat", "Construct quaternion `[x, y, z, w]`"),
    fd("geom", "quat_identity", "geom.quat_identity() -> Quat", "Return identity quaternion `[0, 0, 0, 1]`"),
    fd("geom", "quat_axis_angle", "geom.quat_axis_angle(axis, rad) -> Quat", "Build rotation quaternion from axis vector and radians"),
    fd("geom", "quat_euler", "geom.quat_euler(rx, ry, rz) -> Quat", "Build quaternion from Euler radians"),
    fd("geom", "mat4", "geom.mat4(values16) -> Mat4", "Construct 4x4 column-major matrix"),
    fd("geom", "mat4_identity", "geom.mat4_identity() -> Mat4", "Return 4x4 identity matrix"),
    fd("geom", "mat4_translate", "geom.mat4_translate(x, y, z) -> Mat4", "Build 4x4 translation matrix"),
    fd("geom", "mat4_scale", "geom.mat4_scale(x, y, z) -> Mat4", "Build 4x4 scale matrix"),
    fd("geom", "mat4_rotate_x", "geom.mat4_rotate_x(rad) -> Mat4", "Build X-axis rotation matrix"),
    fd("geom", "mat4_rotate_y", "geom.mat4_rotate_y(rad) -> Mat4", "Build Y-axis rotation matrix"),
    fd("geom", "mat4_rotate_z", "geom.mat4_rotate_z(rad) -> Mat4", "Build Z-axis rotation matrix"),
    fd("geom", "mat4_from_quat", "geom.mat4_from_quat(q) -> Mat4", "Convert unit quaternion to 4x4 rotation matrix"),
    fd("geom", "mat4_trs", "geom.mat4_trs(pos, rot_q, scale) -> Mat4", "Compose translation * rotation * scale 4x4 matrix"),
    fd("geom", "mat4_look_at", "geom.mat4_look_at(eye, target, up) -> Mat4", "Build right-handed view matrix"),
    fd("geom", "mat4_perspective", "geom.mat4_perspective(fov_rad, aspect, near, far) -> Mat4", "Build perspective projection matrix"),
    fd("geom", "mat4_ortho", "geom.mat4_ortho(l, r, b, t, n, f) -> Mat4", "Build orthographic projection matrix"),
    fd("geom", "mat4_mul", "geom.mat4_mul(a, b) -> Mat4", "Multiply two 4x4 matrices"),
    fd("geom", "mat4_vec4", "geom.mat4_vec4(m, v4) -> Vec4", "Multiply 4x4 matrix by 4D vector"),
    fd("geom", "mat4_point", "geom.mat4_point(m, p3) -> Vec3", "Transform 3D point with perspective divide"),
    fd("geom", "mat4_dir", "geom.mat4_dir(m, d3) -> Vec3", "Transform 3D direction ignoring translation"),
    fd("geom", "mat4_transpose", "geom.mat4_transpose(m) -> Mat4", "Transpose 4x4 matrix"),
    fd("geom", "mat4_det", "geom.mat4_det(m) -> float", "Compute 4x4 matrix determinant"),
    fd("geom", "mat4_inverse", "geom.mat4_inverse(m) -> Mat4", "Compute inverse 4x4 matrix or throw on singular"),
    fd("geom", "x", "geom.x(v) -> float", "Extract X component of vec2/vec3/vec4"),
    fd("geom", "y", "geom.y(v) -> float", "Extract Y component"),
    fd("geom", "z", "geom.z(v) -> float", "Extract Z component"),
    fd("geom", "w", "geom.w(v) -> float", "Extract W component"),
    fd("geom", "to_list", "geom.to_list(v) -> list", "Convert vector/quat/mat to plain float list"),
    fd("geom", "rot90", "geom.rot90(v2) -> Vec2", "Rotate Vec2 90 degrees counter-clockwise"),
    fd("geom", "rotate_vec2", "geom.rotate_vec2(v2, rad) -> Vec2", "Rotate Vec2 by radians"),
    fd("geom", "rotate_vec3", "geom.rotate_vec3(q, v3) -> Vec3", "Rotate 3D vector by quaternion"),
    fd("geom", "deg_to_rad", "geom.deg_to_rad(deg) -> float", "Convert degrees to radians"),
    fd("geom", "rad_to_deg", "geom.rad_to_deg(rad) -> float", "Convert radians to degrees"),
    fd("geom", "dot", "geom.dot(a, b) -> float", "Vector dot product"),
    fd("geom", "cross", "geom.cross(a, b) -> Vec3", "3D vector cross product"),
    fd("geom", "len", "geom.len(v) -> float", "Euclidean vector magnitude"),
    fd("geom", "len_sq", "geom.len_sq(v) -> float", "Squared vector magnitude"),
    fd("geom", "dist", "geom.dist(a, b) -> float", "Euclidean distance between points"),
    fd("geom", "normalize", "geom.normalize(v) -> Vec", "Return unit-length vector"),
    fd("geom", "lerp", "geom.lerp(a, b, t) -> any", "Linear interpolation on scalar or vector"),
    fd("geom", "slerp", "geom.slerp(q1, q2, t) -> Quat", "Spherical linear interpolation of quaternions"),
    fd("geom", "conjugate", "geom.conjugate(q) -> Quat", "Quaternion conjugate"),
    fd("geom", "reflect", "geom.reflect(v, normal) -> Vec", "Reflect vector across unit normal"),
    fd("geom", "project", "geom.project(v, onto) -> Vec", "Project vector onto another vector"),
    fd("geom", "transform", "geom.transform(pos, rot, scale) -> dict", "Create TRS transform bundle"),
    fd("geom", "transform_to_mat4", "geom.transform_to_mat4(tf) -> Mat4", "Convert TRS dict into 4x4 matrix"),
    fd("geom", "transform_point", "geom.transform_point(tf, p) -> Vec3", "Transform 3D point by TRS transform"),
    fd("geom", "transform_combine", "geom.transform_combine(parent, child) -> dict", "Compose parent and child TRS transforms"),
    fd("geom", "aabb", "geom.aabb(min3, max3) -> dict", "Construct axis-aligned bounding box"),
    fd("geom", "aabb_contains", "geom.aabb_contains(box, pt) -> bool", "Test point containment in AABB"),
    fd("geom", "aabb_intersects", "geom.aabb_intersects(a, b) -> bool", "Test overlap of two AABBs"),
    fd("geom", "aabb_center", "geom.aabb_center(box) -> Vec3", "Compute AABB centroid"),
    fd("geom", "aabb_size", "geom.aabb_size(box) -> Vec3", "Compute AABB dimensions"),
    fd("geom", "aabb_expand", "geom.aabb_expand(box, pt) -> dict", "Expand AABB to include point"),
    fd("geom", "aabb_transform", "geom.aabb_transform(box, mat4) -> dict", "Re-bound transformed AABB cleanly"),
    fd("geom", "ray", "geom.ray(origin3, dir3) -> dict", "Construct 3D ray"),
    fd("geom", "ray_at", "geom.ray_at(ray, t) -> Vec3", "Evaluate point along ray at parameter t"),
    fd("geom", "ray_aabb", "geom.ray_aabb(ray, box) -> float | unit", "Ray-AABB slab intersection distance or unit"),
    fd("geom", "ray_plane", "geom.ray_plane(ray, plane) -> float | unit", "Ray-plane intersection distance or unit"),
    fd("geom", "plane", "geom.plane(normal3, d) -> dict", "Construct plane equation"),
    fd("geom", "plane_from_point", "geom.plane_from_point(normal3, pt3) -> dict", "Construct plane from normal and point"),
    fd("geom", "plane_distance", "geom.plane_distance(plane, pt3) -> float", "Signed distance from point to plane"),
    fd("geom", "frustum_from_mat4", "geom.frustum_from_mat4(vp_mat4) -> list", "Extract 6 normalized clip planes from VP matrix"),
    fd("geom", "frustum_contains_point", "geom.frustum_contains_point(frustum, pt3) -> bool", "Point frustum test"),
    fd("geom", "frustum_intersects_aabb", "geom.frustum_intersects_aabb(frustum, box) -> bool", "Conservative AABB frustum cull test"),

    // gfx
    fd("gfx", "window", "gfx.window(title, w, h) -> unit", "Open 2D framebuffer window (Win32 / raw Linux X11 / headless)"),
    fd("gfx", "headless", "gfx.headless(enabled) -> unit", "Enable/disable deterministic headless offscreen window mode"),
    fd("gfx", "is_open", "gfx.is_open() -> bool", "Pump window events and report whether window stays open"),
    fd("gfx", "clear", "gfx.clear(r, g, b) -> unit", "Clear active render target to RGB color"),
    fd("gfx", "rect", "gfx.rect(x, y, w, h, r, g, b) -> unit", "Draw filled rectangle"),
    fd("gfx", "rect_alpha", "gfx.rect_alpha(x, y, w, h, r, g, b, a) -> unit", "Draw alpha-blended rectangle"),
    fd("gfx", "circle", "gfx.circle(cx, cy, rad, r, g, b) -> unit", "Draw filled circle"),
    fd("gfx", "circle_alpha", "gfx.circle_alpha(cx, cy, rad, r, g, b, a) -> unit", "Draw alpha-blended filled circle"),
    fd("gfx", "line", "gfx.line(x1, y1, x2, y2, r, g, b) -> unit", "Draw 2D clipped line segment"),
    fd("gfx", "pixel", "gfx.pixel(x, y, r, g, b) -> unit", "Plot single clipped pixel"),
    fd("gfx", "text", "gfx.text(x, y, text, r, g, b) -> unit", "Draw text onto main native window"),
    fd("gfx", "rect_outline", "gfx.rect_outline(x, y, w, h, r, g, b) -> unit", "Stroke 1px rectangle outline"),
    fd("gfx", "circle_outline", "gfx.circle_outline(cx, cy, rad, r, g, b) -> unit", "Stroke circle outline"),
    fd("gfx", "width", "gfx.width() -> int", "Return active render target width in pixels"),
    fd("gfx", "height", "gfx.height() -> int", "Return active render target height in pixels"),
    fdc("gfx", "load_image", "gfx.load_image(path) -> Image", "Load BMP/PNG image via sandbox/bundle VFS", "--allow-read"),
    fd("gfx", "draw_image", "gfx.draw_image(img_or_handle, x, y) -> unit", "Blit image or offscreen surface to active target"),
    fd("gfx", "draw_image_keyed", "gfx.draw_image_keyed(img, x, y, r, g, b) -> unit", "Blit image skipping colorkey RGB"),
    fd("gfx", "draw_sprite_frame", "gfx.draw_sprite_frame(img, dx, dy, fw, fh, frame_idx) -> unit", "Draw cell from uniform sprite grid sheet"),
    fd("gfx", "draw_sprite_ex", "gfx.draw_sprite_ex(img, dx, dy, opts) -> unit", "Draw sprite with scale, flip, rotation & tint"),
    fd("gfx", "draw_atlas", "gfx.draw_atlas(atlas, name, x, y, [opts]) -> unit", "Draw packed named sprite from ImageAtlas"),
    fd("gfx", "atlas_rect", "gfx.atlas_rect(atlas, name) -> dict", "Query sub-rectangle metrics for atlas region"),
    fd("gfx", "make_anim", "gfx.make_anim(frames, fps, [loop]) -> dict", "Create deterministic sprite animation state"),
    fd("gfx", "anim_update", "gfx.anim_update(anim, dt) -> dict", "Advance sprite animation clock by dt"),
    fd("gfx", "anim_draw", "gfx.anim_draw(img, anim, x, y, [opts]) -> unit", "Render current frame of sprite animation"),
    fd("gfx", "anim_frame", "gfx.anim_frame(anim) -> int", "Return current frame index"),
    fd("gfx", "image_width", "gfx.image_width(img_or_handle) -> int", "Return image or surface pixel width"),
    fd("gfx", "image_height", "gfx.image_height(img_or_handle) -> int", "Return image or surface pixel height"),
    fd("gfx", "set_title", "gfx.set_title(title) -> unit", "Update active window title bar"),
    fd("gfx", "delta", "gfx.delta() -> float", "Frame delta time (fixed 1/60s under --deterministic)"),
    fd("gfx", "present", "gfx.present() -> unit", "Flip/present active framebuffer to screen"),
    fd("gfx", "key_down", "gfx.key_down(key) -> bool", "Return true while key is held in active window"),
    fd("gfx", "key_pressed", "gfx.key_pressed(key) -> bool", "Edge-triggered key press since previous present()"),
    fd("gfx", "mouse_x", "gfx.mouse_x() -> int", "Window-relative cursor X coordinate"),
    fd("gfx", "mouse_y", "gfx.mouse_y() -> int", "Window-relative cursor Y coordinate"),
    fd("gfx", "mouse_down", "gfx.mouse_down(button) -> bool", "Check if left/right/middle mouse button is down"),
    fd("gfx", "mouse_pressed", "gfx.mouse_pressed(button) -> bool", "Edge-triggered mouse click"),
    fd("gfx", "close", "gfx.close() -> unit", "Destroy active native or headless window"),
    fd("gfx", "clip", "gfx.clip(x, y, w, h) -> unit", "Set active draw clip rectangle"),
    fd("gfx", "reset_clip", "gfx.reset_clip() -> unit", "Reset clip rectangle to full target bounds"),
    fd("gfx", "surface_new", "gfx.surface_new(w, h) -> int", "Allocate budget-tracked offscreen RGBA surface handle"),
    fd("gfx", "surface_free", "gfx.surface_free(handle) -> unit", "Free offscreen surface and refund budget"),
    fd("gfx", "surface_width", "gfx.surface_width(handle) -> int", "Query surface width"),
    fd("gfx", "surface_height", "gfx.surface_height(handle) -> int", "Query surface height"),
    fd("gfx", "surface_from_image", "gfx.surface_from_image(img) -> int", "Copy decoded Image into mutable offscreen surface"),
    fd("gfx", "draw_to", "gfx.draw_to(handle, [fn]) -> any", "Push offscreen surface/layer as active draw target"),
    fd("gfx", "draw_surface", "gfx.draw_surface(handle, x, y) -> unit", "Blit offscreen surface onto active target"),
    fd("gfx", "draw_surface_scaled", "gfx.draw_surface_scaled(handle, x, y, w, h) -> unit", "Blit scaled offscreen surface"),
    fd("gfx", "draw_surface_keyed", "gfx.draw_surface_keyed(handle, x, y, r, g, b) -> unit", "Blit offscreen surface with colorkey transparency"),
    fd("gfx", "draw_surface_alpha", "gfx.draw_surface_alpha(handle, x, y, alpha) -> unit", "Blit offscreen surface with global alpha 0..255"),
    fd("gfx", "read_pixel", "gfx.read_pixel(x, y) -> dict", "Inspect pixel RGBA components on active target"),
    fd("gfx", "frame_hash", "gfx.frame_hash() -> str", "Compute deterministic SHA-256 hash over framebuffer for regression tests"),
    fd("gfx", "layer_new", "gfx.layer_new(w, h, [z, mode]) -> int", "Create compositable sorted layer handle"),
    fd("gfx", "layer_free", "gfx.layer_free(handle) -> unit", "Release layer handle"),
    fd("gfx", "layer_mode", "gfx.layer_mode(handle, mode) -> unit", "Set blend mode (normal/add/multiply)"),
    fd("gfx", "layer_visible", "gfx.layer_visible(handle, vis) -> unit", "Show or hide layer during composition"),
    fd("gfx", "layer_z", "gfx.layer_z(handle, z) -> unit", "Set layer Z-order"),
    fd("gfx", "layer_parallax", "gfx.layer_parallax(handle, px, py) -> unit", "Set camera parallax scroll factors"),
    fd("gfx", "draw_to_layer", "gfx.draw_to_layer(handle, fn) -> any", "Render closure directly onto layer surface"),
    fd("gfx", "compose_layers", "gfx.compose_layers() -> unit", "Composite all visible Z-sorted layers onto main target"),
    fd("gfx", "set_camera", "gfx.set_camera(x, y, [zoom, rot]) -> unit", "Configure active 2D camera transform"),
    fd("gfx", "camera", "gfx.camera() -> dict", "Query active camera position/zoom/rotation"),
    fd("gfx", "reset_camera", "gfx.reset_camera() -> unit", "Reset active camera to identity"),
    fd("gfx", "set_camera_shake", "gfx.set_camera_shake(amp, decay) -> unit", "Trigger camera screen shake"),
    fd("gfx", "update_camera", "gfx.update_camera(dt) -> unit", "Advance camera shake state manually"),
    fd("gfx", "world_to_screen", "gfx.world_to_screen(wx, wy) -> Vec2", "Project 2D world point into screen pixels"),
    fd("gfx", "screen_to_world", "gfx.screen_to_world(sx, sy) -> Vec2", "Unproject screen pixels into 2D world space"),
    fd("gfx", "mouse_world", "gfx.mouse_world() -> Vec2", "Unproject mouse coordinates into 2D world space"),
    fd("gfx", "font_bitmap", "gfx.font_bitmap() -> int", "Return built-in 8x8 deterministic bitmap font handle (0)"),
    fdc("gfx", "font_load_bmfont", "gfx.font_load_bmfont(fnt_path) -> int", "Load BMFont `.fnt` font descriptor and glyph atlas", "--allow-read"),
    fd("gfx", "font_free", "gfx.font_free(handle) -> unit", "Release custom BMFont handle"),
    fd("gfx", "draw_text", "gfx.draw_text(font, x, y, text, r, g, b, [a]) -> unit", "Rasterize bitmap or BMFont glyphs onto active surface/target"),
    fd("gfx", "measure_text", "gfx.measure_text(font, text) -> dict", "Return pixel width and height metrics for text"),
    fd("gfx", "text_line_height", "gfx.text_line_height(font) -> int", "Return font line height in pixels"),
    fd("gfx", "text_width", "gfx.text_width(font, text) -> int", "Return rendered string pixel width"),

    // glob
    fd("glob", "match", "glob.match(pattern, text) -> bool", "Step-bounded glob pattern matcher (`*`, `**`, `?`)"),

    // hash
    fd("hash", "sha256", "hash.sha256(data) -> str", "Return 64-char lowercase hex SHA-256 digest"),
    fd("hash", "crc32", "hash.crc32(data) -> int", "Compute CRC32 checksum integer"),
    fd("hash", "fnv1a", "hash.fnv1a(data) -> int", "Compute 64-bit FNV-1a non-cryptographic hash"),

    // hex
    fd("hex", "encode", "hex.encode(data) -> str", "Encode bytes or string to lowercase hexadecimal"),
    fd("hex", "decode", "hex.decode(hex_str) -> bytes", "Decode hexadecimal string to bytes"),

    // html
    fd("html", "escape", "html.escape(text) -> str", "Escape `<`, `>`, `&`, quotes for safe HTML output"),
    fd("html", "unescape", "html.unescape(text) -> str", "Decode common HTML character entities"),
    fd("html", "parse", "html.parse(html_str) -> dict", "Parse HTML fragment into bounded DOM tree"),
    fd("html", "select", "html.select(doc, selector) -> list", "Match simple tag/class/id CSS selector against parsed DOM"),
    fd("html", "text", "html.text(node) -> str", "Extract concatenated inner text from DOM node"),

    // http
    fdc("http", "get", "http.get(url, [headers]) -> dict", "HTTPS GET request (`{status, body}`) via WinHTTP or OpenSSL", "--allow-net"),
    fdc("http", "post", "http.post(url, body, [content_type_or_headers]) -> dict", "HTTPS POST request via WinHTTP or OpenSSL", "--allow-net"),

    // image
    fd("image", "new", "image.new(w, h, [opts]) -> Image", "Allocate blank budget-checked RGBA8/R16F Image"),
    fd("image", "from_bytes", "image.from_bytes(w, h, bytes, [opts]) -> Image", "Wrap raw pixel bytes into validated Image"),
    fd("image", "decode", "image.decode(bytes, [opts]) -> Image", "Auto-detect and decode PNG/BMP/QOI/PPM image bytes"),
    fdc("image", "load", "image.load(path, [opts]) -> Image", "Load image file directly from VFS or sandbox disk", "--allow-read"),
    fd("image", "info", "image.info(bytes_or_img) -> dict", "Inspect dimensions, pixel format & color space"),
    fd("image", "meta", "image.meta(img) -> dict", "Return metadata dict for Image"),
    fd("image", "digest", "image.digest(img) -> str", "Deterministic SHA-256 pixel digest"),
    fd("image", "width", "image.width(img) -> int", "Return pixel width"),
    fd("image", "height", "image.height(img) -> int", "Return pixel height"),
    fd("image", "format", "image.format(img) -> str", "Return pixel format name (rgba8, rgb8, r16, ...)"),
    fd("image", "color_space", "image.color_space(img) -> str", "Return color space tag (srgb / linear)"),
    fd("image", "frame_count", "image.frame_count(img) -> int", "Return animation frame count"),
    fd("image", "frame", "image.frame(img, idx) -> Image", "Extract single animation frame"),
    fd("image", "get_pixel", "image.get_pixel(img, x, y) -> dict", "Sample pixel `{r, g, b, a}`"),
    fd("image", "to_bytes", "image.to_bytes(img, [format]) -> bytes", "Encode image to PNG/BMP/raw pixel bytes"),
    fd("image", "convert", "image.convert(img, format, [color_space]) -> Image", "Convert image pixel format cleanly"),
    fd("image", "to_linear", "image.to_linear(img) -> Image", "Convert sRGB pixels to linear space"),
    fd("image", "to_srgb", "image.to_srgb(img) -> Image", "Convert linear pixels to sRGB"),
    fd("image", "premultiply", "image.premultiply(img) -> Image", "Premultiply RGB channels by alpha"),
    fd("image", "crop", "image.crop(img, x, y, w, h) -> Image", "Crop sub-rectangle into new Image"),
    fd("image", "resize", "image.resize(img, w, h, [filter]) -> Image", "Resize Image using nearest or bilinear sampling"),
    fd("image", "flip", "image.flip(img, dir) -> Image", "Flip image (`h` or `v`)"),
    fd("image", "flip_x", "image.flip_x(img) -> Image", "Mirror image horizontally"),
    fd("image", "flip_y", "image.flip_y(img) -> Image", "Flip image vertically"),
    fd("image", "rotate", "image.rotate(img, deg) -> Image", "Rotate image by 90/180/270 degrees"),
    fd("image", "mipmaps", "image.mipmaps(img, [levels]) -> Image", "Generate deterministic mipmap chain"),
    fd("image", "mip_count", "image.mip_count(img) -> int", "Return number of mip levels"),
    fd("image", "mip_level", "image.mip_level(img, level) -> Image", "Extract single mip level Image"),
    fd("image", "mip_digest", "image.mip_digest(img) -> str", "Deterministic hash of full mip chain"),
    fd("image", "height_to_normal", "image.height_to_normal(img, [strength]) -> Image", "Convert grayscale heightmap to tangent-space normal map"),
    fd("image", "texture", "image.texture(img, [opts]) -> Texture", "Promote Image to filterable/wrappable Texture"),
    fd("image", "sample", "image.sample(tex_or_img, u, v) -> dict", "Sample texture with wrap & filter interpolation"),
    fd("image", "atlas", "image.atlas(dict_of_images, [pad]) -> Atlas", "Shelf-pack named images deterministically into ImageAtlas"),
    fdc("image", "atlas_from_paths", "image.atlas_from_paths(paths, [pad]) -> Atlas", "Load and pack list of image paths into ImageAtlas", "--allow-read"),
    fd("image", "sprite_frame", "image.sprite_frame(img, cols, rows, idx) -> Image", "Extract single grid frame"),

    // ini
    fd("ini", "parse", "ini.parse(text) -> dict", "Parse INI sections and key=value pairs safely"),
    fd("ini", "stringify", "ini.stringify(dict) -> str", "Serialize nested dict as deterministic INI text"),

    // input
    fd("input", "vk_code", "input.vk_code(name) -> int", "Resolve key name string to virtual-key code"),
    fd("input", "key_name", "input.key_name(vk) -> str", "Convert virtual-key code to key name"),
    fdc("input", "key_down", "input.key_down(key) -> bool", "Poll async key state (Windows)", "--allow-input"),
    fdc("input", "key_pressed", "input.key_pressed(key) -> bool", "Poll edge-triggered global key state (Windows)", "--allow-input"),
    fdc("input", "hotkey", "input.hotkey(combo) -> bool", "Check chord like \"CTRL+SHIFT+S\" (Windows)", "--allow-input"),
    fdc("input", "mouse_down", "input.mouse_down([button]) -> bool", "Poll async mouse button state (Windows)", "--allow-input"),
    fdc("input", "mouse_pos", "input.mouse_pos() -> list", "Read global desktop cursor `[x, y]` (Windows)", "--allow-input"),

    // json
    fd("json", "parse", "json.parse(json_str) -> any", "Parse strict RFC 8259 JSON enforcing depth & control-char limits"),
    fd("json", "stringify", "json.stringify(value, [pretty]) -> str", "Serialize Askane value to JSON string deterministically"),

    // kv
    fdc("kv", "open", "kv.open(path) -> KVStore", "Open crash-safe CRC32 append-only key-value store", "--allow-read / --allow-write"),
    fd("kv", "get", "kv.get(db, key) -> any | unit", "Lookup value in open KV store"),
    fd("kv", "set", "kv.set(db, key, val) -> unit", "Write key-value pair atomically"),
    fd("kv", "delete", "kv.delete(db, key) -> bool", "Delete key tombstone in KV store"),
    fd("kv", "has", "kv.has(db, key) -> bool", "Check if key exists in KV store"),
    fd("kv", "keys", "kv.keys(db) -> list", "List sorted keys in KV store"),
    fd("kv", "prefix", "kv.prefix(db, pfx) -> list", "List keys starting with prefix"),
    fd("kv", "batch", "kv.batch(db, ops) -> unit", "Apply batch set/delete transaction atomically"),
    fd("kv", "compact", "kv.compact(db) -> unit", "Compact append log removing overwritten records"),
    fd("kv", "len", "kv.len(db) -> int", "Return live key count"),
    fd("kv", "close", "kv.close(db) -> unit", "Flush and close KV store handle"),

    // lighting
    fd("lighting", "point", "lighting.point(x, y, rad, r, g, b, [intensity]) -> dict", "Create 2D point light descriptor"),
    fd("lighting", "point3d", "lighting.point3d(x, y, z, rad, r, g, b, [intensity]) -> dict", "Create 3D point light descriptor"),
    fd("lighting", "directional", "lighting.directional(dx, dy, dz, r, g, b, [intensity]) -> dict", "Create normalized directional sun light"),
    fd("lighting", "spot", "lighting.spot(x, y, dx, dy, cone_deg, rad, r, g, b, [intensity]) -> dict", "Create 2D spotlight cone descriptor"),
    fd("lighting", "lightmap", "lighting.lightmap(w, h, [ambient]) -> int", "Allocate HDR RGB16F-equivalent lightmap handle"),
    fd("lighting", "clear", "lighting.clear(lmap, [r, g, b]) -> unit", "Clear lightmap to ambient radiance"),
    fd("lighting", "apply", "lighting.apply(lmap, lights, [solids]) -> unit", "Accumulate lights with optional DDA hard shadows"),
    fd("lighting", "composite", "lighting.composite(lmap, target_or_img) -> Image", "Multiply lit radiance onto target surface"),
    fd("lighting", "close", "lighting.close(lmap) -> unit", "Release lightmap handle"),
    fd("lighting", "free", "lighting.free(lmap) -> unit", "Alias for lighting.close()"),

    // log
    fd("log", "set_level", "log.set_level(level) -> unit", "Set minimum log threshold (debug/info/warn/error)"),
    fd("log", "json", "log.json(enabled) -> unit", "Toggle structured JSON log line output"),
    fd("log", "format", "log.format(level, msg, [fields]) -> str", "Format redacted log message without printing"),
    fd("log", "debug", "log.debug(msg, [fields]) -> unit", "Emit DEBUG log record"),
    fd("log", "info", "log.info(msg, [fields]) -> unit", "Emit INFO log record"),
    fd("log", "warn", "log.warn(msg, [fields]) -> unit", "Emit WARN log record"),
    fd("log", "error", "log.error(msg, [fields]) -> unit", "Emit ERROR log record"),

    // math
    fd("math", "sqrt", "math.sqrt(x) -> float", "Square root (deterministic IEEE 754)"),
    fd("math", "abs", "math.abs(x) -> number", "Absolute value preserving int/float when safe"),
    fd("math", "pow", "math.pow(base, exp) -> float", "Exponentiation base ** exp"),
    fd("math", "sin", "math.sin(rad) -> float", "Sine of angle in radians"),
    fd("math", "cos", "math.cos(rad) -> float", "Cosine of angle in radians"),
    fd("math", "floor", "math.floor(x) -> float", "Floor toward negative infinity"),
    fd("math", "ceil", "math.ceil(x) -> float", "Ceiling toward positive infinity"),
    fd("math", "round", "math.round(x) -> float", "Round half away from zero"),
    fd("math", "min", "math.min(a, b) -> number", "Return smaller of two numbers"),
    fd("math", "max", "math.max(a, b) -> number", "Return larger of two numbers"),
    fd("math", "clamp", "math.clamp(x, lo, hi) -> number", "Clamp number into closed interval [lo, hi]"),
    fd("math", "lerp", "math.lerp(a, b, t) -> float", "Linear interpolation a + (b - a) * t"),
    fd("math", "dist", "math.dist(x1, y1, x2, y2) -> float", "2D Euclidean distance"),
    fd("math", "atan2", "math.atan2(y, x) -> float", "Four-quadrant inverse tangent"),
    fd("math", "pi", "math.pi: float", "Archimedes constant (3.141592653589793)"),
    fd("math", "e", "math.e: float", "Euler number (2.718281828459045)"),

    // mem
    fdc("mem", "current_pid", "mem.current_pid() -> int", "Return current Win32 process ID", "--unsafe + ASKANE_ENABLE_UNSAFE_MEM=1"),
    fdc("mem", "find_pid", "mem.find_pid(exe_name) -> int | unit", "Find first matching process ID by name (Windows)", "--unsafe + ASKANE_ENABLE_UNSAFE_MEM=1"),
    fdc("mem", "open", "mem.open(pid, [writable]) -> MemHandle", "Open process handle for inspection (Windows)", "--unsafe + ASKANE_ENABLE_UNSAFE_MEM=1"),
    fdc("mem", "close", "mem.close(handle) -> unit", "Close Win32 process handle", "--unsafe + ASKANE_ENABLE_UNSAFE_MEM=1"),
    fdc("mem", "module_base", "mem.module_base(handle, mod_name) -> int | unit", "Lookup loaded DLL base address", "--unsafe + ASKANE_ENABLE_UNSAFE_MEM=1"),
    fdc("mem", "read_i32", "mem.read_i32(handle, addr) -> int", "Read 32-bit signed int from process memory", "--unsafe + ASKANE_ENABLE_UNSAFE_MEM=1"),
    fdc("mem", "read_i64", "mem.read_i64(handle, addr) -> int", "Read 64-bit signed int from process memory", "--unsafe + ASKANE_ENABLE_UNSAFE_MEM=1"),
    fdc("mem", "read_f32", "mem.read_f32(handle, addr) -> float", "Read 32-bit float from process memory", "--unsafe + ASKANE_ENABLE_UNSAFE_MEM=1"),
    fdc("mem", "read_f64", "mem.read_f64(handle, addr) -> float", "Read 64-bit float from process memory", "--unsafe + ASKANE_ENABLE_UNSAFE_MEM=1"),
    fdc("mem", "read_bytes", "mem.read_bytes(handle, addr, len) -> list", "Read bounded raw bytes from process memory", "--unsafe + ASKANE_ENABLE_UNSAFE_MEM=1"),
    fdc("mem", "write_i32", "mem.write_i32(handle, addr, val) -> unit", "Write 32-bit int to process memory", "--unsafe + ASKANE_ENABLE_UNSAFE_MEM=1"),
    fdc("mem", "write_i64", "mem.write_i64(handle, addr, val) -> unit", "Write 64-bit int to process memory", "--unsafe + ASKANE_ENABLE_UNSAFE_MEM=1"),
    fdc("mem", "write_f32", "mem.write_f32(handle, addr, val) -> unit", "Write 32-bit float to process memory", "--unsafe + ASKANE_ENABLE_UNSAFE_MEM=1"),
    fdc("mem", "write_f64", "mem.write_f64(handle, addr, val) -> unit", "Write 64-bit float to process memory", "--unsafe + ASKANE_ENABLE_UNSAFE_MEM=1"),

    // mesh
    fdc("mesh", "load", "mesh.load(path, [opts]) -> MeshHandle", "Parse bounded OBJ, glTF JSON, or GLB binary 3D asset", "--allow-read"),
    fd("mesh", "stats", "mesh.stats(mesh) -> dict", "Return vertex/triangle/submesh counts and bounds"),
    fd("mesh", "aabb", "mesh.aabb(mesh) -> dict", "Return `{min, max}` axis-aligned bounding box"),
    fd("mesh", "nodes", "mesh.nodes(mesh) -> list", "Return scene node hierarchy metadata"),
    fd("mesh", "materials", "mesh.materials(mesh) -> list", "Return material descriptors"),
    fd("mesh", "warnings", "mesh.warnings(mesh) -> list", "Return loader sanity warnings"),
    fd("mesh", "flatten", "mesh.flatten(mesh, [out_bufs]) -> dict", "Flatten submeshes/transforms into draw buffers"),
    fd("mesh", "verify", "mesh.verify(mesh) -> bool", "Validate index bounds, normals & manifold integrity"),
    fd("mesh", "close", "mesh.close(mesh) -> unit", "Release Mesh handle and refund budget"),
    fd("mesh", "free", "mesh.free(mesh) -> unit", "Alias for mesh.close()"),

    // nav
    fd("nav", "grid", "nav.grid(w, h, [default_cost]) -> NavGrid", "Create bounded 2D navigation cost grid"),
    fd("nav", "from_solid", "nav.from_solid(solid_grid) -> NavGrid", "Construct NavGrid from 0/1 collision grid"),
    fd("nav", "from_tiled", "nav.from_tiled(tiled_map, [layer]) -> NavGrid", "Construct NavGrid from Tiled solid layer"),
    fd("nav", "from_terrain", "nav.from_terrain(terrain, max_slope) -> NavGrid", "Construct walkable NavGrid from terrain slope threshold"),
    fd("nav", "set_cost", "nav.set_cost(nav, x, y, cost) -> unit", "Set traversal cost (`< 0` marks blocked cell)"),
    fd("nav", "is_walkable", "nav.is_walkable(nav, x, y) -> bool", "Check if cell is inside grid and walkable"),
    fd("nav", "find_path", "nav.find_path(nav, sx, sy, gx, gy, [opts]) -> list", "Deterministic A* path search returning `[x, y]` waypoints"),
    fd("nav", "path_smooth", "nav.path_smooth(nav, path) -> list", "Line-of-sight string-pulling path smoother"),
    fd("nav", "flow_field", "nav.flow_field(nav, gx, gy) -> dict", "Compute BFS/Dijkstra flow direction field toward goal"),
    fd("nav", "info", "nav.info(nav) -> dict", "Return grid dimensions & walkable cell count"),
    fd("nav", "close", "nav.close(nav) -> unit", "Release NavGrid handle"),
    fd("nav", "free", "nav.free(nav) -> unit", "Alias for nav.close()"),

    // net
    fdc("net", "tcp_listen", "net.tcp_listen(host, port) -> TcpListener", "Bind TCP server socket", "--allow-net"),
    fdc("net", "tcp_accept", "net.tcp_accept(listener, [timeout_ms]) -> TcpStream | unit", "Accept pending client connection", "--allow-net"),
    fdc("net", "tcp_connect", "net.tcp_connect(host, port, [timeout_ms]) -> TcpStream", "Connect TCP client socket", "--allow-net"),
    fdc("net", "send", "net.send(sock, data) -> int", "Send string or bytes over TCP socket", "--allow-net"),
    fdc("net", "recv", "net.recv(sock, [max_bytes]) -> bytes", "Receive bounded chunk from TCP stream", "--allow-net"),
    fdc("net", "udp_bind", "net.udp_bind(host, port) -> UdpSocket", "Bind sandboxed UDP socket", "--allow-net"),
    fdc("net", "udp_send_to", "net.udp_send_to(sock, data, host, port) -> int", "Send UDP datagram checking host allowlist", "--allow-net"),
    fdc("net", "udp_recv_from", "net.udp_recv_from(sock, [max_bytes]) -> dict", "Receive UDP datagram `{data, host, port}`", "--allow-net"),
    fd("net", "local_addr", "net.local_addr(sock) -> str", "Return bound socket address"),
    fd("net", "peer_addr", "net.peer_addr(sock) -> str", "Return connected remote peer address"),
    fd("net", "close", "net.close(sock) -> unit", "Close TCP/UDP socket handle"),

    // os
    fd("os", "platform", "os.platform() -> str", "Normalized OS family (`windows`, `linux`, `macos`)"),
    fd("os", "arch", "os.arch() -> str", "CPU architecture (`x86_64`, `aarch64`, ...)"),
    fd("os", "family", "os.family() -> str", "OS family (`windows` or `unix`)"),
    fd("os", "cpu_count", "os.cpu_count() -> int", "Available hardware logical CPU cores"),
    fd("os", "pid", "os.pid() -> int", "Current interpreter process ID"),
    fd("os", "sep", "os.sep() -> str", "Native filesystem path separator"),
    fd("os", "line_ending", "os.line_ending() -> str", "Platform newline string"),
    fdc("os", "cwd", "os.cwd() -> str", "Current working directory (requires read permission)", "--allow-read"),
    fdc("os", "home_dir", "os.home_dir() -> str | unit", "User home directory path", "--allow-read"),
    fdc("os", "temp_dir", "os.temp_dir() -> str", "System temp directory path", "--allow-read"),
    fdc("os", "env", "os.env(name) -> str | unit", "Read environment variable", "--allow-env"),
    fdc("os", "args", "os.args() -> list", "Read script CLI arguments after `--`", "--allow-args"),

    // particles
    fd("particles", "create", "particles.create([capacity]) -> int", "Allocate deterministic particle pool handle"),
    fd("particles", "seed", "particles.seed(pool, seed) -> unit", "Seed internal PCG32 PRNG deterministically"),
    fd("particles", "set_gravity", "particles.set_gravity(pool, gx, gy) -> unit", "Set constant pool acceleration"),
    fd("particles", "spawn", "particles.spawn(pool, x, y, vx, vy, life, opts) -> unit", "Spawn single particle cleanly"),
    fd("particles", "emit", "particles.emit(pool, count, cfg) -> int", "Emit burst of particles from cone/box emitter"),
    fd("particles", "update", "particles.update(pool, dt) -> unit", "Advance pool simulation by fixed/frame dt"),
    fd("particles", "draw", "particles.draw(pool) -> unit", "Draw live particles onto active gfx surface/window"),
    fd("particles", "count", "particles.count(pool) -> int", "Return live particle count"),
    fd("particles", "clear", "particles.clear(pool) -> unit", "Clear all particles in pool"),
    fd("particles", "close", "particles.close(pool) -> unit", "Release particle pool handle"),
    fd("particles", "free", "particles.free(pool) -> unit", "Alias for particles.close()"),

    // path
    fd("path", "join", "path.join(a, b, ...) -> str", "Join path segments normalizing separators"),
    fd("path", "parent", "path.parent(p) -> str", "Return parent directory component"),
    fd("path", "name", "path.name(p) -> str", "Return trailing filename component"),
    fd("path", "stem", "path.stem(p) -> str", "Return filename without final extension"),
    fd("path", "extension", "path.extension(p) -> str", "Return extension without leading dot"),
    fd("path", "is_absolute", "path.is_absolute(p) -> bool", "Return true if path is rooted/absolute"),
    fd("path", "normalize", "path.normalize(p) -> str", "Lexically collapse `.` and `..` segments"),

    // postfx
    fd("postfx", "bloom", "postfx.bloom(img, thresh, intensity, [radius]) -> Image", "Apply bounded separable Gaussian HDR bloom"),
    fdc("postfx", "lut_load", "postfx.lut_load(cube_or_png_path) -> int", "Load `.cube` or PNG 3D color-grading LUT", "--allow-read"),
    fd("postfx", "lut_apply", "postfx.lut_apply(img, lut, [strength]) -> Image", "Apply trilinear 3D LUT color grading"),
    fd("postfx", "lut_size", "postfx.lut_size(lut) -> int", "Return LUT grid dimension N (NxNxN)"),
    fd("postfx", "vignette", "postfx.vignette(img, strength, [softness]) -> Image", "Apply smooth radial edge darkening"),
    fd("postfx", "scanlines", "postfx.scanlines(img, intensity, [pitch]) -> Image", "Apply CRT scanline darkening"),
    fdc("postfx", "snapshot", "postfx.snapshot(out_bmp_path, [img]) -> unit", "Write deterministic BMP24 screenshot inside write sandbox", "--allow-write"),
    fd("postfx", "close", "postfx.close(lut) -> unit", "Release LUT handle"),
    fd("postfx", "free", "postfx.free(lut) -> unit", "Alias for postfx.close()"),

    // pprint
    fd("pprint", "format", "pprint.format(val, [indent]) -> str", "Bounded pretty-format nested structures cleanly"),
    fd("pprint", "print", "pprint.print(val) -> unit", "Pretty-print value directly to stdout"),

    // process
    fdc("process", "run", "process.run(cmd, [args]) -> dict", "Run allowlisted child command bounded by timeout/memory returning `{status, stdout, stderr}`", "--allow-exec"),

    // procgen
    fd("procgen", "seed", "procgen.seed(seed) -> unit", "Set active 64-bit deterministic seed for procgen"),
    fd("procgen", "hash2d", "procgen.hash2d(x, y, [seed]) -> float", "Pure fixed-point integer 2D hash in [0, 1)"),
    fd("procgen", "noise2d", "procgen.noise2d(x, y, [seed]) -> float", "Smooth value noise in [0, 1]"),
    fd("procgen", "perlin2", "procgen.perlin2(x, y, [seed]) -> float", "Gradient Perlin noise in [-1, 1]"),
    fd("procgen", "worley2", "procgen.worley2(x, y, [seed]) -> float", "Cellular Worley F1 distance noise"),
    fd("procgen", "fbm2d", "procgen.fbm2d(x, y, [octaves, lacunarity, gain, seed]) -> float", "Fractal Brownian Motion noise"),
    fd("procgen", "heightmap", "procgen.heightmap(w, h, [opts]) -> Image", "Generate deterministic heightmap image"),
    fd("procgen", "normal_map", "procgen.normal_map(height_img, [strength]) -> Image", "Derive RGB tangent normal map from heightmap"),
    fd("procgen", "checker", "procgen.checker(w, h, tile_px, c1, c2) -> Image", "Generate crisp test checkerboard Image"),
    fd("procgen", "cellular", "procgen.cellular(w, h, [steps, fill, seed]) -> list", "Generate cellular automata dungeon cave grid"),
    fd("procgen", "scatter", "procgen.scatter(w, h, r, [k, seed]) -> list", "Poisson-disc blue-noise `[x, y]` scatter points"),

    // random
    fd("random", "seed", "random.seed(n) -> unit", "Seed deterministic xorshift64* PRNG"),
    fd("random", "int", "random.int(lo, hi) -> int", "Uniform integer in [lo, hi]"),
    fd("random", "float", "random.float() -> float", "Uniform float in [0.0, 1.0)"),
    fd("random", "bool", "random.bool([p]) -> bool", "Bernoulli trial with optional probability p"),
    fd("random", "choice", "random.choice(list) -> any", "Pick uniform element from non-empty list"),
    fd("random", "shuffle", "random.shuffle(list) -> list", "Fisher-Yates shuffled copy"),
    fd("random", "crypto_int", "random.crypto_int(lo, hi) -> int", "CSPRNG integer in [lo, hi]"),
    fd("random", "crypto_bytes", "random.crypto_bytes(n) -> bytes", "OS CSPRNG random byte buffer"),

    // regex
    fd("regex", "is_match", "regex.is_match(pattern, text) -> bool", "Step-bounded regex match (anti-ReDoS)"),
    fd("regex", "find", "regex.find(pattern, text) -> str | unit", "Find first substring match"),
    fd("regex", "find_all", "regex.find_all(pattern, text) -> list", "Find all non-overlapping matches"),
    fd("regex", "replace", "regex.replace(pattern, text, rep) -> str", "Replace matches cleanly"),

    // scene
    fd("scene", "assemble", "scene.assemble(cfg) -> int", "Compile terrain, tiled map, entities, lighting & postfx into Scene handle"),
    fd("scene", "info", "scene.info(scene) -> dict", "Return scene component summary"),
    fd("scene", "stats", "scene.stats(scene) -> dict", "Return entity, mesh, light & pixel stats"),
    fd("scene", "entities", "scene.entities(scene) -> list", "Return scene entity placements snapped cleanly to terrain/tiles"),
    fd("scene", "draw", "scene.draw(scene, target_or_opts) -> Image", "Render scene deterministically into offscreen Image / depth target"),
    fd("scene", "close", "scene.close(scene) -> unit", "Release Scene handle"),
    fd("scene", "free", "scene.free(scene) -> unit", "Alias for scene.close()"),

    // signal
    fd("signal", "new", "signal.new(init_val) -> Signal", "Create reactive state cell"),
    fd("signal", "of", "signal.of(init_val) -> Signal", "Alias for signal.new()"),
    fd("signal", "get", "signal.get(sig) -> any", "Read current reactive value"),
    fd("signal", "set", "signal.set(sig, val) -> unit", "Update signal notifying synchronous observers"),
    fd("signal", "update", "signal.update(sig, fn) -> any", "Atomically compute new signal value via callback"),
    fd("signal", "watch", "signal.watch(sig, fn) -> int", "Attach observer callback returning watch ID"),
    fd("signal", "unwatch", "signal.unwatch(sig, id) -> bool", "Detach single observer"),
    fd("signal", "unwatch_all", "signal.unwatch_all(sig) -> unit", "Clear all observers on signal"),

    // strings
    fd("strings", "split", "strings.split(s, sep) -> list", "Split string by separator"),
    fd("strings", "trim", "strings.trim(s) -> str", "Strip leading and trailing whitespace"),
    fd("strings", "trim_start", "strings.trim_start(s) -> str", "Strip leading whitespace"),
    fd("strings", "trim_end", "strings.trim_end(s) -> str", "Strip trailing whitespace"),
    fd("strings", "replace", "strings.replace(s, from, to) -> str", "Replace all non-overlapping substrings"),
    fd("strings", "contains", "strings.contains(s, sub) -> bool", "Check substring inclusion"),
    fd("strings", "starts_with", "strings.starts_with(s, pfx) -> bool", "Check prefix"),
    fd("strings", "ends_with", "strings.ends_with(s, sfx) -> bool", "Check suffix"),
    fd("strings", "upper", "strings.upper(s) -> str", "Convert to Unicode uppercase"),
    fd("strings", "lower", "strings.lower(s) -> str", "Convert to Unicode lowercase"),
    fd("strings", "find", "strings.find(s, sub) -> int", "Return char index of first occurrence or -1"),
    fd("strings", "repeat", "strings.repeat(s, n) -> str", "Repeat string `n` times bounded"),
    fd("strings", "lines", "strings.lines(s) -> list", "Split string on LF/CRLF"),
    fd("strings", "pad_start", "strings.pad_start(s, width, [fill]) -> str", "Left-pad to char width"),
    fd("strings", "pad_end", "strings.pad_end(s, width, [fill]) -> str", "Right-pad to char width"),
    fd("strings", "slice", "strings.slice(s, start, [end]) -> str", "Unicode char-indexed substring slice"),
    fd("strings", "join", "strings.join(list, sep) -> str", "Join sequence into string with separator"),
    fd("strings", "byte_len", "strings.byte_len(s) -> int", "Return UTF-8 byte count"),
    fd("strings", "eq_ignore_case", "strings.eq_ignore_case(a, b) -> bool", "ASCII case-insensitive equality"),
    fd("strings", "contains_ignore_case", "strings.contains_ignore_case(s, sub) -> bool", "ASCII case-insensitive substring check"),
    fd("strings", "strip_prefix", "strings.strip_prefix(s, pfx) -> str | unit", "Strip prefix if present"),
    fd("strings", "strip_suffix", "strings.strip_suffix(s, sfx) -> str | unit", "Strip suffix if present"),
    fd("strings", "split_once", "strings.split_once(s, delim) -> list | unit", "Split on first delimiter into `[left, right]`"),
    fd("strings", "rsplit_once", "strings.rsplit_once(s, delim) -> list | unit", "Split on last delimiter into `[left, right]`"),
    fd("strings", "truncate_utf8", "strings.truncate_utf8(s, max_bytes) -> str", "Truncate strictly at valid UTF-8 boundary"),

    // sys
    fdc("sys", "args", "sys.args() -> list", "Return script CLI arguments after `--`", "--allow-args"),
    fdc("sys", "env", "sys.env(name) -> str | unit", "Read environment variable", "--allow-env"),
    fd("sys", "exit", "sys.exit(code) -> never", "Terminate script cleanly with exit status"),
    fd("sys", "os", "sys.os() -> str", "Host OS identifier (`windows`, `linux`, `macos`)"),
    fd("sys", "arch", "sys.arch() -> str", "Host CPU architecture (`x86_64`, `aarch64`)"),
    fd("sys", "sleep", "sys.sleep(ms) -> unit", "Bounded sleep (virtual time under --deterministic)"),
    fd("sys", "budget", "sys.budget() -> dict", "Snapshot current memory, I/O & instruction sandbox counters"),
    fd("sys", "backends", "sys.backends() -> dict", "Probe available HAL backends (gfx, audio, http, clipboard)"),

    // task
    fd("task", "spawn", "task.spawn(fn, ...args) -> TaskHandle", "Spawn cooperative green coroutine"),
    fd("task", "yield_now", "task.yield_now() -> unit", "Yield execution to green scheduler"),
    fd("task", "sleep", "task.sleep(ms) -> unit", "Suspend current task deterministically/bounded"),
    fd("task", "join", "task.join(handle) -> any", "Await task completion returning result"),
    fd("task", "is_done", "task.is_done(handle) -> bool", "Non-blocking completion check"),
    fd("task", "select", "task.select(handles_or_channels) -> dict", "Wait until first handle becomes ready"),
    fd("task", "timeout", "task.timeout(ms, fn) -> dict", "Execute task body under millisecond deadline"),

    // term
    fd("term", "red", "term.red(s) -> str", "Wrap string in ANSI red foreground escape"),
    fd("term", "green", "term.green(s) -> str", "Wrap string in ANSI green foreground escape"),
    fd("term", "yellow", "term.yellow(s) -> str", "Wrap string in ANSI yellow foreground escape"),
    fd("term", "cyan", "term.cyan(s) -> str", "Wrap string in ANSI cyan foreground escape"),
    fd("term", "bold", "term.bold(s) -> str", "Wrap string in ANSI bold style"),
    fd("term", "clear", "term.clear() -> unit", "Clear terminal screen via ANSI escape"),

    // terrain
    fdc("terrain", "load", "terrain.load(path, [opts]) -> int", "Load heightmap (.r16 raw or PNG/grayscale) into Terrain handle", "--allow-read"),
    fd("terrain", "from_image", "terrain.from_image(img, [opts]) -> int", "Create Terrain handle from decoded Image"),
    fd("terrain", "info", "terrain.info(terr) -> dict", "Return width, height & min/max elevation"),
    fd("terrain", "height", "terrain.height(terr, ix, iz) -> float", "Sample exact grid vertex height"),
    fd("terrain", "height_at", "terrain.height_at(terr, x, z) -> float", "Bilinear interpolated height in world XZ coordinates"),
    fd("terrain", "normal", "terrain.normal(terr, ix, iz) -> Vec3", "Compute surface normal at grid vertex"),
    fd("terrain", "normal_at", "terrain.normal_at(terr, x, z) -> Vec3", "Interpolated surface normal at XZ coordinate"),
    fd("terrain", "slope", "terrain.slope(terr, x, z) -> float", "Return slope angle/steepness `[0..1]` at XZ"),
    fd("terrain", "to_solid", "terrain.to_solid(terr, max_slope) -> list", "Convert steep terrain regions to collision grid"),
    fd("terrain", "to_mesh", "terrain.to_mesh(terr, [step]) -> MeshHandle", "Generate indexed 3D triangle mesh from heightmap"),
    fdc("terrain", "splat_from_image", "terrain.splat_from_image(img) -> int", "Create RGBA splatmap handle from Image", "--allow-read"),
    fd("terrain", "splat_weights", "terrain.splat_weights(splat, x, z) -> dict", "Sample normalized RGBA blend weights"),
    fd("terrain", "mode7_draw", "terrain.mode7_draw(terr, img, target, cam_opts) -> Image", "Render SNES Mode-7 perspective ground plane"),
    fd("terrain", "close", "terrain.close(terr) -> unit", "Release Terrain or splatmap handle"),
    fd("terrain", "free", "terrain.free(terr) -> unit", "Alias for terrain.close()"),

    // test
    fd("test", "assert", "test.assert(cond, [msg]) -> unit", "Fail spec if cond is falsy"),
    fd("test", "assert_eq", "test.assert_eq(a, b, [msg]) -> unit", "Structural equality assertion with readable diff"),
    fd("test", "assert_ne", "test.assert_ne(a, b, [msg]) -> unit", "Structural inequality assertion"),
    fd("test", "assert_err", "test.assert_err(fn, [substr]) -> str", "Assert callback throws error optionally containing `substr`"),
    fd("test", "assert_snapshot", "test.assert_snapshot(name, actual) -> unit", "Compare string/bytes against `__snapshots__/<name>` (updated with --update-snapshots)"),

    // tiled
    fdc("tiled", "load", "tiled.load(path, [opts]) -> int", "Parse Tiled `.tmj` JSON or `.tmx` XML map with orthogonal/hex layers", "--allow-read"),
    fd("tiled", "info", "tiled.info(map) -> dict", "Return map dimensions, tile size & orientation"),
    fd("tiled", "stats", "tiled.stats(map) -> dict", "Return layer, tileset & non-empty tile counts"),
    fd("tiled", "layers", "tiled.layers(map) -> list", "List layer metadata dicts"),
    fd("tiled", "tile_at", "tiled.tile_at(map, layer, tx, ty) -> int", "Read decoded GID at tile cell `(tx, ty)`"),
    fd("tiled", "solid_grid", "tiled.solid_grid(map, [layer]) -> dict", "Extract collision boolean grid from solid/collision layer"),
    fd("tiled", "is_solid", "tiled.is_solid(grid_or_map, tx, ty) -> bool", "Test tile solidity"),
    fd("tiled", "place_on_terrain", "tiled.place_on_terrain(map, terr) -> list", "Project map object markers onto terrain elevation"),
    fd("tiled", "draw", "tiled.draw(map, tileset_img, [cam_x, cam_y]) -> Image", "Software-render visible orthogonal Tiled layers"),
    fd("tiled", "close", "tiled.close(map) -> unit", "Release Tiled map or grid handle"),
    fd("tiled", "free", "tiled.free(map) -> unit", "Alias for tiled.close()"),

    // time
    fd("time", "now_ms", "time.now_ms() -> int", "Unix epoch UTC milliseconds (virtual under --deterministic)"),
    fd("time", "now_sec", "time.now_sec() -> float", "Unix epoch UTC seconds as float"),
    fd("time", "elapsed_ms", "time.elapsed_ms() -> int", "Monotonic ms since VM start"),
    fd("time", "sleep_ms", "time.sleep_ms(ms) -> unit", "Bounded millisecond sleep"),

    // toml
    fd("toml", "parse", "toml.parse(text) -> dict", "Parse TOML document enforcing table/array depth bounds"),
    fd("toml", "stringify", "toml.stringify(dict) -> str", "Serialize dict hierarchy cleanly into TOML text"),

    // ui
    fd("ui", "rgb", "ui.rgb(r, g, b, [a]) -> dict", "Construct RGBA color descriptor"),
    fd("ui", "hex", "ui.hex(hex_str) -> dict", "Parse `#RRGGBB` / `#RRGGBBAA` color"),
    fd("ui", "color", "ui.color(name_or_hex) -> dict", "Resolve named palette color or hex"),
    fd("ui", "theme", "ui.theme([overrides]) -> dict", "Build UI theme palette dict"),
    fd("ui", "column", "ui.column(children, [opts]) -> dict", "Vertical flex layout node"),
    fd("ui", "row", "ui.row(children, [opts]) -> dict", "Horizontal flex layout node"),
    fd("ui", "label", "ui.label(text, [opts]) -> dict", "Text label node"),
    fd("ui", "button", "ui.button(id, text, [opts]) -> dict", "Interactive clickable button widget"),
    fd("ui", "text_input", "ui.text_input(id, value, [opts]) -> dict", "Single-line editable text box widget"),
    fd("ui", "checkbox", "ui.checkbox(id, checked, [label]) -> dict", "Toggle checkbox widget"),
    fd("ui", "slider", "ui.slider(id, val, min, max, [opts]) -> dict", "Horizontal value slider widget"),
    fd("ui", "select", "ui.select(id, options, selected, [opts]) -> dict", "Dropdown selection widget"),
    fd("ui", "spacer", "ui.spacer([size]) -> dict", "Flexible or fixed layout spacer"),
    fd("ui", "rect", "ui.rect(w, h, color, [opts]) -> dict", "Solid styled box node"),
    fd("ui", "image", "ui.image(img, [w, h]) -> dict", "Embedded Image display node"),
    fd("ui", "nine_slice", "ui.nine_slice(img, l, r, t, b, w, h) -> dict", "Stretchable 9-slice UI frame"),
    fd("ui", "icon_button", "ui.icon_button(id, img, [label, opts]) -> dict", "Icon + label interactive button"),
    fd("ui", "window", "ui.window(title, w, h, [opts]) -> unit", "Open 2D UI window (Win32 / headless)"),
    fd("ui", "is_open", "ui.is_open() -> bool", "Poll UI window lifetime"),
    fd("ui", "close", "ui.close() -> unit", "Close active UI window"),
    fd("ui", "width", "ui.width() -> int", "Return UI viewport logical width"),
    fd("ui", "height", "ui.height() -> int", "Return UI viewport logical height"),
    fd("ui", "dpi_scale", "ui.dpi_scale() -> float", "Query active DPI scale factor"),
    fd("ui", "set_theme", "ui.set_theme(theme) -> unit", "Apply UI theme overrides"),
    fd("ui", "render", "ui.render(root_node) -> list", "Layout and draw widget tree returning frame events"),
    fd("ui", "events", "ui.events() -> list", "Drain pending UI widget interaction events"),
    fd("ui", "layout", "ui.layout(root_node, [w, h]) -> dict", "Compute widget layout rectangles cleanly without drawing"),
    fd("ui", "state", "ui.state(id, [default]) -> any", "Read persistent widget state by ID"),
    fd("ui", "set_state", "ui.set_state(id, val) -> unit", "Set persistent widget state"),
    fd("ui", "inject_click", "ui.inject_click(x, y) -> unit", "Inject synthetic pointer click for headless UI tests"),
    fd("ui", "inject_key", "ui.inject_key(key) -> unit", "Inject synthetic keystroke for headless UI tests"),
    fd("ui", "inject_resize", "ui.inject_resize(w, h) -> unit", "Inject viewport resize for headless UI tests"),
    fd("ui", "wait", "ui.wait([ms]) -> bool", "Present/wait frame cooperatively"),

    // url
    fd("url", "parse", "url.parse(url_str) -> dict", "Parse URL scheme, host, port, path, query & fragment"),
    fd("url", "encode", "url.encode(s) -> str", "Percent-encode URI component cleanly"),
    fd("url", "decode", "url.decode(s) -> str", "Decode percent-encoded UTF-8 URI component"),
    fd("url", "query_parse", "url.query_parse(qs) -> dict", "Parse `k=v&...` query string into dict"),
    fd("url", "query_stringify", "url.query_stringify(dict) -> str", "Serialize sorted dict keys into URL query string"),

    // uuid
    fd("uuid", "v4", "uuid.v4() -> str", "Generate RFC 4122 random UUID v4 string"),

    // ws
    fdc("ws", "listen", "ws.listen(host, port) -> WsListener", "Bind RFC 6455 WebSocket listener", "--allow-net"),
    fdc("ws", "accept", "ws.accept(listener, [timeout_ms]) -> WsConn | unit", "Accept and upgrade client WebSocket handshake", "--allow-net"),
    fdc("ws", "connect", "ws.connect(ws_url) -> WsConn", "Connect to `ws://` endpoint completing RFC 6455 handshake", "--allow-net"),
    fdc("ws", "send", "ws.send(conn, text) -> unit", "Send WebSocket UTF-8 text frame", "--allow-net"),
    fdc("ws", "send_bytes", "ws.send_bytes(conn, bytes) -> unit", "Send WebSocket binary frame", "--allow-net"),
    fdc("ws", "recv", "ws.recv(conn) -> dict | unit", "Receive next text/binary/close WebSocket frame", "--allow-net"),
    fd("ws", "local_addr", "ws.local_addr(listener_or_conn) -> str", "Return local bound address"),
    fd("ws", "close", "ws.close(conn) -> unit", "Send close frame cleanly and shut down socket"),

    // xml
    fd("xml", "parse", "xml.parse(xml_str) -> dict", "Parse bounded non-DTDD XML document into element dicts"),
    fd("xml", "select", "xml.select(node, tag) -> list", "Collect matching descendant elements by tag name"),
    fd("xml", "text", "xml.text(node) -> str", "Extract concatenated XML text content"),
    fd("xml", "escape", "xml.escape(text) -> str", "Escape `<`, `>`, `&`, quotes cleanly"),
    fd("xml", "unescape", "xml.unescape(text) -> str", "Decode XML built-in entities cleanly"),
];

pub fn lookup_module(name: &str) -> Option<&'static ModuleDoc> {
    REGISTRY.iter().find(|m| m.name == name)
}

pub fn module_info_tuple(name: &str) -> (&'static str, &'static str, &'static str) {
    if let Some(m) = lookup_module(name) {
        (m.category, m.platform, m.summary)
    } else {
        ("other", "all", "")
    }
}

pub fn lookup_fn_doc(module: &str, name: &str) -> Option<&'static FnDoc> {
    FN_DOCS.iter().find(|f| f.module == module && f.name == name)
}

pub fn lookup_global_doc(name: &str) -> Option<(&'static str, &'static str)> {
    GLOBAL_BUILTINS
        .iter()
        .find(|&&(n, _, _)| n == name)
        .map(|&(_, sig, doc)| (sig, doc))
}

pub fn lookup_keyword_doc(word: &str) -> Option<&'static KeywordDoc> {
    KEYWORDS.iter().find(|k| k.word == word)
}

pub fn module_exports_docs(module: &str) -> Vec<&'static FnDoc> {
    FN_DOCS.iter().filter(|f| f.module == module).collect()
}

pub fn check_registry_integrity() -> Result<(), String> {
    let reg_names: BTreeSet<&str> = REGISTRY.iter().map(|m| m.name).collect();
    if reg_names.len() != REGISTRY.len() {
        return Err("Duplicate module entry in registry::REGISTRY".to_string());
    }
    let builtin_names: BTreeSet<&str> = BUILTIN_MODULE_NAMES.iter().copied().collect();
    if reg_names != builtin_names {
        let missing: Vec<&str> = builtin_names.difference(&reg_names).copied().collect();
        let extra: Vec<&str> = reg_names.difference(&builtin_names).copied().collect();
        return Err(format!(
            "REGISTRY mismatch with BUILTIN_MODULE_NAMES: missing={:?}, extra={:?}",
            missing, extra
        ));
    }

    let mut documented_pairs: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for f in FN_DOCS {
        if !documented_pairs.entry(f.module).or_default().insert(f.name) {
            return Err(format!("Duplicate FN_DOCS entry {}.{}", f.module, f.name));
        }
        if f.signature.is_empty() || f.summary.is_empty() {
            return Err(format!("Empty doc fields on {}.{}", f.module, f.name));
        }
    }

    for &mod_name in BUILTIN_MODULE_NAMES {
        let val = get_builtin_module(mod_name)
            .ok_or_else(|| format!("get_builtin_module('{}') returned None", mod_name))?;
        let crate::value::Value::Module(m) = val else {
            return Err(format!("get_builtin_module('{}') is not a Module", mod_name));
        };
        let actual: BTreeSet<String> = m.exports.borrow().keys().cloned().collect();
        let doc_set = documented_pairs
            .remove(mod_name)
            .unwrap_or_default();
        let missing: Vec<String> = actual
            .iter()
            .filter(|k| !doc_set.contains(k.as_str()))
            .cloned()
            .collect();
        let extra: Vec<&str> = doc_set
            .iter()
            .copied()
            .filter(|k| !actual.contains(*k))
            .collect();
        if !missing.is_empty() || !extra.is_empty() {
            return Err(format!(
                "FN_DOCS drift on module '{}': missing={:?}, extra={:?}",
                mod_name, missing, extra
            ));
        }
    }

    if !documented_pairs.is_empty() {
        return Err(format!(
            "FN_DOCS contains unknown modules: {:?}",
            documented_pairs.keys().collect::<Vec<_>>()
        ));
    }
    Ok(())
}

pub fn render_modules_markdown() -> String {
    let mut out = String::new();
    out.push_str("# Askane Standard Library Modules\n\n");
    out.push_str(&format!(
        "Generated from single-source `registry.rs` ({} modules, {} exported symbols).\n\n",
        REGISTRY.len(),
        FN_DOCS.len()
    ));
    let mut by_cat: BTreeMap<&str, Vec<&ModuleDoc>> = BTreeMap::new();
    for m in REGISTRY {
        by_cat.entry(m.category).or_default().push(m);
    }
    for (cat, mut mods) in by_cat {
        mods.sort_by_key(|m| m.name);
        out.push_str(&format!("## {}\n\n", cat));
        for m in mods {
            let fns = module_exports_docs(m.name);
            let names: Vec<&str> = fns.iter().map(|f| f.name).collect();
            out.push_str(&format!(
                "- **`{}`** (`{}` / {}) — {}\n  - Exports ({}): `{}`\n",
                m.name,
                m.platform,
                m.stability,
                m.summary,
                names.len(),
                names.join("`, `")
            ));
        }
        out.push('\n');
    }
    out
}

pub fn render_readme_modules_section() -> String {
    let mut out = String::new();
    out.push_str("<details>\n");
    out.push_str(&format!(
        "<summary><strong>All {} standard library modules (<code>import &lt;name&gt;;</code> — auto-synced from <code>src/registry.rs</code>)</strong></summary>\n\n",
        REGISTRY.len()
    ));
    let mut sorted: Vec<&ModuleDoc> = REGISTRY.iter().collect();
    sorted.sort_by_key(|m| m.name);
    for m in sorted {
        let fns = module_exports_docs(m.name);
        let export_list: Vec<String> = fns.iter().map(|f| format!("`{}`", f.name)).collect();
        let src_file = if m.name == "channel" {
            "src/modules/task.rs".to_string()
        } else {
            format!("src/modules/{}.rs", m.name)
        };
        let plat_note = if m.platform == "all" {
            String::new()
        } else {
            format!(" ({})", m.platform)
        };
        out.push_str(&format!(
            "- [`{}`]({}){}: {} — {}\n",
            m.name,
            src_file,
            plat_note,
            m.summary,
            export_list.join(", ")
        ));
    }
    out.push_str("\n</details>");
    out
}

pub fn render_syntax_modules_block() -> String {
    let mut names: Vec<&str> = REGISTRY.iter().map(|m| m.name).collect();
    names.sort_unstable();
    let joined: Vec<String> = names.into_iter().map(|n| format!("`{}`", n)).collect();
    format!(
        "- Built-in modules ({}, via `import <name>;` or `import <name> as alias;`): {}.",
        REGISTRY.len(),
        joined.join(", ")
    )
}

pub fn render_syntax_keywords_block() -> String {
    let hard: Vec<String> = KEYWORDS
        .iter()
        .filter(|k| k.kind == "keyword")
        .map(|k| format!("`{}`", k.word))
        .collect();
    let ctx: Vec<String> = KEYWORDS
        .iter()
        .filter(|k| k.kind == "contextual")
        .map(|k| format!("`{}`", k.word))
        .collect();
    format!(
        "- Reserved keywords ({}): {}\n- Contextual keywords ({}): {}",
        hard.len(),
        hard.join(", "),
        ctx.len(),
        ctx.join(", ")
    )
}

pub fn render_diagnostics_markdown() -> String {
    let mut out = String::new();
    out.push_str("<!-- askane:diagnostics:begin -->\n");
    out.push_str("# Askane Diagnostic Code Reference\n\n");
    out.push_str(&format!(
        "Total documented codes: `{}` (auto-synced with `src/diag.rs`). Run `askane --explain <CODE>` for full details.\n\n",
        diag::ALL_CODES.len()
    ));
    out.push_str("| Code | Kind | Summary |\n| --- | --- | --- |\n");
    for &code in diag::ALL_CODES {
        let kind = if code.starts_with('W') {
            "warning"
        } else {
            "error"
        };
        let title = diag::explain_code(code)
            .map(|(t, _)| t)
            .unwrap_or("Diagnostic code");
        out.push_str(&format!("| `{}` | {} | {} |\n", code, kind, title));
    }
    out.push_str("\n<!-- askane:diagnostics:end -->\n");
    out
}

fn replace_marker_block(
    content: &str,
    marker_name: &str,
    replacement_inner: &str,
) -> Result<(String, bool), String> {
    let begin_tag = format!("<!-- askane:{}:begin -->", marker_name);
    let end_tag = format!("<!-- askane:{}:end -->", marker_name);
    let Some(b_pos) = content.find(&begin_tag) else {
        return Err(format!("Missing marker '{}' in document", begin_tag));
    };
    let after_begin = b_pos + begin_tag.len();
    let Some(e_rel) = content[after_begin..].find(&end_tag) else {
        return Err(format!("Missing closing marker '{}'", end_tag));
    };
    let e_pos = after_begin + e_rel;
    let new_block = format!("{}\n{}\n{}", begin_tag, replacement_inner.trim(), end_tag);
    let mut updated = String::with_capacity(content.len() + new_block.len());
    updated.push_str(&content[..b_pos]);
    updated.push_str(&new_block);
    updated.push_str(&content[e_pos + end_tag.len()..]);
    let changed = normalize_lf(&updated) != normalize_lf(content);
    Ok((updated, changed))
}

fn normalize_lf(s: &str) -> String {
    s.replace("\r\n", "\n")
}

pub fn sync_docs_at(repo_root: &Path, update: bool) -> Result<Vec<String>, String> {
    check_registry_integrity()?;
    let mut drifted = Vec::new();

    // 1. README.md
    let readme_path = repo_root.join("README.md");
    let readme_raw = fs::read_to_string(&readme_path)
        .map_err(|e| format!("Failed to read '{}': {}", readme_path.display(), e))?;
    let (readme_new, readme_changed) = replace_marker_block(
        &readme_raw,
        "modules",
        &render_readme_modules_section(),
    )?;
    if readme_changed {
        if update {
            fs::write(&readme_path, normalize_lf(&readme_new))
                .map_err(|e| format!("Failed to write '{}': {}", readme_path.display(), e))?;
        }
        drifted.push("README.md".to_string());
    }

    // 2. docs/SYNTAX.md
    let syntax_path = repo_root.join("docs").join("SYNTAX.md");
    let syntax_raw = fs::read_to_string(&syntax_path)
        .map_err(|e| format!("Failed to read '{}': {}", syntax_path.display(), e))?;
    let (syn1, c1) = replace_marker_block(
        &syntax_raw,
        "syntax-modules",
        &render_syntax_modules_block(),
    )?;
    let (syn2, c2) = replace_marker_block(
        &syn1,
        "syntax-keywords",
        &render_syntax_keywords_block(),
    )?;
    if c1 || c2 {
        if update {
            fs::write(&syntax_path, normalize_lf(&syn2))
                .map_err(|e| format!("Failed to write '{}': {}", syntax_path.display(), e))?;
        }
        drifted.push("docs/SYNTAX.md".to_string());
    }

    // 3. docs/diagnostics.md
    let diag_path = repo_root.join("docs").join("diagnostics.md");
    let diag_expected = render_diagnostics_markdown();
    let diag_existing = fs::read_to_string(&diag_path).unwrap_or_default();
    if normalize_lf(&diag_existing) != normalize_lf(&diag_expected) {
        if update {
            fs::write(&diag_path, normalize_lf(&diag_expected))
                .map_err(|e| format!("Failed to write '{}': {}", diag_path.display(), e))?;
        }
        drifted.push("docs/diagnostics.md".to_string());
    }

    if !update && !drifted.is_empty() {
        return Err(format!(
            "Documentation out of sync with registry/diag in: {} (re-run with ASKANE_UPDATE_DOCS=1)",
            drifted.join(", ")
        ));
    }
    Ok(drifted)
}

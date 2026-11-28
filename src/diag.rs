use std::collections::HashSet;

use crate::ast::{AssignTarget, Expr, MatchArm, Pattern, Program, Stmt};

/// Centralized list of all active diagnostic codes; codes 0500..056F are reserved for 0.20+ media, asset, map, and bundle diagnostics.
#[allow(dead_code)]
pub const ALL_CODES: &[&str] = &[
    "E0101", "E0201", "E0202", "E0301", "E0302", "E0401", "E0402", "E0403", "E0404", "E0405",
    "E0501", "E0502", "E0503", "E0504", "E0505", "E0506", "E0507", "E0508", "E0509", "E0510",
    "E0511", "E0512", "E0513", "E0514", "E0515", "E0520", "E0521", "E0530", "E0531", "E0532",
    "E0533", "E0540", "E0550", "E0551", "E0552", "E0560", "E0561", "E0562", "E0563", "E0564",
    "E0565", "W0101", "W0102", "W0501", "W0502",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub severity: &'static str,
    pub file: Option<String>,
    pub line: usize,
    pub col: usize,
    pub end_col: usize,
    pub message: String,
    pub hint: Option<String>,
    pub import_chain: Vec<String>,
    pub stack_trace: Vec<String>,
}

pub fn default_color_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none()
}

pub fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let m = a_chars.len();
    let n = b_chars.len();
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0usize; n + 1];
    for (i, &ac) in a_chars.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &bc) in b_chars.iter().enumerate() {
            let cost = if ac == bc { 0 } else { 1 };
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

pub fn suggest_similar<'a, I>(needle: &str, candidates: I) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    if needle.is_empty() || needle.len() > 64 {
        return None;
    }
    let needle_low = needle.to_ascii_lowercase();
    let max_dist = if needle.len() <= 3 { 1 } else { 2 };
    let mut best: Option<(usize, String)> = None;
    for cand in candidates {
        if cand.is_empty() || cand.starts_with("__") || cand == needle {
            continue;
        }
        if cand.len().abs_diff(needle.len()) > max_dist {
            continue;
        }
        let d = levenshtein(&needle_low, &cand.to_ascii_lowercase());
        if d <= max_dist {
            let replace = match &best {
                None => true,
                Some((bd, bs)) => d < *bd || (d == *bd && cand < bs.as_str()),
            };
            if replace {
                best = Some((d, cand.to_string()));
            }
        }
    }
    best.map(|(_, s)| s)
}

fn parse_location_tag(err: &str) -> (Option<usize>, Option<usize>, &str) {
    let trimmed = err.trim();
    if let Some(rest) = trimmed.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            let tag = &rest[..end];
            let msg = rest[end + 1..].trim_start();
            if let Some(ln_s) = tag.strip_prefix("line ") {
                if let Ok(l) = ln_s.trim().parse::<usize>() {
                    return (Some(l), None, msg);
                }
            } else if let Some((l_s, c_s)) = tag.split_once(':') {
                if let (Ok(l), Ok(c)) = (l_s.trim().parse::<usize>(), c_s.trim().parse::<usize>()) {
                    return (Some(l), Some(c), msg);
                }
            }
        }
    }
    (None, None, trimmed)
}

pub(crate) fn classify_code(msg: &str) -> &'static str {
    let low = msg.to_ascii_lowercase();
    if msg.contains("E_MEDIA_LIMIT") {
        "E0501"
    } else if msg.contains("E_MEDIA_UNSUPPORTED") {
        "E0502"
    } else if msg.contains("E_MEDIA_CORRUPT") {
        "E0503"
    } else if msg.contains("E_MEDIA_BAD_OPT") {
        "E0504"
    } else if msg.contains("E_MEDIA_NONDET") {
        "E0505"
    } else if msg.contains("E_AUDIO_BACKPRESSURE") {
        "E0506"
    } else if msg.contains("E_AUDIO_STREAM_UNSUPPORTED") {
        "E0507"
    } else if msg.contains("E_AUDIO_NONDET") {
        "E0508"
    } else if msg.contains("E_CAP_AUDIO") {
        "E0509"
    } else if msg.contains("E_ASSET_JSON") {
        "E0510"
    } else if msg.contains("E_ASSET_XML") {
        "E0511"
    } else if msg.contains("E_MESH_LIMIT") {
        "E0512"
    } else if msg.contains("E_MESH_CORRUPT") {
        "E0513"
    } else if msg.contains("E_MESH_CYCLE") {
        "E0514"
    } else if msg.contains("E_MESH_UNSUPPORTED") {
        "E0515"
    } else if msg.contains("E_TILED_UNSUPPORTED") {
        "E0520"
    } else if msg.contains("E_TILED_CORRUPT") {
        "E0521"
    } else if msg.contains("E_TERRAIN_LIMIT") {
        "E0530"
    } else if msg.contains("E_TERRAIN_CORRUPT") {
        "E0531"
    } else if msg.contains("E_TERRAIN_BAD_OPT") {
        "E0532"
    } else if msg.contains("E_SPLAT_INVALID") {
        "E0533"
    } else if msg.contains("E_CUBEMAP_INVALID") {
        "E0540"
    } else if msg.contains("E_LUT_INVALID") {
        "E0550"
    } else if msg.contains("E_NAV_INVALID") {
        "E0551"
    } else if msg.contains("E_SCENE_INVALID") {
        "E0552"
    } else if msg.contains("E_BACKEND_UNAVAILABLE") || msg.contains("backend unavailable") {
        "E0560"
    } else if msg.contains("E_BUNDLE_CORRUPT") {
        "E0561"
    } else if msg.contains("E_BUNDLE_READONLY") {
        "E0562"
    } else if msg.contains("E_BUNDLE_NOT_FOUND") {
        "E0563"
    } else if msg.contains("E_BUNDLE_UNSAFE_PATH") {
        "E0564"
    } else if msg.contains("E_ASSET_STATIC") {
        "E0565"
    } else if low.contains("unterminated ")
        || low.contains("unexpected character")
        || low.contains("invalid escape")
        || low.contains("unknown escape sequence")
        || low.contains("invalid hex escape")
        || low.contains("incomplete \\xhh escape")
        || low.contains("invalid number")
        || low.contains("invalid integer literal")
        || low.contains("invalid float literal")
        || low.contains("invalid duration literal")
        || low.contains("duration literal out of range")
        || low.contains("byte literal can only contain ascii")
        || low.contains("raw byte literal can only contain ascii")
        || low.contains("invalid hex byte")
    {
        "E0101"
    } else if low.contains("nesting depth")
        || low.contains("token/chain limit")
        || low.contains("argument limit")
        || low.contains("too many arguments")
    {
        "E0202"
    } else if low.starts_with("expected ")
        || low.starts_with("unexpected token")
        || low.contains("expected ';'")
        || low.contains("expected '}'")
        || low.contains("expected ')'")
        || low.contains("expected expression")
        || low.contains("cannot use placeholder")
    {
        "E0201"
    } else if low.contains("duplicate field")
        || low.contains("duplicate parameter")
        || low.contains("expects ")
        || low.contains("missing required field")
        || low.contains("unknown field")
        || low.contains("already supplied")
        || low.contains("after variadic parameter")
    {
        "E0302"
    } else if low.contains("cannot 'return'")
        || low.contains("used outside of a loop")
        || low.contains("too many local")
        || low.contains("too many constants")
        || low.contains("invalid bytecode")
        || low.contains("too many jump")
    {
        "E0301"
    } else if low.contains("undefined variable")
        || low.contains("has no export")
        || low.contains("has no static member")
        || low.contains("has no field")
        || low.contains("does not export")
    {
        "E0401"
    } else if low.contains("could not import")
        || low.contains("circular import")
        || low.contains("module path")
        || low.contains("import depth")
        || low.contains("needs:")
    {
        "E0402"
    } else if low.contains("sandbox ")
        || low.contains("execution timeout")
        || low.contains("requires --allow-")
        || low.contains("disabled in sandbox")
        || low.contains("blocked in sandbox")
        || low.contains("is outside the sandbox")
    {
        "E0405"
    } else if low.contains("cannot call value of type")
        || low.contains("cannot add ")
        || low.contains("cannot subtract ")
        || low.contains("cannot multiply ")
        || low.contains("cannot divide ")
        || low.contains("cannot compare ")
        || low.contains("cannot index ")
    {
        "E0403"
    } else {
        "E0404"
    }
}

pub fn classify_error(raw: &str, file: Option<&str>) -> Diagnostic {
    let mut working = raw.trim_end().to_string();
    let mut stack_trace = Vec::new();
    while let Some(nl_idx) = working.rfind('\n') {
        let tail = &working[nl_idx + 1..];
        if tail.starts_with("  at ") {
            stack_trace.push(tail.trim_start().to_string());
            working.truncate(nl_idx);
        } else {
            break;
        }
    }
    stack_trace.reverse();
    working = working.trim().to_string();

    let mut import_chain = Vec::new();
    let mut origin_file: Option<String> = None;
    while working.ends_with(')') {
        if let Some(idx) = working.rfind(" (imported from ") {
            if !working[idx..].contains(" (in ") {
                let parent = working[idx + " (imported from ".len()..working.len() - 1]
                    .trim_matches('\'')
                    .to_string();
                if !parent.is_empty() && import_chain.last() != Some(&parent) {
                    import_chain.push(parent);
                }
                working.truncate(idx);
                continue;
            }
        }
        if let Some(idx) = working.rfind(" (in ") {
            if !working[idx..].contains(" (imported from ") {
                let mod_file = working[idx + " (in ".len()..working.len() - 1]
                    .trim_matches('\'')
                    .to_string();
                if origin_file.is_none() && !mod_file.is_empty() {
                    origin_file = Some(mod_file.clone());
                }
                if !mod_file.is_empty() && import_chain.last() != Some(&mod_file) {
                    import_chain.push(mod_file);
                }
                working.truncate(idx);
                continue;
            }
        }
        break;
    }

    let mut hint = None;
    if let Some(hidx) = working.rfind(" (did you mean ") {
        if working.ends_with("?)") {
            let candidate = working[hidx + " (did you mean ".len()..working.len() - 2]
                .trim_matches('\'')
                .to_string();
            hint = Some(format!("did you mean '{}'?", candidate));
            working.truncate(hidx);
        }
    }

    // Peel repeated leading `[line X]` prefixes if nested calls wrapped the error.
    let mut outer_line = None;
    let mut outer_col = None;
    let mut msg_slice = working.as_str();
    while let (Some(l), c, rest) = parse_location_tag(msg_slice) {
        if outer_line.is_none() {
            outer_line = Some(l);
            outer_col = c;
        }
        if rest == msg_slice {
            break;
        }
        msg_slice = rest;
    }

    let message = msg_slice.to_string();
    let code = classify_code(&message);
    if hint.is_none() && code == "E0405" {
        hint = Some(
            "run with required --allow-* flag or inspect script with `askane permissions`"
                .to_string(),
        );
    }

    let col = outer_col.unwrap_or(1);
    let tok_len = extract_quoted_token_len(&message).unwrap_or(1);
    Diagnostic {
        code,
        severity: "error",
        file: origin_file.or_else(|| file.map(|s| s.to_string())),
        line: outer_line.unwrap_or(0),
        col,
        end_col: col.saturating_add(tok_len.saturating_sub(1)),
        message,
        hint,
        import_chain,
        stack_trace,
    }
}

fn extract_quoted_token_len(msg: &str) -> Option<usize> {
    let mut rest = msg;
    while let Some(start) = rest.find('\'') {
        let after = &rest[start + 1..];
        let end = after.find('\'')?;
        let inner = &after[..end];
        if !inner.is_empty() && inner.len() <= 64 && !inner.contains('/') && !inner.contains('\\') {
            return Some(inner.chars().count());
        }
        rest = &after[end + 1..];
    }
    None
}

pub fn format_diagnostic(d: &Diagnostic, source: Option<&str>, use_color: bool) -> String {
    let (sev_start, bold, dim, reset) = if use_color {
        let sev = if d.severity == "warning" {
            "\x1b[33;1m"
        } else {
            "\x1b[31;1m"
        };
        (sev, "\x1b[1m", "\x1b[36m", "\x1b[0m")
    } else {
        ("", "", "", "")
    };

    let mut out = format!(
        "{}{}[{}]{}: {}{}{}",
        sev_start, d.severity, d.code, reset, bold, d.message, reset
    );

    if d.line > 0 {
        let loc = match (&d.file, d.col) {
            (Some(f), c) if c > 0 => format!("{}:{}:{}", f, d.line, c),
            (Some(f), _) => format!("{}:{}", f, d.line),
            (None, c) if c > 0 => format!("{}:{}", d.line, c),
            (None, _) => format!("line {}", d.line),
        };
        out.push_str(&format!("\n  {}-->{} {}", dim, reset, loc));

        if let Some(src) = source {
            if let Some(src_line) = src.lines().nth(d.line.saturating_sub(1)) {
                let width = d.end_col.saturating_sub(d.col).saturating_add(1).max(1);
                let caret = "^".repeat(width.min(40));
                let pad = " ".repeat(d.col.saturating_sub(1));
                out.push_str(&format!(
                    "\n   {}|{}\n{:>4} | {}\n   {}|{} {}{}{}{}",
                    dim, reset, d.line, src_line, dim, reset, pad, sev_start, caret, reset
                ));
            }
        }
    }
    if let Some(ref h) = d.hint {
        out.push_str(&format!("\n   {}={}: hint: {}", dim, reset, h));
    }
    if !d.import_chain.is_empty() {
        out.push_str(&format!(
            "\n   {}={}: note: import chain: {}",
            dim,
            reset,
            d.import_chain.join(" -> ")
        ));
    }
    let show_trace = d.stack_trace.len() > 1
        || (d.stack_trace.len() == 1 && !d.stack_trace[0].starts_with("at <script> "))
        || (d.line == 0 && !d.stack_trace.is_empty());
    if show_trace {
        out.push_str(&format!("\n   {}={}: note: stack trace:", dim, reset));
        for frame in &d.stack_trace {
            out.push_str(&format!("\n       {}", frame));
        }
    }
    out
}

pub fn diagnostic_to_json(d: &Diagnostic, json_escape: fn(&str) -> String) -> String {
    let file_json = match &d.file {
        Some(f) => json_escape(f),
        None => "null".to_string(),
    };
    let hint_json = match &d.hint {
        Some(h) => json_escape(h),
        None => "null".to_string(),
    };
    let chain_json = d
        .import_chain
        .iter()
        .map(|s| json_escape(s))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"code\":{},\"severity\":{},\"file\":{},\"line\":{},\"col\":{},\"end_col\":{},\"message\":{},\"hint\":{},\"import_chain\":[{}]}}",
        json_escape(d.code),
        json_escape(d.severity),
        file_json,
        d.line,
        d.col,
        d.end_col,
        json_escape(&d.message),
        hint_json,
        chain_json
    )
}

pub fn explain_code(code: &str) -> Option<(&'static str, &'static str)> {
    let up = code.trim().to_ascii_uppercase();
    let entry = match up.as_str() {
        "E0101" => (
            "Lexer syntax error",
            "Occurs when the lexer encounters an invalid character, unterminated string/comment, invalid escape sequence, or malformed number.\n\nExample:\n  let s = \"unterminated;\nFix:\n  Close quotes or escape backslashes (`\\\\`).",
        ),
        "E0201" => (
            "Parser syntax error",
            "Occurs when tokens do not match Askane grammar (missing `;`, `}`, `)`, or `=>`).\n\nExample:\n  let x = 10\n  println(x);\nFix:\n  Terminate statements with `;`.",
        ),
        "E0202" => (
            "Parser depth or expression limit exceeded",
            "Askane bounds deeply nested expressions, chains, and expression token counts to avoid stack overflow.\nFix:\n  Split huge literal expressions or deep nesting into smaller helper functions.",
        ),
        "E0301" => (
            "Compiler semantic or bytecode limit error",
            "Occurs on invalid control flow (`return` at top level, `break`/`continue` outside loops) or exceeding per-function local/constant limits.\nFix:\n  Move `return` inside a `fn`, or place `break`/`continue` inside `while`/`for`/`loop`.",
        ),
        "E0302" => (
            "Parameter, field, or argument count mismatch",
            "Occurs when calling a function/struct with the wrong arity, repeating a parameter/field name, or omitting a required struct field.\nFix:\n  Check function signature or struct fields.",
        ),
        "E0401" => (
            "Undefined variable, field, or module export",
            "Referenced variable, struct field, or module export was not defined in the current scope or module.\nFix:\n  Check spelling (Askane suggests close matches via `did you mean`) and ensure exported symbols use `pub`.",
        ),
        "E0402" => (
            "Module import or capability declaration error",
            "Occurs when a module file is missing, escapes `sandbox_root`, creates a circular import, or violates `needs` declarations.\nFix:\n  Verify the relative `.ask` path, inspect the import chain, or run `askane permissions <file>`.",
        ),
        "E0403" => (
            "Type or operator mismatch at runtime",
            "Occurs when applying an operator or calling a value of an incompatible type (e.g. calling an `int` or adding `list + int`).\nFix:\n  Use `type_of`, `is_*`, `try_int`, or `try_float` before converting.",
        ),
        "E0404" => (
            "General runtime error",
            "Covers division by zero, out-of-bounds indexing, `checked_add` overflow, task deadlock, and unhandled `throw` errors.\nFix:\n  Handle recoverable errors with `try / catch` or `recover`.",
        ),
        "E0405" => (
            "Sandbox policy or resource budget violation",
            "Triggered when a script exceeds `--max-steps`, `--max-mem`, `--timeout`, or accesses a gated capability without `--allow-*`.\nFix:\n  Run `askane permissions <file>` to see required capabilities and pass explicit `--allow-*` flags.",
        ),
        "W0101" => (
            "Unused local variable",
            "A `let` binding inside a function or block is never read.\nFix:\n  Remove the unused variable or prefix its name with `_` (e.g. `let _unused = ...`).",
        ),
        "W0102" => (
            "Unreachable match arm",
            "A `match` arm appears after a catch-all `_` / variable arm without a guard, or duplicates an earlier literal pattern.\nFix:\n  Reorder specific patterns before wildcard/catch-all arms or remove duplicates.",
        ),
        "E0501" => ("Media allocation or dimension limit exceeded", "Asset dimensions, frames, or memory usage exceeded safe engine caps."),
        "E0502" => ("Unsupported media format", "Codec or subformat (such as progressive JPEG SOF2) is unsupported in this version."),
        "E0503" => ("Corrupt media payload", "Malformed container, CRC mismatch, or truncated image/audio stream."),
        "E0504" => ("Invalid media option or path", "Invalid decoder option or blocked asset reference path."),
        "E0505" => ("Non-deterministic operation in deterministic mode", "Requested timing/device operation violates --deterministic guarantees."),
        "E0506" => ("Audio command/event queue backpressure", "Audio mixer command or event ring buffer is full."),
        "E0507" => ("Unsupported audio stream format", "Audio format or channel layout is not supported by the PCM/ADPCM/companded WAV loader."),
        "E0508" => ("Real-time audio sink disallowed", "Deterministic playback mode cannot run real-time audio hardware sinks."),
        "E0509" => ("Audio device permission denied", "Audio device initialization requires --allow-audio or --unsafe."),
        "E0510" => ("Asset JSON limit or syntax error", "JSON asset payload failed strict depth, node count, or syntax validation."),
        "E0511" => ("Asset XML limit or syntax error", "XML asset payload failed strict depth, node count, or syntax validation."),
        "E0512" => ("Mesh geometry limit exceeded", "3D mesh exceeded vertex, triangle, node, or polygon limits."),
        "E0513" => ("Corrupt 3D model data", "OBJ/MTL/glTF/GLB buffer bounds, alignment, or attribute layout is invalid."),
        "E0514" => ("3D scene node cycle", "glTF node hierarchy contains a parent-child cycle."),
        "E0515" => ("Unsupported 3D feature", "Model uses unsupported required extension or sparse glTF accessor."),
        "E0520" => ("Unsupported Tiled map feature", "Tiled map uses unsupported compression (e.g. zstd) or non-orthogonal orientation."),
        "E0521" => ("Corrupt Tiled map payload", "Malformed .tmj/.tmx/.tsj/.tsx map dimensions, base64, or cell counts."),
        "E0530" => ("Terrain limit exceeded", "Heightmap dimensions or mesh density exceeded terrain caps."),
        "E0531" => ("Corrupt terrain payload", "Heightmap .r16/.png byte length or non-finite values are invalid."),
        "E0532" => ("Invalid terrain option", "Invalid terrain cell_size, height_scale, or extension."),
        "E0533" => ("Invalid splatmap", "Splatmap dimensions or pixel buffer length is invalid."),
        "E0540" => ("Invalid cubemap", "Cubemap faces must be 6 equal square power-of-two textures."),
        "E0550" => ("Invalid 3D LUT file", "3D LUT .cube/.lut must specify size 16 or 32 within 2 MiB and finite entries."),
        "E0551" => ("Invalid navigation grid or coordinates", "Navigation grid dimensions, cost, or coordinates are invalid."),
        "E0552" => ("Invalid scene assembly spec", "Scene assembly spec, transform, or placement parameter is invalid."),
        "E0560" => ("OS backend unavailable", "Native display, audio, clipboard, or TLS shared library backend is not available on this host."),
        "E0561" => ("Corrupt ASKANE-PAK bundle (unrecoverable)", "ASKANE-PAK header, TOC, payload SHA-256, or EOF trailer verification failed."),
        "E0562" => ("Bundle read-only violation", "Cannot write, overwrite, delete, or rename files in hermetic bundle mode or on bundle-owned keys."),
        "E0563" => ("Asset missing in hermetic bundle", "Requested asset path is not present in the mounted ASKANE-PAK bundle."),
        "E0564" => ("Unsafe ASKANE-PAK entry path", "Bundle entry path contains traversal, reserved Windows names, control chars, or case collision."),
        "E0565" => ("Static asset path escapes sandbox", "Compile-time literal asset path contains traversal, device name, NUL byte, or invalid prefix."),
        "W0501" => ("Dynamic asset path cannot be verified statically", "Asset loader called with a non-literal path; run with explicit capabilities or inspect paths manually."),
        "W0502" => ("Literal asset path missing on disk", "Static asset path did not exist on disk at check time (ignore if supplied by a mounted bundle at runtime)."),
        _ => return None,
    };
    Some(entry)
}

#[derive(Clone)]
struct LocalBind {
    name: String,
    line: usize,
    col: usize,
    used: bool,
    suppress_unused: bool,
}

struct LintWalker<'a> {
    file: Option<&'a str>,
    scopes: Vec<Vec<LocalBind>>,
    warnings: Vec<Diagnostic>,
}

impl<'a> LintWalker<'a> {
    fn new(file: Option<&'a str>) -> Self {
        Self {
            file,
            scopes: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        if let Some(frame) = self.scopes.pop() {
            for b in frame {
                if !b.used
                    && !b.suppress_unused
                    && !b.name.starts_with('_')
                    && !b.name.starts_with('$')
                {
                    self.warnings.push(Diagnostic {
                        code: "W0101",
                        severity: "warning",
                        file: self.file.map(|s| s.to_string()),
                        line: b.line,
                        col: b.col,
                        end_col: b
                            .col
                            .saturating_add(b.name.chars().count().saturating_sub(1)),
                        message: format!("Unused local variable '{}'", b.name),
                        hint: Some(format!("prefix with '_{}' to suppress", b.name)),
                        import_chain: Vec::new(),
                        stack_trace: Vec::new(),
                    });
                }
            }
        }
    }

    fn declare(&mut self, name: &str, line: usize, col: usize) {
        if let Some(top) = self.scopes.last_mut() {
            top.push(LocalBind {
                name: name.to_string(),
                line,
                col,
                used: false,
                suppress_unused: false,
            });
        }
    }

    fn declare_param(&mut self, name: &str) {
        if let Some(top) = self.scopes.last_mut() {
            top.push(LocalBind {
                name: name.to_string(),
                line: 0,
                col: 0,
                used: false,
                suppress_unused: true,
            });
        }
    }

    fn mark_used(&mut self, name: &str) {
        for scope in self.scopes.iter_mut().rev() {
            for bind in scope.iter_mut().rev() {
                if bind.name == name {
                    bind.used = true;
                    return;
                }
            }
        }
    }

    fn visit_stmt(&mut self, stmt: &Stmt, track_locals: bool) {
        match stmt {
            Stmt::Let {
                name,
                name_span,
                value,
                ..
            } => {
                self.visit_expr(value);
                if track_locals {
                    self.declare(name, name_span.line, name_span.col);
                }
            }
            Stmt::LetList {
                names,
                name_spans,
                value,
                span,
                ..
            } => {
                self.visit_expr(value);
                if track_locals {
                    for (i, n) in names.iter().enumerate() {
                        let sp = name_spans.get(i).copied().unwrap_or(*span);
                        self.declare(n, sp.line, sp.col);
                    }
                }
            }
            Stmt::LetDict {
                fields,
                field_spans,
                value,
                span,
                ..
            } => {
                self.visit_expr(value);
                if track_locals {
                    for (i, f) in fields.iter().enumerate() {
                        let sp = field_spans.get(i).copied().unwrap_or(*span);
                        self.declare(f, sp.line, sp.col);
                    }
                }
            }
            Stmt::FnDecl { params, body, .. } => {
                self.push_scope();
                for p in params {
                    if let Some(d) = &p.default {
                        self.visit_expr(d);
                    }
                }
                for p in params {
                    self.declare_param(&p.name);
                }
                self.visit_expr(body);
                self.pop_scope();
            }
            Stmt::StructDecl {
                fields, methods, ..
            } => {
                for f in fields {
                    if let Some(d) = &f.default {
                        self.visit_expr(d);
                    }
                }
                for m in methods {
                    self.push_scope();
                    for p in &m.params {
                        if let Some(d) = &p.default {
                            self.visit_expr(d);
                        }
                    }
                    for p in &m.params {
                        self.declare_param(&p.name);
                    }
                    self.visit_expr(&m.body);
                    self.pop_scope();
                }
            }
            Stmt::While { cond, body, .. } => {
                self.visit_expr(cond);
                self.visit_expr(body);
            }
            Stmt::For {
                var, iter, body, ..
            } => {
                self.visit_expr(iter);
                self.push_scope();
                self.declare_param(var);
                self.visit_expr(body);
                self.pop_scope();
            }
            Stmt::ForPair {
                var1,
                var2,
                iter,
                body,
                ..
            } => {
                self.visit_expr(iter);
                self.push_scope();
                self.declare_param(var1);
                self.declare_param(var2);
                self.visit_expr(body);
                self.pop_scope();
            }
            Stmt::Return { value: Some(v), .. } => self.visit_expr(v),
            Stmt::Defer { body, .. } | Stmt::Spec { body, .. } => self.visit_expr(body),
            Stmt::Every { interval, body, .. } => {
                self.visit_expr(interval);
                self.visit_expr(body);
            }
            Stmt::Expr(e) => self.visit_expr(e),
            Stmt::Import { .. }
            | Stmt::ImportFrom { .. }
            | Stmt::Return { value: None, .. }
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::Needs { .. } => {}
        }
    }

    fn push_w0102(&mut self, span: crate::lexer::Span, msg: String, hint: Option<String>) {
        self.warnings.push(Diagnostic {
            code: "W0102",
            severity: "warning",
            file: self.file.map(|s| s.to_string()),
            line: span.line,
            col: span.col,
            end_col: span.col,
            message: msg,
            hint,
            import_chain: Vec::new(),
            stack_trace: Vec::new(),
        });
    }

    fn range_bounds(start: i64, end: i64, inclusive: bool) -> Option<(i64, i64)> {
        let eff_end = if inclusive {
            end
        } else if end <= start {
            return None;
        } else {
            end.saturating_sub(1)
        };
        if start <= eff_end {
            Some((start, eff_end))
        } else {
            None
        }
    }

    fn check_match_arms(&mut self, arms: &[MatchArm]) {
        let mut catch_all_seen = false;
        let mut seen_literals: HashSet<String> = HashSet::new();
        let mut seen_ranges: Vec<(i64, i64)> = Vec::new();
        for arm in arms {
            if catch_all_seen {
                self.push_w0102(
                    arm.span,
                    "Unreachable match arm after catch-all pattern".to_string(),
                    Some("place wildcard '_' or variable patterns at the end".to_string()),
                );
                continue;
            }
            if arm.guard.is_none() {
                match &arm.pattern {
                    Pattern::Wildcard | Pattern::Ident(_) => {
                        catch_all_seen = true;
                    }
                    Pattern::Int(n) => {
                        let key = format!("int:{}", n);
                        let covered_by_range = seen_ranges.iter().any(|&(s, e)| *n >= s && *n <= e);
                        if !seen_literals.insert(key) || covered_by_range {
                            self.push_w0102(
                                arm.span,
                                format!("Duplicate unreachable match pattern '{}'", n),
                                None,
                            );
                        }
                    }
                    Pattern::Float(f) => {
                        let norm = if *f == 0.0 { 0.0 } else { *f };
                        let key = format!("float:{:#x}", norm.to_bits());
                        if !seen_literals.insert(key) {
                            self.push_w0102(
                                arm.span,
                                format!("Duplicate unreachable match pattern '{}'", f),
                                None,
                            );
                        }
                    }
                    Pattern::Bool(b) => {
                        let key = format!("bool:{}", b);
                        if !seen_literals.insert(key) {
                            self.push_w0102(
                                arm.span,
                                format!("Duplicate unreachable match pattern '{}'", b),
                                None,
                            );
                        }
                    }
                    Pattern::Str(s) => {
                        let key = format!("str:{}", s);
                        if !seen_literals.insert(key) {
                            self.push_w0102(
                                arm.span,
                                format!("Duplicate unreachable match pattern {:?}", s),
                                None,
                            );
                        }
                    }
                    Pattern::Unit => {
                        if !seen_literals.insert("unit".to_string()) {
                            self.push_w0102(
                                arm.span,
                                "Duplicate unreachable match pattern 'unit'".to_string(),
                                None,
                            );
                        }
                    }
                    Pattern::Range {
                        start,
                        end,
                        inclusive,
                    } => {
                        let key = format!("range:{}:{}:{}", start, end, inclusive);
                        let dup_exact = !seen_literals.insert(key);
                        let covered =
                            Self::range_bounds(*start, *end, *inclusive).is_some_and(|(s, e)| {
                                seen_ranges.iter().any(|&(os, oe)| os <= s && e <= oe)
                            });
                        if dup_exact || covered {
                            let op = if *inclusive { "..=" } else { ".." };
                            self.push_w0102(
                                arm.span,
                                format!(
                                    "Duplicate unreachable match range '{}{}{}'",
                                    start, op, end
                                ),
                                None,
                            );
                        } else if let Some(bounds) = Self::range_bounds(*start, *end, *inclusive) {
                            seen_ranges.push(bounds);
                        }
                    }
                }
            }
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Ident(name, _) => self.mark_used(name),
            Expr::List(items, _) => {
                for it in items {
                    self.visit_expr(it);
                }
            }
            Expr::Dict(entries, _) => {
                for (k, v) in entries {
                    self.visit_expr(k);
                    self.visit_expr(v);
                }
            }
            Expr::Unary { expr: e, .. } | Expr::Throw { value: e, .. } => self.visit_expr(e),
            Expr::Binary { left, right, .. } | Expr::NullCoalesce { left, right, .. } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            Expr::Assign {
                target, op, value, ..
            } => {
                match target.as_ref() {
                    AssignTarget::Var(name) => {
                        if op.is_some() {
                            self.mark_used(name);
                        }
                    }
                    AssignTarget::Index { object, index } => {
                        self.visit_expr(object);
                        self.visit_expr(index);
                    }
                    AssignTarget::Attr { object, .. } => self.visit_expr(object),
                }
                self.visit_expr(value);
            }
            Expr::Block { stmts, tail, .. } => {
                self.push_scope();
                for s in stmts {
                    self.visit_stmt(s, true);
                }
                if let Some(t) = tail {
                    self.visit_expr(t);
                }
                self.pop_scope();
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                self.visit_expr(cond);
                self.visit_expr(then_branch);
                if let Some(e) = else_branch {
                    self.visit_expr(e);
                }
            }
            Expr::Match { subject, arms, .. } => {
                self.visit_expr(subject);
                self.check_match_arms(arms);
                for arm in arms {
                    self.push_scope();
                    if let Pattern::Ident(name) = &arm.pattern {
                        self.declare_param(name);
                    }
                    if let Some(g) = &arm.guard {
                        self.visit_expr(g);
                    }
                    self.visit_expr(&arm.body);
                    self.pop_scope();
                }
            }
            Expr::Fn { params, body, .. } => {
                self.push_scope();
                for p in params {
                    if let Some(d) = &p.default {
                        self.visit_expr(d);
                    }
                }
                for p in params {
                    self.declare_param(&p.name);
                }
                self.visit_expr(body);
                self.pop_scope();
            }
            Expr::Call { callee, args, .. } => {
                self.visit_expr(callee);
                for a in args {
                    self.visit_expr(a);
                }
            }
            Expr::CallNamed(c) => {
                self.visit_expr(&c.callee);
                for a in &c.pos_args {
                    self.visit_expr(a);
                }
                for (_, a) in &c.named_args {
                    self.visit_expr(a);
                }
            }
            Expr::StructInit(s) => {
                self.visit_expr(&s.target);
                if let Some(sp) = &s.spread {
                    self.visit_expr(sp);
                }
                for (_, v) in &s.fields {
                    self.visit_expr(v);
                }
            }
            Expr::ChainedCmp(c) => {
                self.visit_expr(&c.first);
                for (_, rhs) in &c.comparisons {
                    self.visit_expr(rhs);
                }
            }
            Expr::Index { object, index, .. } => {
                self.visit_expr(object);
                self.visit_expr(index);
            }
            Expr::Slice {
                object, start, end, ..
            } => {
                self.visit_expr(object);
                if let Some(s) = start {
                    self.visit_expr(s);
                }
                if let Some(e) = end {
                    self.visit_expr(e);
                }
            }
            Expr::ListComp {
                expr: item,
                var,
                iter,
                cond,
                ..
            } => {
                self.visit_expr(iter);
                self.push_scope();
                self.declare_param(var);
                if let Some(c) = cond {
                    self.visit_expr(c);
                }
                self.visit_expr(item);
                self.pop_scope();
            }
            Expr::GetAttr { object, .. } | Expr::OptGetAttr { object, .. } => {
                self.visit_expr(object);
            }
            Expr::TryCatch {
                try_body,
                catch_var,
                catch_body,
                ..
            } => {
                self.visit_expr(try_body);
                self.push_scope();
                if let Some(cv) = catch_var {
                    self.declare_param(cv);
                }
                self.visit_expr(catch_body);
                self.pop_scope();
            }
            Expr::SafePipe(p) => {
                self.visit_expr(&p.left);
                self.push_scope();
                self.declare_param(&p.param);
                self.visit_expr(&p.apply);
                self.pop_scope();
            }
            Expr::MapPipe { left, mapper, .. } => {
                self.visit_expr(left);
                self.visit_expr(mapper);
            }
            Expr::PipeAssign(p) => {
                match p.target.as_ref() {
                    AssignTarget::Var(name) => self.mark_used(name),
                    AssignTarget::Index { object, index } => {
                        self.visit_expr(object);
                        self.visit_expr(index);
                    }
                    AssignTarget::Attr { object, .. } => self.visit_expr(object),
                }
                self.push_scope();
                self.declare_param(&p.param);
                for st in &p.stages {
                    self.visit_expr(st);
                }
                self.pop_scope();
            }
            Expr::TaggedTemplate(t) => {
                self.mark_used(&t.tag);
                for e in &t.exprs {
                    self.visit_expr(e);
                }
            }
            Expr::With(w) => {
                self.visit_expr(&w.resource);
                self.push_scope();
                if let Some(alias) = &w.alias {
                    self.declare_param(alias);
                }
                self.visit_expr(&w.body);
                self.pop_scope();
            }
            Expr::Sandbox(s) => {
                for (_, v) in &s.policy {
                    self.visit_expr(v);
                }
                self.visit_expr(&s.body);
            }
            Expr::After { delay, body, .. } => {
                self.visit_expr(delay);
                self.visit_expr(body);
            }
            Expr::Int(_, _)
            | Expr::Float(_, _)
            | Expr::Str(_, _)
            | Expr::Bytes(_, _)
            | Expr::Bool(_, _)
            | Expr::Unit(_) => {}
        }
    }
}

pub fn analyze_warnings(program: &Program, file: Option<&str>) -> Vec<Diagnostic> {
    let mut walker = LintWalker::new(file);
    for stmt in &program.stmts {
        walker.visit_stmt(stmt, false);
    }
    if let Some(tail) = &program.tail {
        walker.visit_expr(tail);
    }
    walker.warnings
}

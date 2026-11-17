use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use crate::bytecode::{Constant, FunctionProto, Op, verify_proto};
use crate::compiler::Compiler;
use crate::lexer::Lexer;
use crate::modules;
use crate::parser::Parser;
use crate::value::{Closure, NativeFunction, UpvalueState, Value};

struct CallFrame {
    closure: Rc<Closure>,
    ip: usize,
    base: usize,
}

#[derive(Clone)]
struct TryHandler {
    frame_depth: usize,
    stack_len: usize,
    catch_ip: usize,
}

const MAX_CALL_DEPTH: usize = 1024;

#[derive(Clone, Debug)]
pub struct Permissions {
    pub allow_all: bool,
    pub allow_net: Option<Vec<String>>,
    pub allow_read: Option<Vec<PathBuf>>,
    pub allow_write: Option<Vec<PathBuf>>,
    pub allow_env: Option<Vec<String>>,
    pub allow_sys_args: bool,
    pub allow_clipboard: bool,
    pub allow_exec: Option<Vec<String>>,
    pub max_instructions: Option<u64>,
    pub max_duration_ms: Option<u64>,
}

impl Permissions {
    pub fn sandboxed() -> Self {
        Self {
            allow_all: false,
            allow_net: None,
            allow_read: None,
            allow_write: None,
            allow_env: None,
            allow_sys_args: false,
            allow_clipboard: false,
            allow_exec: None,
            max_instructions: Some(10_000_000),
            max_duration_ms: Some(30_000),
        }
    }

    pub fn unrestricted() -> Self {
        Self {
            allow_all: true,
            allow_net: Some(Vec::new()),
            allow_read: Some(Vec::new()),
            allow_write: Some(Vec::new()),
            allow_env: Some(Vec::new()),
            allow_sys_args: true,
            allow_clipboard: true,
            allow_exec: Some(Vec::new()),
            max_instructions: None,
            max_duration_ms: None,
        }
    }
}

pub struct VM {
    stack: Vec<Value>,
    frames: Vec<CallFrame>,
    globals: HashMap<String, Value>,
    open_upvalues: Vec<Rc<RefCell<UpvalueState>>>,
    start_time: Instant,
    current_dir: PathBuf,
    sandbox_root: PathBuf,
    sandboxed: bool,
    permissions: Permissions,
    module_cache: HashMap<String, Value>,
    loading_modules: HashSet<String>,
    instructions_left: u64,
    run_start: Instant,
    try_handlers: Vec<TryHandler>,
}

impl VM {
    pub fn new() -> Self {
        Self::new_with_sandbox(true)
    }

    pub fn new_with_sandbox(sandboxed: bool) -> Self {
        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let permissions = if sandboxed {
            Permissions::sandboxed()
        } else {
            Permissions::unrestricted()
        };
        let instructions_left = permissions.max_instructions.unwrap_or(u64::MAX);
        let mut vm = Self {
            stack: Vec::with_capacity(256),
            frames: Vec::with_capacity(64),
            globals: HashMap::new(),
            open_upvalues: Vec::new(),
            start_time: Instant::now(),
            current_dir,
            sandbox_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            sandboxed,
            permissions,
            module_cache: HashMap::new(),
            loading_modules: HashSet::new(),
            instructions_left,
            run_start: Instant::now(),
            try_handlers: Vec::new(),
        };
        vm.register_natives();
        vm
    }

    pub fn set_permissions(&mut self, permissions: Permissions) {
        self.sandboxed = !permissions.allow_all;
        self.instructions_left = permissions.max_instructions.unwrap_or(u64::MAX);
        self.permissions = permissions;
    }

    pub fn set_current_dir(&mut self, dir: PathBuf) {
        self.current_dir = dir;
    }

    pub fn set_sandbox_root(&mut self, dir: PathBuf) {
        self.sandbox_root = dir;
    }

    pub fn is_sandboxed(&self) -> bool {
        self.sandboxed
    }

    pub fn elapsed_ms(&self) -> i64 {
        self.start_time.elapsed().as_millis() as i64
    }

    pub fn check_timeout(&self) -> Result<(), String> {
        if let Some(limit_ms) = self.permissions.max_duration_ms {
            if self.run_start.elapsed().as_millis() as u64 > limit_ms {
                return Err(format!("Execution timeout ({} ms exceeded)", limit_ms));
            }
        }
        Ok(())
    }

    pub fn is_unrecoverable_error(err: &str) -> bool {
        err.contains("Sandbox instruction limit exceeded") || err.contains("Execution timeout (")
    }

    pub fn sleep_bounded(&self, ms: u64) -> Result<(), String> {
        self.check_timeout()?;
        if self.is_sandboxed() && ms > 1000 {
            return Err("Sandbox sleep limit is 1000 ms per call".to_string());
        }
        if let Some(limit_ms) = self.permissions.max_duration_ms {
            let elapsed = self.run_start.elapsed().as_millis() as u64;
            if elapsed.saturating_add(ms) > limit_ms {
                let rem = limit_ms.saturating_sub(elapsed);
                if rem > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(rem));
                }
                return Err(format!("Execution timeout ({} ms exceeded)", limit_ms));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(ms));
        Ok(())
    }

    pub fn check_net(&self, url: &str) -> Result<(), String> {
        if url.is_empty()
            || url.len() > 8192
            || url
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
        {
            return Err("Invalid URL characters or URL size".to_string());
        }
        let rest = url
            .strip_prefix("https://")
            .ok_or_else(|| "Only https:// URLs are allowed".to_string())?;
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if authority.is_empty() || authority.contains('@') {
            return Err("Invalid or disallowed URL authority".to_string());
        }
        if self.permissions.allow_all {
            return Ok(());
        }
        let Some(allowed) = &self.permissions.allow_net else {
            return Err(
                "Network is disabled in sandbox mode (pass --allow-net or --unsafe)".to_string(),
            );
        };
        if allowed.is_empty() {
            return Ok(());
        }
        let host_port = authority;
        let host_only = if host_port.starts_with('[') {
            host_port
                .find(']')
                .map(|i| &host_port[..=i])
                .unwrap_or(host_port)
        } else {
            host_port.split(':').next().unwrap_or(host_port)
        };
        if allowed
            .iter()
            .any(|p| p.eq_ignore_ascii_case(host_port) || p.eq_ignore_ascii_case(host_only))
        {
            Ok(())
        } else {
            Err(format!(
                "Network access to '{}' is not allowed (pass --allow-net={})",
                host_port, host_port
            ))
        }
    }

    pub fn check_env(&self, name: &str) -> Result<(), String> {
        if self.permissions.allow_all {
            return Ok(());
        }
        let Some(allowed) = &self.permissions.allow_env else {
            return Err(
                "sys.env() is disabled in sandbox mode (run with --allow-env or --unsafe to allow)"
                    .to_string(),
            );
        };
        if allowed.is_empty() || allowed.iter().any(|k| k == name) {
            Ok(())
        } else {
            Err(format!(
                "sys.env('{}') is disabled in sandbox mode (pass --allow-env={})",
                name, name
            ))
        }
    }

    pub fn check_sys_args(&self) -> Result<(), String> {
        if self.permissions.allow_all || self.permissions.allow_sys_args {
            Ok(())
        } else {
            Err(
                "sys.args() is disabled in sandbox mode (pass --allow-args or --unsafe)"
                    .to_string(),
            )
        }
    }

    pub fn check_clipboard(&self) -> Result<(), String> {
        if self.permissions.allow_all || self.permissions.allow_clipboard {
            Ok(())
        } else {
            Err(
                "clipboard is disabled in sandbox mode (pass --allow-clipboard or --unsafe)"
                    .to_string(),
            )
        }
    }

    pub fn check_exec(&self, cmd: &str) -> Result<(), String> {
        if cmd.chars().any(char::is_control) {
            return Err("process.run() command cannot contain control characters".to_string());
        }
        if self.permissions.allow_all {
            return Ok(());
        }
        let Some(allowed) = &self.permissions.allow_exec else {
            return Err(
                "process execution is disabled in sandbox mode (pass --allow-exec or --unsafe)"
                    .to_string(),
            );
        };
        if allowed.is_empty() {
            return Ok(());
        }
        if allowed.iter().any(|c| {
            let has_sep = c.contains('/') || c.contains('\\');
            if !has_sep && (cmd.contains('/') || cmd.contains('\\') || cmd.contains(':')) {
                return false;
            }
            c.eq_ignore_ascii_case(cmd)
        }) {
            Ok(())
        } else {
            Err(format!(
                "process.run('{}') is not allowed (pass --allow-exec={})",
                cmd, cmd
            ))
        }
    }

    pub fn sandbox_root(&self) -> &PathBuf {
        &self.sandbox_root
    }

    pub fn resolve_read_path(&self, input: &str) -> Result<PathBuf, String> {
        self.resolve_path_with_roots(input, &self.permissions.allow_read)
    }

    pub fn resolve_write_path(&self, input: &str) -> Result<PathBuf, String> {
        self.resolve_path_with_roots(input, &self.permissions.allow_write)
    }

    fn resolve_path_with_roots(
        &self,
        input: &str,
        extra_roots: &Option<Vec<PathBuf>>,
    ) -> Result<PathBuf, String> {
        if input.is_empty() || input.contains('\0') {
            return Err("invalid path".to_string());
        }
        let raw = Path::new(input);
        let candidate = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.current_dir.join(raw)
        };
        if self.permissions.allow_all {
            return Ok(candidate);
        }
        if input.starts_with("\\\\") || input.starts_with("//") {
            return Err("path is outside the sandbox policy (UNC or device namespace)".to_string());
        }
        // Reject Windows device names, alternate data streams, drive-relative paths,
        // and ambiguous names before Win32 can reinterpret them.
        for component in raw.components() {
            match component {
                std::path::Component::Prefix(prefix) => {
                    let s = prefix.as_os_str().to_string_lossy();
                    if !raw.is_absolute() || s.starts_with("\\\\") || s.starts_with("//") {
                        return Err(
                            "path is outside the sandbox policy (device or UNC prefix)".to_string()
                        );
                    }
                }
                std::path::Component::Normal(name) => {
                    let name = name.to_string_lossy();
                    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
                    let is_com_lpt = (stem.starts_with("COM") || stem.starts_with("LPT"))
                        && stem.len() >= 4
                        && stem[3..].chars().all(|c| {
                            c.is_ascii_digit() || matches!(c, '\u{00B9}' | '\u{00B2}' | '\u{00B3}')
                        });
                    if name.contains(':')
                        || name.contains('\0')
                        || name.ends_with(['.', ' '])
                        || matches!(
                            stem.as_str(),
                            "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
                        )
                        || is_com_lpt
                    {
                        return Err(
                            "path is outside the sandbox policy (device or ambiguous name)"
                                .to_string(),
                        );
                    }
                }
                _ => {}
            }
        }
        if let Some(dirs) = extra_roots {
            if dirs.is_empty() {
                return Ok(candidate);
            }
        }
        let normalize_lexical = |p: &Path, base: &Path| -> Option<PathBuf> {
            let full = if p.is_absolute() {
                p.to_path_buf()
            } else {
                base.join(p)
            };
            let mut out = PathBuf::new();
            for comp in full.components() {
                match comp {
                    std::path::Component::CurDir => {}
                    std::path::Component::ParentDir => {
                        if !out.pop() {
                            return None;
                        }
                    }
                    other => out.push(other.as_os_str()),
                }
            }
            Some(out)
        };

        let Some(lexical) = normalize_lexical(raw, &self.current_dir) else {
            return Err(format!("path '{}' is outside the sandbox", input));
        };
        let mut lexical_roots = Vec::new();
        if let Some(r) = normalize_lexical(&self.sandbox_root, &self.current_dir) {
            lexical_roots.push(r);
        }
        if let Some(dirs) = extra_roots {
            for dir in dirs {
                if let Some(r) = normalize_lexical(dir, &self.current_dir) {
                    lexical_roots.push(r);
                }
            }
        }
        if !lexical_roots
            .iter()
            .any(|r| lexical == *r || lexical.starts_with(r))
        {
            return Err(format!("path '{}' is outside the sandbox", input));
        }

        let root =
            fs::canonicalize(&self.sandbox_root).map_err(|e| format!("sandbox root: {}", e))?;
        let mut allowed_roots = vec![root];
        if let Some(dirs) = extra_roots {
            for dir in dirs {
                if let Ok(allowed_root) = fs::canonicalize(dir) {
                    allowed_roots.push(allowed_root);
                }
            }
        }

        // Canonicalize the deepest existing ancestor to catch symlinks/junctions inside the root.
        let mut cur = candidate.as_path();
        let mut tail_parts = Vec::new();
        let canonical_base = loop {
            if cur.symlink_metadata().is_ok() {
                break fs::canonicalize(cur).map_err(|e| format!("path '{}': {}", input, e))?;
            }
            let Some(name) = cur.file_name() else {
                return Err(format!("path '{}' is outside the sandbox", input));
            };
            tail_parts.push(name.to_os_string());
            let Some(parent) = cur.parent() else {
                return Err(format!("path '{}' is outside the sandbox", input));
            };
            cur = parent;
        };
        let mut existing = canonical_base;
        for part in tail_parts.into_iter().rev() {
            existing.push(part);
        }
        if allowed_roots
            .iter()
            .any(|r| existing == *r || existing.starts_with(r))
        {
            Ok(existing)
        } else {
            Err(format!("path '{}' is outside the sandbox", input))
        }
    }

    fn register_natives(&mut self) {
        let natives: &[(&str, fn(&mut VM, &[Value]) -> Result<Value, String>)] = &[
            ("print", native_print),
            ("println", native_println),
            ("len", native_len),
            ("push", native_push),
            ("pop", native_pop),
            ("range", native_range),
            ("map", native_map),
            ("filter", native_filter),
            ("reduce", native_reduce),
            ("str", native_str),
            ("int", native_int),
            ("float", native_float),
            ("type_of", native_type_of),
            ("keys", native_keys),
            ("values", native_values),
            ("entries", native_entries),
            ("get", native_get),
            ("remove", native_remove),
            ("sort", native_sort),
            ("reverse", native_reverse),
            ("enumerate", native_enumerate),
            ("zip", native_zip),
            ("take", native_take),
            ("skip", native_skip),
            ("any", native_any),
            ("all", native_all),
            ("find", native_find),
            ("__pairs", native_pairs),
            ("unique", native_unique),
            ("flatten", native_flatten),
            ("chunk", native_chunk),
            ("sum", native_sum),
            ("has", native_has),
            ("read_line", native_read_line),
            ("clock", native_clock),
        ];

        for &(name, func) in natives {
            self.globals.insert(
                name.to_string(),
                Value::NativeFn(NativeFunction { name, func }),
            );
        }
    }

    pub fn run(&mut self, proto: Rc<FunctionProto>) -> Result<Value, String> {
        verify_proto(&proto)?;
        if self.frames.len() >= MAX_CALL_DEPTH || self.loading_modules.len() > 32 {
            return Err("Maximum call/import depth exceeded".to_string());
        }
        if self.frames.is_empty() {
            self.instructions_left = self.permissions.max_instructions.unwrap_or(u64::MAX);
            self.run_start = Instant::now();
            self.try_handlers.clear();
        }
        let closure = Rc::new(Closure {
            proto,
            upvalues: Vec::new(),
        });
        let base_depth = self.frames.len();
        let stack_base = self.stack.len();
        self.stack.push(Value::Closure(closure.clone()));
        self.frames.push(CallFrame {
            closure,
            ip: 0,
            base: stack_base,
        });

        match self.execute(base_depth) {
            Ok(v) => Ok(v),
            Err(e) => {
                let msg = if base_depth == 0 {
                    self.format_stack_trace(&e)
                } else {
                    e
                };
                self.close_upvalues_from(stack_base);
                self.frames.truncate(base_depth);
                self.stack.truncate(stack_base);
                while self
                    .try_handlers
                    .last()
                    .is_some_and(|h| h.frame_depth > base_depth)
                {
                    self.try_handlers.pop();
                }
                Err(msg)
            }
        }
    }

    fn format_stack_trace(&self, err: &str) -> String {
        let mut out = err.to_string();
        for frame in self.frames.iter().rev() {
            let line = if frame.ip > 0 {
                frame
                    .closure
                    .proto
                    .chunk
                    .lines
                    .get(frame.ip - 1)
                    .copied()
                    .unwrap_or(0)
            } else {
                0
            };
            out.push_str(&format!(
                "\n  at {} (line {})",
                frame.closure.proto.name, line
            ));
        }
        out
    }

    pub fn call_value(&mut self, callee: Value, args: &[Value]) -> Result<Value, String> {
        match callee {
            Value::NativeFn(native) => (native.func)(self, args),
            Value::Closure(closure) => {
                if self.frames.len() >= MAX_CALL_DEPTH {
                    return Err(format!(
                        "Stack overflow (maximum call depth {} exceeded)",
                        MAX_CALL_DEPTH
                    ));
                }
                if closure.proto.arity as usize != args.len() {
                    return Err(format!(
                        "Expected {} arguments to '{}', got {}",
                        closure.proto.arity,
                        closure.proto.name,
                        args.len()
                    ));
                }
                let base_depth = self.frames.len();
                let base = self.stack.len();
                self.stack.push(Value::Closure(closure.clone()));
                self.stack.extend_from_slice(args);
                self.frames.push(CallFrame {
                    closure,
                    ip: 0,
                    base,
                });
                match self.execute(base_depth) {
                    Ok(v) => Ok(v),
                    Err(e) => {
                        self.close_upvalues_from(base);
                        self.frames.truncate(base_depth);
                        self.stack.truncate(base);
                        while self
                            .try_handlers
                            .last()
                            .is_some_and(|h| h.frame_depth > base_depth)
                        {
                            self.try_handlers.pop();
                        }
                        Err(e)
                    }
                }
            }
            other => Err(format!("Cannot call value of type '{}'", other.type_name())),
        }
    }

    fn execute(&mut self, target_depth: usize) -> Result<Value, String> {
        loop {
            if self.permissions.max_instructions.is_some() {
                if self.instructions_left == 0 {
                    return Err("Sandbox instruction limit exceeded".to_string());
                }
                self.instructions_left -= 1;
            }
            if let Some(limit_ms) = self.permissions.max_duration_ms {
                if (self.instructions_left & 4095) == 0
                    && self.run_start.elapsed().as_millis() as u64 > limit_ms
                {
                    return Err(format!("Execution timeout ({} ms exceeded)", limit_ms));
                }
            }
            match self.step(target_depth) {
                Ok(Some(val)) => return Ok(val),
                Ok(None) => {}
                Err(err) => {
                    if Self::is_unrecoverable_error(&err) {
                        return Err(err);
                    }
                    if let Some(handler) = self.try_handlers.last().cloned() {
                        if handler.frame_depth > target_depth {
                            self.try_handlers.pop();
                            self.frames.truncate(handler.frame_depth);
                            self.close_upvalues_from(handler.stack_len);
                            self.stack.truncate(handler.stack_len);
                            self.stack.push(Value::Str(err));
                            self.frames.last_mut().unwrap().ip = handler.catch_ip;
                            continue;
                        }
                    }
                    return Err(err);
                }
            }
        }
    }

    fn step(&mut self, target_depth: usize) -> Result<Option<Value>, String> {
        let (op, line) = {
            let frame = self.frames.last_mut().unwrap();
            let op = frame.closure.proto.chunk.code[frame.ip];
            let line = frame.closure.proto.chunk.lines[frame.ip];
            frame.ip += 1;
            (op, line)
        };

        match op {
            Op::Constant(idx) => {
                let val = self.read_constant(idx);
                self.stack.push(val);
            }
            Op::Unit => self.stack.push(Value::Unit),
            Op::True => self.stack.push(Value::Bool(true)),
            Op::False => self.stack.push(Value::Bool(false)),
            Op::Pop => {
                self.stack.pop();
            }
            Op::GetLocal(slot) => {
                let base = self.frames.last().unwrap().base;
                let val = self.stack[base + slot as usize].clone();
                self.stack.push(val);
            }
            Op::SetLocal(slot) => {
                let base = self.frames.last().unwrap().base;
                let val = self.stack.last().unwrap().clone();
                self.stack[base + slot as usize] = val;
            }
            Op::GetUpvalue(slot) => {
                let uv = self.frames.last().unwrap().closure.upvalues[slot as usize].clone();
                let val = match &*uv.borrow() {
                    UpvalueState::Open(idx) => self.stack[*idx].clone(),
                    UpvalueState::Closed(v) => v.clone(),
                };
                self.stack.push(val);
            }
            Op::SetUpvalue(slot) => {
                let uv = self.frames.last().unwrap().closure.upvalues[slot as usize].clone();
                let val = self.stack.last().unwrap().clone();
                let mut state = uv.borrow_mut();
                match &mut *state {
                    UpvalueState::Open(idx) => self.stack[*idx] = val,
                    UpvalueState::Closed(v) => *v = val,
                }
            }
            Op::CloseUpvalue => {
                let slot = self.stack.len() - 1;
                self.close_upvalues_from(slot);
                self.stack.pop();
            }
            Op::GetGlobal(idx) => {
                let frame = self.frames.last().unwrap();
                let name = match &frame.closure.proto.chunk.constants[idx as usize] {
                    Constant::Str(s) => s.as_str(),
                    _ => unreachable!(),
                };
                let val = self
                    .globals
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("[line {}] Undefined variable '{}'", line, name))?;
                self.stack.push(val);
            }
            Op::DefineGlobal(idx) => {
                let name = self.read_string(idx);
                let val = self.stack.pop().unwrap();
                self.globals.insert(name, val);
            }
            Op::SetGlobal(idx) => {
                let frame = self.frames.last().unwrap();
                let name = match &frame.closure.proto.chunk.constants[idx as usize] {
                    Constant::Str(s) => s.as_str(),
                    _ => unreachable!(),
                };
                let val = self.stack.last().unwrap().clone();
                match self.globals.get_mut(name) {
                    Some(slot) => *slot = val,
                    None => {
                        return Err(format!("[line {}] Undefined variable '{}'", line, name));
                    }
                }
            }
            Op::Add => {
                let b = self.stack.pop().unwrap();
                let a = self.stack.pop().unwrap();
                let res = match (a, b) {
                    (Value::Int(x), Value::Int(y)) => Value::Int(x.wrapping_add(y)),
                    (Value::Float(x), Value::Float(y)) => Value::Float(x + y),
                    (Value::Int(x), Value::Float(y)) => Value::Float((x as f64) + y),
                    (Value::Float(x), Value::Int(y)) => Value::Float(x + (y as f64)),
                    (Value::Str(x), Value::Str(y)) => {
                        if x.len().saturating_add(y.len()) > 64 * 1024 * 1024 {
                            return Err(format!(
                                "[line {}] String size limit (64 MiB) exceeded",
                                line
                            ));
                        }
                        Value::Str(format!("{}{}", x, y))
                    }
                    (Value::List(x), Value::List(y)) => {
                        let x_ref = x.borrow();
                        let y_ref = y.borrow();
                        if x_ref.len().saturating_add(y_ref.len()) > 10_000_000 {
                            return Err(format!(
                                "[line {}] List size limit (10,000,000) exceeded",
                                line
                            ));
                        }
                        let mut combined = x_ref.clone();
                        combined.extend(y_ref.iter().cloned());
                        Value::List(Rc::new(RefCell::new(combined)))
                    }
                    (l, r) => {
                        return Err(format!(
                            "[line {}] Cannot add '{}' and '{}'",
                            line,
                            l.type_name(),
                            r.type_name()
                        ));
                    }
                };
                self.stack.push(res);
            }
            Op::Sub => self.numeric_op(line, i64::wrapping_sub, |a, b| a - b, "subtract")?,
            Op::Mul => self.numeric_op(line, i64::wrapping_mul, |a, b| a * b, "multiply")?,
            Op::Div => {
                let b = self.stack.pop().unwrap();
                let a = self.stack.pop().unwrap();
                let res = match (a, b) {
                    (Value::Int(_), Value::Int(0)) => {
                        return Err(format!("[line {}] Division by zero", line));
                    }
                    (Value::Int(x), Value::Int(y)) => {
                        Value::Int(x.checked_div(y).unwrap_or(i64::MIN))
                    }
                    (Value::Float(x), Value::Float(y)) => {
                        if y == 0.0 {
                            return Err(format!("[line {}] Division by zero", line));
                        }
                        Value::Float(x / y)
                    }
                    (Value::Int(x), Value::Float(y)) => {
                        if y == 0.0 {
                            return Err(format!("[line {}] Division by zero", line));
                        }
                        Value::Float((x as f64) / y)
                    }
                    (Value::Float(x), Value::Int(y)) => {
                        if y == 0 {
                            return Err(format!("[line {}] Division by zero", line));
                        }
                        Value::Float(x / (y as f64))
                    }
                    (l, r) => {
                        return Err(format!(
                            "[line {}] Cannot divide '{}' and '{}'",
                            line,
                            l.type_name(),
                            r.type_name()
                        ));
                    }
                };
                self.stack.push(res);
            }
            Op::Mod => {
                let b = self.stack.pop().unwrap();
                let a = self.stack.pop().unwrap();
                match (a, b) {
                    (Value::Int(_), Value::Int(0)) => {
                        return Err(format!("[line {}] Modulo by zero", line));
                    }
                    (Value::Int(x), Value::Int(y)) => {
                        self.stack.push(Value::Int(x.checked_rem(y).unwrap_or(0)))
                    }
                    (Value::Float(x), Value::Float(y)) => self.stack.push(Value::Float(x % y)),
                    (l, r) => {
                        return Err(format!(
                            "[line {}] Cannot modulo '{}' and '{}'",
                            line,
                            l.type_name(),
                            r.type_name()
                        ));
                    }
                }
            }
            Op::Neg => {
                let v = self.stack.pop().unwrap();
                match v {
                    Value::Int(x) => self.stack.push(Value::Int(x.wrapping_neg())),
                    Value::Float(x) => self.stack.push(Value::Float(-x)),
                    other => {
                        return Err(format!(
                            "[line {}] Cannot negate '{}'",
                            line,
                            other.type_name()
                        ));
                    }
                }
            }
            Op::Not => {
                let v = self.stack.pop().unwrap();
                self.stack.push(Value::Bool(!v.is_truthy()));
            }
            Op::Eq => {
                let b = self.stack.pop().unwrap();
                let a = self.stack.pop().unwrap();
                self.stack.push(Value::Bool(a.equals(&b)));
            }
            Op::Ne => {
                let b = self.stack.pop().unwrap();
                let a = self.stack.pop().unwrap();
                self.stack.push(Value::Bool(!a.equals(&b)));
            }
            Op::Lt => self.cmp_op(line, |a, b| a < b, |a, b| a < b, |a, b| a < b)?,
            Op::Le => self.cmp_op(line, |a, b| a <= b, |a, b| a <= b, |a, b| a <= b)?,
            Op::Gt => self.cmp_op(line, |a, b| a > b, |a, b| a > b, |a, b| a > b)?,
            Op::Ge => self.cmp_op(line, |a, b| a >= b, |a, b| a >= b, |a, b| a >= b)?,
            Op::In => {
                let container = self.stack.pop().unwrap();
                let needle = self.stack.pop().unwrap();
                let found = match (needle, container) {
                    (Value::Str(sub), Value::Str(s)) => s.contains(&sub),
                    (val, Value::List(l)) => l.borrow().iter().any(|item| item.equals(&val)),
                    (Value::Str(k), Value::Dict(d)) => d.borrow().contains_key(&k),
                    (Value::Int(n), Value::Range { start, end }) => n >= start && n < end,
                    (n, c) => {
                        return Err(format!(
                            "[line {}] Cannot check '{}' in '{}'",
                            line,
                            n.type_name(),
                            c.type_name()
                        ));
                    }
                };
                self.stack.push(Value::Bool(found));
            }
            Op::Jump(target) => {
                self.frames.last_mut().unwrap().ip = target;
            }
            Op::JumpIfFalsePop(target) => {
                let cond = self.stack.pop().unwrap();
                if !cond.is_truthy() {
                    self.frames.last_mut().unwrap().ip = target;
                }
            }
            Op::JumpIfFalsePeek(target) => {
                if !self.stack.last().unwrap().is_truthy() {
                    self.frames.last_mut().unwrap().ip = target;
                }
            }
            Op::JumpIfTruePeek(target) => {
                if self.stack.last().unwrap().is_truthy() {
                    self.frames.last_mut().unwrap().ip = target;
                }
            }
            Op::JumpIfNotUnitPeek(target) => {
                if !matches!(self.stack.last().unwrap(), Value::Unit) {
                    self.frames.last_mut().unwrap().ip = target;
                }
            }
            Op::Call(argc) => {
                let argc = argc as usize;
                let callee_idx = self.stack.len() - 1 - argc;
                let callee = self.stack[callee_idx].clone();
                match callee {
                    Value::Closure(closure) => {
                        if self.frames.len() >= MAX_CALL_DEPTH {
                            return Err(format!(
                                "[line {}] Stack overflow (maximum call depth {} exceeded)",
                                line, MAX_CALL_DEPTH
                            ));
                        }
                        if closure.proto.arity as usize != argc {
                            return Err(format!(
                                "[line {}] Expected {} arguments to '{}', got {}",
                                line, closure.proto.arity, closure.proto.name, argc
                            ));
                        }
                        self.frames.push(CallFrame {
                            closure,
                            ip: 0,
                            base: callee_idx,
                        });
                    }
                    Value::NativeFn(native) => {
                        let args = self.stack[callee_idx + 1..].to_vec();
                        self.stack.truncate(callee_idx);
                        let res = (native.func)(self, &args)
                            .map_err(|e| format!("[line {}] {}", line, e))?;
                        self.stack.push(res);
                    }
                    other => {
                        return Err(format!(
                            "[line {}] Cannot call value of type '{}'",
                            line,
                            other.type_name()
                        ));
                    }
                }
            }
            Op::Closure(idx, uv_idx) => {
                let (proto, upvalue_descs) = {
                    let frame = self.frames.last().unwrap();
                    let proto = match &frame.closure.proto.chunk.constants[idx as usize] {
                        Constant::Function(p) => p.clone(),
                        _ => unreachable!(),
                    };
                    let descs = frame.closure.proto.chunk.upvalue_lists[uv_idx as usize].clone();
                    (proto, descs)
                };
                let mut upvalues = Vec::with_capacity(upvalue_descs.len());
                for desc in upvalue_descs {
                    if desc.is_local {
                        let base = self.frames.last().unwrap().base;
                        let slot = base + desc.index as usize;
                        upvalues.push(self.capture_upvalue(slot));
                    } else {
                        let uv = self.frames.last().unwrap().closure.upvalues[desc.index as usize]
                            .clone();
                        upvalues.push(uv);
                    }
                }
                self.stack
                    .push(Value::Closure(Rc::new(Closure { proto, upvalues })));
            }
            Op::MakeList(count) => {
                let count = count as usize;
                let start = self.stack.len() - count;
                let items = self.stack.drain(start..).collect::<Vec<_>>();
                self.stack.push(Value::List(Rc::new(RefCell::new(items))));
            }
            Op::MakeDict(count) => {
                let count = count as usize;
                let start = self.stack.len() - count * 2;
                let drained: Vec<Value> = self.stack.drain(start..).collect();
                let mut map = HashMap::with_capacity(count);
                for pair in drained.chunks_exact(2) {
                    let key = match &pair[0] {
                        Value::Str(s) => s.clone(),
                        other => {
                            return Err(format!(
                                "[line {}] Dictionary keys must be strings, got '{}'",
                                line,
                                other.type_name()
                            ));
                        }
                    };
                    map.insert(key, pair[1].clone());
                }
                self.stack.push(Value::Dict(Rc::new(RefCell::new(map))));
            }
            Op::GetIndex => {
                let idx_val = self.stack.pop().unwrap();
                let obj_val = self.stack.pop().unwrap();
                let res = match (obj_val, idx_val) {
                    (Value::List(list), Value::Int(i)) => {
                        let borrowed = list.borrow();
                        let len = borrowed.len() as i64;
                        let actual = if i < 0 { len + i } else { i };
                        if actual < 0 || actual >= len {
                            return Err(format!(
                                "[line {}] Index {} out of bounds for list of length {}",
                                line, i, len
                            ));
                        }
                        borrowed[actual as usize].clone()
                    }
                    (Value::Str(s), Value::Int(i)) => {
                        let chars: Vec<char> = s.chars().collect();
                        let len = chars.len() as i64;
                        let actual = if i < 0 { len + i } else { i };
                        if actual < 0 || actual >= len {
                            return Err(format!(
                                "[line {}] Index {} out of bounds for string of length {}",
                                line, i, len
                            ));
                        }
                        Value::Str(chars[actual as usize].to_string())
                    }
                    (Value::Dict(dict), Value::Str(key)) => {
                        dict.borrow().get(&key).cloned().unwrap_or(Value::Unit)
                    }
                    (Value::Range { start, end }, Value::Int(i)) => {
                        let len = if end > start {
                            (end as i128) - (start as i128)
                        } else {
                            0
                        };
                        let actual = if i < 0 { len + i as i128 } else { i as i128 };
                        if actual < 0 || actual >= len {
                            return Err(format!(
                                "[line {}] Index {} out of bounds for range of length {}",
                                line, i, len
                            ));
                        }
                        Value::Int((start as i128 + actual) as i64)
                    }
                    (o, i) => {
                        return Err(format!(
                            "[line {}] Cannot index '{}' with '{}'",
                            line,
                            o.type_name(),
                            i.type_name()
                        ));
                    }
                };
                self.stack.push(res);
            }
            Op::Slice(inclusive) => {
                let end_val = self.stack.pop().unwrap();
                let start_val = self.stack.pop().unwrap();
                let obj_val = self.stack.pop().unwrap();
                let resolve_bounds = |len: i64| -> Result<(usize, usize), String> {
                    let s = match &start_val {
                        Value::Unit => 0,
                        Value::Int(v) => {
                            let r = if *v < 0 { len + *v } else { *v };
                            r.clamp(0, len) as usize
                        }
                        other => {
                            return Err(format!(
                                "[line {}] Slice start must be int, got '{}'",
                                line,
                                other.type_name()
                            ));
                        }
                    };
                    let e = match &end_val {
                        Value::Unit => len as usize,
                        Value::Int(v) => {
                            let r = if *v < 0 { len + *v } else { *v };
                            let r = if inclusive { r + 1 } else { r };
                            r.clamp(0, len) as usize
                        }
                        other => {
                            return Err(format!(
                                "[line {}] Slice end must be int, got '{}'",
                                line,
                                other.type_name()
                            ));
                        }
                    };
                    Ok((s, e))
                };
                let res = match obj_val {
                    Value::List(list) => {
                        let borrowed = list.borrow();
                        let (s, e) = resolve_bounds(borrowed.len() as i64)?;
                        let sliced = if s < e {
                            borrowed[s..e].to_vec()
                        } else {
                            Vec::new()
                        };
                        Value::List(Rc::new(RefCell::new(sliced)))
                    }
                    Value::Str(text) => {
                        let chars: Vec<char> = text.chars().collect();
                        let (s, e) = resolve_bounds(chars.len() as i64)?;
                        let sliced: String = if s < e {
                            chars[s..e].iter().collect()
                        } else {
                            String::new()
                        };
                        Value::Str(sliced)
                    }
                    Value::Range { start, end } => {
                        let len = if end > start {
                            (end - start).min(i64::MAX)
                        } else {
                            0
                        };
                        let (s, e) = resolve_bounds(len)?;
                        let ns = start.saturating_add(s as i64);
                        let ne = start.saturating_add(e as i64).min(end);
                        Value::Range {
                            start: ns,
                            end: ne.max(ns),
                        }
                    }
                    other => {
                        return Err(format!(
                            "[line {}] Cannot slice '{}'",
                            line,
                            other.type_name()
                        ));
                    }
                };
                self.stack.push(res);
            }
            Op::SetIndex => {
                let val = self.stack.pop().unwrap();
                let idx_val = self.stack.pop().unwrap();
                let obj_val = self.stack.pop().unwrap();
                match (obj_val, idx_val) {
                    (Value::List(list), Value::Int(i)) => {
                        let mut borrowed = list.borrow_mut();
                        let len = borrowed.len() as i64;
                        let actual = if i < 0 { len + i } else { i };
                        if actual < 0 || actual >= len {
                            return Err(format!(
                                "[line {}] Index {} out of bounds for list of length {}",
                                line, i, len
                            ));
                        }
                        borrowed[actual as usize] = val.clone();
                    }
                    (Value::Dict(dict), Value::Str(key)) => {
                        dict.borrow_mut().insert(key, val.clone());
                    }
                    (o, i) => {
                        return Err(format!(
                            "[line {}] Cannot index-assign '{}' with '{}'",
                            line,
                            o.type_name(),
                            i.type_name()
                        ));
                    }
                }
                self.stack.push(val);
            }
            Op::GetAttr(idx) => {
                let attr = self.read_string(idx);
                let obj = self.stack.pop().unwrap();
                match obj {
                    Value::Module { name, exports } => {
                        let val = exports.borrow().get(&attr).cloned().ok_or_else(|| {
                            format!("[line {}] Module '{}' has no export '{}'", line, name, attr)
                        })?;
                        self.stack.push(val);
                    }
                    Value::Dict(dict) => {
                        let val = dict.borrow().get(&attr).cloned().unwrap_or(Value::Unit);
                        self.stack.push(val);
                    }
                    other => {
                        return Err(format!(
                            "[line {}] Cannot access '.{}' on '{}'",
                            line,
                            attr,
                            other.type_name()
                        ));
                    }
                }
            }
            Op::OptGetAttr(idx) => {
                let obj = self.stack.pop().unwrap();
                if matches!(obj, Value::Unit) {
                    self.stack.push(Value::Unit);
                } else {
                    let attr = self.read_string(idx);
                    match obj {
                        Value::Module { name, exports } => {
                            let val = exports.borrow().get(&attr).cloned().ok_or_else(|| {
                                format!(
                                    "[line {}] Module '{}' has no export '{}'",
                                    line, name, attr
                                )
                            })?;
                            self.stack.push(val);
                        }
                        Value::Dict(dict) => {
                            let val = dict.borrow().get(&attr).cloned().unwrap_or(Value::Unit);
                            self.stack.push(val);
                        }
                        other => {
                            return Err(format!(
                                "[line {}] Cannot access '?.{}' on '{}'",
                                line,
                                attr,
                                other.type_name()
                            ));
                        }
                    }
                }
            }
            Op::SetAttr(idx) => {
                let attr = self.read_string(idx);
                let val = self.stack.pop().unwrap();
                let obj = self.stack.pop().unwrap();
                match obj {
                    Value::Dict(dict) => {
                        dict.borrow_mut().insert(attr, val.clone());
                        self.stack.push(val);
                    }
                    other => {
                        return Err(format!(
                            "[line {}] Cannot assign '.{}' on '{}'",
                            line,
                            attr,
                            other.type_name()
                        ));
                    }
                }
            }
            Op::Import(idx, is_path) => {
                let source = self.read_string(idx);
                let module = self
                    .load_module(&source, is_path)
                    .map_err(|e| format!("[line {}] {}", line, e))?;
                self.stack.push(module);
            }
            Op::MakeModule(name_idx, count) => {
                let mod_name = self.read_string(name_idx);
                let count = count as usize;
                let start = self.stack.len() - count * 2;
                let mut exports = HashMap::with_capacity(count);
                let drained: Vec<Value> = self.stack.drain(start..).collect();
                for pair in drained.chunks_exact(2) {
                    if let Value::Str(k) = &pair[0] {
                        exports.insert(k.clone(), pair[1].clone());
                    }
                }
                self.stack.push(Value::Module {
                    name: mod_name,
                    exports: Rc::new(RefCell::new(exports)),
                });
            }
            Op::SetupTry(catch_ip) => {
                self.try_handlers.push(TryHandler {
                    frame_depth: self.frames.len(),
                    stack_len: self.stack.len(),
                    catch_ip,
                });
            }
            Op::PopTry => {
                self.try_handlers.pop();
            }
            Op::Throw => {
                let val = self.stack.pop().unwrap();
                return Err(val.to_string());
            }
            Op::Return => {
                let result = self.stack.pop().unwrap();
                let frame = self.frames.pop().unwrap();
                while self
                    .try_handlers
                    .last()
                    .is_some_and(|h| h.frame_depth > self.frames.len())
                {
                    self.try_handlers.pop();
                }
                self.close_upvalues_from(frame.base);
                self.stack.truncate(frame.base);
                if self.frames.len() == target_depth {
                    return Ok(Some(result));
                }
                self.stack.push(result);
            }
        }
        Ok(None)
    }

    fn read_constant(&self, idx: u16) -> Value {
        let frame = self.frames.last().unwrap();
        match &frame.closure.proto.chunk.constants[idx as usize] {
            Constant::Int(v) => Value::Int(*v),
            Constant::Float(v) => Value::Float(*v),
            Constant::Str(s) => Value::Str(s.clone()),
            Constant::Function(_) => {
                unreachable!("Function constant must be loaded via Op::Closure")
            }
        }
    }

    fn read_string(&self, idx: u16) -> String {
        let frame = self.frames.last().unwrap();
        match &frame.closure.proto.chunk.constants[idx as usize] {
            Constant::Str(s) => s.clone(),
            _ => unreachable!(),
        }
    }

    fn capture_upvalue(&mut self, slot: usize) -> Rc<RefCell<UpvalueState>> {
        for uv in &self.open_upvalues {
            if let UpvalueState::Open(idx) = *uv.borrow() {
                if idx == slot {
                    return uv.clone();
                }
            }
        }
        let uv = Rc::new(RefCell::new(UpvalueState::Open(slot)));
        self.open_upvalues.push(uv.clone());
        uv
    }

    fn close_upvalues_from(&mut self, last_slot: usize) {
        let mut remaining = Vec::with_capacity(self.open_upvalues.len());
        for uv in self.open_upvalues.drain(..) {
            let close_slot = match *uv.borrow() {
                UpvalueState::Open(slot) if slot >= last_slot => Some(slot),
                _ => None,
            };
            if let Some(slot) = close_slot {
                let val = self.stack[slot].clone();
                *uv.borrow_mut() = UpvalueState::Closed(val);
            } else {
                remaining.push(uv);
            }
        }
        self.open_upvalues = remaining;
    }

    fn load_module(&mut self, source: &str, is_path: bool) -> Result<Value, String> {
        if !is_path {
            if source == "mem" && self.sandboxed {
                return Err("mem is disabled in sandbox mode (requires --unsafe and ASKANE_ENABLE_UNSAFE_MEM=1)".to_string());
            }
            if let Some(cached) = self.module_cache.get(source) {
                return Ok(cached.clone());
            }
            if let Some(builtin) = modules::get_builtin_module(source) {
                self.module_cache
                    .insert(source.to_string(), builtin.clone());
                return Ok(builtin);
            }
        }

        let mut file_path = PathBuf::from(source);
        if file_path.extension().is_none() {
            file_path.set_extension("ask");
        }
        let file_path = self.resolve_read_path(&file_path.to_string_lossy())?;

        let cache_key = fs::canonicalize(&file_path)
            .unwrap_or_else(|_| file_path.clone())
            .to_string_lossy()
            .to_string();

        if let Some(cached) = self.module_cache.get(&cache_key) {
            return Ok(cached.clone());
        }
        if !self.loading_modules.insert(cache_key.clone()) {
            return Err(format!(
                "Circular import detected for '{}'",
                file_path.display()
            ));
        }

        let code = match crate::read_text_limited(&file_path, crate::MAX_SOURCE_BYTES) {
            Ok(c) => c,
            Err(e) => {
                self.loading_modules.remove(&cache_key);
                return Err(format!("Could not import '{}': {}", file_path.display(), e));
            }
        };

        let mod_name = file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(source)
            .to_string();

        let proto = (|| {
            let mut lexer = Lexer::new(&code);
            let tokens = lexer.tokenize()?;
            let mut parser = Parser::new(tokens);
            let program = parser.parse_program()?;
            Compiler::compile_module(&program, &mod_name)
        })();
        let proto = match proto {
            Ok(p) => p,
            Err(e) => {
                self.loading_modules.remove(&cache_key);
                return Err(e);
            }
        };

        let prev_dir = self.current_dir.clone();
        if let Some(parent) = file_path.parent() {
            self.current_dir = parent.to_path_buf();
        }
        let result = self.run(proto);
        self.current_dir = prev_dir;
        self.loading_modules.remove(&cache_key);

        let module_val = result?;
        self.module_cache.insert(cache_key, module_val.clone());
        Ok(module_val)
    }

    fn numeric_op(
        &mut self,
        line: usize,
        int_op: fn(i64, i64) -> i64,
        float_op: fn(f64, f64) -> f64,
        name: &str,
    ) -> Result<(), String> {
        let b = self.stack.pop().unwrap();
        let a = self.stack.pop().unwrap();
        let res = match (a, b) {
            (Value::Int(x), Value::Int(y)) => Value::Int(int_op(x, y)),
            (Value::Float(x), Value::Float(y)) => Value::Float(float_op(x, y)),
            (Value::Int(x), Value::Float(y)) => Value::Float(float_op(x as f64, y)),
            (Value::Float(x), Value::Int(y)) => Value::Float(float_op(x, y as f64)),
            (l, r) => {
                return Err(format!(
                    "[line {}] Cannot {} '{}' and '{}'",
                    line,
                    name,
                    l.type_name(),
                    r.type_name()
                ));
            }
        };
        self.stack.push(res);
        Ok(())
    }

    fn cmp_op(
        &mut self,
        line: usize,
        int_op: fn(i64, i64) -> bool,
        float_op: fn(f64, f64) -> bool,
        str_op: fn(&str, &str) -> bool,
    ) -> Result<(), String> {
        let b = self.stack.pop().unwrap();
        let a = self.stack.pop().unwrap();
        let res = match (a, b) {
            (Value::Int(x), Value::Int(y)) => Value::Bool(int_op(x, y)),
            (Value::Float(x), Value::Float(y)) => Value::Bool(float_op(x, y)),
            (Value::Int(x), Value::Float(y)) => Value::Bool(float_op(x as f64, y)),
            (Value::Float(x), Value::Int(y)) => Value::Bool(float_op(x, y as f64)),
            (Value::Str(x), Value::Str(y)) => Value::Bool(str_op(&x, &y)),
            (l, r) => {
                return Err(format!(
                    "[line {}] Cannot compare '{}' and '{}'",
                    line,
                    l.type_name(),
                    r.type_name()
                ));
            }
        };
        self.stack.push(res);
        Ok(())
    }
}

fn native_print(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            print!(" ");
        }
        print!("{}", arg);
    }
    let _ = io::stdout().flush();
    Ok(Value::Unit)
}

fn native_println(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            print!(" ");
        }
        print!("{}", arg);
    }
    println!();
    Ok(Value::Unit)
}

fn native_len(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("len() expects 1 argument, got {}", args.len()));
    }
    match &args[0] {
        Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
        Value::List(l) => Ok(Value::Int(l.borrow().len() as i64)),
        Value::Dict(d) => Ok(Value::Int(d.borrow().len() as i64)),
        Value::Range { start, end } => {
            let len = if end > start {
                (*end as i128) - (*start as i128)
            } else {
                0
            };
            Ok(Value::Int(len.min(i64::MAX as i128) as i64))
        }
        other => Err(format!("len() not supported for '{}'", other.type_name())),
    }
}

fn native_push(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("push() expects 2 arguments, got {}", args.len()));
    }
    match &args[0] {
        Value::List(l) => {
            if l.borrow().len() >= 10_000_000 {
                return Err("push() list size exceeds maximum limit of 10,000,000".to_string());
            }
            l.borrow_mut().push(args[1].clone());
            Ok(args[0].clone())
        }
        other => Err(format!(
            "push() expects a list, got '{}'",
            other.type_name()
        )),
    }
}

fn native_pop(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("pop() expects 1 argument, got {}", args.len()));
    }
    match &args[0] {
        Value::List(l) => l
            .borrow_mut()
            .pop()
            .ok_or_else(|| "pop() called on empty list".to_string()),
        other => Err(format!("pop() expects a list, got '{}'", other.type_name())),
    }
}

fn native_range(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    let (start, end) = match args {
        [Value::Int(end)] => (0, *end),
        [Value::Int(start), Value::Int(end)] => (*start, *end),
        _ => return Err("range() expects (end) or (start, end) integers".to_string()),
    };
    Ok(Value::Range { start, end })
}

fn collect_iterable(val: &Value, fn_name: &str) -> Result<Vec<Value>, String> {
    match val {
        Value::List(l) => Ok(l.borrow().clone()),
        Value::Range { start, end } => {
            if *end > *start && (*end as i128 - *start as i128) > 10_000_000 {
                return Err(format!(
                    "{}() range size exceeds maximum limit of 10,000,000",
                    fn_name
                ));
            }
            Ok((*start..*end).map(Value::Int).collect())
        }
        other => Err(format!(
            "{}() expects a list or range, got '{}'",
            fn_name,
            other.type_name()
        )),
    }
}

fn native_map(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("map() expects 2 arguments, got {}", args.len()));
    }
    let items = collect_iterable(&args[0], "map")?;
    let func = args[1].clone();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(vm.call_value(func.clone(), &[item])?);
    }
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn native_filter(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("filter() expects 2 arguments, got {}", args.len()));
    }
    let items = collect_iterable(&args[0], "filter")?;
    let func = args[1].clone();
    let mut out = Vec::new();
    for item in items {
        let keep = vm.call_value(func.clone(), std::slice::from_ref(&item))?;
        if keep.is_truthy() {
            out.push(item);
        }
    }
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn native_reduce(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(format!("reduce() expects 3 arguments, got {}", args.len()));
    }
    let items = collect_iterable(&args[0], "reduce")?;
    let mut acc = args[1].clone();
    let func = args[2].clone();
    for item in items {
        acc = vm.call_value(func.clone(), &[acc, item])?;
    }
    Ok(acc)
}

fn native_str(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("str() expects 1 argument, got {}", args.len()));
    }
    Ok(Value::Str(format!("{}", args[0])))
}

fn native_int(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("int() expects 1 argument, got {}", args.len()));
    }
    match &args[0] {
        Value::Int(v) => Ok(Value::Int(*v)),
        Value::Float(v) => Ok(Value::Int(*v as i64)),
        Value::Bool(b) => Ok(Value::Int(if *b { 1 } else { 0 })),
        Value::Str(s) => s
            .trim()
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| format!("Cannot convert {:?} to int", s)),
        other => Err(format!("Cannot convert '{}' to int", other.type_name())),
    }
}

fn native_float(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("float() expects 1 argument, got {}", args.len()));
    }
    match &args[0] {
        Value::Int(v) => Ok(Value::Float(*v as f64)),
        Value::Float(v) => Ok(Value::Float(*v)),
        Value::Str(s) => s
            .trim()
            .parse::<f64>()
            .map(Value::Float)
            .map_err(|_| format!("Cannot convert {:?} to float", s)),
        other => Err(format!("Cannot convert '{}' to float", other.type_name())),
    }
}

fn native_type_of(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("type_of() expects 1 argument, got {}", args.len()));
    }
    Ok(Value::Str(args[0].type_name().to_string()))
}

fn native_keys(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("keys() expects 1 argument, got {}", args.len()));
    }
    match &args[0] {
        Value::Dict(d) => {
            let mut keys: Vec<String> = d.borrow().keys().cloned().collect();
            keys.sort();
            let list: Vec<Value> = keys.into_iter().map(Value::Str).collect();
            Ok(Value::List(Rc::new(RefCell::new(list))))
        }
        other => Err(format!(
            "keys() expects a dict, got '{}'",
            other.type_name()
        )),
    }
}

fn native_values(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("values() expects 1 argument, got {}", args.len()));
    }
    match &args[0] {
        Value::Dict(d) => {
            let map = d.borrow();
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let list: Vec<Value> = keys.into_iter().map(|k| map[k].clone()).collect();
            Ok(Value::List(Rc::new(RefCell::new(list))))
        }
        other => Err(format!(
            "values() expects a dict, got '{}'",
            other.type_name()
        )),
    }
}

fn native_entries(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("entries() expects 1 argument, got {}", args.len()));
    }
    match &args[0] {
        Value::Dict(d) => {
            let map = d.borrow();
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let list: Vec<Value> = keys
                .into_iter()
                .map(|k| {
                    Value::List(Rc::new(RefCell::new(vec![
                        Value::Str(k.clone()),
                        map[k].clone(),
                    ])))
                })
                .collect();
            Ok(Value::List(Rc::new(RefCell::new(list))))
        }
        other => Err(format!(
            "entries() expects a dict, got '{}'",
            other.type_name()
        )),
    }
}

fn native_get(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err("get() expects (dict, key, [default])".to_string());
    }
    let default = args.get(2).cloned().unwrap_or(Value::Unit);
    match (&args[0], &args[1]) {
        (Value::Dict(d), Value::Str(k)) => Ok(d.borrow().get(k).cloned().unwrap_or(default)),
        _ => Err("get() expects (dict, string_key, [default])".to_string()),
    }
}

fn native_remove(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("remove() expects 2 arguments, got {}", args.len()));
    }
    match (&args[0], &args[1]) {
        (Value::Dict(d), Value::Str(k)) => Ok(d.borrow_mut().remove(k).unwrap_or(Value::Unit)),
        (Value::List(l), Value::Int(i)) => {
            let mut list = l.borrow_mut();
            let len = list.len() as i64;
            let idx = if *i < 0 { len + *i } else { *i };
            if idx < 0 || idx >= len {
                return Err(format!("remove() index {} out of bounds", i));
            }
            Ok(list.remove(idx as usize))
        }
        _ => Err("remove() expects (dict, key) or (list, index)".to_string()),
    }
}

fn native_sort(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("sort() expects 1 argument, got {}", args.len()));
    }
    let list = match &args[0] {
        Value::List(l) => l,
        other => {
            return Err(format!(
                "sort() expects a list, got '{}'",
                other.type_name()
            ));
        }
    };
    let mut items = list.borrow().clone();
    let mut err: Option<String> = None;
    items.sort_by(|a, b| {
        if err.is_some() {
            return std::cmp::Ordering::Equal;
        }
        match (a, b) {
            (Value::Int(x), Value::Int(y)) => x.cmp(y),
            (Value::Float(x), Value::Float(y)) => {
                x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal)
            }
            (Value::Int(x), Value::Float(y)) => (*x as f64)
                .partial_cmp(y)
                .unwrap_or(std::cmp::Ordering::Equal),
            (Value::Float(x), Value::Int(y)) => x
                .partial_cmp(&(*y as f64))
                .unwrap_or(std::cmp::Ordering::Equal),
            (Value::Str(x), Value::Str(y)) => x.cmp(y),
            (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
            _ => {
                err = Some(format!(
                    "sort() cannot compare '{}' and '{}'",
                    a.type_name(),
                    b.type_name()
                ));
                std::cmp::Ordering::Equal
            }
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    Ok(Value::List(Rc::new(RefCell::new(items))))
}

fn native_reverse(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("reverse() expects 1 argument, got {}", args.len()));
    }
    match &args[0] {
        Value::List(l) => {
            let mut items = l.borrow().clone();
            items.reverse();
            Ok(Value::List(Rc::new(RefCell::new(items))))
        }
        Value::Str(s) => Ok(Value::Str(s.chars().rev().collect())),
        other => Err(format!(
            "reverse() expects a list or string, got '{}'",
            other.type_name()
        )),
    }
}

fn native_has(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("has() expects 2 arguments, got {}", args.len()));
    }
    match (&args[0], &args[1]) {
        (Value::Dict(d), Value::Str(k)) => Ok(Value::Bool(d.borrow().contains_key(k))),
        _ => Err("has() expects (dict, string_key)".to_string()),
    }
}

fn native_read_line(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err(format!(
            "read_line() expects 0 arguments, got {}",
            args.len()
        ));
    }
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|e| format!("Failed to read line: {}", e))?;
    Ok(Value::Str(
        input.trim_end_matches(&['\r', '\n'][..]).to_string(),
    ))
}

fn native_clock(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err(format!("clock() expects 0 arguments, got {}", args.len()));
    }
    Ok(Value::Float(vm.start_time.elapsed().as_secs_f64()))
}

fn native_enumerate(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!(
            "enumerate() expects 1 argument, got {}",
            args.len()
        ));
    }
    let items = collect_iterable(&args[0], "enumerate")?;
    let out: Vec<Value> = items
        .into_iter()
        .enumerate()
        .map(|(i, v)| Value::List(Rc::new(RefCell::new(vec![Value::Int(i as i64), v]))))
        .collect();
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn native_zip(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("zip() expects 2 arguments, got {}", args.len()));
    }
    let a = collect_iterable(&args[0], "zip")?;
    let b = collect_iterable(&args[1], "zip")?;
    let out: Vec<Value> = a
        .into_iter()
        .zip(b)
        .map(|(x, y)| Value::List(Rc::new(RefCell::new(vec![x, y]))))
        .collect();
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn native_take(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("take() expects 2 arguments, got {}", args.len()));
    }
    let n = match &args[1] {
        Value::Int(v) if *v >= 0 => *v as usize,
        _ => return Err("take() count must be a non-negative integer".to_string()),
    };
    if n > 10_000_000 {
        return Err("take() count exceeds maximum limit of 10,000,000".to_string());
    }
    match &args[0] {
        Value::Range { start, end } => {
            if *end <= *start || n == 0 {
                return Ok(Value::List(Rc::new(RefCell::new(Vec::new()))));
            }
            let max_end = (*start as i128 + n as i128).min(*end as i128) as i64;
            let out: Vec<Value> = (*start..max_end).map(Value::Int).collect();
            Ok(Value::List(Rc::new(RefCell::new(out))))
        }
        Value::List(l) => {
            let out: Vec<Value> = l.borrow().iter().take(n).cloned().collect();
            Ok(Value::List(Rc::new(RefCell::new(out))))
        }
        other => Err(format!(
            "take() expects a list or range, got '{}'",
            other.type_name()
        )),
    }
}

fn native_skip(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("skip() expects 2 arguments, got {}", args.len()));
    }
    let n = match &args[1] {
        Value::Int(v) if *v >= 0 => *v as usize,
        _ => return Err("skip() count must be a non-negative integer".to_string()),
    };
    match &args[0] {
        Value::Range { start, end } => {
            let new_start = (*start as i128 + n as i128).min(*end as i128) as i64;
            Ok(Value::Range {
                start: new_start,
                end: *end,
            })
        }
        Value::List(l) => {
            let out: Vec<Value> = l.borrow().iter().skip(n).cloned().collect();
            Ok(Value::List(Rc::new(RefCell::new(out))))
        }
        other => Err(format!(
            "skip() expects a list or range, got '{}'",
            other.type_name()
        )),
    }
}

fn native_any(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("any() expects 2 arguments, got {}", args.len()));
    }
    let items = collect_iterable(&args[0], "any")?;
    let func = args[1].clone();
    for item in items {
        if vm.call_value(func.clone(), &[item])?.is_truthy() {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn native_all(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("all() expects 2 arguments, got {}", args.len()));
    }
    let items = collect_iterable(&args[0], "all")?;
    let func = args[1].clone();
    for item in items {
        if !vm.call_value(func.clone(), &[item])?.is_truthy() {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

fn native_find(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("find() expects 2 arguments, got {}", args.len()));
    }
    let items = collect_iterable(&args[0], "find")?;
    let func = args[1].clone();
    for item in items {
        if vm
            .call_value(func.clone(), std::slice::from_ref(&item))?
            .is_truthy()
        {
            return Ok(item);
        }
    }
    Ok(Value::Unit)
}

fn native_pairs(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("for a, b expects 1 iterable".to_string());
    }
    match &args[0] {
        Value::Dict(_) => native_entries(vm, args),
        Value::List(_) => Ok(args[0].clone()),
        other => Err(format!(
            "for a, b requires a list of pairs or dict, got '{}'",
            other.type_name()
        )),
    }
}

fn native_unique(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("unique() expects 1 argument, got {}", args.len()));
    }
    let items = collect_iterable(&args[0], "unique")?;
    let mut out: Vec<Value> = Vec::new();
    for item in items {
        if !out.iter().any(|existing| existing.equals(&item)) {
            out.push(item);
        }
    }
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn native_flatten(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("flatten() expects 1 argument, got {}", args.len()));
    }
    let items = collect_iterable(&args[0], "flatten")?;
    let mut out = Vec::new();
    for item in items {
        match item {
            Value::List(inner) => {
                let b = inner.borrow();
                if out.len().saturating_add(b.len()) > 10_000_000 {
                    return Err(
                        "flatten() list size exceeds maximum limit of 10,000,000".to_string()
                    );
                }
                out.extend(b.iter().cloned());
            }
            other => {
                if out.len() >= 10_000_000 {
                    return Err(
                        "flatten() list size exceeds maximum limit of 10,000,000".to_string()
                    );
                }
                out.push(other);
            }
        }
    }
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn native_chunk(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("chunk() expects 2 arguments, got {}", args.len()));
    }
    let items = collect_iterable(&args[0], "chunk")?;
    let size = match &args[1] {
        Value::Int(n) if *n > 0 => *n as usize,
        _ => return Err("chunk() size must be a positive integer".to_string()),
    };
    let out: Vec<Value> = items
        .chunks(size)
        .map(|c| Value::List(Rc::new(RefCell::new(c.to_vec()))))
        .collect();
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn native_sum(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("sum() expects 1 argument, got {}", args.len()));
    }
    let items = collect_iterable(&args[0], "sum")?;
    let mut int_acc: i64 = 0;
    let mut float_acc: f64 = 0.0;
    let mut is_float = false;
    for item in items {
        match item {
            Value::Int(v) => {
                if is_float {
                    float_acc += v as f64;
                } else {
                    int_acc = int_acc.wrapping_add(v);
                }
            }
            Value::Float(v) => {
                if !is_float {
                    is_float = true;
                    float_acc = int_acc as f64;
                }
                float_acc += v;
            }
            other => {
                return Err(format!(
                    "sum() expects numeric elements, got '{}'",
                    other.type_name()
                ));
            }
        }
    }
    Ok(if is_float {
        Value::Float(float_acc)
    } else {
        Value::Int(int_acc)
    })
}

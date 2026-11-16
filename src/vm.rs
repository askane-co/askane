use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use crate::bytecode::{Constant, FunctionProto, Op};
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
    pub max_instructions: Option<u64>,
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
            max_instructions: Some(10_000_000),
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
            max_instructions: None,
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

    pub fn check_net(&self, url: &str) -> Result<(), String> {
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
        let rest = url
            .strip_prefix("https://")
            .ok_or_else(|| "Only https:// URLs are allowed".to_string())?;
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let host_port = authority.rsplit('@').next().unwrap_or(authority);
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
        let raw = Path::new(input);
        let candidate = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.current_dir.join(raw)
        };
        if self.permissions.allow_all {
            return Ok(candidate);
        }
        // Reject Windows device names, alternate data streams and ambiguous names
        // before Win32 can reinterpret them. The drive prefix is handled separately.
        for component in raw.components() {
            if let std::path::Component::Normal(name) = component {
                let name = name.to_string_lossy();
                let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
                if name.contains(':')
                    || name.contains('\0')
                    || name.ends_with(['.', ' '])
                    || matches!(
                        stem.as_str(),
                        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
                    )
                    || (stem.len() == 4
                        && (stem.starts_with("COM") || stem.starts_with("LPT"))
                        && stem.as_bytes()[3].is_ascii_digit())
                {
                    return Err(
                        "path is outside the sandbox policy (device or ambiguous name)".to_string(),
                    );
                }
            }
        }
        if let Some(dirs) = extra_roots {
            if dirs.is_empty() {
                return Ok(candidate);
            }
        }
        let existing = if candidate.symlink_metadata().is_ok() {
            fs::canonicalize(&candidate).map_err(|e| format!("path '{}': {}", input, e))?
        } else {
            let parent = candidate
                .parent()
                .ok_or_else(|| "invalid path".to_string())?;
            let parent =
                fs::canonicalize(parent).map_err(|e| format!("path '{}': {}", input, e))?;
            parent.join(
                candidate
                    .file_name()
                    .ok_or_else(|| "invalid path".to_string())?,
            )
        };
        let root =
            fs::canonicalize(&self.sandbox_root).map_err(|e| format!("sandbox root: {}", e))?;
        if existing == root || existing.starts_with(&root) {
            return Ok(existing);
        }
        if let Some(dirs) = extra_roots {
            for dir in dirs {
                if let Ok(allowed_root) = fs::canonicalize(dir) {
                    if existing == allowed_root || existing.starts_with(&allowed_root) {
                        return Ok(existing);
                    }
                }
            }
        }
        Err(format!("path '{}' is outside the sandbox", input))
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
        if self.frames.len() >= MAX_CALL_DEPTH || self.loading_modules.len() > 32 {
            return Err("Maximum call/import depth exceeded".to_string());
        }
        if self.frames.is_empty() {
            self.instructions_left = self.permissions.max_instructions.unwrap_or(u64::MAX);
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
                self.execute(base_depth)
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
            match self.step(target_depth) {
                Ok(Some(val)) => return Ok(val),
                Ok(None) => {}
                Err(err) => {
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
                    (Value::Str(x), Value::Str(y)) => Value::Str(format!("{}{}", x, y)),
                    (Value::List(x), Value::List(y)) => {
                        let mut combined = x.borrow().clone();
                        combined.extend(y.borrow().iter().cloned());
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

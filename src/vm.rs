use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::bytecode::{Constant, FunctionProto, Op, verify_proto};
use crate::compiler::Compiler;
use crate::lexer::Lexer;
use crate::modules;
use crate::parser::Parser;
use crate::value::{
    AskaneSet, BoundMethod, Closure, ModuleData, NativeFunction, StructDef, StructInstance,
    UpvalueState, Value,
};

struct CallFrame {
    closure: Rc<Closure>,
    ip: usize,
    base: usize,
    arg_bits: [u64; 4],
    enter_step: u64,
    enter_time: Option<Instant>,
}

#[derive(Clone, Debug, Default)]
pub struct ProfileEntry {
    pub calls: u64,
    pub steps: u64,
    pub total_ns: u64,
}

fn make_positional_arg_bits(count: usize) -> [u64; 4] {
    let mut bits = [0u64; 4];
    if count < 64 {
        bits[0] = (1u64 << count) - 1;
        return bits;
    }
    for i in 0..count.min(256) {
        bits[i >> 6] |= 1u64 << (i & 63);
    }
    bits
}

fn arg_is_present(bits: &[u64; 4], idx: usize) -> bool {
    idx < 256 && ((bits[idx >> 6] >> (idx & 63)) & 1) != 0
}

fn set_arg_present(bits: &mut [u64; 4], idx: usize) {
    if idx < 256 {
        bits[idx >> 6] |= 1u64 << (idx & 63);
    }
}

#[derive(Clone)]
struct TryHandler {
    frame_depth: usize,
    stack_len: usize,
    cleanup_len: usize,
    catch_ip: usize,
}

#[derive(Clone)]
struct SandboxSnapshot {
    perms: Permissions,
    sandboxed: bool,
    saved_instr_extra: Option<u64>,
}

#[derive(Clone)]
enum CleanupKind {
    Defer(Rc<Closure>),
    Sandbox(SandboxSnapshot),
}

#[derive(Clone)]
struct CleanupEntry {
    frame_depth: usize,
    scope_depth: u16,
    kind: CleanupKind,
}

#[derive(Clone)]
pub(crate) enum YieldRequest {
    Yield,
    Sleep(u64),
    Join(u64),
    ChanSend { ch_id: u64, val: Value },
    ChanRecv { ch_id: u64 },
    SelectRecv { ch_ids: Vec<u64> },
}

#[derive(Clone)]
enum TaskWait {
    Ready,
    Sleeping(u64),
    Join(u64),
    ChanSend(u64),
    ChanRecv(u64),
    SelectRecv(Vec<u64>),
    Done,
}

struct GreenTask {
    stack: Vec<Value>,
    frames: Vec<CallFrame>,
    try_handlers: Vec<TryHandler>,
    cleanup_stack: Vec<CleanupEntry>,
    open_upvalues: Vec<Rc<RefCell<UpvalueState>>>,
    permissions: Permissions,
    sandboxed: bool,
    wait: TaskWait,
    wake_value: Option<Result<Value, String>>,
    result: Option<Result<Value, String>>,
}

struct ChannelState {
    capacity: usize,
    queue: VecDeque<Value>,
    closed: bool,
    send_waiters: VecDeque<(u64, Value)>,
    recv_waiters: VecDeque<u64>,
}

struct GreenScheduler {
    next_task_id: u64,
    next_chan_id: u64,
    current_task_id: u64,
    tasks: HashMap<u64, GreenTask>,
    ready_queue: VecDeque<u64>,
    join_waiters: HashMap<u64, Vec<u64>>,
    channels: HashMap<u64, ChannelState>,
    pending_yield: Option<YieldRequest>,
}

impl GreenScheduler {
    fn new() -> Self {
        Self {
            next_task_id: 1,
            next_chan_id: 1,
            current_task_id: 0,
            tasks: HashMap::new(),
            ready_queue: VecDeque::new(),
            join_waiters: HashMap::new(),
            channels: HashMap::new(),
            pending_yield: None,
        }
    }

    fn reset(&mut self) {
        self.next_task_id = 1;
        self.next_chan_id = 1;
        self.current_task_id = 0;
        self.tasks.clear();
        self.ready_queue.clear();
        self.join_waiters.clear();
        self.channels.clear();
        self.pending_yield = None;
    }
}

const MAX_CALL_DEPTH: usize = 1024;
const MAX_GREEN_TASKS: usize = 1024;
const MAX_CHANNELS: usize = 4096;

#[derive(Clone, Debug)]
pub struct Permissions {
    pub allow_all: bool,
    pub allow_net: Option<Vec<String>>,
    pub allow_read: Option<Vec<PathBuf>>,
    pub allow_write: Option<Vec<PathBuf>>,
    pub allow_env: Option<Vec<String>>,
    pub allow_sys_args: bool,
    pub allow_clipboard: bool,
    pub allow_input: bool,
    pub allow_exec: Option<Vec<String>>,
    pub max_instructions: Option<u64>,
    pub max_duration_ms: Option<u64>,
    pub max_memory_bytes: Option<u64>,
    pub max_io_bytes: Option<u64>,
    pub restrict_read_to_explicit_roots: bool,
    pub restrict_write_to_explicit_roots: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ResourceBudget {
    pub bytes_allocated: u64,
    pub io_bytes_used: u64,
    pub native_steps: u64,
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
            allow_input: false,
            allow_exec: None,
            max_instructions: Some(10_000_000),
            max_duration_ms: Some(30_000),
            max_memory_bytes: Some(64 * 1024 * 1024),
            max_io_bytes: Some(128 * 1024 * 1024),
            restrict_read_to_explicit_roots: false,
            restrict_write_to_explicit_roots: false,
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
            allow_input: true,
            allow_exec: Some(Vec::new()),
            max_instructions: None,
            max_duration_ms: None,
            max_memory_bytes: None,
            max_io_bytes: None,
            restrict_read_to_explicit_roots: false,
            restrict_write_to_explicit_roots: false,
        }
    }
}

pub struct VM {
    stack: Vec<Value>,
    frames: Vec<CallFrame>,
    global_slots: HashMap<String, usize>,
    global_values: Vec<Value>,
    global_ic: Vec<(usize, u64, usize, usize)>,
    attr_ic: Vec<(usize, u64, usize, u64, usize)>,
    module_ic: Vec<(usize, u64, usize, u64, Value)>,
    ic_epoch: usize,
    open_upvalues: Vec<Rc<RefCell<UpvalueState>>>,
    start_time: Instant,
    current_dir: PathBuf,
    sandbox_root: PathBuf,
    sandboxed: bool,
    permissions: Permissions,
    module_cache: HashMap<String, Value>,
    loading_modules: HashSet<String>,
    instructions_left: u64,
    step_counter: u64,
    budget: ResourceBudget,
    run_start: Instant,
    try_handlers: Vec<TryHandler>,
    cleanup_stack: Vec<CleanupEntry>,
    spec_results: Vec<(String, Result<(), String>)>,
    fmt_depth: usize,
    profile_enabled: bool,
    profile_stats: HashMap<String, ProfileEntry>,
    virtual_time_ms: Option<u64>,
    update_snapshots: bool,
    execute_depth: usize,
    scheduler: GreenScheduler,
}

impl Drop for VM {
    fn drop(&mut self) {
        modules::reset_runtime_state();
    }
}

impl VM {
    pub fn new() -> Self {
        Self::new_with_sandbox(true)
    }

    pub fn new_with_sandbox(sandboxed: bool) -> Self {
        let raw_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let current_dir = fs::canonicalize(&raw_dir).unwrap_or(raw_dir);
        let sandbox_root = current_dir.clone();
        let permissions = if sandboxed {
            Permissions::sandboxed()
        } else {
            Permissions::unrestricted()
        };
        let instructions_left = permissions.max_instructions.unwrap_or(u64::MAX);
        let mut vm = Self {
            stack: Vec::with_capacity(256),
            frames: Vec::with_capacity(64),
            global_slots: HashMap::new(),
            global_values: Vec::with_capacity(64),
            global_ic: vec![(0, 0, 0, usize::MAX); 1024],
            attr_ic: vec![(0, 0, 0, 0, 0); 1024],
            module_ic: vec![(0, 0, 0, 0, Value::Unit); 512],
            ic_epoch: 1,
            open_upvalues: Vec::new(),
            start_time: Instant::now(),
            current_dir,
            sandbox_root,
            sandboxed,
            permissions,
            module_cache: HashMap::new(),
            loading_modules: HashSet::new(),
            instructions_left,
            step_counter: 0,
            budget: ResourceBudget::default(),
            run_start: Instant::now(),
            try_handlers: Vec::new(),
            cleanup_stack: Vec::new(),
            spec_results: Vec::new(),
            fmt_depth: 0,
            profile_enabled: false,
            profile_stats: HashMap::new(),
            virtual_time_ms: None,
            update_snapshots: false,
            execute_depth: 0,
            scheduler: GreenScheduler::new(),
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
        self.current_dir = fs::canonicalize(&dir).unwrap_or(dir);
    }

    pub fn set_sandbox_root(&mut self, dir: PathBuf) {
        self.sandbox_root = fs::canonicalize(&dir).unwrap_or(dir);
    }

    pub fn is_sandboxed(&self) -> bool {
        self.sandboxed
    }

    pub fn spec_results(&self) -> &[(String, Result<(), String>)] {
        &self.spec_results
    }

    pub fn set_profile(&mut self, enabled: bool) {
        self.profile_enabled = enabled;
    }

    pub fn set_deterministic(&mut self, enabled: bool) {
        if enabled {
            self.virtual_time_ms = Some(0);
            crate::modules::random::set_deterministic_seed(0x5EED_0170_2026_0001);
        } else {
            self.virtual_time_ms = None;
        }
    }

    pub fn set_update_snapshots(&mut self, enabled: bool) {
        self.update_snapshots = enabled;
    }

    pub fn should_update_snapshots(&self) -> bool {
        self.update_snapshots
    }

    pub fn profile_report(&self) -> Vec<(String, ProfileEntry)> {
        let mut out: Vec<(String, ProfileEntry)> = self
            .profile_stats
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        out.sort_by(|a, b| {
            b.1.total_ns
                .cmp(&a.1.total_ns)
                .then(b.1.steps.cmp(&a.1.steps))
        });
        out
    }

    fn record_profile(&mut self, name: &str, enter_step: u64, enter_time: Option<Instant>) {
        if !self.profile_enabled {
            return;
        }
        let elapsed = enter_time
            .map(|t| t.elapsed().as_nanos().min(u64::MAX as u128) as u64)
            .unwrap_or(0);
        let steps = self.step_counter.saturating_sub(enter_step);
        let entry = self.profile_stats.entry(name.to_string()).or_default();
        entry.calls += 1;
        entry.steps = entry.steps.saturating_add(steps);
        entry.total_ns = entry.total_ns.saturating_add(elapsed);
    }

    pub fn elapsed_ms(&self) -> i64 {
        if let Some(vt) = self.virtual_time_ms {
            vt as i64
        } else {
            self.start_time.elapsed().as_millis() as i64
        }
    }

    pub fn now_unix_ms(&self) -> Result<i64, String> {
        if let Some(vt) = self.virtual_time_ms {
            return Ok(1_700_000_000_000i64.saturating_add(vt as i64));
        }
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis() as i64;
        Ok(ms)
    }

    fn current_clock_ms(&self) -> u64 {
        if let Some(vt) = self.virtual_time_ms {
            vt
        } else {
            self.run_start.elapsed().as_millis() as u64
        }
    }

    pub fn check_timeout(&self) -> Result<(), String> {
        if let Some(limit_ms) = self.permissions.max_duration_ms {
            if self.current_clock_ms() > limit_ms {
                return Err(format!("Execution timeout ({} ms exceeded)", limit_ms));
            }
        }
        Ok(())
    }

    pub fn charge_alloc(&mut self, bytes: usize) -> Result<(), String> {
        let next = self.budget.bytes_allocated.saturating_add(bytes as u64);
        if let Some(limit) = self.permissions.max_memory_bytes {
            if next > limit {
                return Err(format!(
                    "Sandbox memory budget exceeded (limit {} bytes)",
                    limit
                ));
            }
        }
        self.budget.bytes_allocated = next;
        Ok(())
    }

    pub fn charge_io(&mut self, bytes: usize) -> Result<(), String> {
        let next = self.budget.io_bytes_used.saturating_add(bytes as u64);
        if let Some(limit) = self.permissions.max_io_bytes {
            if next > limit {
                return Err(format!(
                    "Sandbox I/O budget exceeded (limit {} bytes)",
                    limit
                ));
            }
        }
        self.budget.io_bytes_used = next;
        Ok(())
    }

    pub fn charge_work(&mut self, steps: u64) -> Result<(), String> {
        self.budget.native_steps = self.budget.native_steps.saturating_add(steps);
        if self.permissions.max_instructions.is_some() {
            if self.instructions_left < steps {
                self.instructions_left = 0;
                return Err("Sandbox instruction limit exceeded".to_string());
            }
            self.instructions_left -= steps;
        }
        self.check_timeout()
    }

    pub fn budget_snapshot(&self) -> (u64, Option<u64>, u64, Option<u64>, Option<u64>) {
        (
            self.budget.bytes_allocated,
            self.permissions.max_memory_bytes,
            self.budget.io_bytes_used,
            self.permissions.max_io_bytes,
            self.permissions
                .max_instructions
                .map(|_| self.instructions_left),
        )
    }

    pub fn is_unrecoverable_error(err: &str) -> bool {
        err.contains("Sandbox instruction limit exceeded")
            || err.contains("Sandbox memory budget exceeded")
            || err.contains("Sandbox I/O budget exceeded")
            || err.contains("Execution timeout (")
    }

    pub fn sleep_bounded(&mut self, ms: u64) -> Result<(), String> {
        self.check_timeout()?;
        if self.virtual_time_ms.is_none() && self.is_sandboxed() && ms > 1000 {
            return Err("Sandbox sleep limit is 1000 ms per call".to_string());
        }
        if let Some(vt) = self.virtual_time_ms.as_mut() {
            let next = vt.saturating_add(ms);
            if let Some(limit_ms) = self.permissions.max_duration_ms {
                if next > limit_ms {
                    *vt = limit_ms;
                    return Err(format!("Execution timeout ({} ms exceeded)", limit_ms));
                }
            }
            *vt = next;
            return Ok(());
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

    pub fn check_host_port(&self, authority: &str) -> Result<(), String> {
        if authority.is_empty()
            || authority.len() > 1024
            || authority.contains('@')
            || authority
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || c == '\\' || c == '/')
        {
            return Err("Invalid or disallowed network address".to_string());
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
        let host_only = if authority.starts_with('[') {
            authority
                .find(']')
                .map(|i| &authority[..=i])
                .unwrap_or(authority)
        } else {
            authority.split(':').next().unwrap_or(authority)
        };
        if allowed
            .iter()
            .any(|p| p.eq_ignore_ascii_case(authority) || p.eq_ignore_ascii_case(host_only))
        {
            Ok(())
        } else {
            Err(format!(
                "Network access to '{}' is not allowed (pass --allow-net={})",
                authority, authority
            ))
        }
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
        self.check_host_port(authority)
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

    pub fn check_input(&self) -> Result<(), String> {
        if self.permissions.allow_all || self.permissions.allow_input {
            Ok(())
        } else {
            Err(
                "global input access is disabled in sandbox mode (pass --allow-input or --unsafe)"
                    .to_string(),
            )
        }
    }

    pub fn sleep_checked(&mut self, ms: u64) -> Result<(), String> {
        if let Some(vt) = self.virtual_time_ms.as_mut() {
            let next = vt.saturating_add(ms);
            if let Some(limit_ms) = self.permissions.max_duration_ms {
                if next > limit_ms {
                    *vt = limit_ms;
                    return Err(format!("Execution timeout ({} ms exceeded)", limit_ms));
                }
            }
            *vt = next;
            return Ok(());
        }
        let mut rem = ms;
        while rem > 0 {
            self.check_timeout()?;
            let slice = rem.min(50);
            if let Some(limit_ms) = self.permissions.max_duration_ms {
                let elapsed = self.run_start.elapsed().as_millis() as u64;
                if elapsed.saturating_add(slice) > limit_ms {
                    let left = limit_ms.saturating_sub(elapsed);
                    if left > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(left));
                    }
                    return Err(format!("Execution timeout ({} ms exceeded)", limit_ms));
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(slice));
            rem -= slice;
        }
        self.check_timeout()
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
        self.resolve_path_with_roots_from(
            &self.current_dir,
            input,
            &self.permissions.allow_read,
            self.permissions.restrict_read_to_explicit_roots,
            false,
        )
    }

    pub fn resolve_read_path_from(&self, base_dir: &Path, input: &str) -> Result<PathBuf, String> {
        self.resolve_path_with_roots_from(
            base_dir,
            input,
            &self.permissions.allow_read,
            self.permissions.restrict_read_to_explicit_roots,
            false,
        )
    }

    pub fn resolve_write_path(&self, input: &str) -> Result<PathBuf, String> {
        self.resolve_path_with_roots_from(
            &self.current_dir,
            input,
            &self.permissions.allow_write,
            self.permissions.restrict_write_to_explicit_roots,
            false,
        )
    }

    pub fn resolve_write_link_or_path(&self, input: &str) -> Result<PathBuf, String> {
        self.resolve_path_with_roots_from(
            &self.current_dir,
            input,
            &self.permissions.allow_write,
            self.permissions.restrict_write_to_explicit_roots,
            true,
        )
    }

    fn canonicalize_existing_prefix(path: &Path) -> Result<PathBuf, String> {
        let mut cur = path;
        let mut tail_parts = Vec::new();
        let canonical_base = loop {
            if cur.symlink_metadata().is_ok() {
                break fs::canonicalize(cur).map_err(|e| e.to_string())?;
            }
            let Some(name) = cur.file_name() else {
                return Err("path has no existing ancestor".to_string());
            };
            tail_parts.push(name.to_os_string());
            let Some(parent) = cur.parent() else {
                return Err("path has no existing ancestor".to_string());
            };
            cur = parent;
        };
        let mut existing = canonical_base;
        for part in tail_parts.into_iter().rev() {
            existing.push(part);
        }
        Ok(existing)
    }

    fn resolve_path_with_roots_from(
        &self,
        base_dir: &Path,
        input: &str,
        extra_roots: &Option<Vec<PathBuf>>,
        restrict_explicit: bool,
        preserve_leaf_symlink: bool,
    ) -> Result<PathBuf, String> {
        if input.is_empty() || input.contains('\0') {
            return Err("invalid path".to_string());
        }
        let raw = Path::new(input);
        let candidate = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            base_dir.join(raw)
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
        let normalize_lexical = |p: &Path, base: &Path| -> Option<Vec<String>> {
            let full = if p.is_absolute() {
                p.to_path_buf()
            } else {
                base.join(p)
            };
            let mut out: Vec<String> = Vec::new();
            for comp in full.components() {
                match comp {
                    std::path::Component::CurDir => {}
                    std::path::Component::ParentDir => {
                        if out.is_empty() {
                            return None;
                        }
                        if out.last().is_some_and(|s| s == "/" || s.ends_with(':')) {
                            return None;
                        }
                        out.pop();
                    }
                    std::path::Component::Prefix(prefix) => {
                        let key = match prefix.kind() {
                            std::path::Prefix::Disk(d) | std::path::Prefix::VerbatimDisk(d) => {
                                format!("{}:", (d as char).to_ascii_lowercase())
                            }
                            _ => prefix.as_os_str().to_string_lossy().into_owned(),
                        };
                        out.push(key);
                    }
                    std::path::Component::RootDir => {
                        out.push("/".to_string());
                    }
                    std::path::Component::Normal(s) => {
                        let text = s.to_string_lossy();
                        #[cfg(windows)]
                        out.push(text.to_lowercase());
                        #[cfg(not(windows))]
                        out.push(text.into_owned());
                    }
                }
            }
            Some(out)
        };

        if restrict_explicit && extra_roots.is_none() {
            return Err("filesystem access is disabled inside this sandbox block".to_string());
        }
        let Some(lexical) = normalize_lexical(raw, base_dir) else {
            return Err(format!("path '{}' is outside the sandbox", input));
        };
        let mut lexical_roots = Vec::new();
        if !restrict_explicit {
            if let Some(r) = normalize_lexical(&self.sandbox_root, base_dir) {
                lexical_roots.push(r);
            }
            if let Ok(canon_root) = fs::canonicalize(&self.sandbox_root) {
                if let Some(r) = normalize_lexical(&canon_root, &self.current_dir) {
                    lexical_roots.push(r);
                }
            }
        }
        if let Some(dirs) = extra_roots {
            for dir in dirs {
                if let Some(r) = normalize_lexical(dir, base_dir) {
                    lexical_roots.push(r);
                }
                let abs = if dir.is_absolute() {
                    dir.to_path_buf()
                } else {
                    self.current_dir.join(dir)
                };
                if let Ok(allowed_root) = Self::canonicalize_existing_prefix(&abs) {
                    if let Some(r) = normalize_lexical(&allowed_root, &self.current_dir) {
                        lexical_roots.push(r);
                    }
                }
            }
        }
        if !lexical_roots.iter().any(|r| lexical.starts_with(r)) {
            return Err(format!("path '{}' is outside the sandbox", input));
        }

        let mut allowed_roots = Vec::new();
        if !restrict_explicit {
            let root =
                fs::canonicalize(&self.sandbox_root).map_err(|e| format!("sandbox root: {}", e))?;
            if let Some(r) = normalize_lexical(&root, &self.current_dir) {
                allowed_roots.push(r);
            }
        }
        if let Some(dirs) = extra_roots {
            for dir in dirs {
                let abs = if dir.is_absolute() {
                    dir.to_path_buf()
                } else {
                    self.current_dir.join(dir)
                };
                if let Ok(allowed_root) = Self::canonicalize_existing_prefix(&abs) {
                    if let Some(r) = normalize_lexical(&allowed_root, &self.current_dir) {
                        allowed_roots.push(r);
                    }
                }
            }
        }

        if preserve_leaf_symlink
            && candidate
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
        {
            let Some(parent) = candidate.parent() else {
                return Err(format!("path '{}' is outside the sandbox", input));
            };
            let Some(name) = candidate.file_name() else {
                return Err(format!("path '{}' is outside the sandbox", input));
            };
            let parent_path = if parent.as_os_str().is_empty() {
                self.current_dir.as_path()
            } else {
                parent
            };
            let canon_parent = Self::canonicalize_existing_prefix(parent_path)
                .map_err(|e| format!("path '{}': {}", input, e))?;
            let Some(parent_lex) = normalize_lexical(&canon_parent, &self.current_dir) else {
                return Err(format!("path '{}' is outside the sandbox", input));
            };
            if allowed_roots.iter().any(|r| parent_lex.starts_with(r)) {
                return Ok(canon_parent.join(name));
            }
            return Err(format!("path '{}' is outside the sandbox", input));
        }

        // Canonicalize the deepest existing ancestor to catch symlinks/junctions inside the root.
        let existing = Self::canonicalize_existing_prefix(&candidate).map_err(|e| {
            if e == "path has no existing ancestor" {
                format!("path '{}' is outside the sandbox", input)
            } else {
                format!("path '{}': {}", input, e)
            }
        })?;
        let Some(existing_lex) = normalize_lexical(&existing, &self.current_dir) else {
            return Err(format!("path '{}' is outside the sandbox", input));
        };
        if allowed_roots.iter().any(|r| existing_lex.starts_with(r)) {
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
            ("bytes", native_bytes),
            ("set", native_set),
            ("dict", native_dict),
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
            ("__pipe_map", native_map),
            ("__tag_eval", native_tag_eval),
            ("__check_needs", native_check_needs),
            ("__close_resource", native_close_resource),
            ("__sleep_checked", native_sleep_checked),
            ("__run_spec", native_run_spec),
            ("__record_spec", native_record_spec),
            ("unique", native_unique),
            ("flatten", native_flatten),
            ("chunk", native_chunk),
            ("sum", native_sum),
            ("has", native_has),
            ("is_main", native_is_main),
            ("read_line", native_read_line),
            ("clock", native_clock),
        ];

        for &(name, func) in natives {
            let val = Value::NativeFn(NativeFunction { name, func });
            if let Some(&slot) = self.global_slots.get(name) {
                self.global_values[slot] = val;
            } else {
                let slot = self.global_values.len();
                self.global_values.push(val);
                self.global_slots.insert(name.to_string(), slot);
            }
        }
    }

    pub fn run(&mut self, proto: Rc<FunctionProto>) -> Result<Value, String> {
        let base_dir = self.current_dir.clone();
        self.run_with_base_dir(proto, Some(base_dir))
    }

    pub fn run_with_base_dir(
        &mut self,
        proto: Rc<FunctionProto>,
        base_dir: Option<PathBuf>,
    ) -> Result<Value, String> {
        verify_proto(&proto)?;
        if self.frames.len() >= MAX_CALL_DEPTH || self.loading_modules.len() > 32 {
            return Err("Maximum call/import depth exceeded".to_string());
        }
        if self.frames.is_empty() {
            self.instructions_left = self.permissions.max_instructions.unwrap_or(u64::MAX);
            self.step_counter = 0;
            self.budget = ResourceBudget::default();
            self.run_start = Instant::now();
            if self.virtual_time_ms.is_some() {
                self.virtual_time_ms = Some(0);
            }
            self.try_handlers.clear();
            self.cleanup_stack.clear();
            self.spec_results.clear();
            self.fmt_depth = 0;
            self.scheduler.reset();
            self.ic_epoch = self.ic_epoch.wrapping_add(1).max(1);
            if self.profile_enabled {
                self.profile_stats.clear();
            }
        }
        let closure = Rc::new(Closure {
            proto,
            upvalues: Vec::new(),
            base_dir,
        });
        let base_depth = self.frames.len();
        let stack_base = self.stack.len();
        let prev_cleanup_len = self.cleanup_stack.len();
        self.stack.push(Value::Closure(closure.clone()));
        let enter_time = if self.profile_enabled {
            Some(Instant::now())
        } else {
            None
        };
        let enter_step = self.step_counter;
        self.frames.push(CallFrame {
            closure,
            ip: 0,
            base: stack_base,
            arg_bits: [0; 4],
            enter_step,
            enter_time,
        });

        match self.execute(base_depth) {
            Ok(v) => Ok(v),
            Err(e) => {
                let run_defers = !Self::is_unrecoverable_error(&e);
                let _ = self.unwind_cleanups_to_len(prev_cleanup_len, run_defers);
                let msg = if base_depth == 0 {
                    self.format_stack_trace(&e)
                } else {
                    e
                };
                if !self.open_upvalues.is_empty() {
                    self.close_upvalues_from(stack_base);
                }
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

    fn build_closure_slots(
        &mut self,
        closure: &Closure,
        pos_args: &[Value],
        kw: Option<(&[String], &[Value])>,
    ) -> Result<(Vec<Value>, [u64; 4]), String> {
        let total_arity = closure.proto.arity as usize;
        let is_variadic = closure.proto.is_variadic;
        let fixed = if is_variadic {
            total_arity.saturating_sub(1)
        } else {
            total_arity
        };

        if !is_variadic && pos_args.len() > fixed {
            return Err(format!(
                "Expected at most {} arguments to '{}', got {}",
                fixed,
                closure.proto.name,
                pos_args.len()
            ));
        }

        let mut slots = vec![Value::Unit; total_arity];
        let mut bits = [0u64; 4];
        for (i, v) in pos_args.iter().take(fixed).enumerate() {
            slots[i] = v.clone();
            set_arg_present(&mut bits, i);
        }
        if is_variadic {
            let rest = if pos_args.len() > fixed {
                let extra = &pos_args[fixed..];
                self.charge_alloc(extra.len().saturating_mul(std::mem::size_of::<Value>()))?;
                extra.to_vec()
            } else {
                Vec::new()
            };
            slots[fixed] = Value::List(Rc::new(RefCell::new(rest)));
            set_arg_present(&mut bits, fixed);
        }

        if let Some((kw_names, kw_vals)) = kw {
            for (kname, kval) in kw_names.iter().zip(kw_vals.iter()) {
                let Some(idx) = closure.proto.param_names[..fixed]
                    .iter()
                    .position(|p| p == kname)
                else {
                    return Err(format!(
                        "Function '{}' got unexpected keyword argument '{}'",
                        closure.proto.name, kname
                    ));
                };
                if arg_is_present(&bits, idx) {
                    return Err(format!(
                        "Function '{}' got multiple values for argument '{}'",
                        closure.proto.name, kname
                    ));
                }
                slots[idx] = kval.clone();
                set_arg_present(&mut bits, idx);
            }
        }

        for i in 0..closure.proto.min_arity as usize {
            if !arg_is_present(&bits, i) {
                let pname = closure
                    .proto
                    .param_names
                    .get(i)
                    .map(String::as_str)
                    .unwrap_or("?");
                return Err(format!(
                    "Missing required argument '{}' in call to '{}'",
                    pname, closure.proto.name
                ));
            }
        }

        Ok((slots, bits))
    }

    fn instantiate_struct(
        &mut self,
        def: &Rc<StructDef>,
        spread: Option<Value>,
        field_names: &[String],
        field_vals: &[Value],
    ) -> Result<Value, String> {
        let base_vals = match spread {
            Some(Value::StructInstance(inst)) => {
                if !Rc::ptr_eq(&inst.def, def) {
                    return Err(format!(
                        "Struct spread '..' expects '{}', got '{}'",
                        def.name, inst.def.name
                    ));
                }
                Some(inst.fields.borrow().clone())
            }
            Some(other) => {
                return Err(format!(
                    "Struct spread '..' expects '{}', got '{}'",
                    def.name,
                    other.type_name()
                ));
            }
            None => None,
        };

        let mut provided: Vec<Option<Value>> = vec![None; def.field_names.len()];
        for (fname, fval) in field_names.iter().zip(field_vals.iter()) {
            let Some(&idx) = def.field_index.get(fname) else {
                return Err(format!("Struct '{}' has no field '{}'", def.name, fname));
            };
            if provided[idx].is_some() {
                return Err(format!(
                    "Multiple values provided for field '{}' of struct '{}'",
                    fname, def.name
                ));
            }
            provided[idx] = Some(fval.clone());
        }

        let mut out = Vec::with_capacity(def.field_names.len());
        for i in 0..def.field_names.len() {
            let v = if let Some(val) = provided[i].take() {
                val
            } else if let Some(ref b) = base_vals {
                b[i].clone()
            } else if let Some(ref default_fn) = def.field_defaults[i] {
                self.call_value(default_fn.clone(), &[])?
            } else {
                return Err(format!(
                    "Missing required field '{}' for struct '{}'",
                    def.field_names[i], def.name
                ));
            };
            out.push(v);
        }
        self.charge_alloc(
            out.len()
                .saturating_mul(std::mem::size_of::<Value>())
                .max(16),
        )?;
        Ok(Value::StructInstance(Rc::new(StructInstance {
            def: def.clone(),
            fields: RefCell::new(out),
        })))
    }

    fn format_value_protocol(&mut self, val: &Value) -> Result<String, String> {
        if self.fmt_depth < 16 {
            if let Value::StructInstance(inst) = val {
                if let Some(fmt_fn) = inst.def.methods.get("fmt").cloned() {
                    self.fmt_depth += 1;
                    let res = self.call_value(fmt_fn, &[val.clone()]);
                    self.fmt_depth -= 1;
                    let out = res?;
                    return Ok(match out {
                        Value::Str(s) => s,
                        other => format!("{}", other),
                    });
                }
            }
        }
        Ok(format!("{}", val))
    }

    fn get_attr_value(
        &self,
        obj: Value,
        attr: &str,
        line: usize,
        op_sym: &str,
    ) -> Result<Value, String> {
        match &obj {
            Value::Module(m) => m.exports.borrow().get(attr).cloned().ok_or_else(|| {
                format!(
                    "[line {}] Module '{}' has no export '{}'",
                    line, m.name, attr
                )
            }),
            Value::StructDef(def) => def.methods.get(attr).cloned().ok_or_else(|| {
                format!(
                    "[line {}] Struct '{}' has no static member '{}'",
                    line, def.name, attr
                )
            }),
            Value::StructInstance(inst) => {
                if let Some(&field_idx) = inst.def.field_index.get(attr) {
                    return Ok(inst.fields.borrow()[field_idx].clone());
                }
                if let Some(m) = inst.def.methods.get(attr) {
                    return Ok(Value::BoundMethod(Rc::new(BoundMethod {
                        receiver: obj.clone(),
                        func: m.clone(),
                    })));
                }
                if attr == "to_dict" {
                    return Ok(Value::BoundMethod(Rc::new(BoundMethod {
                        receiver: obj.clone(),
                        func: Value::NativeFn(NativeFunction {
                            name: "struct.to_dict",
                            func: method_struct_to_dict,
                        }),
                    })));
                }
                Err(format!(
                    "[line {}] Struct '{}' has no field or method '{}'",
                    line, inst.def.name, attr
                ))
            }
            Value::Dict(dict) => {
                if let Some(val) = dict.borrow().get(attr).cloned() {
                    return Ok(val);
                }
                if let Some((name, func)) = resolve_builtin_method(&obj, attr) {
                    return Ok(Value::BoundMethod(Rc::new(BoundMethod {
                        receiver: obj,
                        func: Value::NativeFn(NativeFunction { name, func }),
                    })));
                }
                Ok(Value::Unit)
            }
            Value::Str(_) | Value::List(_) | Value::Set(_) | Value::Bytes(_) => {
                if let Some((name, func)) = resolve_builtin_method(&obj, attr) {
                    Ok(Value::BoundMethod(Rc::new(BoundMethod {
                        receiver: obj,
                        func: Value::NativeFn(NativeFunction { name, func }),
                    })))
                } else {
                    Err(format!(
                        "[line {}] Type '{}' has no method '{}'",
                        line,
                        obj.type_name(),
                        attr
                    ))
                }
            }
            other => Err(format!(
                "[line {}] Cannot access '{}{}' on '{}'",
                line,
                op_sym,
                attr,
                other.type_name()
            )),
        }
    }

    pub fn call_value(&mut self, callee: Value, args: &[Value]) -> Result<Value, String> {
        match callee {
            Value::NativeFn(native) => {
                let enter_step = self.step_counter;
                let enter_time = if self.profile_enabled {
                    Some(Instant::now())
                } else {
                    None
                };
                let res = (native.func)(self, args);
                self.record_profile(native.name, enter_step, enter_time);
                res
            }
            Value::BoundMethod(m) => {
                let mut combined = Vec::with_capacity(1 + args.len());
                combined.push(m.receiver.clone());
                combined.extend_from_slice(args);
                self.call_value(m.func.clone(), &combined)
            }
            Value::StructDef(def) => {
                if args.len() > def.field_names.len() {
                    return Err(format!(
                        "Struct '{}' takes at most {} positional arguments, got {}",
                        def.name,
                        def.field_names.len(),
                        args.len()
                    ));
                }
                let names = &def.field_names[..args.len()];
                self.instantiate_struct(&def, None, names, args)
            }
            Value::Closure(closure) => {
                if self.frames.len() >= MAX_CALL_DEPTH {
                    return Err(format!(
                        "Stack overflow (maximum call depth {} exceeded)",
                        MAX_CALL_DEPTH
                    ));
                }
                let base_depth = self.frames.len();
                let base = self.stack.len();
                let prev_cleanup_len = self.cleanup_stack.len();
                let (slots, bits) =
                    if args.len() == closure.proto.arity as usize && !closure.proto.is_variadic {
                        (args.to_vec(), make_positional_arg_bits(args.len()))
                    } else {
                        self.build_closure_slots(&closure, args, None)?
                    };
                self.stack.push(Value::Closure(closure.clone()));
                self.stack.extend(slots);
                let enter_step = self.step_counter;
                let enter_time = if self.profile_enabled {
                    Some(Instant::now())
                } else {
                    None
                };
                self.frames.push(CallFrame {
                    closure,
                    ip: 0,
                    base,
                    arg_bits: bits,
                    enter_step,
                    enter_time,
                });
                match self.execute(base_depth) {
                    Ok(v) => Ok(v),
                    Err(e) => {
                        let run_defers = !Self::is_unrecoverable_error(&e);
                        let _ = self.unwind_cleanups_to_len(prev_cleanup_len, run_defers);
                        if !self.open_upvalues.is_empty() {
                            self.close_upvalues_from(base);
                        }
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
        self.execute_depth += 1;
        let res = 'exec: loop {
            match self.execute_fast(target_depth) {
                Ok(val) => break 'exec Ok(val),
                Err(mut err) => 'dispatch_err: loop {
                    if Self::is_unrecoverable_error(&err)
                        || err == "Deadlock: all tasks are blocked"
                    {
                        break 'exec Err(err);
                    }
                    if let Some(handler) = self.try_handlers.last().cloned() {
                        if handler.frame_depth > target_depth {
                            self.try_handlers.pop();
                            if self.cleanup_stack.len() > handler.cleanup_len {
                                if let Err(cleanup_err) =
                                    self.unwind_cleanups_to_len(handler.cleanup_len, true)
                                {
                                    if Self::is_unrecoverable_error(&cleanup_err) {
                                        break 'exec Err(cleanup_err);
                                    }
                                    err = cleanup_err;
                                }
                            }
                            self.frames.truncate(handler.frame_depth);
                            if !self.open_upvalues.is_empty() {
                                self.close_upvalues_from(handler.stack_len);
                            }
                            self.stack.truncate(handler.stack_len);
                            self.stack.push(Value::Str(err));
                            self.frames.last_mut().unwrap().ip = handler.catch_ip;
                            break 'dispatch_err;
                        }
                    }
                    if target_depth == 0 && self.scheduler.current_task_id != 0 {
                        let _ = self.unwind_cleanups_to_len(0, true);
                        match self.finish_current_green_task(Err(err)) {
                            Ok(()) => break 'dispatch_err,
                            Err(sched_err) => {
                                err = sched_err;
                                continue 'dispatch_err;
                            }
                        }
                    }
                    break 'exec Err(err);
                },
            }
        };
        self.execute_depth -= 1;
        res
    }

    #[inline(always)]
    fn check_budget_step(&mut self) -> Result<(), String> {
        self.step_counter = self.step_counter.wrapping_add(1);
        if self.permissions.max_instructions.is_some() {
            if self.instructions_left == 0 {
                return Err("Sandbox instruction limit exceeded".to_string());
            }
            self.instructions_left -= 1;
        }
        if let Some(limit_ms) = self.permissions.max_duration_ms {
            if (self.step_counter & 4095) == 0 && self.current_clock_ms() > limit_ms {
                return Err(format!("Execution timeout ({} ms exceeded)", limit_ms));
            }
        }
        Ok(())
    }

    fn execute_fast(&mut self, target_depth: usize) -> Result<Value, String> {
        let mut cur_closure;
        let mut cur_base;
        let mut ip;
        let mut proto_id: u64;
        {
            let f = self.frames.last().unwrap();
            cur_closure = f.closure.clone();
            cur_base = f.base;
            ip = f.ip;
            proto_id = cur_closure.proto.ic_id;
        }

        loop {
            self.check_budget_step()?;
            let chunk = &cur_closure.proto.chunk;
            let op_ip = ip;
            let op = chunk.code[op_ip];
            ip += 1;

            match op {
                Op::Constant(idx) => {
                    let val = match &chunk.constants[idx as usize] {
                        Constant::Int(v) => Value::Int(*v),
                        Constant::Float(v) => Value::Float(*v),
                        Constant::Str(s) => Value::Str(s.clone()),
                        Constant::Bytes(b) => Value::Bytes(Rc::new(b.clone())),
                        Constant::Function(_) | Constant::Struct(_) => {
                            unreachable!(
                                "Function/struct constant cannot be loaded via Op::Constant"
                            )
                        }
                    };
                    self.stack.push(val);
                }
                Op::ConstInt(imm) => self.stack.push(Value::Int(imm as i64)),
                Op::Unit => self.stack.push(Value::Unit),
                Op::True => self.stack.push(Value::Bool(true)),
                Op::False => self.stack.push(Value::Bool(false)),
                Op::Pop => {
                    self.stack.pop();
                }
                Op::Dup => {
                    let v = self.stack.last().unwrap().clone();
                    self.stack.push(v);
                }
                Op::Dup2 => {
                    let len = self.stack.len();
                    let a = self.stack[len - 2].clone();
                    let b = self.stack[len - 1].clone();
                    self.stack.push(a);
                    self.stack.push(b);
                }
                Op::GetLocal(slot) => {
                    let val = self.stack[cur_base + slot as usize].clone();
                    self.stack.push(val);
                }
                Op::SetLocal(slot) => {
                    let val = self.stack.last().unwrap().clone();
                    self.stack[cur_base + slot as usize] = val;
                }
                Op::AddLocalInt(slot, imm) => {
                    let imm64 = imm as i64;
                    if let Value::Int(x) = &self.stack[cur_base + slot as usize] {
                        self.stack.push(Value::Int(x.wrapping_add(imm64)));
                    } else {
                        let a = self.stack[cur_base + slot as usize].clone();
                        let line = chunk.lines[op_ip];
                        match a {
                            Value::Float(x) => self.stack.push(Value::Float(x + (imm64 as f64))),
                            other => {
                                return Err(format!(
                                    "[line {}] Cannot add '{}' and 'int'",
                                    line,
                                    other.type_name()
                                ));
                            }
                        }
                    }
                }
                Op::SubLocalInt(slot, imm) => {
                    let imm64 = imm as i64;
                    if let Value::Int(x) = &self.stack[cur_base + slot as usize] {
                        self.stack.push(Value::Int(x.wrapping_sub(imm64)));
                    } else {
                        let a = self.stack[cur_base + slot as usize].clone();
                        let line = chunk.lines[op_ip];
                        match a {
                            Value::Float(x) => self.stack.push(Value::Float(x - (imm64 as f64))),
                            other => {
                                return Err(format!(
                                    "[line {}] Cannot subtract '{}' and 'int'",
                                    line,
                                    other.type_name()
                                ));
                            }
                        }
                    }
                }
                Op::LtLocalInt(slot, imm) => {
                    let imm64 = imm as i64;
                    if let Value::Int(x) = &self.stack[cur_base + slot as usize] {
                        self.stack.push(Value::Bool(*x < imm64));
                    } else {
                        let a = &self.stack[cur_base + slot as usize];
                        let line = chunk.lines[op_ip];
                        match a {
                            Value::Float(x) => self.stack.push(Value::Bool(*x < (imm64 as f64))),
                            other => {
                                return Err(format!(
                                    "[line {}] Cannot compare '{}' and 'int'",
                                    line,
                                    other.type_name()
                                ));
                            }
                        }
                    }
                }
                Op::LeLocalInt(slot, imm) => {
                    let imm64 = imm as i64;
                    if let Value::Int(x) = &self.stack[cur_base + slot as usize] {
                        self.stack.push(Value::Bool(*x <= imm64));
                    } else {
                        let a = &self.stack[cur_base + slot as usize];
                        let line = chunk.lines[op_ip];
                        match a {
                            Value::Float(x) => self.stack.push(Value::Bool(*x <= (imm64 as f64))),
                            other => {
                                return Err(format!(
                                    "[line {}] Cannot compare '{}' and 'int'",
                                    line,
                                    other.type_name()
                                ));
                            }
                        }
                    }
                }
                Op::AddLocals(a, b) => {
                    let va = &self.stack[cur_base + a as usize];
                    let vb = &self.stack[cur_base + b as usize];
                    if let (Value::Int(x), Value::Int(y)) = (va, vb) {
                        self.stack.push(Value::Int(x.wrapping_add(*y)));
                    } else {
                        let lhs = va.clone();
                        let rhs = vb.clone();
                        self.stack.push(lhs);
                        self.stack.push(rhs);
                        self.execute_add(chunk.lines[op_ip])?;
                    }
                }
                Op::SubLocals(a, b) => {
                    let va = &self.stack[cur_base + a as usize];
                    let vb = &self.stack[cur_base + b as usize];
                    if let (Value::Int(x), Value::Int(y)) = (va, vb) {
                        self.stack.push(Value::Int(x.wrapping_sub(*y)));
                    } else {
                        let lhs = va.clone();
                        let rhs = vb.clone();
                        self.stack.push(lhs);
                        self.stack.push(rhs);
                        let line = chunk.lines[op_ip];
                        self.numeric_op(line, i64::wrapping_sub, |x, y| x - y, "subtract")?;
                    }
                }
                Op::LtLocals(a, b) => {
                    let va = &self.stack[cur_base + a as usize];
                    let vb = &self.stack[cur_base + b as usize];
                    if let (Value::Int(x), Value::Int(y)) = (va, vb) {
                        self.stack.push(Value::Bool(*x < *y));
                    } else {
                        let lhs = va.clone();
                        let rhs = vb.clone();
                        self.stack.push(lhs);
                        self.stack.push(rhs);
                        let line = chunk.lines[op_ip];
                        self.cmp_op(line, |x, y| x < y, |x, y| x < y, |x, y| x < y)?;
                    }
                }
                Op::IncLocalPop(slot, delta) => {
                    let pos = cur_base + slot as usize;
                    let delta64 = delta as i64;
                    match &mut self.stack[pos] {
                        Value::Int(x) => *x = x.wrapping_add(delta64),
                        Value::Float(x) => *x += delta64 as f64,
                        other => {
                            let line = chunk.lines[op_ip];
                            return Err(format!(
                                "[line {}] Cannot add '{}' and 'int'",
                                line,
                                other.type_name()
                            ));
                        }
                    }
                }
                Op::GetUpvalue(slot) => {
                    let uv = &cur_closure.upvalues[slot as usize];
                    let val = match &*uv.borrow() {
                        UpvalueState::Open(idx) => self.stack[*idx].clone(),
                        UpvalueState::Closed(v) => v.clone(),
                    };
                    self.stack.push(val);
                }
                Op::SetUpvalue(slot) => {
                    let uv = &cur_closure.upvalues[slot as usize];
                    let val = self.stack.last().unwrap().clone();
                    let mut state = uv.borrow_mut();
                    match &mut *state {
                        UpvalueState::Open(idx) => self.stack[*idx] = val,
                        UpvalueState::Closed(v) => *v = val,
                    }
                }
                Op::CloseUpvalue => {
                    let slot = self.stack.len() - 1;
                    if !self.open_upvalues.is_empty() {
                        self.close_upvalues_from(slot);
                    }
                    self.stack.pop();
                }
                Op::GetGlobal(idx) => {
                    let ic_idx = ((proto_id as usize).wrapping_mul(31) ^ op_ip) & 1023;
                    let cached = self.global_ic[ic_idx];
                    if cached.0 == self.ic_epoch
                        && cached.1 == proto_id
                        && cached.2 == op_ip
                        && cached.3 < self.global_values.len()
                    {
                        self.stack.push(self.global_values[cached.3].clone());
                    } else {
                        let name = match &chunk.constants[idx as usize] {
                            Constant::Str(s) => s.as_str(),
                            _ => unreachable!(),
                        };
                        let line = chunk.lines[op_ip];
                        let slot = *self.global_slots.get(name).ok_or_else(|| {
                            format!("[line {}] Undefined variable '{}'", line, name)
                        })?;
                        self.global_ic[ic_idx] = (self.ic_epoch, proto_id, op_ip, slot);
                        self.stack.push(self.global_values[slot].clone());
                    }
                }
                Op::DefineGlobal(idx) => {
                    let val = self.stack.pop().unwrap();
                    let name = match &chunk.constants[idx as usize] {
                        Constant::Str(s) => s.as_str(),
                        _ => unreachable!(),
                    };
                    if let Some(&slot) = self.global_slots.get(name) {
                        self.global_values[slot] = val;
                    } else {
                        let slot = self.global_values.len();
                        let name_owned = name.to_string();
                        self.global_values.push(val);
                        self.global_slots.insert(name_owned, slot);
                    }
                }
                Op::SetGlobal(idx) => {
                    let name = match &chunk.constants[idx as usize] {
                        Constant::Str(s) => s.as_str(),
                        _ => unreachable!(),
                    };
                    let val = self.stack.last().unwrap().clone();
                    let Some(&slot) = self.global_slots.get(name) else {
                        let line = chunk.lines[op_ip];
                        return Err(format!("[line {}] Undefined variable '{}'", line, name));
                    };
                    self.global_values[slot] = val;
                }
                Op::Add => {
                    let len = self.stack.len();
                    if let (Value::Int(x), Value::Int(y)) =
                        (&self.stack[len - 2], &self.stack[len - 1])
                    {
                        let res = x.wrapping_add(*y);
                        self.stack[len - 2] = Value::Int(res);
                        self.stack.pop();
                    } else {
                        self.execute_add(chunk.lines[op_ip])?;
                    }
                }
                Op::Sub => {
                    let len = self.stack.len();
                    if let (Value::Int(x), Value::Int(y)) =
                        (&self.stack[len - 2], &self.stack[len - 1])
                    {
                        let res = x.wrapping_sub(*y);
                        self.stack[len - 2] = Value::Int(res);
                        self.stack.pop();
                    } else {
                        let line = chunk.lines[op_ip];
                        self.numeric_op(line, i64::wrapping_sub, |a, b| a - b, "subtract")?;
                    }
                }
                Op::Mul => {
                    let len = self.stack.len();
                    if let (Value::Int(x), Value::Int(y)) =
                        (&self.stack[len - 2], &self.stack[len - 1])
                    {
                        let res = x.wrapping_mul(*y);
                        self.stack[len - 2] = Value::Int(res);
                        self.stack.pop();
                    } else {
                        let line = chunk.lines[op_ip];
                        self.numeric_op(line, i64::wrapping_mul, |a, b| a * b, "multiply")?;
                    }
                }
                Op::Div => {
                    let line = chunk.lines[op_ip];
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
                    let len = self.stack.len();
                    if let (Value::Int(x), Value::Int(y)) =
                        (&self.stack[len - 2], &self.stack[len - 1])
                    {
                        if *y == 0 {
                            let line = chunk.lines[op_ip];
                            return Err(format!("[line {}] Modulo by zero", line));
                        }
                        let res = x.checked_rem(*y).unwrap_or(0);
                        self.stack[len - 2] = Value::Int(res);
                        self.stack.pop();
                    } else {
                        let line = chunk.lines[op_ip];
                        let b = self.stack.pop().unwrap();
                        let a = self.stack.pop().unwrap();
                        match (a, b) {
                            (Value::Float(x), Value::Float(y)) => {
                                self.stack.push(Value::Float(x % y))
                            }
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
                }
                Op::Neg => {
                    let len = self.stack.len();
                    match &mut self.stack[len - 1] {
                        Value::Int(x) => *x = x.wrapping_neg(),
                        Value::Float(x) => *x = -*x,
                        other => {
                            let line = chunk.lines[op_ip];
                            return Err(format!(
                                "[line {}] Cannot negate '{}'",
                                line,
                                other.type_name()
                            ));
                        }
                    }
                }
                Op::Not => {
                    let len = self.stack.len();
                    let b = !self.stack[len - 1].is_truthy();
                    self.stack[len - 1] = Value::Bool(b);
                }
                Op::Eq => {
                    let len = self.stack.len();
                    match (&self.stack[len - 2], &self.stack[len - 1]) {
                        (Value::Int(a), Value::Int(b)) => {
                            let eq = a == b;
                            self.stack[len - 2] = Value::Bool(eq);
                            self.stack.pop();
                        }
                        (Value::Bool(a), Value::Bool(b)) => {
                            let eq = a == b;
                            self.stack[len - 2] = Value::Bool(eq);
                            self.stack.pop();
                        }
                        (Value::Str(a), Value::Str(b)) => {
                            let eq = a == b;
                            self.stack[len - 2] = Value::Bool(eq);
                            self.stack.pop();
                        }
                        (Value::Unit, Value::Unit) => {
                            self.stack[len - 2] = Value::Bool(true);
                            self.stack.pop();
                        }
                        _ => {
                            let line = chunk.lines[op_ip];
                            self.frames.last_mut().unwrap().ip = ip;
                            let b = self.stack.pop().unwrap();
                            let a = self.stack.pop().unwrap();
                            let eq = a
                                .equals_vm(&b, self)
                                .map_err(|e| format!("[line {}] {}", line, e))?;
                            self.stack.push(Value::Bool(eq));
                        }
                    }
                }
                Op::Ne => {
                    let len = self.stack.len();
                    match (&self.stack[len - 2], &self.stack[len - 1]) {
                        (Value::Int(a), Value::Int(b)) => {
                            let ne = a != b;
                            self.stack[len - 2] = Value::Bool(ne);
                            self.stack.pop();
                        }
                        (Value::Bool(a), Value::Bool(b)) => {
                            let ne = a != b;
                            self.stack[len - 2] = Value::Bool(ne);
                            self.stack.pop();
                        }
                        (Value::Str(a), Value::Str(b)) => {
                            let ne = a != b;
                            self.stack[len - 2] = Value::Bool(ne);
                            self.stack.pop();
                        }
                        (Value::Unit, Value::Unit) => {
                            self.stack[len - 2] = Value::Bool(false);
                            self.stack.pop();
                        }
                        _ => {
                            let line = chunk.lines[op_ip];
                            self.frames.last_mut().unwrap().ip = ip;
                            let b = self.stack.pop().unwrap();
                            let a = self.stack.pop().unwrap();
                            let eq = a
                                .equals_vm(&b, self)
                                .map_err(|e| format!("[line {}] {}", line, e))?;
                            self.stack.push(Value::Bool(!eq));
                        }
                    }
                }
                Op::Lt => {
                    let len = self.stack.len();
                    if let (Value::Int(x), Value::Int(y)) =
                        (&self.stack[len - 2], &self.stack[len - 1])
                    {
                        let res = *x < *y;
                        self.stack[len - 2] = Value::Bool(res);
                        self.stack.pop();
                    } else {
                        let line = chunk.lines[op_ip];
                        self.cmp_op(line, |a, b| a < b, |a, b| a < b, |a, b| a < b)?;
                    }
                }
                Op::Le => {
                    let len = self.stack.len();
                    if let (Value::Int(x), Value::Int(y)) =
                        (&self.stack[len - 2], &self.stack[len - 1])
                    {
                        let res = *x <= *y;
                        self.stack[len - 2] = Value::Bool(res);
                        self.stack.pop();
                    } else {
                        let line = chunk.lines[op_ip];
                        self.cmp_op(line, |a, b| a <= b, |a, b| a <= b, |a, b| a <= b)?;
                    }
                }
                Op::Gt => {
                    let len = self.stack.len();
                    if let (Value::Int(x), Value::Int(y)) =
                        (&self.stack[len - 2], &self.stack[len - 1])
                    {
                        let res = *x > *y;
                        self.stack[len - 2] = Value::Bool(res);
                        self.stack.pop();
                    } else {
                        let line = chunk.lines[op_ip];
                        self.cmp_op(line, |a, b| a > b, |a, b| a > b, |a, b| a > b)?;
                    }
                }
                Op::Ge => {
                    let len = self.stack.len();
                    if let (Value::Int(x), Value::Int(y)) =
                        (&self.stack[len - 2], &self.stack[len - 1])
                    {
                        let res = *x >= *y;
                        self.stack[len - 2] = Value::Bool(res);
                        self.stack.pop();
                    } else {
                        let line = chunk.lines[op_ip];
                        self.cmp_op(line, |a, b| a >= b, |a, b| a >= b, |a, b| a >= b)?;
                    }
                }
                Op::In => {
                    let line = chunk.lines[op_ip];
                    let container = self.stack.pop().unwrap();
                    let needle = self.stack.pop().unwrap();
                    let found = match (needle, container) {
                        (Value::Str(sub), Value::Str(s)) => s.contains(&sub),
                        (Value::Int(byte), Value::Bytes(b)) => {
                            (0..=255).contains(&byte) && b.contains(&(byte as u8))
                        }
                        (Value::Bytes(sub), Value::Bytes(b)) => {
                            sub.is_empty() || b.windows(sub.len()).any(|w| w == sub.as_slice())
                        }
                        (val, Value::List(l)) => {
                            self.frames.last_mut().unwrap().ip = ip;
                            let items = l.borrow().clone();
                            let mut any_eq = false;
                            for item in items.iter() {
                                if item
                                    .equals_vm(&val, self)
                                    .map_err(|e| format!("[line {}] {}", line, e))?
                                {
                                    any_eq = true;
                                    break;
                                }
                            }
                            any_eq
                        }
                        (val, Value::Set(s)) => s.borrow().contains(&val),
                        (Value::Str(k), Value::Dict(d)) => d.borrow().contains_key(&k),
                        (Value::Int(n), Value::Range { start, end }) => n >= start && n < end,
                        (val, Value::StructInstance(inst)) => {
                            let Some(method) = inst.def.methods.get("contains").cloned() else {
                                return Err(format!(
                                    "[line {}] Cannot check '{}' in '{}'",
                                    line,
                                    val.type_name(),
                                    inst.def.name
                                ));
                            };
                            self.frames.last_mut().unwrap().ip = ip;
                            let res = self
                                .call_value(method, &[Value::StructInstance(inst), val])
                                .map_err(|e| format!("[line {}] {}", line, e))?;
                            res.is_truthy()
                        }
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
                Op::IsNumeric => {
                    let len = self.stack.len();
                    let b = matches!(self.stack[len - 1], Value::Int(_) | Value::Float(_));
                    self.stack[len - 1] = Value::Bool(b);
                }
                Op::Jump(target) => {
                    ip = target;
                }
                Op::JumpIfFalsePop(target) => {
                    let cond = self.stack.pop().unwrap();
                    if !cond.is_truthy() {
                        ip = target;
                    }
                }
                Op::JumpIfFalsePeek(target) => {
                    if !self.stack.last().unwrap().is_truthy() {
                        ip = target;
                    }
                }
                Op::JumpIfTruePeek(target) => {
                    if self.stack.last().unwrap().is_truthy() {
                        ip = target;
                    }
                }
                Op::JumpIfNotUnitPeek(target) => {
                    if !matches!(self.stack.last().unwrap(), Value::Unit) {
                        ip = target;
                    }
                }
                Op::JumpIfArgPresent(arg_idx, target) => {
                    if arg_is_present(&self.frames.last().unwrap().arg_bits, arg_idx as usize) {
                        ip = target;
                    }
                }
                Op::Call(argc) => {
                    let argc_usize = argc as usize;
                    let callee_idx = self.stack.len() - 1 - argc_usize;
                    match &self.stack[callee_idx] {
                        Value::Closure(closure_ref) => {
                            if self.frames.len() >= MAX_CALL_DEPTH {
                                let line = chunk.lines[op_ip];
                                return Err(format!(
                                    "[line {}] Stack overflow (maximum call depth {} exceeded)",
                                    line, MAX_CALL_DEPTH
                                ));
                            }
                            let fixed_fast = closure_ref.proto.arity as usize == argc_usize
                                && !closure_ref.proto.is_variadic;
                            let Value::Closure(closure) =
                                std::mem::replace(&mut self.stack[callee_idx], Value::Unit)
                            else {
                                unreachable!()
                            };
                            let bits = if fixed_fast {
                                make_positional_arg_bits(argc_usize)
                            } else {
                                let line = chunk.lines[op_ip];
                                self.frames.last_mut().unwrap().ip = ip;
                                let pos_args = self.stack[callee_idx + 1..].to_vec();
                                let (slots, bits) = self
                                    .build_closure_slots(&closure, &pos_args, None)
                                    .map_err(|e| format!("[line {}] {}", line, e))?;
                                self.stack.truncate(callee_idx + 1);
                                self.stack.extend(slots);
                                bits
                            };
                            let enter_step = self.step_counter;
                            let enter_time = if self.profile_enabled {
                                Some(Instant::now())
                            } else {
                                None
                            };
                            self.frames.last_mut().unwrap().ip = ip;
                            cur_closure = closure.clone();
                            cur_base = callee_idx;
                            ip = 0;
                            proto_id = cur_closure.proto.ic_id;
                            self.frames.push(CallFrame {
                                closure,
                                ip: 0,
                                base: callee_idx,
                                arg_bits: bits,
                                enter_step,
                                enter_time,
                            });
                        }
                        Value::NativeFn(native) => {
                            let native = *native;
                            self.frames.last_mut().unwrap().ip = ip;
                            let line = chunk.lines[op_ip];
                            let enter_step = self.step_counter;
                            let enter_time = if self.profile_enabled {
                                Some(Instant::now())
                            } else {
                                None
                            };
                            let res = if argc_usize <= 8 {
                                let mut small_args: [Value; 8] = [const { Value::Unit }; 8];
                                for i in 0..argc_usize {
                                    small_args[i] = std::mem::replace(
                                        &mut self.stack[callee_idx + 1 + i],
                                        Value::Unit,
                                    );
                                }
                                self.stack.truncate(callee_idx);
                                (native.func)(self, &small_args[..argc_usize])
                            } else {
                                let args = self.stack[callee_idx + 1..].to_vec();
                                self.stack.truncate(callee_idx);
                                (native.func)(self, &args)
                            }
                            .map_err(|e| format!("[line {}] {}", line, e));
                            if self.profile_enabled {
                                self.record_profile(native.name, enter_step, enter_time);
                            }
                            let val = res?;
                            if let Some(req) = self.scheduler.pending_yield.take() {
                                self.resume_after_suspend(req, target_depth)?;
                                let caller = self.frames.last().unwrap();
                                cur_closure = caller.closure.clone();
                                cur_base = caller.base;
                                ip = caller.ip;
                                proto_id = cur_closure.proto.ic_id;
                                continue;
                            }
                            self.stack.push(val);
                        }
                        Value::BoundMethod(_) | Value::StructDef(_) => {
                            let callee =
                                std::mem::replace(&mut self.stack[callee_idx], Value::Unit);
                            let line = chunk.lines[op_ip];
                            match callee {
                                Value::BoundMethod(m) => {
                                    let mut combined = Vec::with_capacity(1 + argc_usize);
                                    combined.push(m.receiver.clone());
                                    combined.extend_from_slice(&self.stack[callee_idx + 1..]);
                                    self.stack.truncate(callee_idx);
                                    let func = m.func.clone();
                                    match func {
                                        Value::Closure(closure) => {
                                            if self.frames.len() >= MAX_CALL_DEPTH {
                                                return Err(format!(
                                                    "[line {}] Stack overflow (maximum call depth {} exceeded)",
                                                    line, MAX_CALL_DEPTH
                                                ));
                                            }
                                            self.frames.last_mut().unwrap().ip = ip;
                                            let (slots, bits) = self
                                                .build_closure_slots(&closure, &combined, None)
                                                .map_err(|e| format!("[line {}] {}", line, e))?;
                                            self.stack.push(Value::Unit);
                                            self.stack.extend(slots);
                                            let enter_step = self.step_counter;
                                            let enter_time = if self.profile_enabled {
                                                Some(Instant::now())
                                            } else {
                                                None
                                            };
                                            cur_closure = closure.clone();
                                            cur_base = callee_idx;
                                            ip = 0;
                                            proto_id = cur_closure.proto.ic_id;
                                            self.frames.push(CallFrame {
                                                closure,
                                                ip: 0,
                                                base: callee_idx,
                                                arg_bits: bits,
                                                enter_step,
                                                enter_time,
                                            });
                                        }
                                        other => {
                                            self.frames.last_mut().unwrap().ip = ip;
                                            let res = self
                                                .call_value(other, &combined)
                                                .map_err(|e| format!("[line {}] {}", line, e))?;
                                            if let Some(req) = self.scheduler.pending_yield.take() {
                                                self.resume_after_suspend(req, target_depth)?;
                                                let caller = self.frames.last().unwrap();
                                                cur_closure = caller.closure.clone();
                                                cur_base = caller.base;
                                                ip = caller.ip;
                                                proto_id = cur_closure.proto.ic_id;
                                                continue;
                                            }
                                            self.stack.push(res);
                                        }
                                    }
                                }
                                Value::StructDef(def) => {
                                    let args = self.stack[callee_idx + 1..].to_vec();
                                    self.stack.truncate(callee_idx);
                                    if args.len() > def.field_names.len() {
                                        return Err(format!(
                                            "[line {}] Struct '{}' takes at most {} positional arguments, got {}",
                                            line,
                                            def.name,
                                            def.field_names.len(),
                                            args.len()
                                        ));
                                    }
                                    self.frames.last_mut().unwrap().ip = ip;
                                    let names = def.field_names[..args.len()].to_vec();
                                    let res = self
                                        .instantiate_struct(&def, None, &names, &args)
                                        .map_err(|e| format!("[line {}] {}", line, e))?;
                                    self.stack.push(res);
                                }
                                _ => unreachable!(),
                            }
                        }
                        other => {
                            let line = chunk.lines[op_ip];
                            return Err(format!(
                                "[line {}] Cannot call value of type '{}'",
                                line,
                                other.type_name()
                            ));
                        }
                    }
                }
                Op::TailCall(argc) => {
                    let argc_usize = argc as usize;
                    let callee_idx = self.stack.len() - 1 - argc_usize;
                    if let Value::Closure(closure_ref) = &self.stack[callee_idx] {
                        let fixed_fast = closure_ref.proto.arity as usize == argc_usize
                            && !closure_ref.proto.is_variadic;
                        let Value::Closure(closure) =
                            std::mem::replace(&mut self.stack[callee_idx], Value::Unit)
                        else {
                            unreachable!()
                        };
                        if self.frames.len() < 16 {
                            let bits = if fixed_fast {
                                make_positional_arg_bits(argc_usize)
                            } else {
                                let line = chunk.lines[op_ip];
                                self.frames.last_mut().unwrap().ip = ip;
                                let pos_args = self.stack[callee_idx + 1..].to_vec();
                                let (slots, bits) = self
                                    .build_closure_slots(&closure, &pos_args, None)
                                    .map_err(|e| format!("[line {}] {}", line, e))?;
                                self.stack.truncate(callee_idx + 1);
                                self.stack.extend(slots);
                                bits
                            };
                            let enter_step = self.step_counter;
                            let enter_time = if self.profile_enabled {
                                Some(Instant::now())
                            } else {
                                None
                            };
                            self.frames.last_mut().unwrap().ip = ip;
                            cur_closure = closure.clone();
                            cur_base = callee_idx;
                            ip = 0;
                            proto_id = cur_closure.proto.ic_id;
                            self.frames.push(CallFrame {
                                closure,
                                ip: 0,
                                base: callee_idx,
                                arg_bits: bits,
                                enter_step,
                                enter_time,
                            });
                        } else {
                            let line = chunk.lines[op_ip];
                            self.frames.last_mut().unwrap().ip = ip;
                            if self.profile_enabled {
                                let cur_frame = self.frames.last().unwrap();
                                let prev_name = cur_frame.closure.proto.name.clone();
                                let prev_step = cur_frame.enter_step;
                                let prev_time = cur_frame.enter_time;
                                self.record_profile(&prev_name, prev_step, prev_time);
                            }
                            let (slots, bits) = if fixed_fast {
                                (
                                    self.stack[callee_idx + 1..].to_vec(),
                                    make_positional_arg_bits(argc_usize),
                                )
                            } else {
                                let pos_args = self.stack[callee_idx + 1..].to_vec();
                                self.build_closure_slots(&closure, &pos_args, None)
                                    .map_err(|e| format!("[line {}] {}", line, e))?
                            };
                            if !self.open_upvalues.is_empty() {
                                self.close_upvalues_from(cur_base);
                            }
                            self.stack.truncate(cur_base);
                            self.stack.push(Value::Unit);
                            self.stack.extend(slots);
                            let enter_step = self.step_counter;
                            let enter_time = if self.profile_enabled {
                                Some(Instant::now())
                            } else {
                                None
                            };
                            cur_closure = closure.clone();
                            ip = 0;
                            proto_id = cur_closure.proto.ic_id;
                            let frame = self.frames.last_mut().unwrap();
                            frame.closure = closure;
                            frame.ip = 0;
                            frame.arg_bits = bits;
                            frame.enter_step = enter_step;
                            frame.enter_time = enter_time;
                        }
                    } else {
                        let line = chunk.lines[op_ip];
                        let callee = std::mem::replace(&mut self.stack[callee_idx], Value::Unit);
                        let args = self.stack[callee_idx + 1..].to_vec();
                        self.stack.truncate(callee_idx);
                        self.frames.last_mut().unwrap().ip = ip;
                        let res = self
                            .call_value(callee, &args)
                            .map_err(|e| format!("[line {}] {}", line, e))?;
                        let frame = self.frames.pop().unwrap();
                        if self.profile_enabled {
                            self.record_profile(
                                &frame.closure.proto.name,
                                frame.enter_step,
                                frame.enter_time,
                            );
                        }
                        if !self.try_handlers.is_empty() {
                            while self
                                .try_handlers
                                .last()
                                .is_some_and(|h| h.frame_depth > self.frames.len())
                            {
                                self.try_handlers.pop();
                            }
                        }
                        if !self.open_upvalues.is_empty() {
                            self.close_upvalues_from(frame.base);
                        }
                        self.stack.truncate(frame.base);
                        if let Some(req) = self.scheduler.pending_yield.take() {
                            self.resume_after_suspend(req, target_depth)?;
                            let caller = self.frames.last().unwrap();
                            cur_closure = caller.closure.clone();
                            cur_base = caller.base;
                            ip = caller.ip;
                            proto_id = cur_closure.proto.ic_id;
                            continue;
                        }
                        if self.frames.len() == target_depth {
                            if target_depth == 0 && self.scheduler.current_task_id != 0 {
                                self.finish_current_green_task(Ok(res))?;
                                let caller = self.frames.last().unwrap();
                                cur_closure = caller.closure.clone();
                                cur_base = caller.base;
                                ip = caller.ip;
                                proto_id = cur_closure.proto.ic_id;
                                continue;
                            }
                            return Ok(res);
                        }
                        self.stack.push(res);
                        let caller = self.frames.last().unwrap();
                        cur_closure = caller.closure.clone();
                        cur_base = caller.base;
                        ip = caller.ip;
                        proto_id = cur_closure.proto.ic_id;
                    }
                }
                Op::CallNamed(pos_argc, kw_idx) => {
                    let line = chunk.lines[op_ip];
                    let pos_argc = pos_argc as usize;
                    let kw_names = chunk.kw_lists[kw_idx as usize].clone();
                    let kw_len = kw_names.len();
                    let total_argc = pos_argc + kw_len;
                    let callee_idx = self.stack.len() - 1 - total_argc;
                    let callee = std::mem::replace(&mut self.stack[callee_idx], Value::Unit);
                    let pos_args = self.stack[callee_idx + 1..callee_idx + 1 + pos_argc].to_vec();
                    let kw_vals = self.stack[callee_idx + 1 + pos_argc..].to_vec();
                    self.stack.truncate(callee_idx);
                    match callee {
                        Value::Closure(closure) => {
                            if self.frames.len() >= MAX_CALL_DEPTH {
                                return Err(format!(
                                    "[line {}] Stack overflow (maximum call depth {} exceeded)",
                                    line, MAX_CALL_DEPTH
                                ));
                            }
                            self.frames.last_mut().unwrap().ip = ip;
                            let (slots, bits) = self
                                .build_closure_slots(
                                    &closure,
                                    &pos_args,
                                    Some((&kw_names, &kw_vals)),
                                )
                                .map_err(|e| format!("[line {}] {}", line, e))?;
                            self.stack.push(Value::Unit);
                            self.stack.extend(slots);
                            let enter_step = self.step_counter;
                            let enter_time = if self.profile_enabled {
                                Some(Instant::now())
                            } else {
                                None
                            };
                            cur_closure = closure.clone();
                            cur_base = callee_idx;
                            ip = 0;
                            proto_id = cur_closure.proto.ic_id;
                            self.frames.push(CallFrame {
                                closure,
                                ip: 0,
                                base: callee_idx,
                                arg_bits: bits,
                                enter_step,
                                enter_time,
                            });
                        }
                        Value::BoundMethod(m) => {
                            let mut combined = Vec::with_capacity(1 + pos_args.len());
                            combined.push(m.receiver.clone());
                            combined.extend(pos_args);
                            match m.func.clone() {
                                Value::Closure(closure) => {
                                    if self.frames.len() >= MAX_CALL_DEPTH {
                                        return Err(format!(
                                            "[line {}] Stack overflow (maximum call depth {} exceeded)",
                                            line, MAX_CALL_DEPTH
                                        ));
                                    }
                                    self.frames.last_mut().unwrap().ip = ip;
                                    let (slots, bits) = self
                                        .build_closure_slots(
                                            &closure,
                                            &combined,
                                            Some((&kw_names, &kw_vals)),
                                        )
                                        .map_err(|e| format!("[line {}] {}", line, e))?;
                                    self.stack.push(Value::Unit);
                                    self.stack.extend(slots);
                                    let enter_step = self.step_counter;
                                    let enter_time = if self.profile_enabled {
                                        Some(Instant::now())
                                    } else {
                                        None
                                    };
                                    cur_closure = closure.clone();
                                    cur_base = callee_idx;
                                    ip = 0;
                                    proto_id = cur_closure.proto.ic_id;
                                    self.frames.push(CallFrame {
                                        closure,
                                        ip: 0,
                                        base: callee_idx,
                                        arg_bits: bits,
                                        enter_step,
                                        enter_time,
                                    });
                                }
                                other => {
                                    return Err(format!(
                                        "[line {}] Named arguments are not supported for '{}'",
                                        line,
                                        other.type_name()
                                    ));
                                }
                            }
                        }
                        Value::StructDef(def) => {
                            if pos_args.len() > def.field_names.len() {
                                return Err(format!(
                                    "[line {}] Struct '{}' takes at most {} positional arguments",
                                    line,
                                    def.name,
                                    def.field_names.len()
                                ));
                            }
                            self.frames.last_mut().unwrap().ip = ip;
                            let mut names = def.field_names[..pos_args.len()].to_vec();
                            let mut vals = pos_args;
                            names.extend(kw_names);
                            vals.extend(kw_vals);
                            let res = self
                                .instantiate_struct(&def, None, &names, &vals)
                                .map_err(|e| format!("[line {}] {}", line, e))?;
                            self.stack.push(res);
                        }
                        other => {
                            return Err(format!(
                                "[line {}] Named arguments are not supported for '{}'",
                                line,
                                other.type_name()
                            ));
                        }
                    }
                }
                Op::Closure(idx, uv_idx) => {
                    let proto = match &chunk.constants[idx as usize] {
                        Constant::Function(p) => p.clone(),
                        _ => unreachable!(),
                    };
                    let descs = &chunk.upvalue_lists[uv_idx as usize];
                    let mut upvalues = Vec::with_capacity(descs.len());
                    for desc in descs {
                        if desc.is_local {
                            let slot = cur_base + desc.index as usize;
                            upvalues.push(self.capture_upvalue(slot));
                        } else {
                            upvalues.push(cur_closure.upvalues[desc.index as usize].clone());
                        }
                    }
                    let base_dir = cur_closure.base_dir.clone();
                    self.stack.push(Value::Closure(Rc::new(Closure {
                        proto,
                        upvalues,
                        base_dir,
                    })));
                }
                Op::MakeList(count) => {
                    let count = count as usize;
                    self.charge_alloc(count.saturating_mul(std::mem::size_of::<Value>()))?;
                    let start = self.stack.len() - count;
                    let items = self.stack.drain(start..).collect::<Vec<_>>();
                    self.stack.push(Value::List(Rc::new(RefCell::new(items))));
                }
                Op::MakeDict(count) => {
                    let line = chunk.lines[op_ip];
                    let count = count as usize;
                    self.charge_alloc(count.saturating_mul(64))?;
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
                Op::MakeStructDef(idx) => {
                    let st_proto = match &chunk.constants[idx as usize] {
                        Constant::Struct(s) => s.clone(),
                        _ => unreachable!(),
                    };
                    let default_count = st_proto
                        .fields
                        .iter()
                        .filter(|(_, has_def)| *has_def)
                        .count();
                    let method_count = st_proto.methods.len();
                    let total = default_count + method_count;
                    let start = self.stack.len() - total;
                    let popped: Vec<Value> = self.stack.drain(start..).collect();
                    let mut field_names = Vec::with_capacity(st_proto.fields.len());
                    let mut field_index = HashMap::with_capacity(st_proto.fields.len());
                    let mut field_defaults = Vec::with_capacity(st_proto.fields.len());
                    let mut p_idx = 0usize;
                    for (i, (fname, has_def)) in st_proto.fields.iter().enumerate() {
                        field_names.push(fname.clone());
                        field_index.insert(fname.clone(), i);
                        if *has_def {
                            field_defaults.push(Some(popped[p_idx].clone()));
                            p_idx += 1;
                        } else {
                            field_defaults.push(None);
                        }
                    }
                    let mut methods = HashMap::with_capacity(method_count);
                    for mname in &st_proto.methods {
                        methods.insert(mname.clone(), popped[p_idx].clone());
                        p_idx += 1;
                    }
                    self.stack.push(Value::StructDef(Rc::new(StructDef {
                        ic_id: crate::bytecode::next_ic_id(),
                        name: st_proto.name.clone(),
                        field_names,
                        field_index,
                        field_defaults,
                        methods,
                    })));
                }
                Op::InitStruct(has_spread, kw_idx) => {
                    let line = chunk.lines[op_ip];
                    let field_names = chunk.kw_lists[kw_idx as usize].clone();
                    let field_count = field_names.len();
                    let fstart = self.stack.len() - field_count;
                    let field_vals: Vec<Value> = self.stack.drain(fstart..).collect();
                    let spread = if has_spread != 0 {
                        Some(self.stack.pop().unwrap())
                    } else {
                        None
                    };
                    let target = self.stack.pop().unwrap();
                    let Value::StructDef(def) = target else {
                        return Err(format!(
                            "[line {}] Expected struct type before '{{ ... }}', got '{}'",
                            line,
                            target.type_name()
                        ));
                    };
                    self.frames.last_mut().unwrap().ip = ip;
                    let inst = self
                        .instantiate_struct(&def, spread, &field_names, &field_vals)
                        .map_err(|e| format!("[line {}] {}", line, e))?;
                    self.stack.push(inst);
                }
                Op::GetIndex => {
                    let len = self.stack.len();
                    match (&self.stack[len - 2], &self.stack[len - 1]) {
                        (Value::List(list), Value::Int(i)) => {
                            let borrowed = list.borrow();
                            let llen = borrowed.len() as i64;
                            let actual = if *i < 0 { llen + *i } else { *i };
                            if actual < 0 || actual >= llen {
                                let line = chunk.lines[op_ip];
                                return Err(format!(
                                    "[line {}] Index {} out of bounds for list of length {}",
                                    line, *i, llen
                                ));
                            }
                            let val = borrowed[actual as usize].clone();
                            drop(borrowed);
                            self.stack[len - 2] = val;
                            self.stack.pop();
                        }
                        (Value::Dict(dict), Value::Str(key)) => {
                            let val = dict.borrow().get(key).cloned().unwrap_or(Value::Unit);
                            self.stack[len - 2] = val;
                            self.stack.pop();
                        }
                        (Value::Bytes(b), Value::Int(i)) => {
                            let blen = b.len() as i64;
                            let actual = if *i < 0 { blen + *i } else { *i };
                            if actual < 0 || actual >= blen {
                                let line = chunk.lines[op_ip];
                                return Err(format!(
                                    "[line {}] Index {} out of bounds for bytes of length {}",
                                    line, *i, blen
                                ));
                            }
                            let val = Value::Int(b[actual as usize] as i64);
                            self.stack[len - 2] = val;
                            self.stack.pop();
                        }
                        _ => {
                            let line = chunk.lines[op_ip];
                            let idx_val = self.stack.pop().unwrap();
                            let obj_val = self.stack.pop().unwrap();
                            let res = match (obj_val, idx_val) {
                                (Value::Str(s), Value::Int(i)) => {
                                    let chars: Vec<char> = s.chars().collect();
                                    let slen = chars.len() as i64;
                                    let actual = if i < 0 { slen + i } else { i };
                                    if actual < 0 || actual >= slen {
                                        return Err(format!(
                                            "[line {}] Index {} out of bounds for string of length {}",
                                            line, i, slen
                                        ));
                                    }
                                    Value::Str(chars[actual as usize].to_string())
                                }
                                (Value::Range { start, end }, Value::Int(i)) => {
                                    let rlen = if end > start {
                                        (end as i128) - (start as i128)
                                    } else {
                                        0
                                    };
                                    let actual = if i < 0 { rlen + i as i128 } else { i as i128 };
                                    if actual < 0 || actual >= rlen {
                                        return Err(format!(
                                            "[line {}] Index {} out of bounds for range of length {}",
                                            line, i, rlen
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
                    }
                }
                Op::Slice(inclusive) => {
                    let line = chunk.lines[op_ip];
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
                                let r = if inclusive { r.saturating_add(1) } else { r };
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
                            let (s, e) = {
                                let borrowed = list.borrow();
                                resolve_bounds(borrowed.len() as i64)?
                            };
                            let slice_len = e.saturating_sub(s);
                            self.charge_alloc(
                                slice_len.saturating_mul(std::mem::size_of::<Value>()),
                            )?;
                            let sliced = if slice_len > 0 {
                                list.borrow()[s..e].to_vec()
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
                            self.charge_alloc(sliced.len())?;
                            Value::Str(sliced)
                        }
                        Value::Bytes(bytes) => {
                            let (s, e) = resolve_bounds(bytes.len() as i64)?;
                            let slice_len = e.saturating_sub(s);
                            self.charge_alloc(slice_len)?;
                            let sliced = if slice_len > 0 {
                                bytes[s..e].to_vec()
                            } else {
                                Vec::new()
                            };
                            Value::Bytes(Rc::new(sliced))
                        }
                        Value::Range { start, end } => {
                            let len = if end > start {
                                ((end as i128) - (start as i128)).min(i64::MAX as i128) as i64
                            } else {
                                0
                            };
                            let (s, e) = resolve_bounds(len)?;
                            let ns = (start as i128 + s as i128).min(end as i128) as i64;
                            let ne = (start as i128 + e as i128).min(end as i128) as i64;
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
                                let line = chunk.lines[op_ip];
                                return Err(format!(
                                    "[line {}] Index {} out of bounds for list of length {}",
                                    line, i, len
                                ));
                            }
                            borrowed[actual as usize] = val.clone();
                        }
                        (Value::Dict(dict), Value::Str(key)) => {
                            let is_new = !dict.borrow().contains_key(&key);
                            if is_new {
                                self.charge_alloc(key.len().saturating_add(64))?;
                            }
                            dict.borrow_mut().insert(key, val.clone());
                        }
                        (o, i) => {
                            let line = chunk.lines[op_ip];
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
                    let len = self.stack.len();
                    match &self.stack[len - 1] {
                        Value::Module(m) => {
                            let mic_idx = ((proto_id as usize).wrapping_mul(31) ^ op_ip) & 511;
                            let mod_id = m.ic_id;
                            let cached = &self.module_ic[mic_idx];
                            if cached.0 == self.ic_epoch
                                && cached.1 == proto_id
                                && cached.2 == op_ip
                                && cached.3 == mod_id
                            {
                                let val = cached.4.clone();
                                self.stack[len - 1] = val;
                                continue;
                            }
                            let attr_name = match &chunk.constants[idx as usize] {
                                Constant::Str(s) => s.as_str(),
                                _ => unreachable!(),
                            };
                            let val =
                                m.exports.borrow().get(attr_name).cloned().ok_or_else(|| {
                                    let line = chunk.lines[op_ip];
                                    format!(
                                        "[line {}] Module '{}' has no export '{}'",
                                        line, m.name, attr_name
                                    )
                                })?;
                            self.module_ic[mic_idx] =
                                (self.ic_epoch, proto_id, op_ip, mod_id, val.clone());
                            self.stack[len - 1] = val;
                            continue;
                        }
                        Value::StructInstance(inst) => {
                            let ic_idx = ((proto_id as usize).wrapping_mul(31) ^ op_ip) & 1023;
                            let cached = self.attr_ic[ic_idx];
                            let def_id = inst.def.ic_id;
                            if cached.0 == self.ic_epoch
                                && cached.1 == proto_id
                                && cached.2 == op_ip
                                && cached.3 == def_id
                            {
                                let val = inst.fields.borrow()[cached.4].clone();
                                self.stack[len - 1] = val;
                                continue;
                            }
                            let attr_name = match &chunk.constants[idx as usize] {
                                Constant::Str(s) => s.as_str(),
                                _ => unreachable!(),
                            };
                            if let Some(&field_idx) = inst.def.field_index.get(attr_name) {
                                self.attr_ic[ic_idx] =
                                    (self.ic_epoch, proto_id, op_ip, def_id, field_idx);
                                let val = inst.fields.borrow()[field_idx].clone();
                                self.stack[len - 1] = val;
                                continue;
                            }
                        }
                        _ => {}
                    }
                    let attr = match &chunk.constants[idx as usize] {
                        Constant::Str(s) => s.as_str(),
                        _ => unreachable!(),
                    };
                    let line = chunk.lines[op_ip];
                    let obj = self.stack.pop().unwrap();
                    let val = self.get_attr_value(obj, attr, line, ".")?;
                    self.stack.push(val);
                }
                Op::OptGetAttr(idx) => {
                    let obj = self.stack.pop().unwrap();
                    if matches!(obj, Value::Unit) {
                        self.stack.push(Value::Unit);
                    } else {
                        let attr = match &chunk.constants[idx as usize] {
                            Constant::Str(s) => s.as_str(),
                            _ => unreachable!(),
                        };
                        let line = chunk.lines[op_ip];
                        let val = self.get_attr_value(obj, attr, line, "?.")?;
                        self.stack.push(val);
                    }
                }
                Op::ForPrep(slot, exit_ip) => {
                    let line = chunk.lines[op_ip];
                    let iter_pos = cur_base + slot as usize;
                    let iter_val = self.stack[iter_pos].clone();
                    let (normalized, start_i, end_i) = match iter_val {
                        Value::Range { start, end } => (None, start, end),
                        Value::List(ref l) => (None, 0, l.borrow().len() as i64),
                        Value::Bytes(ref b) => (None, 0, b.len() as i64),
                        Value::Str(ref s) => self.materialize_str_iter(s)?,
                        Value::Set(ref s) => self.materialize_set_iter(s)?,
                        Value::StructInstance(inst) => {
                            let Some(iter_fn) = inst.def.methods.get("iter").cloned() else {
                                return Err(format!(
                                    "[line {}] Struct '{}' does not implement iter()",
                                    line, inst.def.name
                                ));
                            };
                            self.frames.last_mut().unwrap().ip = ip;
                            let ret = self
                                .call_value(iter_fn, &[Value::StructInstance(inst)])
                                .map_err(|e| format!("[line {}] {}", line, e))?;
                            match &ret {
                                Value::Range { start, end } => (Some(ret.clone()), *start, *end),
                                Value::List(l) => (Some(ret.clone()), 0, l.borrow().len() as i64),
                                Value::Bytes(b) => (Some(ret.clone()), 0, b.len() as i64),
                                Value::Str(st) => self.materialize_str_iter(st)?,
                                Value::Set(st) => self.materialize_set_iter(st)?,
                                other => {
                                    return Err(format!(
                                        "[line {}] iter() returned non-iterable '{}'",
                                        line,
                                        other.type_name()
                                    ));
                                }
                            }
                        }
                        other => {
                            return Err(format!(
                                "[line {}] Cannot iterate over '{}'",
                                line,
                                other.type_name()
                            ));
                        }
                    };
                    if let Some(norm) = normalized {
                        self.stack[iter_pos] = norm;
                    }
                    if start_i >= end_i {
                        ip = exit_ip;
                    } else {
                        self.stack[iter_pos + 1] = Value::Int(start_i);
                        self.stack[iter_pos + 2] = Value::Int(end_i);
                    }
                }
                Op::ForIter(slot, exit_ip) => {
                    let iter_pos = cur_base + slot as usize;
                    let (Value::Int(idx), Value::Int(end)) =
                        (&self.stack[iter_pos + 1], &self.stack[iter_pos + 2])
                    else {
                        unreachable!();
                    };
                    let idx = *idx;
                    let end = *end;
                    if idx >= end {
                        ip = exit_ip;
                    } else {
                        let next_idx = idx.wrapping_add(1);
                        let item = match &self.stack[iter_pos] {
                            Value::Range { .. } => Value::Int(idx),
                            Value::List(l) => {
                                let b = l.borrow();
                                if (idx as usize) >= b.len() {
                                    ip = exit_ip;
                                    continue;
                                }
                                b[idx as usize].clone()
                            }
                            Value::Bytes(b) => Value::Int(b[idx as usize] as i64),
                            _ => unreachable!(),
                        };
                        self.stack[iter_pos + 1] = Value::Int(next_idx);
                        self.stack.push(item);
                    }
                }
                Op::SetAttr(idx) => {
                    let attr = match &chunk.constants[idx as usize] {
                        Constant::Str(s) => s.as_str(),
                        _ => unreachable!(),
                    };
                    let val = self.stack.pop().unwrap();
                    let obj = self.stack.pop().unwrap();
                    match obj {
                        Value::Dict(dict) => {
                            let is_new = !dict.borrow().contains_key(attr);
                            if is_new {
                                self.charge_alloc(attr.len().saturating_add(64))?;
                            }
                            dict.borrow_mut().insert(attr.to_string(), val.clone());
                            self.stack.push(val);
                        }
                        Value::StructInstance(inst) => {
                            let Some(&field_idx) = inst.def.field_index.get(attr) else {
                                let line = chunk.lines[op_ip];
                                return Err(format!(
                                    "[line {}] Struct '{}' has no field '{}'",
                                    line, inst.def.name, attr
                                ));
                            };
                            inst.fields.borrow_mut()[field_idx] = val.clone();
                            self.stack.push(val);
                        }
                        other => {
                            let line = chunk.lines[op_ip];
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
                    let source = match &chunk.constants[idx as usize] {
                        Constant::Str(s) => s.clone(),
                        _ => unreachable!(),
                    };
                    let line = chunk.lines[op_ip];
                    self.frames.last_mut().unwrap().ip = ip;
                    let module = self
                        .load_module(&source, is_path)
                        .map_err(|e| format!("[line {}] {}", line, e))?;
                    self.stack.push(module);
                }
                Op::MakeModule(name_idx, count) => {
                    let mod_name = match &chunk.constants[name_idx as usize] {
                        Constant::Str(s) => s.clone(),
                        _ => unreachable!(),
                    };
                    let count = count as usize;
                    let start = self.stack.len() - count * 2;
                    let mut exports = HashMap::with_capacity(count);
                    let drained: Vec<Value> = self.stack.drain(start..).collect();
                    for pair in drained.chunks_exact(2) {
                        if let Value::Str(k) = &pair[0] {
                            exports.insert(k.clone(), pair[1].clone());
                        }
                    }
                    self.stack.push(Value::Module(Rc::new(ModuleData {
                        ic_id: crate::bytecode::next_ic_id(),
                        name: mod_name,
                        exports: Rc::new(RefCell::new(exports)),
                    })));
                }
                Op::SetupTry(catch_ip) => {
                    self.try_handlers.push(TryHandler {
                        frame_depth: self.frames.len(),
                        stack_len: self.stack.len(),
                        cleanup_len: self.cleanup_stack.len(),
                        catch_ip,
                    });
                }
                Op::PopTry => {
                    self.try_handlers.pop();
                }
                Op::Throw => {
                    self.frames.last_mut().unwrap().ip = ip;
                    let val = self.stack.pop().unwrap();
                    return Err(val.to_string());
                }
                Op::PushDefer(scope_depth) => {
                    let val = self.stack.pop().unwrap();
                    let Value::Closure(closure) = val else {
                        unreachable!("PushDefer expects closure");
                    };
                    self.cleanup_stack.push(CleanupEntry {
                        frame_depth: self.frames.len(),
                        scope_depth,
                        kind: CleanupKind::Defer(closure),
                    });
                }
                Op::EnterSandbox(scope_depth) => {
                    let line = chunk.lines[op_ip];
                    let policy = self.stack.pop().unwrap();
                    let snapshot = self
                        .attenuate_permissions(&policy)
                        .map_err(|e| format!("[line {}] {}", line, e))?;
                    self.cleanup_stack.push(CleanupEntry {
                        frame_depth: self.frames.len(),
                        scope_depth,
                        kind: CleanupKind::Sandbox(snapshot),
                    });
                }
                Op::RunCleanups(min_scope) => {
                    if !self.cleanup_stack.is_empty() {
                        self.frames.last_mut().unwrap().ip = ip;
                        let target_frame_depth = self.frames.len();
                        self.unwind_cleanups_scope(target_frame_depth, min_scope, true)?;
                    }
                }
                Op::Return => {
                    if !self.cleanup_stack.is_empty() {
                        let fdepth = self.frames.len();
                        if self
                            .cleanup_stack
                            .last()
                            .is_some_and(|e| e.frame_depth >= fdepth)
                        {
                            self.frames.last_mut().unwrap().ip = ip;
                            self.unwind_cleanups_scope(fdepth, 0, true)?;
                        }
                    }
                    let result = self.stack.pop().unwrap();
                    let frame = self.frames.pop().unwrap();
                    if self.profile_enabled {
                        self.record_profile(
                            &frame.closure.proto.name,
                            frame.enter_step,
                            frame.enter_time,
                        );
                    }
                    if !self.try_handlers.is_empty() {
                        while self
                            .try_handlers
                            .last()
                            .is_some_and(|h| h.frame_depth > self.frames.len())
                        {
                            self.try_handlers.pop();
                        }
                    }
                    if !self.open_upvalues.is_empty() {
                        self.close_upvalues_from(frame.base);
                    }
                    self.stack.truncate(frame.base);
                    if self.frames.len() == target_depth {
                        if target_depth == 0 && self.scheduler.current_task_id != 0 {
                            self.finish_current_green_task(Ok(result))?;
                            let caller = self.frames.last().unwrap();
                            cur_closure = caller.closure.clone();
                            cur_base = caller.base;
                            ip = caller.ip;
                            proto_id = cur_closure.proto.ic_id;
                            continue;
                        }
                        return Ok(result);
                    }
                    self.stack.push(result);
                    let caller = self.frames.last().unwrap();
                    cur_closure = caller.closure.clone();
                    cur_base = caller.base;
                    ip = caller.ip;
                    proto_id = cur_closure.proto.ic_id;
                }
            }
        }
    }

    fn execute_add(&mut self, line: usize) -> Result<(), String> {
        let b = self.stack.pop().unwrap();
        let a = self.stack.pop().unwrap();
        let res = match (a, b) {
            (Value::Int(x), Value::Int(y)) => Value::Int(x.wrapping_add(y)),
            (Value::Float(x), Value::Float(y)) => Value::Float(x + y),
            (Value::Int(x), Value::Float(y)) => Value::Float((x as f64) + y),
            (Value::Float(x), Value::Int(y)) => Value::Float(x + (y as f64)),
            (Value::Str(mut x), Value::Str(y)) => {
                let new_len = x.len().saturating_add(y.len());
                if new_len > 64 * 1024 * 1024 {
                    return Err(format!(
                        "[line {}] String size limit (64 MiB) exceeded",
                        line
                    ));
                }
                self.charge_alloc(new_len)?;
                x.push_str(&y);
                Value::Str(x)
            }
            (Value::Bytes(x), Value::Bytes(y)) => {
                let new_len = x.len().saturating_add(y.len());
                if new_len > 64 * 1024 * 1024 {
                    return Err(format!(
                        "[line {}] Bytes size limit (64 MiB) exceeded",
                        line
                    ));
                }
                self.charge_alloc(new_len)?;
                let mut out = Vec::with_capacity(new_len);
                out.extend_from_slice(x.as_slice());
                out.extend_from_slice(y.as_slice());
                Value::Bytes(Rc::new(out))
            }
            (Value::List(x), Value::List(y)) => {
                let (x_ref, y_ref) = (x.borrow(), y.borrow());
                let new_len = x_ref.len().saturating_add(y_ref.len());
                if new_len > 10_000_000 {
                    return Err(format!(
                        "[line {}] List size limit (10,000,000) exceeded",
                        line
                    ));
                }
                self.charge_alloc(new_len.saturating_mul(std::mem::size_of::<Value>()))?;
                let mut combined = Vec::with_capacity(new_len);
                combined.extend(x_ref.iter().cloned());
                combined.extend(y_ref.iter().cloned());
                drop((x_ref, y_ref));
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
        Ok(())
    }

    fn materialize_str_iter(&mut self, s: &str) -> Result<(Option<Value>, i64, i64), String> {
        let count = s.chars().count();
        self.charge_alloc(count.saturating_mul(std::mem::size_of::<Value>().saturating_add(24)))?;
        self.charge_work((count / 16).max(1) as u64)?;
        let chars: Vec<Value> = s.chars().map(|c| Value::Str(c.to_string())).collect();
        let len = chars.len() as i64;
        Ok((Some(Value::List(Rc::new(RefCell::new(chars)))), 0, len))
    }

    fn materialize_set_iter(
        &mut self,
        s: &Rc<RefCell<crate::value::AskaneSet>>,
    ) -> Result<(Option<Value>, i64, i64), String> {
        let len = s.borrow().items.len();
        self.charge_alloc(len.saturating_mul(std::mem::size_of::<Value>()))?;
        let items = s.borrow().items.clone();
        Ok((
            Some(Value::List(Rc::new(RefCell::new(items)))),
            0,
            len as i64,
        ))
    }

    fn restore_sandbox_snapshot(&mut self, snap: SandboxSnapshot) {
        self.permissions = snap.perms;
        self.sandboxed = snap.sandboxed;
        if let Some(extra) = snap.saved_instr_extra {
            self.instructions_left = self.instructions_left.saturating_add(extra);
        }
    }

    fn unwind_cleanups_scope(
        &mut self,
        target_frame_depth: usize,
        min_scope: u16,
        run_defers: bool,
    ) -> Result<(), String> {
        let mut first_err: Option<String> = None;
        while let Some(top) = self.cleanup_stack.last() {
            if top.frame_depth < target_frame_depth
                || (top.frame_depth == target_frame_depth && top.scope_depth < min_scope)
            {
                break;
            }
            let next_idx = self.cleanup_stack.len() - 1;
            if let Some(active_try_len) = self
                .try_handlers
                .iter()
                .rev()
                .find(|h| h.frame_depth == target_frame_depth)
                .map(|h| h.cleanup_len)
            {
                if first_err.is_some() && next_idx < active_try_len {
                    break;
                }
                if next_idx < active_try_len {
                    while self.try_handlers.last().is_some_and(|h| {
                        h.frame_depth == target_frame_depth && h.cleanup_len > next_idx
                    }) {
                        self.try_handlers.pop();
                    }
                }
            }
            let entry = self.cleanup_stack.pop().unwrap();
            match entry.kind {
                CleanupKind::Sandbox(snap) => self.restore_sandbox_snapshot(snap),
                CleanupKind::Defer(closure) => {
                    if run_defers && first_err.is_none() {
                        if let Err(e) = self.call_value(Value::Closure(closure), &[]) {
                            first_err = Some(e);
                        }
                    }
                }
            }
        }
        if let Some(err) = first_err {
            Err(err)
        } else {
            Ok(())
        }
    }

    fn unwind_cleanups_to_len(
        &mut self,
        target_len: usize,
        run_defers: bool,
    ) -> Result<(), String> {
        let mut first_err: Option<String> = None;
        while self.cleanup_stack.len() > target_len {
            let entry = self.cleanup_stack.pop().unwrap();
            match entry.kind {
                CleanupKind::Sandbox(snap) => self.restore_sandbox_snapshot(snap),
                CleanupKind::Defer(closure) => {
                    if run_defers && first_err.is_none() {
                        if let Err(e) = self.call_value(Value::Closure(closure), &[]) {
                            first_err = Some(e);
                        }
                    }
                }
            }
        }
        if let Some(err) = first_err {
            Err(err)
        } else {
            Ok(())
        }
    }

    fn attenuate_permissions(&mut self, policy: &Value) -> Result<SandboxSnapshot, String> {
        let prev_perms = self.permissions.clone();
        let prev_sandboxed = self.sandboxed;
        let mut next = Permissions::sandboxed();
        next.restrict_read_to_explicit_roots = true;
        next.restrict_write_to_explicit_roots = true;
        next.max_instructions = prev_perms.max_instructions;
        next.max_duration_ms = prev_perms.max_duration_ms;
        next.max_memory_bytes = prev_perms.max_memory_bytes;
        next.max_io_bytes = prev_perms.max_io_bytes;

        let mut next_instr_left: Option<u64> = None;
        let mut saved_instr_extra: Option<u64> = None;

        if let Value::Dict(dict_rc) = policy {
            let dict = dict_rc.borrow();
            for (k, v) in dict.iter() {
                match k.as_str() {
                    "read" => match v {
                        Value::Bool(false) | Value::Unit => {
                            next.allow_read = None;
                            next.restrict_read_to_explicit_roots = true;
                        }
                        Value::Bool(true) => {
                            if !prev_perms.allow_all
                                && prev_perms.restrict_read_to_explicit_roots
                                && prev_perms.allow_read.is_none()
                            {
                                return Err("sandbox cannot escalate read access".to_string());
                            }
                            next.allow_read = prev_perms.allow_read.clone();
                            next.restrict_read_to_explicit_roots =
                                prev_perms.restrict_read_to_explicit_roots;
                        }
                        Value::List(list_rc) => {
                            let items = list_rc.borrow();
                            if items.is_empty() {
                                next.allow_read = None;
                                next.restrict_read_to_explicit_roots = true;
                            } else {
                                let mut roots = Vec::with_capacity(items.len());
                                for item in items.iter() {
                                    let Value::Str(p) = item else {
                                        return Err(
                                            "sandbox read policy paths must be strings".to_string()
                                        );
                                    };
                                    let resolved = self.resolve_read_path(p).map_err(|_| {
                                        format!("sandbox cannot escalate read access to '{}'", p)
                                    })?;
                                    roots.push(resolved);
                                }
                                next.allow_read = Some(roots);
                                next.restrict_read_to_explicit_roots = true;
                            }
                        }
                        Value::Str(p) => {
                            let resolved = self.resolve_read_path(p).map_err(|_| {
                                format!("sandbox cannot escalate read access to '{}'", p)
                            })?;
                            next.allow_read = Some(vec![resolved]);
                            next.restrict_read_to_explicit_roots = true;
                        }
                        _ => {
                            return Err(
                                "sandbox 'read' policy must be list, string, or bool".to_string()
                            );
                        }
                    },
                    "write" => match v {
                        Value::Bool(false) | Value::Unit => {
                            next.allow_write = None;
                            next.restrict_write_to_explicit_roots = true;
                        }
                        Value::Bool(true) => {
                            if !prev_perms.allow_all
                                && prev_perms.restrict_write_to_explicit_roots
                                && prev_perms.allow_write.is_none()
                            {
                                return Err("sandbox cannot escalate write access".to_string());
                            }
                            next.allow_write = prev_perms.allow_write.clone();
                            next.restrict_write_to_explicit_roots =
                                prev_perms.restrict_write_to_explicit_roots;
                        }
                        Value::List(list_rc) => {
                            let items = list_rc.borrow();
                            if items.is_empty() {
                                next.allow_write = None;
                                next.restrict_write_to_explicit_roots = true;
                            } else {
                                let mut roots = Vec::with_capacity(items.len());
                                for item in items.iter() {
                                    let Value::Str(p) = item else {
                                        return Err("sandbox write policy paths must be strings"
                                            .to_string());
                                    };
                                    let resolved = self.resolve_write_path(p).map_err(|_| {
                                        format!("sandbox cannot escalate write access to '{}'", p)
                                    })?;
                                    roots.push(resolved);
                                }
                                next.allow_write = Some(roots);
                                next.restrict_write_to_explicit_roots = true;
                            }
                        }
                        Value::Str(p) => {
                            let resolved = self.resolve_write_path(p).map_err(|_| {
                                format!("sandbox cannot escalate write access to '{}'", p)
                            })?;
                            next.allow_write = Some(vec![resolved]);
                            next.restrict_write_to_explicit_roots = true;
                        }
                        _ => {
                            return Err(
                                "sandbox 'write' policy must be list, string, or bool".to_string()
                            );
                        }
                    },
                    "net" => match v {
                        Value::Bool(false) | Value::Unit => next.allow_net = None,
                        Value::Bool(true) => {
                            if !prev_perms.allow_all && prev_perms.allow_net.is_none() {
                                return Err("sandbox cannot escalate network access".to_string());
                            }
                            next.allow_net = prev_perms.allow_net.clone();
                        }
                        Value::List(list_rc) => {
                            let items = list_rc.borrow();
                            if items.is_empty() {
                                next.allow_net = None;
                            } else {
                                let mut hosts = Vec::with_capacity(items.len());
                                for item in items.iter() {
                                    let Value::Str(h) = item else {
                                        return Err(
                                            "sandbox net policy hosts must be strings".to_string()
                                        );
                                    };
                                    let url = if h.starts_with("https://") {
                                        h.clone()
                                    } else {
                                        format!("https://{}", h)
                                    };
                                    self.check_net(&url).map_err(|_| {
                                        format!("sandbox cannot escalate net access to '{}'", h)
                                    })?;
                                    hosts.push(h.clone());
                                }
                                next.allow_net = Some(hosts);
                            }
                        }
                        _ => return Err("sandbox 'net' policy must be bool or list".to_string()),
                    },
                    "env" => match v {
                        Value::Bool(false) | Value::Unit => next.allow_env = None,
                        Value::Bool(true) => {
                            if !prev_perms.allow_all && prev_perms.allow_env.is_none() {
                                return Err("sandbox cannot escalate env access".to_string());
                            }
                            next.allow_env = prev_perms.allow_env.clone();
                        }
                        Value::List(list_rc) => {
                            let items = list_rc.borrow();
                            if items.is_empty() {
                                next.allow_env = None;
                            } else {
                                let mut keys = Vec::with_capacity(items.len());
                                for item in items.iter() {
                                    let Value::Str(key) = item else {
                                        return Err(
                                            "sandbox env policy keys must be strings".to_string()
                                        );
                                    };
                                    self.check_env(key).map_err(|_| {
                                        format!("sandbox cannot escalate env access to '{}'", key)
                                    })?;
                                    keys.push(key.clone());
                                }
                                next.allow_env = Some(keys);
                            }
                        }
                        _ => return Err("sandbox 'env' policy must be bool or list".to_string()),
                    },
                    "exec" => match v {
                        Value::Bool(false) | Value::Unit => next.allow_exec = None,
                        Value::Bool(true) => {
                            if !prev_perms.allow_all && prev_perms.allow_exec.is_none() {
                                return Err("sandbox cannot escalate process exec".to_string());
                            }
                            next.allow_exec = prev_perms.allow_exec.clone();
                        }
                        Value::List(list_rc) => {
                            let items = list_rc.borrow();
                            if items.is_empty() {
                                next.allow_exec = None;
                            } else {
                                let mut cmds = Vec::with_capacity(items.len());
                                for item in items.iter() {
                                    let Value::Str(c) = item else {
                                        return Err("sandbox exec policy commands must be strings"
                                            .to_string());
                                    };
                                    self.check_exec(c).map_err(|_| {
                                        format!("sandbox cannot escalate exec access to '{}'", c)
                                    })?;
                                    cmds.push(c.clone());
                                }
                                next.allow_exec = Some(cmds);
                            }
                        }
                        _ => return Err("sandbox 'exec' policy must be bool or list".to_string()),
                    },
                    "clipboard" => match v {
                        Value::Bool(false) | Value::Unit => next.allow_clipboard = false,
                        Value::Bool(true) => {
                            self.check_clipboard().map_err(|_| {
                                "sandbox cannot escalate clipboard access".to_string()
                            })?;
                            next.allow_clipboard = true;
                        }
                        _ => return Err("sandbox 'clipboard' policy must be bool".to_string()),
                    },
                    "input" => match v {
                        Value::Bool(false) | Value::Unit => next.allow_input = false,
                        Value::Bool(true) => {
                            self.check_input()
                                .map_err(|_| "sandbox cannot escalate input access".to_string())?;
                            next.allow_input = true;
                        }
                        _ => return Err("sandbox 'input' policy must be bool".to_string()),
                    },
                    "args" => match v {
                        Value::Bool(false) | Value::Unit => next.allow_sys_args = false,
                        Value::Bool(true) => {
                            self.check_sys_args().map_err(|_| {
                                "sandbox cannot escalate sys.args access".to_string()
                            })?;
                            next.allow_sys_args = true;
                        }
                        _ => return Err("sandbox 'args' policy must be bool".to_string()),
                    },
                    "timeout" => match v {
                        Value::Int(ms) if *ms > 0 => {
                            let elapsed = self.run_start.elapsed().as_millis() as u64;
                            let desired = elapsed.saturating_add(*ms as u64);
                            next.max_duration_ms = Some(match prev_perms.max_duration_ms {
                                Some(prev) => prev.min(desired),
                                None => desired,
                            });
                        }
                        _ => return Err("sandbox 'timeout' must be positive int (ms)".to_string()),
                    },
                    "max_mem" => match v {
                        Value::Int(mb) if *mb > 0 => {
                            let cap = self
                                .budget
                                .bytes_allocated
                                .saturating_add((*mb as u64).saturating_mul(1024 * 1024));
                            next.max_memory_bytes = Some(match prev_perms.max_memory_bytes {
                                Some(prev) => prev.min(cap),
                                None => cap,
                            });
                        }
                        _ => return Err("sandbox 'max_mem' must be positive int (MiB)".to_string()),
                    },
                    "max_steps" => match v {
                        Value::Int(steps) if *steps > 0 => {
                            let req = *steps as u64;
                            next.max_instructions = Some(match prev_perms.max_instructions {
                                Some(prev) => prev.min(req),
                                None => req,
                            });
                            let base_left = next_instr_left.unwrap_or(self.instructions_left);
                            if base_left > req {
                                saved_instr_extra = Some(
                                    saved_instr_extra
                                        .unwrap_or(0)
                                        .saturating_add(base_left - req),
                                );
                                next_instr_left = Some(req);
                            }
                        }
                        _ => return Err("sandbox 'max_steps' must be positive int".to_string()),
                    },
                    other => {
                        return Err(format!("Unknown sandbox policy option '{}'", other));
                    }
                }
            }
        } else if !matches!(policy, Value::Unit) {
            return Err("sandbox policy must be a dictionary".to_string());
        }

        if let Some(left) = next_instr_left {
            self.instructions_left = left;
        }
        self.permissions = next;
        self.sandboxed = true;
        Ok(SandboxSnapshot {
            perms: prev_perms,
            sandboxed: prev_sandboxed,
            saved_instr_extra,
        })
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

        let import_base = self
            .frames
            .last()
            .and_then(|f| f.closure.base_dir.clone())
            .unwrap_or_else(|| self.current_dir.clone());

        let raw_path = PathBuf::from(source);
        let file_path = if raw_path.extension().is_none() {
            let mod_candidate = raw_path.join("mod.ask");
            if let Ok(resolved_mod) =
                self.resolve_read_path_from(&import_base, &mod_candidate.to_string_lossy())
            {
                if resolved_mod.is_file() {
                    resolved_mod
                } else {
                    let mut ask_path = raw_path;
                    ask_path.set_extension("ask");
                    self.resolve_read_path_from(&import_base, &ask_path.to_string_lossy())?
                }
            } else {
                let mut ask_path = raw_path;
                ask_path.set_extension("ask");
                self.resolve_read_path_from(&import_base, &ask_path.to_string_lossy())?
            }
        } else {
            self.resolve_read_path_from(&import_base, &raw_path.to_string_lossy())?
        };

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

        let mod_dir = file_path.parent().map(|p| p.to_path_buf());
        let prev_dir = self.current_dir.clone();
        if let Some(parent) = &mod_dir {
            self.current_dir = parent.clone();
        }
        let result = self.run_with_base_dir(proto, mod_dir);
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

    fn ensure_main_task_slot(&mut self) {
        if !self.scheduler.tasks.contains_key(&0) {
            self.scheduler.tasks.insert(
                0,
                GreenTask {
                    stack: Vec::new(),
                    frames: Vec::new(),
                    try_handlers: Vec::new(),
                    cleanup_stack: Vec::new(),
                    open_upvalues: Vec::new(),
                    permissions: self.permissions.clone(),
                    sandboxed: self.sandboxed,
                    wait: TaskWait::Ready,
                    wake_value: None,
                    result: None,
                },
            );
        }
    }

    fn save_current_task_state(&mut self, wait: TaskWait) {
        self.ensure_main_task_slot();
        if !self.open_upvalues.is_empty() {
            self.close_upvalues_from(0);
        }
        let cur_id = self.scheduler.current_task_id;
        let stack = std::mem::take(&mut self.stack);
        let frames = std::mem::take(&mut self.frames);
        let try_handlers = std::mem::take(&mut self.try_handlers);
        let cleanup_stack = std::mem::take(&mut self.cleanup_stack);
        let open_upvalues = std::mem::take(&mut self.open_upvalues);
        let perms = self.permissions.clone();
        let sandboxed = self.sandboxed;
        if let Some(t) = self.scheduler.tasks.get_mut(&cur_id) {
            t.stack = stack;
            t.frames = frames;
            t.try_handlers = try_handlers;
            t.cleanup_stack = cleanup_stack;
            t.open_upvalues = open_upvalues;
            t.permissions = perms;
            t.sandboxed = sandboxed;
            t.wait = wait;
        }
    }

    fn load_task_state(&mut self, next_id: u64) -> Result<Option<Value>, String> {
        let t = self
            .scheduler
            .tasks
            .get_mut(&next_id)
            .ok_or_else(|| "Scheduler error: missing target task".to_string())?;
        self.stack = std::mem::take(&mut t.stack);
        self.frames = std::mem::take(&mut t.frames);
        self.try_handlers = std::mem::take(&mut t.try_handlers);
        self.cleanup_stack = std::mem::take(&mut t.cleanup_stack);
        self.open_upvalues = std::mem::take(&mut t.open_upvalues);
        self.permissions = t.permissions.clone();
        self.sandboxed = t.sandboxed;
        t.wait = TaskWait::Ready;
        self.scheduler.current_task_id = next_id;
        match t.wake_value.take() {
            Some(Ok(v)) => Ok(Some(v)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    fn pick_next_ready_or_advance(&mut self) -> Result<u64, String> {
        loop {
            self.check_timeout()?;
            while let Some(id) = self.scheduler.ready_queue.pop_front() {
                if let Some(t) = self.scheduler.tasks.get(&id) {
                    if matches!(t.wait, TaskWait::Ready)
                        && (!t.frames.is_empty() || t.result.is_none())
                    {
                        return Ok(id);
                    }
                }
            }

            let mut earliest: Option<u64> = None;
            for t in self.scheduler.tasks.values() {
                if let TaskWait::Sleeping(deadline) = t.wait {
                    earliest = Some(earliest.map_or(deadline, |e| e.min(deadline)));
                }
            }
            let Some(deadline) = earliest else {
                return Err("Deadlock: all tasks are blocked".to_string());
            };
            let now = self.current_clock_ms();
            if now < deadline {
                let delta = deadline - now;
                if let Some(vt) = self.virtual_time_ms.as_mut() {
                    let next = deadline;
                    if let Some(limit_ms) = self.permissions.max_duration_ms {
                        if next > limit_ms {
                            *vt = limit_ms;
                            return Err(format!("Execution timeout ({} ms exceeded)", limit_ms));
                        }
                    }
                    *vt = next;
                } else {
                    self.sleep_checked(delta.min(25))?;
                }
            }
            let now_after = self.current_clock_ms();
            let mut woken = Vec::new();
            for (&id, t) in self.scheduler.tasks.iter_mut() {
                if let TaskWait::Sleeping(d) = t.wait {
                    if d <= now_after {
                        t.wait = TaskWait::Ready;
                        t.wake_value = Some(Ok(Value::Unit));
                        woken.push(id);
                    }
                }
            }
            woken.sort_unstable();
            for id in woken {
                self.scheduler.ready_queue.push_back(id);
            }
        }
    }

    fn suspend_and_switch(&mut self, req: YieldRequest) -> Result<Option<Value>, String> {
        let cur_id = self.scheduler.current_task_id;
        let wait_kind = match req {
            YieldRequest::Yield => {
                self.scheduler.ready_queue.push_back(cur_id);
                TaskWait::Ready
            }
            YieldRequest::Sleep(ms) => {
                if self.virtual_time_ms.is_none() && self.is_sandboxed() && ms > 1000 {
                    return Err("Sandbox sleep limit is 1000 ms per call".to_string());
                }
                let deadline = self.current_clock_ms().saturating_add(ms);
                TaskWait::Sleeping(deadline)
            }
            YieldRequest::Join(target_id) => {
                self.scheduler
                    .join_waiters
                    .entry(target_id)
                    .or_default()
                    .push(cur_id);
                TaskWait::Join(target_id)
            }
            YieldRequest::ChanSend { ch_id, val } => {
                let ch = self
                    .scheduler
                    .channels
                    .get_mut(&ch_id)
                    .ok_or_else(|| "channel.send(): invalid channel".to_string())?;
                ch.send_waiters.push_back((cur_id, val));
                TaskWait::ChanSend(ch_id)
            }
            YieldRequest::ChanRecv { ch_id } => {
                let ch = self
                    .scheduler
                    .channels
                    .get_mut(&ch_id)
                    .ok_or_else(|| "channel.recv(): invalid channel".to_string())?;
                ch.recv_waiters.push_back(cur_id);
                TaskWait::ChanRecv(ch_id)
            }
            YieldRequest::SelectRecv { ch_ids } => {
                for &cid in &ch_ids {
                    if let Some(ch) = self.scheduler.channels.get_mut(&cid) {
                        ch.recv_waiters.push_back(cur_id);
                    }
                }
                TaskWait::SelectRecv(ch_ids)
            }
        };
        let mut wake_on_yield = matches!(wait_kind, TaskWait::Ready);
        self.save_current_task_state(wait_kind);
        if wake_on_yield {
            if let Some(t) = self.scheduler.tasks.get_mut(&cur_id) {
                t.wake_value = Some(Ok(Value::Unit));
            }
            wake_on_yield = false;
            let _ = wake_on_yield;
        }
        let next_id = self.pick_next_ready_or_advance()?;
        self.load_task_state(next_id)
    }

    fn resume_after_suspend(
        &mut self,
        req: YieldRequest,
        target_depth: usize,
    ) -> Result<(), String> {
        match self.suspend_and_switch(req) {
            Ok(wake_val) => {
                if self.frames.len() == target_depth
                    && target_depth == 0
                    && self.scheduler.current_task_id != 0
                {
                    return self.finish_current_green_task(Ok(wake_val.unwrap_or(Value::Unit)));
                }
                if let Some(v) = wake_val {
                    self.stack.push(v);
                }
                Ok(())
            }
            Err(err) => {
                if self.frames.len() == target_depth
                    && target_depth == 0
                    && self.scheduler.current_task_id != 0
                {
                    return self.finish_current_green_task(Err(err));
                }
                Err(err)
            }
        }
    }

    fn finish_current_green_task(
        &mut self,
        mut outcome: Result<Value, String>,
    ) -> Result<(), String> {
        loop {
            let cur_id = self.scheduler.current_task_id;
            let waiters = self
                .scheduler
                .join_waiters
                .remove(&cur_id)
                .unwrap_or_default();
            self.save_current_task_state(TaskWait::Done);
            if let Some(t) = self.scheduler.tasks.get_mut(&cur_id) {
                t.result = Some(outcome.clone());
                t.stack.clear();
                t.frames.clear();
                t.try_handlers.clear();
                t.cleanup_stack.clear();
            }
            for wid in waiters {
                if let Some(wt) = self.scheduler.tasks.get_mut(&wid) {
                    if matches!(wt.wait, TaskWait::Join(tid) if tid == cur_id) {
                        wt.wait = TaskWait::Ready;
                        wt.wake_value = Some(outcome.clone());
                        self.scheduler.ready_queue.push_back(wid);
                    }
                }
            }
            let next_id = self.pick_next_ready_or_advance()?;
            match self.load_task_state(next_id) {
                Ok(wake) => {
                    if self.frames.is_empty() && self.scheduler.current_task_id != 0 {
                        outcome = Ok(wake.unwrap_or(Value::Unit));
                        continue;
                    }
                    if let Some(val) = wake {
                        self.stack.push(val);
                    }
                    return Ok(());
                }
                Err(err) => {
                    if self.frames.is_empty() && self.scheduler.current_task_id != 0 {
                        outcome = Err(err);
                        continue;
                    }
                    return Err(err);
                }
            }
        }
    }

    fn select_result_dict(idx: usize, val: Value, closed: bool) -> Value {
        let mut map = HashMap::new();
        map.insert("index".to_string(), Value::Int(idx as i64));
        map.insert("value".to_string(), val);
        map.insert("closed".to_string(), Value::Bool(closed));
        Value::Dict(Rc::new(RefCell::new(map)))
    }

    pub(crate) fn spawn_green_task(
        &mut self,
        callee: Value,
        args: &[Value],
    ) -> Result<u64, String> {
        if self.scheduler.tasks.len() >= MAX_GREEN_TASKS {
            return Err(format!(
                "task.spawn(): maximum active/completed tasks ({}) exceeded",
                MAX_GREEN_TASKS
            ));
        }
        let Value::Closure(closure) = callee else {
            return Err(format!(
                "task.spawn() expects a function/closure, got '{}'",
                callee.type_name()
            ));
        };
        let (slots, bits) = self.build_closure_slots(&closure, args, None)?;
        self.ensure_main_task_slot();
        let id = self.scheduler.next_task_id;
        self.scheduler.next_task_id += 1;

        let mut stack = Vec::with_capacity(slots.len() + 1);
        stack.push(Value::Closure(closure.clone()));
        stack.extend(slots);
        let frames = vec![CallFrame {
            closure,
            ip: 0,
            base: 0,
            arg_bits: bits,
            enter_step: self.step_counter,
            enter_time: if self.profile_enabled {
                Some(Instant::now())
            } else {
                None
            },
        }];
        self.scheduler.tasks.insert(
            id,
            GreenTask {
                stack,
                frames,
                try_handlers: Vec::new(),
                cleanup_stack: Vec::new(),
                open_upvalues: Vec::new(),
                permissions: self.permissions.clone(),
                sandboxed: self.sandboxed,
                wait: TaskWait::Ready,
                wake_value: None,
                result: None,
            },
        );
        self.scheduler.ready_queue.push_back(id);
        Ok(id)
    }

    pub(crate) fn yield_current_task(&mut self) -> Result<(), String> {
        if self.execute_depth != 1 {
            return Err("Cannot yield task from inside nested native callback".to_string());
        }
        if !self.scheduler.tasks.is_empty() {
            self.scheduler.pending_yield = Some(YieldRequest::Yield);
        }
        Ok(())
    }

    pub(crate) fn sleep_green_task(&mut self, ms: u64) -> Result<(), String> {
        if self.execute_depth == 1 && !self.scheduler.tasks.is_empty() {
            self.scheduler.pending_yield = Some(YieldRequest::Sleep(ms));
            return Ok(());
        }
        self.sleep_bounded(ms)
    }

    pub(crate) fn join_green_task(&mut self, target_id: u64) -> Result<Value, String> {
        if target_id == self.scheduler.current_task_id {
            return Err("task.join(): cannot join current task".to_string());
        }
        let task = self
            .scheduler
            .tasks
            .get(&target_id)
            .ok_or_else(|| "task.join(): unknown task handle".to_string())?;
        if let Some(res) = &task.result {
            return res.clone();
        }
        if self.execute_depth != 1 {
            return Err(
                "Cannot join unfinished task from inside nested native callback".to_string(),
            );
        }
        self.scheduler.pending_yield = Some(YieldRequest::Join(target_id));
        Ok(Value::Unit)
    }

    pub(crate) fn is_task_done(&self, target_id: u64) -> bool {
        self.scheduler
            .tasks
            .get(&target_id)
            .is_some_and(|t| t.result.is_some())
    }

    pub(crate) fn run_with_timeout(&mut self, ms: u64, callee: Value) -> Result<Value, String> {
        let prev_limit = self.permissions.max_duration_ms;
        let now = self.current_clock_ms();
        let target_limit = now.saturating_add(ms);
        self.permissions.max_duration_ms =
            Some(prev_limit.map_or(target_limit, |p| p.min(target_limit)));
        let res = self.call_value(callee, &[]);
        self.permissions.max_duration_ms = prev_limit;
        self.check_timeout()?;

        let mut map = HashMap::new();
        match res {
            Ok(v) => {
                map.insert("ok".to_string(), Value::Bool(true));
                map.insert("value".to_string(), v);
            }
            Err(e) => {
                if Self::is_unrecoverable_error(&e) && !e.contains("Execution timeout (") {
                    return Err(e);
                }
                map.insert("ok".to_string(), Value::Bool(false));
                map.insert("error".to_string(), Value::Str(e));
            }
        }
        Ok(Value::Dict(Rc::new(RefCell::new(map))))
    }

    pub(crate) fn create_channel(&mut self, capacity: usize) -> Result<u64, String> {
        if self.scheduler.channels.len() >= MAX_CHANNELS {
            return Err(format!(
                "channel.bounded(): maximum channels ({}) exceeded",
                MAX_CHANNELS
            ));
        }
        let id = self.scheduler.next_chan_id;
        self.scheduler.next_chan_id += 1;
        self.scheduler.channels.insert(
            id,
            ChannelState {
                capacity,
                queue: VecDeque::with_capacity(capacity.min(64)),
                closed: false,
                send_waiters: VecDeque::new(),
                recv_waiters: VecDeque::new(),
            },
        );
        Ok(id)
    }

    fn wake_one_receiver(&mut self, ch_id: u64, val: Value, closed: bool) -> bool {
        loop {
            let rx_id = {
                let Some(ch) = self.scheduler.channels.get_mut(&ch_id) else {
                    return false;
                };
                ch.recv_waiters.pop_front()
            };
            let Some(tid) = rx_id else {
                return false;
            };
            if let Some(t) = self.scheduler.tasks.get_mut(&tid) {
                match &t.wait {
                    TaskWait::ChanRecv(c) if *c == ch_id => {
                        t.wait = TaskWait::Ready;
                        t.wake_value = Some(Ok(val));
                        self.scheduler.ready_queue.push_back(tid);
                        return true;
                    }
                    TaskWait::SelectRecv(list) => {
                        if let Some(idx) = list.iter().position(|&c| c == ch_id) {
                            t.wait = TaskWait::Ready;
                            t.wake_value = Some(Ok(Self::select_result_dict(idx, val, closed)));
                            self.scheduler.ready_queue.push_back(tid);
                            return true;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn promote_one_sender(&mut self, ch_id: u64) {
        loop {
            let item = {
                let Some(ch) = self.scheduler.channels.get_mut(&ch_id) else {
                    return;
                };
                if ch.queue.len() >= ch.capacity {
                    return;
                }
                ch.send_waiters.pop_front()
            };
            let Some((tid, val)) = item else {
                return;
            };
            let is_waiting = self
                .scheduler
                .tasks
                .get(&tid)
                .is_some_and(|t| matches!(t.wait, TaskWait::ChanSend(c) if c == ch_id));
            if is_waiting {
                if let Some(ch) = self.scheduler.channels.get_mut(&ch_id) {
                    ch.queue.push_back(val);
                }
                if let Some(t) = self.scheduler.tasks.get_mut(&tid) {
                    t.wait = TaskWait::Ready;
                    t.wake_value = Some(Ok(Value::Unit));
                }
                self.scheduler.ready_queue.push_back(tid);
                return;
            }
        }
    }

    pub(crate) fn channel_send(&mut self, ch_id: u64, val: Value) -> Result<(), String> {
        let closed = self
            .scheduler
            .channels
            .get(&ch_id)
            .ok_or_else(|| "channel.send(): invalid channel".to_string())?
            .closed;
        if closed {
            return Err("channel.send(): channel is closed".to_string());
        }
        if self.wake_one_receiver(ch_id, val.clone(), false) {
            return Ok(());
        }
        let has_room = {
            let ch = self.scheduler.channels.get_mut(&ch_id).unwrap();
            if ch.queue.len() < ch.capacity {
                ch.queue.push_back(val.clone());
                true
            } else {
                false
            }
        };
        if has_room {
            return Ok(());
        }
        if self.execute_depth != 1 {
            return Err("channel.send(): channel is full inside native callback".to_string());
        }
        self.scheduler.pending_yield = Some(YieldRequest::ChanSend { ch_id, val });
        Ok(())
    }

    pub(crate) fn channel_try_send(&mut self, ch_id: u64, val: Value) -> Result<bool, String> {
        let closed = self
            .scheduler
            .channels
            .get(&ch_id)
            .ok_or_else(|| "channel.try_send(): invalid channel".to_string())?
            .closed;
        if closed {
            return Err("channel.try_send(): channel is closed".to_string());
        }
        if self.wake_one_receiver(ch_id, val.clone(), false) {
            return Ok(true);
        }
        let ch = self.scheduler.channels.get_mut(&ch_id).unwrap();
        if ch.queue.len() < ch.capacity {
            ch.queue.push_back(val);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub(crate) fn channel_recv(&mut self, ch_id: u64) -> Result<Value, String> {
        let (popped, closed) = {
            let ch = self
                .scheduler
                .channels
                .get_mut(&ch_id)
                .ok_or_else(|| "channel.recv(): invalid channel".to_string())?;
            (ch.queue.pop_front(), ch.closed)
        };
        if let Some(v) = popped {
            self.promote_one_sender(ch_id);
            return Ok(v);
        }
        if closed {
            return Ok(Value::Unit);
        }
        if self.execute_depth != 1 {
            return Err("channel.recv(): channel is empty inside native callback".to_string());
        }
        self.scheduler.pending_yield = Some(YieldRequest::ChanRecv { ch_id });
        Ok(Value::Unit)
    }

    pub(crate) fn channel_try_recv(&mut self, ch_id: u64) -> Result<(bool, Value, bool), String> {
        let (popped, closed) = {
            let ch = self
                .scheduler
                .channels
                .get_mut(&ch_id)
                .ok_or_else(|| "channel.try_recv(): invalid channel".to_string())?;
            (ch.queue.pop_front(), ch.closed)
        };
        if let Some(v) = popped {
            self.promote_one_sender(ch_id);
            Ok((true, v, false))
        } else {
            Ok((false, Value::Unit, closed))
        }
    }

    pub(crate) fn channel_close(&mut self, ch_id: u64) -> Result<(), String> {
        let ch = self
            .scheduler
            .channels
            .get_mut(&ch_id)
            .ok_or_else(|| "channel.close(): invalid channel".to_string())?;
        if ch.closed {
            return Ok(());
        }
        ch.closed = true;
        let senders: Vec<(u64, Value)> = ch.send_waiters.drain(..).collect();
        for (sid, _) in senders {
            if let Some(t) = self.scheduler.tasks.get_mut(&sid) {
                if matches!(t.wait, TaskWait::ChanSend(c) if c == ch_id) {
                    t.wait = TaskWait::Ready;
                    t.wake_value = Some(Err("channel.send(): channel closed".to_string()));
                    self.scheduler.ready_queue.push_back(sid);
                }
            }
        }
        while self.wake_one_receiver(ch_id, Value::Unit, true) {}
        Ok(())
    }

    pub(crate) fn channel_len(&self, ch_id: u64) -> Result<usize, String> {
        let ch = self
            .scheduler
            .channels
            .get(&ch_id)
            .ok_or_else(|| "channel.len(): invalid channel".to_string())?;
        Ok(ch.queue.len())
    }

    pub(crate) fn channel_is_closed(&self, ch_id: u64) -> Result<bool, String> {
        let ch = self
            .scheduler
            .channels
            .get(&ch_id)
            .ok_or_else(|| "channel.is_closed(): invalid channel".to_string())?;
        Ok(ch.closed)
    }

    pub(crate) fn select_channels(&mut self, ch_ids: &[u64]) -> Result<Value, String> {
        for (idx, &ch_id) in ch_ids.iter().enumerate() {
            let (popped, closed) = {
                let ch = self
                    .scheduler
                    .channels
                    .get_mut(&ch_id)
                    .ok_or_else(|| "task.select(): invalid channel".to_string())?;
                (ch.queue.pop_front(), ch.closed)
            };
            if let Some(v) = popped {
                self.promote_one_sender(ch_id);
                return Ok(Self::select_result_dict(idx, v, false));
            }
            if closed {
                return Ok(Self::select_result_dict(idx, Value::Unit, true));
            }
        }
        if self.execute_depth != 1 {
            return Err("task.select(): cannot block inside nested native callback".to_string());
        }
        self.scheduler.pending_yield = Some(YieldRequest::SelectRecv {
            ch_ids: ch_ids.to_vec(),
        });
        Ok(Value::Unit)
    }
}

fn native_print(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            print!(" ");
        }
        print!("{}", vm.format_value_protocol(arg)?);
    }
    let _ = io::stdout().flush();
    Ok(Value::Unit)
}

fn native_println(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            print!(" ");
        }
        print!("{}", vm.format_value_protocol(arg)?);
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
        Value::Bytes(b) => Ok(Value::Int(b.len() as i64)),
        Value::List(l) => Ok(Value::Int(l.borrow().len() as i64)),
        Value::Dict(d) => Ok(Value::Int(d.borrow().len() as i64)),
        Value::Set(s) => Ok(Value::Int(s.borrow().items.len() as i64)),
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

fn native_push(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("push() expects 2 arguments, got {}", args.len()));
    }
    match &args[0] {
        Value::List(l) => {
            if l.borrow().len() >= 10_000_000 {
                return Err("push() list size exceeds maximum limit of 10,000,000".to_string());
            }
            vm.charge_alloc(std::mem::size_of::<Value>())?;
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

fn collect_iterable(vm: &mut VM, val: &Value, fn_name: &str) -> Result<Vec<Value>, String> {
    collect_iterable_inner(vm, val, fn_name, true)
}

fn collect_iterable_inner(
    vm: &mut VM,
    val: &Value,
    fn_name: &str,
    allow_struct_iter: bool,
) -> Result<Vec<Value>, String> {
    match val {
        Value::List(l) => {
            let len = l.borrow().len();
            vm.charge_alloc(len.saturating_mul(std::mem::size_of::<Value>()))?;
            Ok(l.borrow().clone())
        }
        Value::Set(s) => {
            let len = s.borrow().items.len();
            vm.charge_alloc(len.saturating_mul(std::mem::size_of::<Value>()))?;
            Ok(s.borrow().items.clone())
        }
        Value::Str(s) => {
            let len = s.chars().count();
            vm.charge_alloc(len.saturating_mul(std::mem::size_of::<Value>()))?;
            Ok(s.chars().map(|c| Value::Str(c.to_string())).collect())
        }
        Value::Bytes(b) => {
            if b.len() > 10_000_000 {
                return Err(format!(
                    "{}() bytes size exceeds maximum limit of 10,000,000",
                    fn_name
                ));
            }
            vm.charge_alloc(b.len().saturating_mul(std::mem::size_of::<Value>()))?;
            Ok(b.iter().map(|&x| Value::Int(x as i64)).collect())
        }
        Value::Range { start, end } => {
            let count = if *end > *start {
                (*end as i128 - *start as i128) as u128
            } else {
                0
            };
            if count > 10_000_000 {
                return Err(format!(
                    "{}() range size exceeds maximum limit of 10,000,000",
                    fn_name
                ));
            }
            vm.charge_alloc((count as usize).saturating_mul(std::mem::size_of::<Value>()))?;
            Ok((*start..*end).map(Value::Int).collect())
        }
        Value::StructInstance(inst) if allow_struct_iter => {
            let Some(iter_fn) = inst.def.methods.get("iter").cloned() else {
                return Err(format!(
                    "{}() expects an iterable collection, got '{}'",
                    fn_name, inst.def.name
                ));
            };
            let next_val = vm.call_value(iter_fn, &[val.clone()])?;
            collect_iterable_inner(vm, &next_val, fn_name, false)
        }
        other => Err(format!(
            "{}() expects a list, set, bytes, or range, got '{}'",
            fn_name,
            other.type_name()
        )),
    }
}

fn native_map(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("map() expects 2 arguments, got {}", args.len()));
    }
    let items = collect_iterable(vm, &args[0], "map")?;
    let func = args[1].clone();
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.into_iter().enumerate() {
        if (i & 1023) == 0 {
            vm.check_timeout()?;
        }
        out.push(vm.call_value(func.clone(), &[item])?);
    }
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn native_filter(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("filter() expects 2 arguments, got {}", args.len()));
    }
    let items = collect_iterable(vm, &args[0], "filter")?;
    let func = args[1].clone();
    let mut out = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        if (i & 1023) == 0 {
            vm.check_timeout()?;
        }
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
    let items = collect_iterable(vm, &args[0], "reduce")?;
    let mut acc = args[1].clone();
    let func = args[2].clone();
    for (i, item) in items.into_iter().enumerate() {
        if (i & 1023) == 0 {
            vm.check_timeout()?;
        }
        acc = vm.call_value(func.clone(), &[acc, item])?;
    }
    Ok(acc)
}

fn native_str(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("str() expects 1 argument, got {}", args.len()));
    }
    let s = vm.format_value_protocol(&args[0])?;
    vm.charge_alloc(s.len())?;
    Ok(Value::Str(s))
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

fn native_bytes(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("bytes() expects 1 argument, got {}", args.len()));
    }
    match &args[0] {
        Value::Bytes(b) => Ok(Value::Bytes(b.clone())),
        Value::Int(n) => {
            if *n < 0 || *n > 64 * 1024 * 1024 {
                return Err("bytes() length must be between 0 and 64 MiB".to_string());
            }
            let len = *n as usize;
            vm.charge_alloc(len)?;
            Ok(Value::Bytes(Rc::new(vec![0u8; len])))
        }
        other => {
            let raw = other.as_bytes_cow("bytes()")?;
            if raw.len() > 64 * 1024 * 1024 {
                return Err("bytes() size exceeds 64 MiB limit".to_string());
            }
            vm.charge_alloc(raw.len())?;
            Ok(Value::Bytes(Rc::new(raw.into_owned())))
        }
    }
}

fn native_type_of(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("type_of() expects 1 argument, got {}", args.len()));
    }
    Ok(Value::Str(args[0].type_name().to_string()))
}

fn native_keys(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("keys() expects 1 argument, got {}", args.len()));
    }
    match &args[0] {
        Value::Dict(d) => {
            let key_bytes: usize = {
                let map = d.borrow();
                map.keys()
                    .map(|k| std::mem::size_of::<Value>().saturating_add(k.len()))
                    .sum()
            };
            vm.charge_alloc(key_bytes)?;
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

fn native_values(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("values() expects 1 argument, got {}", args.len()));
    }
    match &args[0] {
        Value::Dict(d) => {
            let count = d.borrow().len();
            vm.charge_alloc(count.saturating_mul(std::mem::size_of::<Value>()))?;
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

fn native_entries(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("entries() expects 1 argument, got {}", args.len()));
    }
    match &args[0] {
        Value::Dict(d) => {
            let bytes: usize = {
                let map = d.borrow();
                map.keys()
                    .map(|k| {
                        std::mem::size_of::<Value>()
                            .saturating_mul(3)
                            .saturating_add(k.len())
                            .saturating_add(24)
                    })
                    .sum()
            };
            vm.charge_alloc(bytes)?;
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

fn native_sort(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("sort() expects 1 argument, got {}", args.len()));
    }
    vm.check_timeout()?;
    let list = match &args[0] {
        Value::List(l) => l,
        other => {
            return Err(format!(
                "sort() expects a list, got '{}'",
                other.type_name()
            ));
        }
    };
    let item_count = list.borrow().len();
    vm.charge_alloc(item_count.saturating_mul(std::mem::size_of::<Value>()))?;
    vm.charge_work((item_count / 16).max(1) as u64)?;
    let mut items = list.borrow().clone();
    let mut err: Option<String> = None;
    let mut cmp_count = 0usize;
    items.sort_by(|a, b| {
        if err.is_some() {
            return std::cmp::Ordering::Equal;
        }
        cmp_count += 1;
        if (cmp_count & 4095) == 0 {
            if let Err(e) = vm.check_timeout() {
                err = Some(e);
                return std::cmp::Ordering::Equal;
            }
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

fn native_reverse(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("reverse() expects 1 argument, got {}", args.len()));
    }
    match &args[0] {
        Value::List(l) => {
            let count = l.borrow().len();
            vm.charge_alloc(count.saturating_mul(std::mem::size_of::<Value>()))?;
            let mut items = l.borrow().clone();
            items.reverse();
            Ok(Value::List(Rc::new(RefCell::new(items))))
        }
        Value::Str(s) => {
            vm.charge_alloc(s.len())?;
            Ok(Value::Str(s.chars().rev().collect()))
        }
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

fn native_enumerate(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!(
            "enumerate() expects 1 argument, got {}",
            args.len()
        ));
    }
    let items = collect_iterable(vm, &args[0], "enumerate")?;
    vm.charge_alloc(items.len().saturating_mul(std::mem::size_of::<Value>() * 2))?;
    let out: Vec<Value> = items
        .into_iter()
        .enumerate()
        .map(|(i, v)| Value::List(Rc::new(RefCell::new(vec![Value::Int(i as i64), v]))))
        .collect();
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn native_zip(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("zip() expects 2 arguments, got {}", args.len()));
    }
    let a = collect_iterable(vm, &args[0], "zip")?;
    let b = collect_iterable(vm, &args[1], "zip")?;
    let pairs = a.len().min(b.len());
    vm.charge_alloc(pairs.saturating_mul(std::mem::size_of::<Value>() * 2))?;
    let out: Vec<Value> = a
        .into_iter()
        .zip(b)
        .map(|(x, y)| Value::List(Rc::new(RefCell::new(vec![x, y]))))
        .collect();
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn native_take(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
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
            let count = (max_end as i128 - *start as i128).max(0) as usize;
            vm.charge_alloc(count.saturating_mul(std::mem::size_of::<Value>()))?;
            let out: Vec<Value> = (*start..max_end).map(Value::Int).collect();
            Ok(Value::List(Rc::new(RefCell::new(out))))
        }
        Value::List(l) => {
            let take_n = l.borrow().len().min(n);
            vm.charge_alloc(take_n.saturating_mul(std::mem::size_of::<Value>()))?;
            let out: Vec<Value> = l.borrow().iter().take(n).cloned().collect();
            Ok(Value::List(Rc::new(RefCell::new(out))))
        }
        Value::Bytes(b) => {
            let take_n = b.len().min(n);
            vm.charge_alloc(take_n)?;
            Ok(Value::Bytes(Rc::new(b[..take_n].to_vec())))
        }
        other => Err(format!(
            "take() expects a list, bytes, or range, got '{}'",
            other.type_name()
        )),
    }
}

fn native_skip(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
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
            let rem = l.borrow().len().saturating_sub(n);
            vm.charge_alloc(rem.saturating_mul(std::mem::size_of::<Value>()))?;
            let out: Vec<Value> = l.borrow().iter().skip(n).cloned().collect();
            Ok(Value::List(Rc::new(RefCell::new(out))))
        }
        Value::Bytes(b) => {
            let start = n.min(b.len());
            let rem = b.len() - start;
            vm.charge_alloc(rem)?;
            Ok(Value::Bytes(Rc::new(b[start..].to_vec())))
        }
        other => Err(format!(
            "skip() expects a list, bytes, or range, got '{}'",
            other.type_name()
        )),
    }
}

fn native_any(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("any() expects 2 arguments, got {}", args.len()));
    }
    let items = collect_iterable(vm, &args[0], "any")?;
    let func = args[1].clone();
    for (i, item) in items.into_iter().enumerate() {
        if (i & 1023) == 0 {
            vm.check_timeout()?;
        }
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
    let items = collect_iterable(vm, &args[0], "all")?;
    let func = args[1].clone();
    for (i, item) in items.into_iter().enumerate() {
        if (i & 1023) == 0 {
            vm.check_timeout()?;
        }
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
    let items = collect_iterable(vm, &args[0], "find")?;
    let func = args[1].clone();
    for (i, item) in items.into_iter().enumerate() {
        if (i & 1023) == 0 {
            vm.check_timeout()?;
        }
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

fn is_sensitive_template_label(label: &str) -> bool {
    let lower = label.to_ascii_lowercase();
    lower.contains("secret")
        || lower.contains("token")
        || lower.contains("password")
        || lower.contains("passwd")
        || lower.contains("api_key")
        || lower.contains("apikey")
        || lower.contains("private_key")
        || lower.contains("auth")
}

fn native_tag_eval(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 4 && args.len() != 5 {
        return Err("__tag_eval expects 4 or 5 arguments".to_string());
    }
    let Value::Str(tag) = &args[0] else {
        return Err("__tag_eval tag must be string".to_string());
    };
    let Value::List(parts_rc) = &args[1] else {
        return Err("__tag_eval parts must be list".to_string());
    };
    let Value::List(values_rc) = &args[2] else {
        return Err("__tag_eval values must be list".to_string());
    };
    let Value::List(labels_rc) = &args[3] else {
        return Err("__tag_eval labels must be list".to_string());
    };

    if let Some(local_binding) = args.get(4) {
        if matches!(
            local_binding,
            Value::Closure(_) | Value::NativeFn(_) | Value::BoundMethod(_)
        ) {
            return vm.call_value(local_binding.clone(), &[args[1].clone(), args[2].clone()]);
        } else if !matches!(local_binding, Value::Module(_)) {
            return Err(format!(
                "Cannot call value of type '{}'",
                local_binding.type_name()
            ));
        }
    } else if let Some(&slot) = vm.global_slots.get(tag) {
        let maybe_fn = vm.global_values[slot].clone();
        if matches!(
            maybe_fn,
            Value::Closure(_) | Value::NativeFn(_) | Value::BoundMethod(_)
        ) {
            return vm.call_value(maybe_fn, &[args[1].clone(), args[2].clone()]);
        }
    }

    let parts = parts_rc.borrow();
    let values = values_rc.borrow();
    let labels = labels_rc.borrow();

    match tag.as_str() {
        "log" => {
            let mut out = String::new();
            for (i, part) in parts.iter().enumerate() {
                if let Value::Str(s) = part {
                    out.push_str(s);
                }
                if i < values.len() {
                    let lbl = match labels.get(i) {
                        Some(Value::Str(s)) => s.as_str(),
                        _ => "",
                    };
                    if is_sensitive_template_label(lbl) {
                        out.push_str("[REDACTED]");
                    } else {
                        out.push_str(&values[i].to_string());
                    }
                }
            }
            let safe = crate::modules::log::redact_secrets(&out);
            vm.charge_alloc(safe.len())?;
            Ok(Value::Str(safe))
        }
        "sql" => {
            let mut query = String::new();
            for (i, part) in parts.iter().enumerate() {
                if let Value::Str(s) = part {
                    query.push_str(s);
                }
                if i < values.len() {
                    query.push('?');
                }
            }
            let params_vec = values.clone();
            drop((parts, values, labels));
            vm.charge_alloc(query.len() + params_vec.len() * std::mem::size_of::<Value>() + 64)?;
            let mut map = HashMap::with_capacity(2);
            map.insert("query".to_string(), Value::Str(query));
            map.insert(
                "params".to_string(),
                Value::List(Rc::new(RefCell::new(params_vec))),
            );
            Ok(Value::Dict(Rc::new(RefCell::new(map))))
        }
        other => Err(format!("Unknown template tag '{}'", other)),
    }
}

fn native_check_needs(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("__check_needs expects 1 list argument".to_string());
    }
    let Value::List(caps_rc) = &args[0] else {
        return Err("__check_needs expects a list".to_string());
    };
    let caps = caps_rc.borrow().clone();
    for entry in caps {
        let Value::List(pair_rc) = entry else {
            continue;
        };
        let pair = pair_rc.borrow();
        let Some(Value::Str(cap_name)) = pair.first() else {
            continue;
        };
        let cap_args: Vec<String> = match pair.get(1) {
            Some(Value::List(l)) => l
                .borrow()
                .iter()
                .filter_map(|v| match v {
                    Value::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        match cap_name.as_str() {
            "read" => {
                if cap_args.is_empty() {
                    if !vm.permissions.allow_all
                        && vm.permissions.restrict_read_to_explicit_roots
                        && vm.permissions.allow_read.is_none()
                    {
                        return Err(
                            "Contract failed ('needs read'): read access is disabled".to_string()
                        );
                    }
                } else {
                    for p in &cap_args {
                        vm.resolve_read_path(p).map_err(|e| {
                            format!("Contract failed ('needs read(\"{}\")'): {}", p, e)
                        })?;
                    }
                }
            }
            "write" => {
                if cap_args.is_empty() {
                    if !vm.permissions.allow_all
                        && vm.permissions.allow_write.is_none()
                        && vm.permissions.restrict_write_to_explicit_roots
                    {
                        return Err(
                            "Contract failed ('needs write'): write access is disabled".to_string()
                        );
                    }
                } else {
                    for p in &cap_args {
                        vm.resolve_write_path(p).map_err(|e| {
                            format!("Contract failed ('needs write(\"{}\")'): {}", p, e)
                        })?;
                    }
                }
            }
            "net" => {
                if !vm.permissions.allow_all {
                    if vm.permissions.allow_net.is_none() {
                        return Err("Contract failed ('needs net'): network access is disabled (pass --allow-net or --unsafe)".to_string());
                    }
                    for h in &cap_args {
                        let url = if h.starts_with("https://") {
                            h.clone()
                        } else {
                            format!("https://{}", h)
                        };
                        vm.check_net(&url).map_err(|e| {
                            format!("Contract failed ('needs net(\"{}\")'): {}", h, e)
                        })?;
                    }
                }
            }
            "env" => {
                if !vm.permissions.allow_all {
                    if cap_args.is_empty() && vm.permissions.allow_env.is_none() {
                        return Err("Contract failed ('needs env'): env access is disabled (pass --allow-env or --unsafe)".to_string());
                    }
                    for k in &cap_args {
                        vm.check_env(k).map_err(|e| {
                            format!("Contract failed ('needs env(\"{}\")'): {}", k, e)
                        })?;
                    }
                }
            }
            "exec" => {
                if !vm.permissions.allow_all {
                    if vm.permissions.allow_exec.is_none() {
                        return Err("Contract failed ('needs exec'): process execution is disabled (pass --allow-exec or --unsafe)".to_string());
                    }
                    for c in &cap_args {
                        vm.check_exec(c).map_err(|e| {
                            format!("Contract failed ('needs exec(\"{}\")'): {}", c, e)
                        })?;
                    }
                }
            }
            "clipboard" => {
                vm.check_clipboard()
                    .map_err(|e| format!("Contract failed ('needs clipboard'): {}", e))?;
            }
            "input" => {
                vm.check_input()
                    .map_err(|e| format!("Contract failed ('needs input'): {}", e))?;
            }
            "args" | "sys_args" => {
                vm.check_sys_args()
                    .map_err(|e| format!("Contract failed ('needs args'): {}", e))?;
            }
            other => {
                return Err(format!(
                    "Unknown capability '{}' in 'needs' contract",
                    other
                ));
            }
        }
    }
    Ok(Value::Unit)
}

fn native_close_resource(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() {
        return Ok(Value::Unit);
    }
    match &args[0] {
        Value::Unit => Ok(Value::Unit),
        Value::StructInstance(inst) => {
            if let Some(close_fn) = inst.def.methods.get("close").cloned() {
                vm.call_value(close_fn, &[Value::StructInstance(inst.clone())])?;
            }
            Ok(Value::Unit)
        }
        Value::Dict(d) => {
            let close_val = d.borrow().get("close").cloned();
            if let Some(c @ (Value::Closure(_) | Value::NativeFn(_) | Value::BoundMethod(_))) =
                close_val
            {
                vm.call_value(c, &[])?;
            }
            Ok(Value::Unit)
        }
        _ => Ok(Value::Unit),
    }
}

fn native_sleep_checked(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("sleep/after/every expects duration in milliseconds".to_string());
    }
    let ms = match &args[0] {
        Value::Int(v) if *v >= 0 => *v as u64,
        Value::Float(v) if v.is_finite() && *v >= 0.0 => v.round() as u64,
        other => {
            return Err(format!(
                "Expected non-negative duration (ms), got '{}'",
                other.type_name()
            ));
        }
    };
    vm.sleep_checked(ms)?;
    Ok(Value::Unit)
}

fn native_record_spec(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("__record_spec expects 3 arguments (name, ok, err)".to_string());
    }
    let Value::Str(name) = &args[0] else {
        return Err("__record_spec name must be string".to_string());
    };
    let ok = matches!(args[1], Value::Bool(true));
    if ok {
        vm.spec_results.push((name.clone(), Ok(())));
    } else {
        vm.spec_results
            .push((name.clone(), Err(args[2].to_string())));
    }
    Ok(Value::Bool(ok))
}

fn native_run_spec(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("__run_spec expects 2 arguments (name, fn)".to_string());
    }
    let Value::Str(name) = &args[0] else {
        return Err("__run_spec name must be string".to_string());
    };
    let name = name.clone();
    let closure = args[1].clone();
    match vm.call_value(closure, &[]) {
        Ok(_) => {
            vm.spec_results.push((name, Ok(())));
            Ok(Value::Bool(true))
        }
        Err(e) => {
            if VM::is_unrecoverable_error(&e) {
                return Err(e);
            }
            vm.spec_results.push((name, Err(e)));
            Ok(Value::Bool(false))
        }
    }
}

fn native_unique(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("unique() expects 1 argument, got {}", args.len()));
    }
    vm.check_timeout()?;
    let items = collect_iterable(vm, &args[0], "unique")?;
    let mut out: Vec<Value> = Vec::new();
    for item in items {
        let mut seen = false;
        let cmp_work = out.len().max(1) as u64;
        vm.charge_work(cmp_work)?;
        for existing in &out {
            if existing.equals_vm(&item, vm)? {
                seen = true;
                break;
            }
        }
        if !seen {
            out.push(item);
        }
    }
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn native_flatten(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("flatten() expects 1 argument, got {}", args.len()));
    }
    vm.check_timeout()?;
    let items = collect_iterable(vm, &args[0], "flatten")?;
    let mut out = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        if (i & 1023) == 0 {
            vm.check_timeout()?;
        }
        match item {
            Value::List(inner) => {
                let b = inner.borrow();
                if out.len().saturating_add(b.len()) > 10_000_000 {
                    return Err(
                        "flatten() list size exceeds maximum limit of 10,000,000".to_string()
                    );
                }
                let add_len = b.len();
                let cloned = b.clone();
                drop(b);
                vm.charge_alloc(add_len.saturating_mul(std::mem::size_of::<Value>()))?;
                out.extend(cloned);
            }
            other => {
                if out.len() >= 10_000_000 {
                    return Err(
                        "flatten() list size exceeds maximum limit of 10,000,000".to_string()
                    );
                }
                vm.charge_alloc(std::mem::size_of::<Value>())?;
                out.push(other);
            }
        }
    }
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn native_chunk(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("chunk() expects 2 arguments, got {}", args.len()));
    }
    let items = collect_iterable(vm, &args[0], "chunk")?;
    let size = match &args[1] {
        Value::Int(n) if *n > 0 => *n as usize,
        _ => return Err("chunk() size must be a positive integer".to_string()),
    };
    vm.charge_alloc(items.len().saturating_mul(std::mem::size_of::<Value>()))?;
    let out: Vec<Value> = items
        .chunks(size)
        .map(|c| Value::List(Rc::new(RefCell::new(c.to_vec()))))
        .collect();
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn native_sum(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("sum() expects 1 argument, got {}", args.len()));
    }
    let items = collect_iterable(vm, &args[0], "sum")?;
    vm.charge_work((items.len() as u64) / 16 + 1)?;
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

fn native_is_main(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err(format!("is_main() expects 0 arguments, got {}", args.len()));
    }
    Ok(Value::Bool(
        vm.frames.last().is_none_or(|f| f.closure.proto.is_main),
    ))
}

fn set_item_alloc_bytes(v: &Value) -> usize {
    let extra = match v {
        Value::Str(s) => s.len(),
        Value::Bytes(b) => b.len(),
        _ => 0,
    };
    std::mem::size_of::<Value>().saturating_add(extra)
}

fn native_set(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() > 1 {
        return Err(format!(
            "set() expects 0 or 1 arguments, got {}",
            args.len()
        ));
    }
    let mut set = AskaneSet::default();
    if let Some(arg) = args.first() {
        let items = collect_iterable(vm, arg, "set")?;
        for item in items {
            let cost = set_item_alloc_bytes(&item);
            if set.insert(item)? {
                vm.charge_alloc(cost)?;
            }
        }
    }
    Ok(Value::Set(Rc::new(RefCell::new(set))))
}

fn native_dict(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() > 1 {
        return Err(format!(
            "dict() expects 0 or 1 arguments, got {}",
            args.len()
        ));
    }
    let Some(arg) = args.first() else {
        return Ok(Value::Dict(Rc::new(RefCell::new(HashMap::new()))));
    };
    match arg {
        Value::StructInstance(inst) => {
            let vals = inst.fields.borrow();
            let mut map = HashMap::with_capacity(inst.def.field_names.len());
            for (k, v) in inst.def.field_names.iter().zip(vals.iter()) {
                map.insert(k.clone(), v.clone());
            }
            vm.charge_alloc(map.len().saturating_mul(64).max(32))?;
            Ok(Value::Dict(Rc::new(RefCell::new(map))))
        }
        Value::Dict(d) => {
            let cloned = d.borrow().clone();
            vm.charge_alloc(cloned.len().saturating_mul(64).max(32))?;
            Ok(Value::Dict(Rc::new(RefCell::new(cloned))))
        }
        other => Err(format!(
            "dict() expects a struct instance or dict, got '{}'",
            other.type_name()
        )),
    }
}

fn resolve_builtin_method(
    val: &Value,
    attr: &str,
) -> Option<(&'static str, fn(&mut VM, &[Value]) -> Result<Value, String>)> {
    match val {
        Value::Str(_) => match attr {
            "trim" => Some(("str.trim", modules::strings::str_trim)),
            "trim_start" => Some(("str.trim_start", modules::strings::str_trim_start)),
            "trim_end" => Some(("str.trim_end", modules::strings::str_trim_end)),
            "split" => Some(("str.split", modules::strings::str_split)),
            "replace" => Some(("str.replace", modules::strings::str_replace)),
            "contains" | "has" => Some(("str.contains", modules::strings::str_contains)),
            "starts_with" => Some(("str.starts_with", modules::strings::str_starts_with)),
            "ends_with" => Some(("str.ends_with", modules::strings::str_ends_with)),
            "upper" => Some(("str.upper", modules::strings::str_upper)),
            "lower" => Some(("str.lower", modules::strings::str_lower)),
            "find" => Some(("str.find", modules::strings::str_find)),
            "repeat" => Some(("str.repeat", modules::strings::str_repeat)),
            "lines" => Some(("str.lines", modules::strings::str_lines)),
            "pad_start" => Some(("str.pad_start", modules::strings::str_pad_start)),
            "pad_end" => Some(("str.pad_end", modules::strings::str_pad_end)),
            "slice" => Some(("str.slice", modules::strings::str_slice)),
            "len" => Some(("str.len", native_len)),
            "to_bytes" => Some(("str.to_bytes", native_bytes)),
            "chars" => Some(("str.chars", method_str_chars)),
            _ => None,
        },
        Value::List(_) => match attr {
            "push" => Some(("list.push", native_push)),
            "pop" => Some(("list.pop", native_pop)),
            "len" => Some(("list.len", native_len)),
            "remove" => Some(("list.remove", native_remove)),
            "clear" => Some(("list.clear", method_list_clear)),
            "contains" | "has" => Some(("list.contains", method_list_contains)),
            "join" => Some(("list.join", modules::strings::str_join)),
            "sort" => Some(("list.sort", native_sort)),
            "reverse" => Some(("list.reverse", native_reverse)),
            "unique" => Some(("list.unique", native_unique)),
            "flatten" => Some(("list.flatten", native_flatten)),
            "map" => Some(("list.map", native_map)),
            "filter" => Some(("list.filter", native_filter)),
            "reduce" => Some(("list.reduce", native_reduce)),
            "any" => Some(("list.any", native_any)),
            "all" => Some(("list.all", native_all)),
            "find" => Some(("list.find", native_find)),
            "sum" => Some(("list.sum", native_sum)),
            "chunk" => Some(("list.chunk", native_chunk)),
            "take" => Some(("list.take", native_take)),
            "skip" => Some(("list.skip", native_skip)),
            _ => None,
        },
        Value::Set(_) => match attr {
            "add" => Some(("set.add", method_set_add)),
            "remove" => Some(("set.remove", method_set_remove)),
            "has" | "contains" => Some(("set.has", method_set_has)),
            "len" => Some(("set.len", native_len)),
            "clear" => Some(("set.clear", method_set_clear)),
            "to_list" => Some(("set.to_list", method_set_to_list)),
            "union" => Some(("set.union", method_set_union)),
            "intersect" => Some(("set.intersect", method_set_intersect)),
            "diff" => Some(("set.diff", method_set_diff)),
            _ => None,
        },
        Value::Bytes(_) => match attr {
            "len" => Some(("bytes.len", native_len)),
            "to_list" => Some(("bytes.to_list", method_bytes_to_list)),
            "to_utf8" => Some(("bytes.to_utf8", method_bytes_to_utf8)),
            _ => None,
        },
        Value::Dict(_) => match attr {
            "keys" => Some(("dict.keys", native_keys)),
            "values" => Some(("dict.values", native_values)),
            "entries" => Some(("dict.entries", native_entries)),
            "has" => Some(("dict.has", native_has)),
            "get" => Some(("dict.get", native_get)),
            "remove" => Some(("dict.remove", native_remove)),
            "len" => Some(("dict.len", native_len)),
            _ => None,
        },
        _ => None,
    }
}

fn method_str_chars(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("str.chars() expects 0 arguments".to_string());
    }
    let Value::Str(s) = &args[0] else {
        return Err("str.chars() expects a string".to_string());
    };
    let count = s.chars().count();
    vm.charge_alloc(count.saturating_mul(std::mem::size_of::<Value>().saturating_add(24)))?;
    vm.charge_work((count / 16).max(1) as u64)?;
    let list: Vec<Value> = s.chars().map(|c| Value::Str(c.to_string())).collect();
    Ok(Value::List(Rc::new(RefCell::new(list))))
}

fn method_list_clear(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("list.clear() expects 0 arguments".to_string());
    }
    let Value::List(l) = &args[0] else {
        return Err("list.clear() expects a list".to_string());
    };
    l.borrow_mut().clear();
    Ok(Value::Unit)
}

fn method_list_contains(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("list.contains() expects 1 argument".to_string());
    }
    let Value::List(l) = &args[0] else {
        return Err("list.contains() expects a list".to_string());
    };
    let items = l.borrow().clone();
    for x in items.iter() {
        if x.equals_vm(&args[1], vm)? {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn method_bytes_to_list(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("bytes.to_list() expects 0 arguments".to_string());
    }
    let Value::Bytes(b) = &args[0] else {
        return Err("bytes.to_list() expects bytes".to_string());
    };
    if b.len() > 10_000_000 {
        return Err("bytes.to_list() size exceeds 10,000,000 limit".to_string());
    }
    vm.charge_alloc(b.len().saturating_mul(std::mem::size_of::<Value>()))?;
    vm.charge_work((b.len() / 64).max(1) as u64)?;
    let out: Vec<Value> = b.iter().map(|&x| Value::Int(x as i64)).collect();
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn method_bytes_to_utf8(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("bytes.to_utf8() expects 0 arguments".to_string());
    }
    let Value::Bytes(b) = &args[0] else {
        return Err("bytes.to_utf8() expects bytes".to_string());
    };
    vm.charge_work((b.len() / 64).max(1) as u64)?;
    let s = std::str::from_utf8(b).map_err(|e| format!("bytes.to_utf8() invalid UTF-8: {}", e))?;
    vm.charge_alloc(s.len())?;
    Ok(Value::Str(s.to_string()))
}

fn method_set_add(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("set.add() expects 1 argument".to_string());
    }
    let Value::Set(s) = &args[0] else {
        return Err("set.add() expects a set".to_string());
    };
    let cost = set_item_alloc_bytes(&args[1]);
    if s.borrow_mut().insert(args[1].clone())? {
        vm.charge_alloc(cost)?;
    }
    Ok(args[0].clone())
}

fn method_set_remove(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("set.remove() expects 1 argument".to_string());
    }
    let Value::Set(s) = &args[0] else {
        return Err("set.remove() expects a set".to_string());
    };
    Ok(Value::Bool(s.borrow_mut().remove(&args[1])?))
}

fn method_set_has(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("set.has() expects 1 argument".to_string());
    }
    let Value::Set(s) = &args[0] else {
        return Err("set.has() expects a set".to_string());
    };
    Ok(Value::Bool(s.borrow().contains(&args[1])))
}

fn method_set_clear(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("set.clear() expects 0 arguments".to_string());
    }
    let Value::Set(s) = &args[0] else {
        return Err("set.clear() expects a set".to_string());
    };
    let mut borrowed = s.borrow_mut();
    borrowed.items.clear();
    borrowed.index.clear();
    Ok(Value::Unit)
}

fn method_set_to_list(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("set.to_list() expects 0 arguments".to_string());
    }
    let Value::Set(s) = &args[0] else {
        return Err("set.to_list() expects a set".to_string());
    };
    let items = s.borrow().items.clone();
    vm.charge_alloc(items.len().saturating_mul(std::mem::size_of::<Value>()))?;
    Ok(Value::List(Rc::new(RefCell::new(items))))
}

fn expect_other_set<'a>(v: &'a Value, method: &str) -> Result<&'a Rc<RefCell<AskaneSet>>, String> {
    match v {
        Value::Set(s) => Ok(s),
        other => Err(format!(
            "set.{}() expects a set argument, got '{}'",
            method,
            other.type_name()
        )),
    }
}

fn method_set_union(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("set.union() expects 1 argument".to_string());
    }
    let a = expect_other_set(&args[0], "union")?;
    let b = expect_other_set(&args[1], "union")?;
    let mut out = a.borrow().clone();
    for item in &b.borrow().items {
        let _ = out.insert(item.clone())?;
    }
    vm.charge_alloc(out.items.len().saturating_mul(std::mem::size_of::<Value>()))?;
    Ok(Value::Set(Rc::new(RefCell::new(out))))
}

fn method_set_intersect(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("set.intersect() expects 1 argument".to_string());
    }
    let a = expect_other_set(&args[0], "intersect")?;
    let b = expect_other_set(&args[1], "intersect")?;
    let mut out = AskaneSet::default();
    let b_ref = b.borrow();
    for item in &a.borrow().items {
        if b_ref.contains(item) {
            let _ = out.insert(item.clone())?;
        }
    }
    vm.charge_alloc(out.items.len().saturating_mul(std::mem::size_of::<Value>()))?;
    Ok(Value::Set(Rc::new(RefCell::new(out))))
}

fn method_set_diff(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("set.diff() expects 1 argument".to_string());
    }
    let a = expect_other_set(&args[0], "diff")?;
    let b = expect_other_set(&args[1], "diff")?;
    let mut out = AskaneSet::default();
    let b_ref = b.borrow();
    for item in &a.borrow().items {
        if !b_ref.contains(item) {
            let _ = out.insert(item.clone())?;
        }
    }
    vm.charge_alloc(out.items.len().saturating_mul(std::mem::size_of::<Value>()))?;
    Ok(Value::Set(Rc::new(RefCell::new(out))))
}

fn method_struct_to_dict(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("struct.to_dict() expects 0 arguments".to_string());
    }
    native_dict(vm, args)
}

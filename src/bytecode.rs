use std::fmt;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_IC_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_ic_id() -> u64 {
    NEXT_IC_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpvalueDesc {
    pub index: u16,
    pub is_local: bool,
}

#[derive(Debug, Clone)]
pub struct StructProto {
    pub name: String,
    pub fields: Vec<(String, bool)>,
    pub methods: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Constant {
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    Function(Rc<FunctionProto>),
    Struct(Rc<StructProto>),
}

impl fmt::Display for Constant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Constant::Int(v) => write!(f, "{}", v),
            Constant::Float(v) => write!(f, "{}", v),
            Constant::Str(s) => write!(f, "{:?}", s),
            Constant::Bytes(b) => write!(f, "<bytes {}>", b.len()),
            Constant::Function(proto) => write!(f, "<fn {}>", proto.name),
            Constant::Struct(st) => write!(f, "<struct {}>", st.name),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FunctionProto {
    pub ic_id: u64,
    pub name: String,
    pub arity: u8,
    pub min_arity: u8,
    pub is_variadic: bool,
    pub param_names: Vec<String>,
    pub local_count: u16,
    pub upvalue_count: u16,
    pub is_main: bool,
    pub chunk: Chunk,
}

#[derive(Debug, Clone, Copy)]
pub enum Op {
    Constant(u16),
    ConstInt(i32),
    Unit,
    True,
    False,
    Pop,
    Dup,
    Dup2,

    GetLocal(u16),
    SetLocal(u16),
    AddLocalInt(u16, i32),
    SubLocalInt(u16, i32),
    LtLocalInt(u16, i32),
    LeLocalInt(u16, i32),
    AddLocals(u16, u16),
    SubLocals(u16, u16),
    LtLocals(u16, u16),
    IncLocalPop(u16, i32),
    GetUpvalue(u16),
    SetUpvalue(u16),
    CloseUpvalue,

    GetGlobal(u16),
    DefineGlobal(u16),
    SetGlobal(u16),

    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Neg,
    Not,

    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    IsNumeric,

    Jump(usize),
    JumpIfFalsePop(usize),
    JumpIfFalsePeek(usize),
    JumpIfTruePeek(usize),
    JumpIfNotUnitPeek(usize),
    JumpIfArgPresent(u16, usize),

    Call(u8),
    TailCall(u8),
    CallNamed(u8, u16),
    Closure(u16, u16),
    ForPrep(u16, usize),
    ForIter(u16, usize),

    MakeList(u16),
    MakeDict(u16),
    MakeStructDef(u16),
    InitStruct(u16, u16),
    GetIndex,
    Slice(bool),
    SetIndex,
    GetAttr(u16),
    SetAttr(u16),
    OptGetAttr(u16),
    Import(u16, bool),
    MakeModule(u16, u16),

    SetupTry(usize),
    PopTry,
    Throw,
    PushDefer(u16),
    EnterSandbox(u16),
    RunCleanups(u16),

    Return,
}

#[derive(Debug, Clone, Default)]
pub struct Chunk {
    pub code: Vec<Op>,
    pub constants: Vec<Constant>,
    pub lines: Vec<usize>,
    pub upvalue_lists: Vec<Vec<UpvalueDesc>>,
    pub kw_lists: Vec<Vec<String>>,
    pub limit_exceeded: bool,
}

impl Chunk {
    pub fn emit(&mut self, op: Op, line: usize) -> usize {
        let idx = self.code.len();
        self.code.push(op);
        self.lines.push(line);
        idx
    }

    pub fn add_constant(&mut self, c: Constant) -> u16 {
        if self.constants.len() > u16::MAX as usize {
            self.limit_exceeded = true;
            return 0;
        }
        self.constants.push(c);
        (self.constants.len() - 1) as u16
    }

    pub fn add_string_constant(&mut self, s: &str) -> u16 {
        for (i, c) in self.constants.iter().enumerate() {
            if let Constant::Str(existing) = c {
                if existing == s {
                    return i as u16;
                }
            }
        }
        self.add_constant(Constant::Str(s.to_string()))
    }

    pub fn add_bytes_constant(&mut self, b: &[u8]) -> u16 {
        for (i, c) in self.constants.iter().enumerate() {
            if let Constant::Bytes(existing) = c {
                if existing.as_slice() == b {
                    return i as u16;
                }
            }
        }
        self.add_constant(Constant::Bytes(b.to_vec()))
    }

    pub fn add_upvalue_list(&mut self, list: Vec<UpvalueDesc>) -> u16 {
        if self.upvalue_lists.len() > u16::MAX as usize {
            self.limit_exceeded = true;
            return 0;
        }
        self.upvalue_lists.push(list);
        (self.upvalue_lists.len() - 1) as u16
    }

    pub fn add_kw_list(&mut self, list: Vec<String>) -> u16 {
        if self.kw_lists.len() > u16::MAX as usize {
            self.limit_exceeded = true;
            return 0;
        }
        self.kw_lists.push(list);
        (self.kw_lists.len() - 1) as u16
    }

    pub fn patch_jump(&mut self, op_idx: usize, target_ip: usize) {
        match &mut self.code[op_idx] {
            Op::Jump(target)
            | Op::JumpIfFalsePop(target)
            | Op::JumpIfFalsePeek(target)
            | Op::JumpIfTruePeek(target)
            | Op::JumpIfNotUnitPeek(target)
            | Op::JumpIfArgPresent(_, target)
            | Op::ForPrep(_, target)
            | Op::ForIter(_, target)
            | Op::SetupTry(target) => {
                *target = target_ip;
            }
            other => panic!("Cannot patch non-jump op: {:?}", other),
        }
    }

    pub fn disassemble(&self, name: &str) -> String {
        let mut out = format!("== {} ==\n", name);
        let mut nested = Vec::new();

        for (ip, op) in self.code.iter().enumerate() {
            let line = self.lines.get(ip).copied().unwrap_or(0);
            let detail = match op {
                Op::Constant(idx) => {
                    format!("Constant({}) ; {}", idx, self.constants[*idx as usize])
                }
                Op::GetGlobal(idx)
                | Op::DefineGlobal(idx)
                | Op::SetGlobal(idx)
                | Op::GetAttr(idx)
                | Op::SetAttr(idx)
                | Op::OptGetAttr(idx)
                | Op::Import(idx, _) => {
                    format!("{:?} ; {}", op, self.constants[*idx as usize])
                }
                Op::Closure(idx, uv_idx) => {
                    if let Constant::Function(proto) = &self.constants[*idx as usize] {
                        nested.push(proto.clone());
                    }
                    let upval_len = self.upvalue_lists[*uv_idx as usize].len();
                    format!(
                        "Closure({}, {} upvalues) ; {}",
                        idx, upval_len, self.constants[*idx as usize]
                    )
                }
                other => format!("{:?}", other),
            };
            out.push_str(&format!("{:04} [line {:>3}] {}\n", ip, line, detail));
        }

        for proto in nested {
            out.push('\n');
            out.push_str(&proto.chunk.disassemble(&proto.name));
        }

        out
    }
}

pub fn verify_proto(proto: &FunctionProto) -> Result<(), String> {
    verify_proto_depth(proto, 0)
}

fn verify_proto_depth(proto: &FunctionProto, depth: usize) -> Result<(), String> {
    if depth > 64 {
        return Err("Bytecode verification failed: function nesting too deep".to_string());
    }
    let chunk = &proto.chunk;
    if chunk.code.len() != chunk.lines.len() {
        return Err(format!(
            "Bytecode verification failed in '{}': code/lines length mismatch",
            proto.name
        ));
    }
    if chunk.code.is_empty() {
        return Err(format!(
            "Bytecode verification failed in '{}': empty chunk",
            proto.name
        ));
    }
    let code_len = chunk.code.len();
    if depth == 0 && proto.upvalue_count != 0 {
        return Err(format!(
            "Bytecode verification failed in '{}': root function cannot declare upvalues",
            proto.name
        ));
    }
    if proto.is_variadic && proto.arity == 0 {
        return Err(format!(
            "Bytecode verification failed in '{}': variadic function must have arity >= 1",
            proto.name
        ));
    }
    if proto.min_arity > proto.arity {
        return Err(format!(
            "Bytecode verification failed in '{}': min_arity exceeds arity",
            proto.name
        ));
    }
    let fixed_arity = if proto.is_variadic {
        proto.arity as usize - 1
    } else {
        proto.arity as usize
    };
    if proto.param_names.len() < fixed_arity {
        return Err(format!(
            "Bytecode verification failed in '{}': param_names shorter than fixed arity",
            proto.name
        ));
    }
    if proto.local_count < proto.arity as u16 + 1 {
        return Err(format!(
            "Bytecode verification failed in '{}': invalid local count",
            proto.name
        ));
    }
    let check_str = |idx: u16, op_name: &str| -> Result<(), String> {
        match chunk.constants.get(idx as usize) {
            Some(Constant::Str(_)) => Ok(()),
            _ => Err(format!(
                "Bytecode verification failed in '{}': {} constant {} is not a string",
                proto.name, op_name, idx
            )),
        }
    };

    for (ip, op) in chunk.code.iter().enumerate() {
        match *op {
            Op::GetLocal(idx)
            | Op::SetLocal(idx)
            | Op::AddLocalInt(idx, _)
            | Op::SubLocalInt(idx, _)
            | Op::LtLocalInt(idx, _)
            | Op::LeLocalInt(idx, _)
            | Op::IncLocalPop(idx, _) => {
                if idx >= proto.local_count {
                    return Err(format!(
                        "Bytecode verification failed in '{}': local {} out of bounds at ip {}",
                        proto.name, idx, ip
                    ));
                }
            }
            Op::AddLocals(a, b) | Op::SubLocals(a, b) | Op::LtLocals(a, b) => {
                if a >= proto.local_count || b >= proto.local_count {
                    return Err(format!(
                        "Bytecode verification failed in '{}': local ({}, {}) out of bounds at ip {}",
                        proto.name, a, b, ip
                    ));
                }
            }
            Op::GetUpvalue(idx) | Op::SetUpvalue(idx) => {
                if idx >= proto.upvalue_count {
                    return Err(format!(
                        "Bytecode verification failed in '{}': upvalue {} out of bounds at ip {}",
                        proto.name, idx, ip
                    ));
                }
            }
            Op::Constant(idx) => match chunk.constants.get(idx as usize) {
                Some(
                    Constant::Int(_) | Constant::Float(_) | Constant::Str(_) | Constant::Bytes(_),
                ) => {}
                Some(Constant::Function(_) | Constant::Struct(_)) => {
                    return Err(format!(
                        "Bytecode verification failed in '{}': Constant({}) cannot be loaded directly",
                        proto.name, idx
                    ));
                }
                None => {
                    return Err(format!(
                        "Bytecode verification failed in '{}': Constant({}) out of bounds at ip {}",
                        proto.name, idx, ip
                    ));
                }
            },
            Op::MakeStructDef(idx) => match chunk.constants.get(idx as usize) {
                Some(Constant::Struct(_)) => {}
                _ => {
                    return Err(format!(
                        "Bytecode verification failed in '{}': MakeStructDef({}) is not a struct",
                        proto.name, idx
                    ));
                }
            },
            Op::CallNamed(_, kw_idx) => {
                if chunk.kw_lists.get(kw_idx as usize).is_none() {
                    return Err(format!(
                        "Bytecode verification failed in '{}': kw_list {} out of bounds",
                        proto.name, kw_idx
                    ));
                }
            }
            Op::InitStruct(has_spread, kw_idx) => {
                if has_spread > 1 || chunk.kw_lists.get(kw_idx as usize).is_none() {
                    return Err(format!(
                        "Bytecode verification failed in '{}': invalid InitStruct operands at ip {}",
                        proto.name, ip
                    ));
                }
            }
            Op::GetGlobal(idx)
            | Op::DefineGlobal(idx)
            | Op::SetGlobal(idx)
            | Op::GetAttr(idx)
            | Op::SetAttr(idx)
            | Op::OptGetAttr(idx)
            | Op::Import(idx, _)
            | Op::MakeModule(idx, _) => {
                check_str(idx, "string-operand")?;
            }
            Op::Closure(fn_idx, uv_idx) => {
                let child = match chunk.constants.get(fn_idx as usize) {
                    Some(Constant::Function(child)) => {
                        verify_proto_depth(child, depth + 1)?;
                        child
                    }
                    _ => {
                        return Err(format!(
                            "Bytecode verification failed in '{}': Closure({}) is not a function",
                            proto.name, fn_idx
                        ));
                    }
                };
                let Some(uv_list) = chunk.upvalue_lists.get(uv_idx as usize) else {
                    return Err(format!(
                        "Bytecode verification failed in '{}': upvalue list {} out of bounds",
                        proto.name, uv_idx
                    ));
                };
                if uv_list.len() != child.upvalue_count as usize {
                    return Err(format!(
                        "Bytecode verification failed in '{}': upvalue count mismatch for '{}'",
                        proto.name, child.name
                    ));
                }
                for desc in uv_list {
                    if desc.is_local {
                        if desc.index >= proto.local_count {
                            return Err(format!(
                                "Bytecode verification failed in '{}': captured local {} out of bounds",
                                proto.name, desc.index
                            ));
                        }
                    } else if desc.index >= proto.upvalue_count {
                        return Err(format!(
                            "Bytecode verification failed in '{}': captured upvalue {} out of bounds",
                            proto.name, desc.index
                        ));
                    }
                }
            }
            Op::JumpIfArgPresent(arg_idx, t) => {
                if arg_idx >= proto.arity as u16 || t >= code_len {
                    return Err(format!(
                        "Bytecode verification failed in '{}': invalid JumpIfArgPresent({}, {}) at ip {}",
                        proto.name, arg_idx, t, ip
                    ));
                }
            }
            Op::ForPrep(slot, t) | Op::ForIter(slot, t) => {
                if slot as usize + 2 >= proto.local_count as usize || t >= code_len {
                    return Err(format!(
                        "Bytecode verification failed in '{}': invalid loop op ({}, {}) at ip {}",
                        proto.name, slot, t, ip
                    ));
                }
            }
            Op::Jump(t)
            | Op::JumpIfFalsePop(t)
            | Op::JumpIfFalsePeek(t)
            | Op::JumpIfTruePeek(t)
            | Op::JumpIfNotUnitPeek(t)
            | Op::SetupTry(t) => {
                if t >= code_len {
                    return Err(format!(
                        "Bytecode verification failed in '{}': jump target {} out of bounds (len {})",
                        proto.name, t, code_len
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub const BYTECODE_CACHE_MAGIC: &[u8; 4] = b"ASKC";
pub const BYTECODE_CACHE_FORMAT_VERSION: u16 = 1;
const MAX_CACHE_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_CACHE_CODE_LEN: usize = 1_000_000;
const MAX_CACHE_CONST_LEN: usize = 65_536;
const MAX_CACHE_STR_LEN: usize = 4 * 1024 * 1024;

struct CacheWriter {
    buf: Vec<u8>,
}

impl CacheWriter {
    fn new() -> Self {
        Self {
            buf: Vec::with_capacity(512),
        }
    }
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn bool(&mut self, v: bool) {
        self.buf.push(if v { 1 } else { 0 });
    }
    fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_bits().to_le_bytes());
    }
    fn usize32(&mut self, v: usize) -> Result<(), String> {
        let u = u32::try_from(v).map_err(|_| "Bytecode cache integer overflow".to_string())?;
        self.u32(u);
        Ok(())
    }
    fn str(&mut self, s: &str) -> Result<(), String> {
        self.bytes(s.as_bytes())
    }
    fn bytes(&mut self, b: &[u8]) -> Result<(), String> {
        self.usize32(b.len())?;
        self.buf.extend_from_slice(b);
        Ok(())
    }
}

struct CacheReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> CacheReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| "Truncated bytecode cache".to_string())?;
        if end > self.data.len() {
            return Err("Truncated bytecode cache".to_string());
        }
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn bool(&mut self) -> Result<bool, String> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err("Invalid bool in bytecode cache".to_string()),
        }
    }
    fn u16(&mut self) -> Result<u16, String> {
        let b: [u8; 2] = self.take(2)?.try_into().unwrap();
        Ok(u16::from_le_bytes(b))
    }
    fn u32(&mut self) -> Result<u32, String> {
        let b: [u8; 4] = self.take(4)?.try_into().unwrap();
        Ok(u32::from_le_bytes(b))
    }
    fn i32(&mut self) -> Result<i32, String> {
        let b: [u8; 4] = self.take(4)?.try_into().unwrap();
        Ok(i32::from_le_bytes(b))
    }
    fn i64(&mut self) -> Result<i64, String> {
        let b: [u8; 8] = self.take(8)?.try_into().unwrap();
        Ok(i64::from_le_bytes(b))
    }
    fn f64(&mut self) -> Result<f64, String> {
        let b: [u8; 8] = self.take(8)?.try_into().unwrap();
        Ok(f64::from_bits(u64::from_le_bytes(b)))
    }
    fn usize_bounded(&mut self, max: usize, what: &str) -> Result<usize, String> {
        let n = self.u32()? as usize;
        if n > max {
            return Err(format!("Bytecode cache '{}' exceeds limit", what));
        }
        Ok(n)
    }
    fn bytes(&mut self) -> Result<Vec<u8>, String> {
        let n = self.usize_bounded(MAX_CACHE_STR_LEN, "bytes")?;
        Ok(self.take(n)?.to_vec())
    }
    fn str(&mut self) -> Result<String, String> {
        let b = self.bytes()?;
        String::from_utf8(b).map_err(|_| "Invalid UTF-8 in bytecode cache".to_string())
    }
}

fn write_op(w: &mut CacheWriter, op: &Op) -> Result<(), String> {
    match *op {
        Op::Constant(a) => {
            w.u8(0);
            w.u16(a);
        }
        Op::ConstInt(a) => {
            w.u8(1);
            w.i32(a);
        }
        Op::Unit => w.u8(2),
        Op::True => w.u8(3),
        Op::False => w.u8(4),
        Op::Pop => w.u8(5),
        Op::Dup => w.u8(6),
        Op::Dup2 => w.u8(7),
        Op::GetLocal(a) => {
            w.u8(8);
            w.u16(a);
        }
        Op::SetLocal(a) => {
            w.u8(9);
            w.u16(a);
        }
        Op::AddLocalInt(a, b) => {
            w.u8(10);
            w.u16(a);
            w.i32(b);
        }
        Op::SubLocalInt(a, b) => {
            w.u8(11);
            w.u16(a);
            w.i32(b);
        }
        Op::LtLocalInt(a, b) => {
            w.u8(12);
            w.u16(a);
            w.i32(b);
        }
        Op::LeLocalInt(a, b) => {
            w.u8(13);
            w.u16(a);
            w.i32(b);
        }
        Op::AddLocals(a, b) => {
            w.u8(14);
            w.u16(a);
            w.u16(b);
        }
        Op::SubLocals(a, b) => {
            w.u8(15);
            w.u16(a);
            w.u16(b);
        }
        Op::LtLocals(a, b) => {
            w.u8(16);
            w.u16(a);
            w.u16(b);
        }
        Op::IncLocalPop(a, b) => {
            w.u8(17);
            w.u16(a);
            w.i32(b);
        }
        Op::GetUpvalue(a) => {
            w.u8(18);
            w.u16(a);
        }
        Op::SetUpvalue(a) => {
            w.u8(19);
            w.u16(a);
        }
        Op::CloseUpvalue => w.u8(20),
        Op::GetGlobal(a) => {
            w.u8(21);
            w.u16(a);
        }
        Op::DefineGlobal(a) => {
            w.u8(22);
            w.u16(a);
        }
        Op::SetGlobal(a) => {
            w.u8(23);
            w.u16(a);
        }
        Op::Add => w.u8(24),
        Op::Sub => w.u8(25),
        Op::Mul => w.u8(26),
        Op::Div => w.u8(27),
        Op::Mod => w.u8(28),
        Op::Neg => w.u8(29),
        Op::Not => w.u8(30),
        Op::Eq => w.u8(31),
        Op::Ne => w.u8(32),
        Op::Lt => w.u8(33),
        Op::Le => w.u8(34),
        Op::Gt => w.u8(35),
        Op::Ge => w.u8(36),
        Op::In => w.u8(37),
        Op::IsNumeric => w.u8(38),
        Op::Jump(t) => {
            w.u8(39);
            w.usize32(t)?;
        }
        Op::JumpIfFalsePop(t) => {
            w.u8(40);
            w.usize32(t)?;
        }
        Op::JumpIfFalsePeek(t) => {
            w.u8(41);
            w.usize32(t)?;
        }
        Op::JumpIfTruePeek(t) => {
            w.u8(42);
            w.usize32(t)?;
        }
        Op::JumpIfNotUnitPeek(t) => {
            w.u8(43);
            w.usize32(t)?;
        }
        Op::JumpIfArgPresent(a, t) => {
            w.u8(44);
            w.u16(a);
            w.usize32(t)?;
        }
        Op::Call(a) => {
            w.u8(45);
            w.u8(a);
        }
        Op::TailCall(a) => {
            w.u8(46);
            w.u8(a);
        }
        Op::CallNamed(a, b) => {
            w.u8(47);
            w.u8(a);
            w.u16(b);
        }
        Op::Closure(a, b) => {
            w.u8(48);
            w.u16(a);
            w.u16(b);
        }
        Op::ForPrep(a, t) => {
            w.u8(49);
            w.u16(a);
            w.usize32(t)?;
        }
        Op::ForIter(a, t) => {
            w.u8(50);
            w.u16(a);
            w.usize32(t)?;
        }
        Op::MakeList(a) => {
            w.u8(51);
            w.u16(a);
        }
        Op::MakeDict(a) => {
            w.u8(52);
            w.u16(a);
        }
        Op::MakeStructDef(a) => {
            w.u8(53);
            w.u16(a);
        }
        Op::InitStruct(a, b) => {
            w.u8(54);
            w.u16(a);
            w.u16(b);
        }
        Op::GetIndex => w.u8(55),
        Op::Slice(b) => {
            w.u8(56);
            w.bool(b);
        }
        Op::SetIndex => w.u8(57),
        Op::GetAttr(a) => {
            w.u8(58);
            w.u16(a);
        }
        Op::SetAttr(a) => {
            w.u8(59);
            w.u16(a);
        }
        Op::OptGetAttr(a) => {
            w.u8(60);
            w.u16(a);
        }
        Op::Import(a, b) => {
            w.u8(61);
            w.u16(a);
            w.bool(b);
        }
        Op::MakeModule(a, b) => {
            w.u8(62);
            w.u16(a);
            w.u16(b);
        }
        Op::SetupTry(t) => {
            w.u8(63);
            w.usize32(t)?;
        }
        Op::PopTry => w.u8(64),
        Op::Throw => w.u8(65),
        Op::PushDefer(a) => {
            w.u8(66);
            w.u16(a);
        }
        Op::EnterSandbox(a) => {
            w.u8(67);
            w.u16(a);
        }
        Op::RunCleanups(a) => {
            w.u8(68);
            w.u16(a);
        }
        Op::Return => w.u8(69),
    }
    Ok(())
}

fn read_op(r: &mut CacheReader<'_>) -> Result<Op, String> {
    let tag = r.u8()?;
    Ok(match tag {
        0 => Op::Constant(r.u16()?),
        1 => Op::ConstInt(r.i32()?),
        2 => Op::Unit,
        3 => Op::True,
        4 => Op::False,
        5 => Op::Pop,
        6 => Op::Dup,
        7 => Op::Dup2,
        8 => Op::GetLocal(r.u16()?),
        9 => Op::SetLocal(r.u16()?),
        10 => Op::AddLocalInt(r.u16()?, r.i32()?),
        11 => Op::SubLocalInt(r.u16()?, r.i32()?),
        12 => Op::LtLocalInt(r.u16()?, r.i32()?),
        13 => Op::LeLocalInt(r.u16()?, r.i32()?),
        14 => Op::AddLocals(r.u16()?, r.u16()?),
        15 => Op::SubLocals(r.u16()?, r.u16()?),
        16 => Op::LtLocals(r.u16()?, r.u16()?),
        17 => Op::IncLocalPop(r.u16()?, r.i32()?),
        18 => Op::GetUpvalue(r.u16()?),
        19 => Op::SetUpvalue(r.u16()?),
        20 => Op::CloseUpvalue,
        21 => Op::GetGlobal(r.u16()?),
        22 => Op::DefineGlobal(r.u16()?),
        23 => Op::SetGlobal(r.u16()?),
        24 => Op::Add,
        25 => Op::Sub,
        26 => Op::Mul,
        27 => Op::Div,
        28 => Op::Mod,
        29 => Op::Neg,
        30 => Op::Not,
        31 => Op::Eq,
        32 => Op::Ne,
        33 => Op::Lt,
        34 => Op::Le,
        35 => Op::Gt,
        36 => Op::Ge,
        37 => Op::In,
        38 => Op::IsNumeric,
        39 => Op::Jump(r.u32()? as usize),
        40 => Op::JumpIfFalsePop(r.u32()? as usize),
        41 => Op::JumpIfFalsePeek(r.u32()? as usize),
        42 => Op::JumpIfTruePeek(r.u32()? as usize),
        43 => Op::JumpIfNotUnitPeek(r.u32()? as usize),
        44 => Op::JumpIfArgPresent(r.u16()?, r.u32()? as usize),
        45 => Op::Call(r.u8()?),
        46 => Op::TailCall(r.u8()?),
        47 => Op::CallNamed(r.u8()?, r.u16()?),
        48 => Op::Closure(r.u16()?, r.u16()?),
        49 => Op::ForPrep(r.u16()?, r.u32()? as usize),
        50 => Op::ForIter(r.u16()?, r.u32()? as usize),
        51 => Op::MakeList(r.u16()?),
        52 => Op::MakeDict(r.u16()?),
        53 => Op::MakeStructDef(r.u16()?),
        54 => Op::InitStruct(r.u16()?, r.u16()?),
        55 => Op::GetIndex,
        56 => Op::Slice(r.bool()?),
        57 => Op::SetIndex,
        58 => Op::GetAttr(r.u16()?),
        59 => Op::SetAttr(r.u16()?),
        60 => Op::OptGetAttr(r.u16()?),
        61 => Op::Import(r.u16()?, r.bool()?),
        62 => Op::MakeModule(r.u16()?, r.u16()?),
        63 => Op::SetupTry(r.u32()? as usize),
        64 => Op::PopTry,
        65 => Op::Throw,
        66 => Op::PushDefer(r.u16()?),
        67 => Op::EnterSandbox(r.u16()?),
        68 => Op::RunCleanups(r.u16()?),
        69 => Op::Return,
        _ => return Err(format!("Unknown opcode tag {} in bytecode cache", tag)),
    })
}

fn write_proto_cache(
    w: &mut CacheWriter,
    proto: &FunctionProto,
    depth: usize,
) -> Result<(), String> {
    if depth > 64 {
        return Err("Function nesting too deep for bytecode cache".to_string());
    }
    w.str(&proto.name)?;
    w.u8(proto.arity);
    w.u8(proto.min_arity);
    w.bool(proto.is_variadic);
    w.usize32(proto.param_names.len())?;
    for p in &proto.param_names {
        w.str(p)?;
    }
    w.u16(proto.local_count);
    w.u16(proto.upvalue_count);
    w.bool(proto.is_main);

    let c = &proto.chunk;
    w.usize32(c.code.len())?;
    for (ip, op) in c.code.iter().enumerate() {
        write_op(w, op)?;
        w.usize32(c.lines.get(ip).copied().unwrap_or(0))?;
    }
    w.usize32(c.constants.len())?;
    for constant in &c.constants {
        match constant {
            Constant::Int(v) => {
                w.u8(0);
                w.i64(*v);
            }
            Constant::Float(v) => {
                w.u8(1);
                w.f64(*v);
            }
            Constant::Str(s) => {
                w.u8(2);
                w.str(s)?;
            }
            Constant::Bytes(b) => {
                w.u8(3);
                w.bytes(b)?;
            }
            Constant::Function(child) => {
                w.u8(4);
                write_proto_cache(w, child, depth + 1)?;
            }
            Constant::Struct(st) => {
                w.u8(5);
                w.str(&st.name)?;
                w.usize32(st.fields.len())?;
                for (fname, has_def) in &st.fields {
                    w.str(fname)?;
                    w.bool(*has_def);
                }
                w.usize32(st.methods.len())?;
                for m in &st.methods {
                    w.str(m)?;
                }
            }
        }
    }
    w.usize32(c.upvalue_lists.len())?;
    for uv in &c.upvalue_lists {
        w.usize32(uv.len())?;
        for desc in uv {
            w.u16(desc.index);
            w.bool(desc.is_local);
        }
    }
    w.usize32(c.kw_lists.len())?;
    for kw in &c.kw_lists {
        w.usize32(kw.len())?;
        for k in kw {
            w.str(k)?;
        }
    }
    Ok(())
}

fn read_proto_cache(r: &mut CacheReader<'_>, depth: usize) -> Result<FunctionProto, String> {
    if depth > 64 {
        return Err("Function nesting too deep in bytecode cache".to_string());
    }
    let name = r.str()?;
    let arity = r.u8()?;
    let min_arity = r.u8()?;
    let is_variadic = r.bool()?;
    let pn_len = r.usize_bounded(512, "param_names")?;
    let mut param_names = Vec::with_capacity(pn_len);
    for _ in 0..pn_len {
        param_names.push(r.str()?);
    }
    let local_count = r.u16()?;
    let upvalue_count = r.u16()?;
    let is_main = r.bool()?;

    let code_len = r.usize_bounded(MAX_CACHE_CODE_LEN, "code")?;
    let mut code = Vec::with_capacity(code_len);
    let mut lines = Vec::with_capacity(code_len);
    for _ in 0..code_len {
        code.push(read_op(r)?);
        lines.push(r.u32()? as usize);
    }

    let const_len = r.usize_bounded(MAX_CACHE_CONST_LEN, "constants")?;
    let mut constants = Vec::with_capacity(const_len);
    for _ in 0..const_len {
        let tag = r.u8()?;
        let c = match tag {
            0 => Constant::Int(r.i64()?),
            1 => Constant::Float(r.f64()?),
            2 => Constant::Str(r.str()?),
            3 => Constant::Bytes(r.bytes()?),
            4 => Constant::Function(Rc::new(read_proto_cache(r, depth + 1)?)),
            5 => {
                let sname = r.str()?;
                let flen = r.usize_bounded(4096, "struct fields")?;
                let mut fields = Vec::with_capacity(flen);
                for _ in 0..flen {
                    fields.push((r.str()?, r.bool()?));
                }
                let mlen = r.usize_bounded(4096, "struct methods")?;
                let mut methods = Vec::with_capacity(mlen);
                for _ in 0..mlen {
                    methods.push(r.str()?);
                }
                Constant::Struct(Rc::new(StructProto {
                    name: sname,
                    fields,
                    methods,
                }))
            }
            _ => return Err("Invalid constant tag in bytecode cache".to_string()),
        };
        constants.push(c);
    }

    let uv_lists_len = r.usize_bounded(MAX_CACHE_CONST_LEN, "upvalue_lists")?;
    let mut upvalue_lists = Vec::with_capacity(uv_lists_len);
    for _ in 0..uv_lists_len {
        let n = r.usize_bounded(4096, "upvalue_list")?;
        let mut list = Vec::with_capacity(n);
        for _ in 0..n {
            list.push(UpvalueDesc {
                index: r.u16()?,
                is_local: r.bool()?,
            });
        }
        upvalue_lists.push(list);
    }

    let kw_lists_len = r.usize_bounded(MAX_CACHE_CONST_LEN, "kw_lists")?;
    let mut kw_lists = Vec::with_capacity(kw_lists_len);
    for _ in 0..kw_lists_len {
        let n = r.usize_bounded(512, "kw_list")?;
        let mut list = Vec::with_capacity(n);
        for _ in 0..n {
            list.push(r.str()?);
        }
        kw_lists.push(list);
    }

    Ok(FunctionProto {
        ic_id: next_ic_id(),
        name,
        arity,
        min_arity,
        is_variadic,
        param_names,
        local_count,
        upvalue_count,
        is_main,
        chunk: Chunk {
            code,
            constants,
            lines,
            upvalue_lists,
            kw_lists,
            limit_exceeded: false,
        },
    })
}

pub fn encode_cached_proto(
    proto: &FunctionProto,
    source_sha256: &[u8; 32],
    is_module: bool,
    include_specs: bool,
) -> Result<Vec<u8>, String> {
    let mut w = CacheWriter::new();
    w.buf.extend_from_slice(BYTECODE_CACHE_MAGIC);
    w.u16(BYTECODE_CACHE_FORMAT_VERSION);
    w.str(env!("CARGO_PKG_VERSION"))?;
    w.bool(is_module);
    w.bool(include_specs);
    w.buf.extend_from_slice(source_sha256);
    write_proto_cache(&mut w, proto, 0)?;
    if w.buf.len() > MAX_CACHE_FILE_BYTES {
        return Err("Compiled bytecode exceeds cache size limit".to_string());
    }
    Ok(w.buf)
}

pub fn decode_cached_proto(
    bytes: &[u8],
    expected_sha256: &[u8; 32],
    expected_is_module: bool,
    expected_include_specs: bool,
) -> Result<FunctionProto, String> {
    if bytes.len() > MAX_CACHE_FILE_BYTES {
        return Err("Bytecode cache file exceeds size limit".to_string());
    }
    let mut r = CacheReader::new(bytes);
    if r.take(4)? != BYTECODE_CACHE_MAGIC {
        return Err("Invalid bytecode cache magic".to_string());
    }
    if r.u16()? != BYTECODE_CACHE_FORMAT_VERSION {
        return Err("Bytecode cache format version mismatch".to_string());
    }
    if r.str()? != env!("CARGO_PKG_VERSION") {
        return Err("Askane version mismatch in bytecode cache".to_string());
    }
    if r.bool()? != expected_is_module || r.bool()? != expected_include_specs {
        return Err("Compilation mode mismatch in bytecode cache".to_string());
    }
    if r.take(32)? != expected_sha256 {
        return Err("Source hash mismatch in bytecode cache".to_string());
    }
    let proto = read_proto_cache(&mut r, 0)?;
    if r.pos != bytes.len() {
        return Err("Trailing bytes in bytecode cache".to_string());
    }
    verify_proto(&proto)?;
    Ok(proto)
}

pub fn cache_path_for(source_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let fname = source_path.file_name()?.to_str()?;
    let parent = match source_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => std::path::Path::new("."),
    };
    Some(parent.join("__askcache__").join(format!("{}c", fname)))
}

pub fn try_load_cached_proto(
    source_path: &std::path::Path,
    source: &str,
    is_module: bool,
    include_specs: bool,
) -> Option<Rc<FunctionProto>> {
    let cache_file = cache_path_for(source_path)?;
    let dir_meta = std::fs::symlink_metadata(cache_file.parent()?).ok()?;
    if dir_meta.file_type().is_symlink() || !dir_meta.is_dir() {
        return None;
    }
    let file_meta = std::fs::symlink_metadata(&cache_file).ok()?;
    if file_meta.file_type().is_symlink() || !file_meta.is_file() {
        return None;
    }
    if file_meta.len() > MAX_CACHE_FILE_BYTES as u64 {
        return None;
    }
    let data = std::fs::read(&cache_file).ok()?;
    let expected_hash = crate::modules::hash::sha256_bytes(source.as_bytes());
    let proto = decode_cached_proto(&data, &expected_hash, is_module, include_specs).ok()?;
    Some(Rc::new(proto))
}

pub fn try_save_cached_proto(
    source_path: &std::path::Path,
    source: &str,
    is_module: bool,
    include_specs: bool,
    proto: &FunctionProto,
) {
    let Some(cache_file) = cache_path_for(source_path) else {
        return;
    };
    let Some(cache_dir) = cache_file.parent() else {
        return;
    };
    if let Ok(meta) = std::fs::symlink_metadata(cache_dir) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return;
        }
    } else if std::fs::create_dir_all(cache_dir).is_err() {
        return;
    }
    if let Ok(meta) = std::fs::symlink_metadata(&cache_file) {
        if meta.file_type().is_symlink() {
            return;
        }
    }
    let hash = crate::modules::hash::sha256_bytes(source.as_bytes());
    let Ok(bytes) = encode_cached_proto(proto, &hash, is_module, include_specs) else {
        return;
    };
    let tmp = cache_dir.join(format!("askane_cache_{}.tmp", std::process::id()));
    if std::fs::write(&tmp, &bytes).is_ok() {
        if std::fs::rename(&tmp, &cache_file).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
}

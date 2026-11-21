use std::fmt;
use std::rc::Rc;

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
    if proto.min_arity > proto.arity {
        return Err(format!(
            "Bytecode verification failed in '{}': min_arity exceeds arity",
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

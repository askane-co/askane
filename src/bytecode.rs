use std::fmt;
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpvalueDesc {
    pub index: u16,
    pub is_local: bool,
}

#[derive(Debug, Clone)]
pub enum Constant {
    Int(i64),
    Float(f64),
    Str(String),
    Function(Rc<FunctionProto>),
}

impl fmt::Display for Constant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Constant::Int(v) => write!(f, "{}", v),
            Constant::Float(v) => write!(f, "{}", v),
            Constant::Str(s) => write!(f, "{:?}", s),
            Constant::Function(proto) => write!(f, "<fn {}>", proto.name),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FunctionProto {
    pub name: String,
    pub arity: u8,
    pub chunk: Chunk,
}

#[derive(Debug, Clone)]
pub enum Op {
    Constant(u16),
    Unit,
    True,
    False,
    Pop,

    GetLocal(u16),
    SetLocal(u16),
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

    Jump(usize),
    JumpIfFalsePop(usize),
    JumpIfFalsePeek(usize),
    JumpIfTruePeek(usize),

    Call(u8),
    Closure(u16, Vec<UpvalueDesc>),

    MakeList(u16),
    MakeDict(u16),
    GetIndex,
    SetIndex,
    GetAttr(u16),
    Import(u16, bool),
    MakeModule(u16, u16),

    Return,
}

#[derive(Debug, Clone, Default)]
pub struct Chunk {
    pub code: Vec<Op>,
    pub constants: Vec<Constant>,
    pub lines: Vec<usize>,
}

impl Chunk {
    pub fn emit(&mut self, op: Op, line: usize) -> usize {
        let idx = self.code.len();
        self.code.push(op);
        self.lines.push(line);
        idx
    }

    pub fn add_constant(&mut self, c: Constant) -> u16 {
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

    pub fn patch_jump(&mut self, op_idx: usize, target_ip: usize) {
        match &mut self.code[op_idx] {
            Op::Jump(target)
            | Op::JumpIfFalsePop(target)
            | Op::JumpIfFalsePeek(target)
            | Op::JumpIfTruePeek(target) => {
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
                | Op::Import(idx, _) => {
                    format!("{:?} ; {}", op, self.constants[*idx as usize])
                }
                Op::Closure(idx, upvals) => {
                    if let Constant::Function(proto) = &self.constants[*idx as usize] {
                        nested.push(proto.clone());
                    }
                    format!(
                        "Closure({}, {} upvalues) ; {}",
                        idx,
                        upvals.len(),
                        self.constants[*idx as usize]
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

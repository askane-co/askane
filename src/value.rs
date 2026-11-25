use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;

use crate::bytecode::FunctionProto;
use crate::vm::VM;

#[derive(Clone)]
pub struct Closure {
    pub proto: Rc<FunctionProto>,
    pub upvalues: Vec<Rc<RefCell<UpvalueState>>>,
    pub base_dir: Option<PathBuf>,
    pub source_file: Option<String>,
}

#[derive(Debug, Clone)]
pub enum UpvalueState {
    Open(usize),
    Closed(Value),
}

#[derive(Clone, Copy)]
pub struct NativeFunction {
    pub name: &'static str,
    pub func: fn(&mut VM, &[Value]) -> Result<Value, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SetKey {
    Int(i64),
    FloatBits(u64),
    Bool(bool),
    Str(String),
    Bytes(Vec<u8>),
    Unit,
}

#[inline]
pub(crate) fn float_as_exact_i64(f: f64) -> Option<i64> {
    if !f.is_finite() || f.fract() != 0.0 || f < (i64::MIN as f64) || f >= 9223372036854775808.0 {
        return None;
    }
    let as_int = f as i64;
    if (as_int as f64) == f {
        Some(as_int)
    } else {
        None
    }
}

#[inline]
fn int_eq_float(i: i64, f: f64) -> bool {
    float_as_exact_i64(f).is_some_and(|fi| fi == i)
}

#[inline]
pub(crate) fn cmp_int_float(i: i64, f: f64) -> Option<std::cmp::Ordering> {
    if f.is_nan() {
        return None;
    }
    if f < -9223372036854775808.0 {
        return Some(std::cmp::Ordering::Greater);
    }
    if f >= 9223372036854775808.0 {
        return Some(std::cmp::Ordering::Less);
    }
    let trunc = f.trunc() as i64;
    match i.cmp(&trunc) {
        std::cmp::Ordering::Equal => 0.0_f64.partial_cmp(&f.fract()),
        ord => Some(ord),
    }
}

impl SetKey {
    pub fn from_value(v: &Value) -> Result<Self, String> {
        match v {
            Value::Int(i) => Ok(SetKey::Int(*i)),
            Value::Float(f) => {
                if f.is_nan() {
                    return Err("Cannot store NaN in a set".to_string());
                }
                let normalized = if *f == 0.0 { 0.0 } else { *f };
                if let Some(as_int) = float_as_exact_i64(normalized) {
                    return Ok(SetKey::Int(as_int));
                }
                Ok(SetKey::FloatBits(normalized.to_bits()))
            }
            Value::Bool(b) => Ok(SetKey::Bool(*b)),
            Value::Str(s) => Ok(SetKey::Str(s.clone())),
            Value::Bytes(b) => Ok(SetKey::Bytes((**b).clone())),
            Value::Unit => Ok(SetKey::Unit),
            other => Err(format!(
                "Type '{}' is not hashable and cannot be stored in a set",
                other.type_name()
            )),
        }
    }
}

#[derive(Default, Clone)]
pub struct AskaneSet {
    pub items: Vec<Value>,
    pub index: HashMap<SetKey, usize>,
}

impl AskaneSet {
    pub fn contains(&self, v: &Value) -> bool {
        SetKey::from_value(v)
            .ok()
            .is_some_and(|k| self.index.contains_key(&k))
    }

    pub fn insert(&mut self, v: Value) -> Result<bool, String> {
        let key = SetKey::from_value(&v)?;
        if self.index.contains_key(&key) {
            return Ok(false);
        }
        let pos = self.items.len();
        self.items.push(v);
        self.index.insert(key, pos);
        Ok(true)
    }

    pub fn remove(&mut self, v: &Value) -> Result<bool, String> {
        let key = SetKey::from_value(v)?;
        let Some(pos) = self.index.remove(&key) else {
            return Ok(false);
        };
        self.items.swap_remove(pos);
        if pos < self.items.len() {
            let moved_key = SetKey::from_value(&self.items[pos])?;
            self.index.insert(moved_key, pos);
        }
        Ok(true)
    }
}

pub struct StructDef {
    pub ic_id: u64,
    pub name: String,
    pub field_names: Vec<String>,
    pub field_index: HashMap<String, usize>,
    pub field_defaults: Vec<Option<Value>>,
    pub methods: HashMap<String, Value>,
}

pub struct StructInstance {
    pub def: Rc<StructDef>,
    pub fields: RefCell<Vec<Value>>,
}

#[derive(Clone)]
pub struct BoundMethod {
    pub receiver: Value,
    pub func: Value,
}

pub struct ModuleData {
    pub ic_id: u64,
    pub name: String,
    pub exports: Rc<RefCell<HashMap<String, Value>>>,
}

#[derive(Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Bytes(Rc<Vec<u8>>),
    List(Rc<RefCell<Vec<Value>>>),
    Dict(Rc<RefCell<HashMap<String, Value>>>),
    Set(Rc<RefCell<AskaneSet>>),
    Range { start: i64, end: i64 },
    Unit,
    Closure(Rc<Closure>),
    NativeFn(NativeFunction),
    BoundMethod(Rc<BoundMethod>),
    StructDef(Rc<StructDef>),
    StructInstance(Rc<StructInstance>),
    Module(Rc<ModuleData>),
}

impl Value {
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Unit => false,
            Value::Int(0) => false,
            Value::Float(f) => *f != 0.0 && !f.is_nan(),
            Value::Str(s) => !s.is_empty(),
            Value::Bytes(b) => !b.is_empty(),
            Value::List(l) => !l.borrow().is_empty(),
            Value::Dict(d) => !d.borrow().is_empty(),
            Value::Set(s) => !s.borrow().items.is_empty(),
            Value::Range { start, end } => end > start,
            _ => true,
        }
    }

    pub fn type_name(&self) -> &str {
        match self {
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Bool(_) => "bool",
            Value::Str(_) => "str",
            Value::Bytes(_) => "bytes",
            Value::List(_) => "list",
            Value::Dict(_) => "dict",
            Value::Set(_) => "set",
            Value::Range { .. } => "range",
            Value::Unit => "unit",
            Value::Closure(_) | Value::NativeFn(_) | Value::BoundMethod(_) => "fn",
            Value::StructDef(_) => "struct",
            Value::StructInstance(inst) => &inst.def.name,
            Value::Module(_) => "module",
        }
    }

    pub fn as_bytes_cow(&self, ctx: &str) -> Result<Cow<'_, [u8]>, String> {
        match self {
            Value::Bytes(b) => Ok(Cow::Borrowed(b.as_slice())),
            Value::Str(s) => Ok(Cow::Borrowed(s.as_bytes())),
            Value::List(l) => {
                let items = l.borrow();
                let mut out = Vec::with_capacity(items.len());
                for item in items.iter() {
                    match item {
                        Value::Int(b) if (0..=255).contains(b) => out.push(*b as u8),
                        _ => {
                            return Err(format!("{} list items must be integers 0..=255", ctx));
                        }
                    }
                }
                Ok(Cow::Owned(out))
            }
            other => Err(format!(
                "{} expects bytes, str, or byte list, got '{}'",
                ctx,
                other.type_name()
            )),
        }
    }

    pub fn is_same(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => {
                (a.is_nan() && b.is_nan()) || a.to_bits() == b.to_bits()
            }
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Unit, Value::Unit) => true,
            (Value::Range { start: s1, end: e1 }, Value::Range { start: s2, end: e2 }) => {
                s1 == s2 && e1 == e2
            }
            (Value::Bytes(a), Value::Bytes(b)) => Rc::ptr_eq(a, b),
            (Value::List(a), Value::List(b)) => Rc::ptr_eq(a, b),
            (Value::Dict(a), Value::Dict(b)) => Rc::ptr_eq(a, b),
            (Value::Set(a), Value::Set(b)) => Rc::ptr_eq(a, b),
            (Value::Closure(a), Value::Closure(b)) => Rc::ptr_eq(a, b),
            (Value::NativeFn(a), Value::NativeFn(b)) => {
                a.name == b.name && std::ptr::fn_addr_eq(a.func, b.func)
            }
            (Value::BoundMethod(a), Value::BoundMethod(b)) => {
                a.receiver.is_same(&b.receiver) && a.func.is_same(&b.func)
            }
            (Value::StructDef(a), Value::StructDef(b)) => Rc::ptr_eq(a, b),
            (Value::StructInstance(a), Value::StructInstance(b)) => Rc::ptr_eq(a, b),
            (Value::Module(a), Value::Module(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }

    pub fn repr_string(&self) -> String {
        match self {
            Value::Str(s) => format!("{:?}", s),
            Value::Float(v) if v.is_nan() => "NaN".to_string(),
            Value::Float(v) if v.is_infinite() => {
                if v.is_sign_positive() {
                    "inf".to_string()
                } else {
                    "-inf".to_string()
                }
            }
            other => format!("{}", other),
        }
    }

    pub fn equals(&self, other: &Value) -> bool {
        self.equals_inner(other, None).unwrap_or(false)
    }

    pub fn equals_vm(&self, other: &Value, vm: &mut VM) -> Result<bool, String> {
        self.equals_inner(other, Some(vm))
    }

    fn equals_inner(&self, other: &Value, vm: Option<&mut VM>) -> Result<bool, String> {
        let mut budget = 1_000_005usize;
        let mut work_since_check = 0usize;
        let mut visiting: HashMap<(usize, usize), usize> = HashMap::new();
        let mut memo: HashMap<(usize, usize), bool> = HashMap::new();
        let mut vm = vm;
        let (eq, _) = self.equals_depth(
            other,
            0,
            &mut budget,
            &mut work_since_check,
            &mut visiting,
            &mut memo,
            &mut vm,
        )?;
        Ok(eq)
    }

    #[allow(clippy::too_many_arguments)]
    fn equals_depth(
        &self,
        other: &Value,
        depth: usize,
        budget: &mut usize,
        work_since_check: &mut usize,
        visiting: &mut HashMap<(usize, usize), usize>,
        memo: &mut HashMap<(usize, usize), bool>,
        vm: &mut Option<&mut VM>,
    ) -> Result<(bool, usize), String> {
        if depth > 256 || *budget == 0 {
            return Ok((false, usize::MAX));
        }
        *budget -= 1;
        *work_since_check += 1;
        if (*work_since_check & 1023) == 0 {
            if let Some(v) = vm.as_deref_mut() {
                v.charge_work(64)?;
            }
        }
        let eq = match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Int(a), Value::Float(b)) => int_eq_float(*a, *b),
            (Value::Float(a), Value::Int(b)) => int_eq_float(*b, *a),
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Bytes(a), Value::Bytes(b)) => Rc::ptr_eq(a, b) || **a == **b,
            (Value::Unit, Value::Unit) => true,
            (Value::List(a), Value::List(b)) => {
                if Rc::ptr_eq(a, b) {
                    return Ok((true, usize::MAX));
                }
                let pa = Rc::as_ptr(a) as usize;
                let pb = Rc::as_ptr(b) as usize;
                let key = if pa <= pb { (pa, pb) } else { (pb, pa) };
                if let Some(&d) = visiting.get(&key) {
                    return Ok((true, d));
                }
                if let Some(&cached) = memo.get(&key) {
                    return Ok((cached, usize::MAX));
                }
                let a_ref = a.borrow();
                let b_ref = b.borrow();
                if a_ref.len() != b_ref.len() {
                    if memo.len() < 100_000 {
                        memo.insert(key, false);
                    }
                    return Ok((false, usize::MAX));
                }
                visiting.insert(key, depth);
                let mut eq = true;
                let mut min_assump = usize::MAX;
                for (x, y) in a_ref.iter().zip(b_ref.iter()) {
                    let (item_eq, item_assump) =
                        x.equals_depth(y, depth + 1, budget, work_since_check, visiting, memo, vm)?;
                    min_assump = min_assump.min(item_assump);
                    if !item_eq {
                        eq = false;
                        break;
                    }
                }
                visiting.remove(&key);
                if (!eq && *budget > 0) || (eq && min_assump >= depth) {
                    if memo.len() < 100_000 {
                        memo.insert(key, eq);
                    }
                }
                return Ok((eq, min_assump));
            }
            (Value::Dict(a), Value::Dict(b)) => {
                if Rc::ptr_eq(a, b) {
                    return Ok((true, usize::MAX));
                }
                let pa = Rc::as_ptr(a) as usize;
                let pb = Rc::as_ptr(b) as usize;
                let key = if pa <= pb { (pa, pb) } else { (pb, pa) };
                if let Some(&d) = visiting.get(&key) {
                    return Ok((true, d));
                }
                if let Some(&cached) = memo.get(&key) {
                    return Ok((cached, usize::MAX));
                }
                let a_ref = a.borrow();
                let b_ref = b.borrow();
                if a_ref.len() != b_ref.len() {
                    if memo.len() < 100_000 {
                        memo.insert(key, false);
                    }
                    return Ok((false, usize::MAX));
                }
                visiting.insert(key, depth);
                let mut eq = true;
                let mut min_assump = usize::MAX;
                for (k, v) in a_ref.iter() {
                    let Some(bv) = b_ref.get(k) else {
                        eq = false;
                        break;
                    };
                    let (item_eq, item_assump) = v.equals_depth(
                        bv,
                        depth + 1,
                        budget,
                        work_since_check,
                        visiting,
                        memo,
                        vm,
                    )?;
                    min_assump = min_assump.min(item_assump);
                    if !item_eq {
                        eq = false;
                        break;
                    }
                }
                visiting.remove(&key);
                if (!eq && *budget > 0) || (eq && min_assump >= depth) {
                    if memo.len() < 100_000 {
                        memo.insert(key, eq);
                    }
                }
                return Ok((eq, min_assump));
            }
            (Value::Closure(a), Value::Closure(b)) => Rc::ptr_eq(a, b),
            (Value::NativeFn(a), Value::NativeFn(b)) => std::ptr::fn_addr_eq(a.func, b.func),
            (Value::BoundMethod(a), Value::BoundMethod(b)) => {
                if Rc::ptr_eq(a, b) {
                    return Ok((true, usize::MAX));
                }
                let (req, rd) = a.receiver.equals_depth(
                    &b.receiver,
                    depth + 1,
                    budget,
                    work_since_check,
                    visiting,
                    memo,
                    vm,
                )?;
                if !req {
                    return Ok((false, usize::MAX));
                }
                let (feq, fd) = a.func.equals_depth(
                    &b.func,
                    depth + 1,
                    budget,
                    work_since_check,
                    visiting,
                    memo,
                    vm,
                )?;
                return Ok((feq, rd.min(fd)));
            }
            (Value::StructDef(a), Value::StructDef(b)) => Rc::ptr_eq(a, b),
            (Value::StructInstance(a), Value::StructInstance(b)) => {
                if Rc::ptr_eq(a, b) {
                    return Ok((true, usize::MAX));
                }
                if !Rc::ptr_eq(&a.def, &b.def) {
                    return Ok((false, usize::MAX));
                }
                let pa = Rc::as_ptr(a) as usize;
                let pb = Rc::as_ptr(b) as usize;
                let key = if pa <= pb { (pa, pb) } else { (pb, pa) };
                if let Some(&d) = visiting.get(&key) {
                    return Ok((true, d));
                }
                if let Some(&cached) = memo.get(&key) {
                    return Ok((cached, usize::MAX));
                }
                let a_fields = a.fields.borrow();
                let b_fields = b.fields.borrow();
                if a_fields.len() != b_fields.len() {
                    if memo.len() < 100_000 {
                        memo.insert(key, false);
                    }
                    return Ok((false, usize::MAX));
                }
                visiting.insert(key, depth);
                let mut eq = true;
                let mut min_assump = usize::MAX;
                for (x, y) in a_fields.iter().zip(b_fields.iter()) {
                    let (item_eq, item_assump) =
                        x.equals_depth(y, depth + 1, budget, work_since_check, visiting, memo, vm)?;
                    min_assump = min_assump.min(item_assump);
                    if !item_eq {
                        eq = false;
                        break;
                    }
                }
                visiting.remove(&key);
                if (!eq && *budget > 0) || (eq && min_assump >= depth) {
                    if memo.len() < 100_000 {
                        memo.insert(key, eq);
                    }
                }
                return Ok((eq, min_assump));
            }
            (Value::Set(a), Value::Set(b)) => {
                if Rc::ptr_eq(a, b) {
                    return Ok((true, usize::MAX));
                }
                let a_ref = a.borrow();
                let b_ref = b.borrow();
                a_ref.items.len() == b_ref.items.len()
                    && a_ref.index.keys().all(|k| b_ref.index.contains_key(k))
            }
            (Value::Range { start: s1, end: e1 }, Value::Range { start: s2, end: e2 }) => {
                s1 == s2 && e1 == e2
            }
            (Value::Module(a), Value::Module(b)) => Rc::ptr_eq(&a.exports, &b.exports),
            _ => false,
        };
        Ok((eq, usize::MAX))
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self)
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut seen = HashSet::new();
        let mut budget = FmtBudget {
            steps: 0,
            approx_bytes: 0,
        };
        self.fmt_with_seen(f, &mut seen, &mut budget)
    }
}

struct FmtBudget {
    steps: usize,
    approx_bytes: usize,
}

impl FmtBudget {
    const MAX_STEPS: usize = 500_000;
    const MAX_BYTES: usize = 16 * 1024 * 1024;

    fn exhausted(&self) -> bool {
        self.steps >= Self::MAX_STEPS || self.approx_bytes >= Self::MAX_BYTES
    }

    fn consume(&mut self, bytes: usize) {
        self.steps = self.steps.saturating_add(1);
        self.approx_bytes = self.approx_bytes.saturating_add(bytes);
    }
}

impl Value {
    fn fmt_with_seen(
        &self,
        f: &mut fmt::Formatter<'_>,
        seen: &mut HashSet<usize>,
        budget: &mut FmtBudget,
    ) -> fmt::Result {
        if budget.exhausted() {
            return write!(f, "...");
        }
        if seen.len() >= 64 {
            return write!(f, "<depth limit>");
        }
        match self {
            Value::Int(v) => {
                budget.consume(8);
                write!(f, "{}", v)
            }
            Value::Float(v) => {
                budget.consume(8);
                if v.fract() == 0.0 {
                    write!(f, "{:.1}", v)
                } else {
                    write!(f, "{}", v)
                }
            }
            Value::Bool(b) => {
                budget.consume(5);
                write!(f, "{}", b)
            }
            Value::Str(s) => {
                budget.consume(s.len());
                write!(f, "{}", s)
            }
            Value::Bytes(b) => {
                budget.consume(b.len().saturating_add(3));
                write!(f, "b\"")?;
                for &byte in b.iter() {
                    if budget.exhausted() {
                        write!(f, "...")?;
                        break;
                    }
                    match byte {
                        b'\n' => write!(f, "\\n")?,
                        b'\r' => write!(f, "\\r")?,
                        b'\t' => write!(f, "\\t")?,
                        b'\\' => write!(f, "\\\\")?,
                        b'"' => write!(f, "\\\"")?,
                        0 => write!(f, "\\0")?,
                        0x20..=0x7E => write!(f, "{}", byte as char)?,
                        other => write!(f, "\\x{:02x}", other)?,
                    }
                    budget.consume(1);
                }
                write!(f, "\"")
            }
            Value::Unit => {
                budget.consume(4);
                write!(f, "unit")
            }
            Value::List(items) => {
                let ptr = Rc::as_ptr(items) as usize;
                if !seen.insert(ptr) {
                    return write!(f, "[...]");
                }
                budget.consume(2);
                let list = items.borrow();
                write!(f, "[")?;
                for (i, item) in list.iter().enumerate() {
                    if budget.exhausted() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "...")?;
                        break;
                    }
                    if i > 0 {
                        budget.approx_bytes = budget.approx_bytes.saturating_add(2);
                        write!(f, ", ")?;
                    }
                    match item {
                        Value::Str(s) => {
                            budget.consume(s.len() + 2);
                            write!(f, "{:?}", s)?;
                        }
                        other => other.fmt_with_seen(f, seen, budget)?,
                    }
                }
                seen.remove(&ptr);
                write!(f, "]")
            }
            Value::Closure(c) => {
                budget.consume(c.proto.name.len() + 6);
                write!(f, "<fn {}>", c.proto.name)
            }
            Value::NativeFn(n) => {
                budget.consume(n.name.len() + 13);
                write!(f, "<native fn {}>", n.name)
            }
            Value::BoundMethod(m) => {
                budget.consume(14);
                write!(f, "<bound method ")?;
                m.func.fmt_with_seen(f, seen, budget)?;
                write!(f, ">")
            }
            Value::StructDef(def) => {
                budget.consume(def.name.len() + 10);
                write!(f, "<struct {}>", def.name)
            }
            Value::StructInstance(inst) => {
                let ptr = Rc::as_ptr(inst) as usize;
                if !seen.insert(ptr) {
                    return write!(f, "{} {{...}}", inst.def.name);
                }
                budget.consume(inst.def.name.len() + 4);
                let vals = inst.fields.borrow();
                write!(f, "{} {{", inst.def.name)?;
                for (i, (name, val)) in inst.def.field_names.iter().zip(vals.iter()).enumerate() {
                    if budget.exhausted() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "...")?;
                        break;
                    }
                    if i > 0 {
                        budget.approx_bytes = budget.approx_bytes.saturating_add(2);
                        write!(f, ", ")?;
                    } else {
                        write!(f, " ")?;
                    }
                    budget.consume(name.len() + 2);
                    write!(f, "{}: ", name)?;
                    match val {
                        Value::Str(s) => {
                            budget.consume(s.len() + 2);
                            write!(f, "{:?}", s)?;
                        }
                        other => other.fmt_with_seen(f, seen, budget)?,
                    }
                }
                if !inst.def.field_names.is_empty() {
                    write!(f, " ")?;
                }
                seen.remove(&ptr);
                write!(f, "}}")
            }
            Value::Set(s) => {
                budget.consume(6);
                let set = s.borrow();
                write!(f, "set([")?;
                for (i, item) in set.items.iter().enumerate() {
                    if budget.exhausted() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "...")?;
                        break;
                    }
                    if i > 0 {
                        budget.approx_bytes = budget.approx_bytes.saturating_add(2);
                        write!(f, ", ")?;
                    }
                    match item {
                        Value::Str(st) => {
                            budget.consume(st.len() + 2);
                            write!(f, "{:?}", st)?;
                        }
                        other => other.fmt_with_seen(f, seen, budget)?,
                    }
                }
                write!(f, "])")
            }
            Value::Dict(d) => {
                let ptr = Rc::as_ptr(d) as usize;
                if !seen.insert(ptr) {
                    return write!(f, "#{{...}}");
                }
                budget.consume(3);
                let map = d.borrow();
                let mut entries: Vec<(&String, &Value)> = map.iter().collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                write!(f, "#{{")?;
                for (i, (k, v)) in entries.into_iter().enumerate() {
                    if budget.exhausted() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "...")?;
                        break;
                    }
                    if i > 0 {
                        budget.approx_bytes = budget.approx_bytes.saturating_add(2);
                        write!(f, ", ")?;
                    }
                    match v {
                        Value::Str(s) => {
                            budget.consume(k.len() + s.len() + 6);
                            write!(f, "{:?}: {:?}", k, s)?;
                        }
                        other => {
                            budget.approx_bytes = budget.approx_bytes.saturating_add(k.len() + 4);
                            write!(f, "{:?}: ", k)?;
                            other.fmt_with_seen(f, seen, budget)?;
                        }
                    }
                }
                seen.remove(&ptr);
                write!(f, "}}")
            }
            Value::Module(m) => {
                budget.consume(m.name.len() + 10);
                write!(f, "<module {}>", m.name)
            }
            Value::Range { start, end } => {
                budget.consume(20);
                write!(f, "range({}, {})", start, end)
            }
        }
    }
}

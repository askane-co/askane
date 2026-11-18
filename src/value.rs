use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::rc::Rc;

use crate::bytecode::FunctionProto;
use crate::vm::VM;

#[derive(Clone)]
pub struct Closure {
    pub proto: Rc<FunctionProto>,
    pub upvalues: Vec<Rc<RefCell<UpvalueState>>>,
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

#[derive(Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    List(Rc<RefCell<Vec<Value>>>),
    Dict(Rc<RefCell<HashMap<String, Value>>>),
    Range {
        start: i64,
        end: i64,
    },
    Unit,
    Closure(Rc<Closure>),
    NativeFn(NativeFunction),
    Module {
        name: String,
        exports: Rc<RefCell<HashMap<String, Value>>>,
    },
}

impl Value {
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Unit => false,
            Value::Int(0) => false,
            Value::Float(f) => *f != 0.0 && !f.is_nan(),
            Value::Str(s) => !s.is_empty(),
            Value::List(l) => !l.borrow().is_empty(),
            Value::Dict(d) => !d.borrow().is_empty(),
            Value::Range { start, end } => end > start,
            _ => true,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Bool(_) => "bool",
            Value::Str(_) => "str",
            Value::List(_) => "list",
            Value::Dict(_) => "dict",
            Value::Range { .. } => "range",
            Value::Unit => "unit",
            Value::Closure(_) | Value::NativeFn(_) => "fn",
            Value::Module { .. } => "module",
        }
    }

    pub fn equals(&self, other: &Value) -> bool {
        let mut budget = 10_000_005usize;
        let mut memo = HashSet::new();
        self.equals_depth(other, 0, &mut budget, &mut memo)
    }

    fn equals_depth(
        &self,
        other: &Value,
        depth: usize,
        budget: &mut usize,
        memo: &mut HashSet<(usize, usize)>,
    ) -> bool {
        if depth > 128 || *budget == 0 {
            return false;
        }
        *budget -= 1;
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
            (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Unit, Value::Unit) => true,
            (Value::List(a), Value::List(b)) => {
                if Rc::ptr_eq(a, b) {
                    return true;
                }
                let key = (Rc::as_ptr(a) as usize, Rc::as_ptr(b) as usize);
                if memo.contains(&key) {
                    return true;
                }
                let a_ref = a.borrow();
                let b_ref = b.borrow();
                if a_ref.len() != b_ref.len() {
                    return false;
                }
                let eq = a_ref
                    .iter()
                    .zip(b_ref.iter())
                    .all(|(x, y)| x.equals_depth(y, depth + 1, budget, memo));
                if eq && memo.len() < 100_000 {
                    memo.insert(key);
                }
                eq
            }
            (Value::Dict(a), Value::Dict(b)) => {
                if Rc::ptr_eq(a, b) {
                    return true;
                }
                let key = (Rc::as_ptr(a) as usize, Rc::as_ptr(b) as usize);
                if memo.contains(&key) {
                    return true;
                }
                let a_ref = a.borrow();
                let b_ref = b.borrow();
                if a_ref.len() != b_ref.len() {
                    return false;
                }
                let eq = a_ref.iter().all(|(k, v)| {
                    b_ref
                        .get(k)
                        .is_some_and(|bv| v.equals_depth(bv, depth + 1, budget, memo))
                });
                if eq && memo.len() < 100_000 {
                    memo.insert(key);
                }
                eq
            }
            (Value::Closure(a), Value::Closure(b)) => Rc::ptr_eq(a, b),
            (Value::NativeFn(a), Value::NativeFn(b)) => std::ptr::fn_addr_eq(a.func, b.func),
            (Value::Range { start: s1, end: e1 }, Value::Range { start: s2, end: e2 }) => {
                s1 == s2 && e1 == e2
            }
            (Value::Module { exports: a, .. }, Value::Module { exports: b, .. }) => {
                Rc::ptr_eq(a, b)
            }
            _ => false,
        }
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
            Value::Dict(d) => {
                let ptr = Rc::as_ptr(d) as usize;
                if !seen.insert(ptr) {
                    return write!(f, "#{{...}}");
                }
                budget.consume(3);
                let map = d.borrow();
                write!(f, "#{{")?;
                for (i, (k, v)) in map.iter().enumerate() {
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
            Value::Module { name, .. } => {
                budget.consume(name.len() + 10);
                write!(f, "<module {}>", name)
            }
            Value::Range { start, end } => {
                budget.consume(20);
                write!(f, "range({}, {})", start, end)
            }
        }
    }
}

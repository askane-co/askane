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
            Value::Str(s) => !s.is_empty(),
            Value::List(l) => !l.borrow().is_empty(),
            Value::Dict(d) => !d.borrow().is_empty(),
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
            Value::Unit => "unit",
            Value::Closure(_) | Value::NativeFn(_) => "fn",
            Value::Module { .. } => "module",
        }
    }

    pub fn equals(&self, other: &Value) -> bool {
        self.equals_depth(other, 0)
    }

    fn equals_depth(&self, other: &Value, depth: usize) -> bool {
        if depth > 128 {
            return false;
        }
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
                let a_ref = a.borrow();
                let b_ref = b.borrow();
                if a_ref.len() != b_ref.len() {
                    return false;
                }
                a_ref
                    .iter()
                    .zip(b_ref.iter())
                    .all(|(x, y)| x.equals_depth(y, depth + 1))
            }
            (Value::Dict(a), Value::Dict(b)) => {
                if Rc::ptr_eq(a, b) {
                    return true;
                }
                let a_ref = a.borrow();
                let b_ref = b.borrow();
                if a_ref.len() != b_ref.len() {
                    return false;
                }
                a_ref
                    .iter()
                    .all(|(k, v)| b_ref.get(k).is_some_and(|bv| v.equals_depth(bv, depth + 1)))
            }
            (Value::Closure(a), Value::Closure(b)) => Rc::ptr_eq(a, b),
            (Value::NativeFn(a), Value::NativeFn(b)) => std::ptr::fn_addr_eq(a.func, b.func),
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
        self.fmt_with_seen(f, &mut seen)
    }
}

impl Value {
    fn fmt_with_seen(&self, f: &mut fmt::Formatter<'_>, seen: &mut HashSet<usize>) -> fmt::Result {
        match self {
            Value::Int(v) => write!(f, "{}", v),
            Value::Float(v) => {
                if v.fract() == 0.0 {
                    write!(f, "{:.1}", v)
                } else {
                    write!(f, "{}", v)
                }
            }
            Value::Bool(b) => write!(f, "{}", b),
            Value::Str(s) => write!(f, "{}", s),
            Value::Unit => write!(f, "unit"),
            Value::List(items) => {
                let ptr = Rc::as_ptr(items) as usize;
                if !seen.insert(ptr) {
                    return write!(f, "[...]");
                }
                let list = items.borrow();
                write!(f, "[")?;
                for (i, item) in list.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    match item {
                        Value::Str(s) => write!(f, "{:?}", s)?,
                        other => other.fmt_with_seen(f, seen)?,
                    }
                }
                seen.remove(&ptr);
                write!(f, "]")
            }
            Value::Closure(c) => write!(f, "<fn {}>", c.proto.name),
            Value::NativeFn(n) => write!(f, "<native fn {}>", n.name),
            Value::Dict(d) => {
                let ptr = Rc::as_ptr(d) as usize;
                if !seen.insert(ptr) {
                    return write!(f, "#{{...}}");
                }
                let map = d.borrow();
                write!(f, "#{{")?;
                for (i, (k, v)) in map.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    match v {
                        Value::Str(s) => write!(f, "{:?}: {:?}", k, s)?,
                        other => {
                            write!(f, "{:?}: ", k)?;
                            other.fmt_with_seen(f, seen)?;
                        }
                    }
                }
                seen.remove(&ptr);
                write!(f, "}}")
            }
            Value::Module { name, .. } => write!(f, "<module {}>", name),
        }
    }
}

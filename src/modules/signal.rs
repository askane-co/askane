use std::cell::RefCell;
use std::rc::Rc;

use crate::modules::{NativeFnPtr, build_module};
use crate::value::{FastMap, StructDef, StructInstance, Value, new_dict_with_capacity};
use crate::vm::VM;

const MAX_SIGNAL_CASCADE: i64 = 32;

thread_local! {
    static SIGNAL_DEF: Rc<StructDef> = make_signal_def();
}

fn make_signal_def() -> Rc<StructDef> {
    let methods_spec: &[(&'static str, NativeFnPtr)] = &[
        ("get", sig_get),
        ("set", sig_set),
        ("update", sig_update),
        ("watch", sig_watch),
        ("unwatch", sig_unwatch),
        ("unwatch_all", sig_unwatch_all),
    ];
    let field_names = vec![
        "value".to_string(),
        "__watchers".to_string(),
        "__next_id".to_string(),
        "__depth".to_string(),
    ];
    let mut field_index = FastMap::default();
    field_index.reserve(field_names.len());
    for (i, n) in field_names.iter().enumerate() {
        field_index.insert(n.clone(), i);
    }
    let mut methods = new_dict_with_capacity(methods_spec.len());
    for &(name, func) in methods_spec {
        methods.insert(name.to_string(), Value::native_fn(name, func));
    }
    Rc::new(StructDef {
        ic_id: crate::bytecode::next_ic_id(),
        name: "Signal".to_string(),
        field_names,
        field_index,
        field_defaults: vec![None; 4],
        methods,
    })
}

pub fn module() -> Value {
    build_module(
        "signal",
        &[
            ("new", sig_new),
            ("of", sig_new),
            ("get", sig_get),
            ("set", sig_set),
            ("update", sig_update),
            ("watch", sig_watch),
            ("unwatch", sig_unwatch),
            ("unwatch_all", sig_unwatch_all),
        ],
        &[],
    )
}

fn expect_signal<'a>(v: &'a Value, ctx: &str) -> Result<&'a Rc<StructInstance>, String> {
    let Value::StructInstance(inst) = v else {
        return Err(format!("{} expects a Signal, got '{}'", ctx, v.type_name()));
    };
    let expected_ic = SIGNAL_DEF.with(|d| d.ic_id);
    if inst.def.ic_id != expected_ic {
        return Err(format!("{} expects a Signal", ctx));
    }
    Ok(inst)
}

fn sig_new(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("signal.new() expects 1 argument (initial_value)".to_string());
    }
    vm.charge_alloc(128)?;
    let def = SIGNAL_DEF.with(Rc::clone);
    Ok(Value::StructInstance(Rc::new(StructInstance {
        def,
        fields: RefCell::new(vec![
            args[0].clone(),
            Value::List(Rc::new(RefCell::new(Vec::new()))),
            Value::Int(1),
            Value::Int(0),
        ]),
    })))
}

fn sig_get(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("signal.get() expects 1 argument (signal)".to_string());
    }
    let inst = expect_signal(&args[0], "signal.get()")?;
    Ok(inst.fields.borrow()[0].clone())
}

fn sig_set(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("signal.set() expects 2 arguments (signal, value)".to_string());
    }
    let inst = expect_signal(&args[0], "signal.set()")?;
    let new_val = args[1].clone();

    let (old_val, depth, callbacks) = {
        let mut fields = inst.fields.borrow_mut();
        let old = fields[0].clone();
        if old.equals_vm(&new_val, vm)? {
            return Ok(Value::Bool(false));
        }
        let depth = match fields[3] {
            Value::Int(d) => d,
            _ => 0,
        };
        if depth >= MAX_SIGNAL_CASCADE {
            return Err(format!(
                "signal.set(): recursive signal cascade exceeded max depth ({})",
                MAX_SIGNAL_CASCADE
            ));
        }
        fields[0] = new_val.clone();
        fields[3] = Value::Int(depth + 1);
        let cbs: Vec<Value> = match &fields[1] {
            Value::List(l) => l.borrow().clone(),
            _ => Vec::new(),
        };
        (old, depth, cbs)
    };

    let mut run_res = Ok(());
    for entry in callbacks {
        if let Value::List(pair) = entry {
            let cb = pair.borrow().get(1).cloned().unwrap_or(Value::Unit);
            if !matches!(cb, Value::Unit) {
                if let Err(e) = vm.call_value(cb, &[new_val.clone(), old_val.clone()]) {
                    run_res = Err(e);
                    break;
                }
            }
        }
    }
    inst.fields.borrow_mut()[3] = Value::Int(depth);
    run_res?;
    Ok(Value::Bool(true))
}

fn sig_update(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("signal.update() expects 2 arguments (signal, fn)".to_string());
    }
    let inst = expect_signal(&args[0], "signal.update()")?;
    let old = inst.fields.borrow()[0].clone();
    let updated = vm.call_value(args[1].clone(), &[old])?;
    let changed = sig_set(vm, &[args[0].clone(), updated.clone()])?;
    let _ = changed;
    Ok(updated)
}

fn sig_watch(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("signal.watch() expects 2 arguments (signal, callback)".to_string());
    }
    let inst = expect_signal(&args[0], "signal.watch()")?;
    if !matches!(
        args[1],
        Value::Closure(_) | Value::NativeFn(_) | Value::BoundMethod(_)
    ) {
        return Err(format!(
            "signal.watch() callback must be callable, got '{}'",
            args[1].type_name()
        ));
    }
    vm.charge_alloc(48)?;
    let mut fields = inst.fields.borrow_mut();
    let id = match fields[2] {
        Value::Int(n) => n,
        _ => 1,
    };
    fields[2] = Value::Int(id + 1);
    if let Value::List(list) = &fields[1] {
        list.borrow_mut()
            .push(Value::List(Rc::new(RefCell::new(vec![
                Value::Int(id),
                args[1].clone(),
            ]))));
    }
    Ok(Value::Int(id))
}

fn sig_unwatch(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("signal.unwatch() expects 2 arguments (signal, watcher_id)".to_string());
    }
    let inst = expect_signal(&args[0], "signal.unwatch()")?;
    let target_id = match &args[1] {
        Value::Int(n) => *n,
        other => {
            return Err(format!(
                "signal.unwatch() watcher_id must be integer, got '{}'",
                other.type_name()
            ));
        }
    };
    let fields = inst.fields.borrow();
    if let Value::List(list) = &fields[1] {
        let mut items = list.borrow_mut();
        let before = items.len();
        items.retain(|entry| {
            if let Value::List(pair) = entry {
                pair.borrow()
                    .first()
                    .is_none_or(|v| !matches!(v, Value::Int(id) if *id == target_id))
            } else {
                true
            }
        });
        return Ok(Value::Bool(items.len() != before));
    }
    Ok(Value::Bool(false))
}

fn sig_unwatch_all(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("signal.unwatch_all() expects 1 argument (signal)".to_string());
    }
    let inst = expect_signal(&args[0], "signal.unwatch_all()")?;
    let fields = inst.fields.borrow();
    if let Value::List(list) = &fields[1] {
        list.borrow_mut().clear();
    }
    Ok(Value::Unit)
}

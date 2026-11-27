use std::cell::RefCell;
use std::rc::Rc;

use crate::modules::{NativeFnPtr, build_module};
use crate::value::{FastMap, StructDef, StructInstance, Value, new_dict_with_capacity};
use crate::vm::VM;

thread_local! {
    static TASK_HANDLE_DEF: Rc<StructDef> = make_def("TaskHandle", &[
        ("join", task_join),
        ("is_done", task_is_done),
    ]);
    static CHANNEL_DEF: Rc<StructDef> = make_def("Channel", &[
        ("send", chan_send),
        ("try_send", chan_try_send),
        ("recv", chan_recv),
        ("try_recv", chan_try_recv),
        ("close", chan_close),
        ("len", chan_len),
        ("is_closed", chan_is_closed),
    ]);
}

fn make_def(name: &str, methods_spec: &[(&'static str, NativeFnPtr)]) -> Rc<StructDef> {
    let field_names = vec!["__id".to_string()];
    let mut field_index = FastMap::default();
    field_index.insert("__id".to_string(), 0);
    let mut methods = new_dict_with_capacity(methods_spec.len());
    for &(mname, func) in methods_spec {
        methods.insert(mname.to_string(), Value::native_fn(mname, func));
    }
    Rc::new(StructDef {
        ic_id: crate::bytecode::next_ic_id(),
        name: name.to_string(),
        field_names,
        field_index,
        field_defaults: vec![None],
        methods,
    })
}

pub fn task_module() -> Value {
    build_module(
        "task",
        &[
            ("spawn", task_spawn),
            ("yield_now", task_yield_now),
            ("sleep", task_sleep),
            ("join", task_join),
            ("is_done", task_is_done),
            ("select", task_select),
            ("timeout", task_timeout),
        ],
        &[],
    )
}

pub fn channel_module() -> Value {
    build_module(
        "channel",
        &[
            ("bounded", chan_bounded),
            ("send", chan_send),
            ("try_send", chan_try_send),
            ("recv", chan_recv),
            ("try_recv", chan_try_recv),
            ("close", chan_close),
            ("len", chan_len),
            ("is_closed", chan_is_closed),
        ],
        &[],
    )
}

fn extract_task_id(v: &Value, ctx: &str) -> Result<u64, String> {
    let Value::StructInstance(inst) = v else {
        return Err(format!(
            "{} expects a TaskHandle, got '{}'",
            ctx,
            v.type_name()
        ));
    };
    let expected = TASK_HANDLE_DEF.with(|d| d.ic_id);
    if inst.def.ic_id != expected {
        return Err(format!("{} expects a TaskHandle", ctx));
    }
    match inst.fields.borrow().first() {
        Some(Value::Int(id)) if *id > 0 => Ok(*id as u64),
        _ => Err(format!("{}: invalid TaskHandle", ctx)),
    }
}

pub(crate) fn extract_chan_id(v: &Value, ctx: &str) -> Result<u64, String> {
    let Value::StructInstance(inst) = v else {
        return Err(format!(
            "{} expects a Channel, got '{}'",
            ctx,
            v.type_name()
        ));
    };
    let expected = CHANNEL_DEF.with(|d| d.ic_id);
    if inst.def.ic_id != expected {
        return Err(format!("{} expects a Channel", ctx));
    }
    match inst.fields.borrow().first() {
        Some(Value::Int(id)) if *id > 0 => Ok(*id as u64),
        _ => Err(format!("{}: invalid Channel", ctx)),
    }
}

fn task_spawn(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() {
        return Err("task.spawn() expects at least 1 argument (fn, [...args])".to_string());
    }
    let id = vm.spawn_green_task(args[0].clone(), &args[1..])?;
    let def = TASK_HANDLE_DEF.with(Rc::clone);
    Ok(Value::StructInstance(Rc::new(StructInstance {
        def,
        fields: RefCell::new(vec![Value::Int(id as i64)]),
    })))
}

fn task_yield_now(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("task.yield_now() expects 0 arguments".to_string());
    }
    vm.yield_current_task()?;
    Ok(Value::Unit)
}

fn task_sleep(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("task.sleep() expects 1 integer argument (ms)".to_string());
    }
    let ms = match &args[0] {
        Value::Int(n) if *n >= 0 => *n as u64,
        _ => return Err("task.sleep() ms must be a non-negative integer".to_string()),
    };
    vm.sleep_green_task(ms)?;
    Ok(Value::Unit)
}

fn task_join(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("task.join() expects 1 argument (handle)".to_string());
    }
    let id = extract_task_id(&args[0], "task.join()")?;
    vm.join_green_task(id)
}

fn task_is_done(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("task.is_done() expects 1 argument (handle)".to_string());
    }
    let id = extract_task_id(&args[0], "task.is_done()")?;
    Ok(Value::Bool(vm.is_task_done(id)))
}

fn task_select(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("task.select() expects 1 list argument (channels)".to_string());
    }
    let Value::List(list) = &args[0] else {
        return Err(format!(
            "task.select() expects a list of Channels, got '{}'",
            args[0].type_name()
        ));
    };
    let items = list.borrow().clone();
    if items.is_empty() || items.len() > 256 {
        return Err("task.select() requires 1..=256 channels".to_string());
    }
    let mut ids = Vec::with_capacity(items.len());
    for item in &items {
        ids.push(extract_chan_id(item, "task.select()")?);
    }
    vm.select_channels(&ids)
}

fn task_timeout(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("task.timeout() expects 2 arguments (ms, fn)".to_string());
    }
    let ms = match &args[0] {
        Value::Int(n) if *n >= 0 => *n as u64,
        _ => return Err("task.timeout() ms must be a non-negative integer".to_string()),
    };
    vm.run_with_timeout(ms, args[1].clone())
}

fn chan_bounded(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("channel.bounded() expects 1 integer argument (capacity)".to_string());
    }
    let cap = match &args[0] {
        Value::Int(n) if *n >= 1 && *n <= 65536 => *n as usize,
        _ => return Err("channel.bounded() capacity must be in 1..=65536".to_string()),
    };
    let id = vm.create_channel(cap)?;
    let def = CHANNEL_DEF.with(Rc::clone);
    Ok(Value::StructInstance(Rc::new(StructInstance {
        def,
        fields: RefCell::new(vec![Value::Int(id as i64)]),
    })))
}

fn chan_send(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("channel.send() expects 2 arguments (chan, value)".to_string());
    }
    let id = extract_chan_id(&args[0], "channel.send()")?;
    vm.channel_send(id, args[1].clone())?;
    Ok(Value::Unit)
}

fn chan_try_send(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("channel.try_send() expects 2 arguments (chan, value)".to_string());
    }
    let id = extract_chan_id(&args[0], "channel.try_send()")?;
    Ok(Value::Bool(vm.channel_try_send(id, args[1].clone())?))
}

fn chan_recv(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("channel.recv() expects 1 argument (chan)".to_string());
    }
    let id = extract_chan_id(&args[0], "channel.recv()")?;
    vm.channel_recv(id)
}

fn chan_try_recv(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("channel.try_recv() expects 1 argument (chan)".to_string());
    }
    let id = extract_chan_id(&args[0], "channel.try_recv()")?;
    let (ok, val, closed) = vm.channel_try_recv(id)?;
    let mut map = new_dict_with_capacity(3);
    map.insert("ok".to_string(), Value::Bool(ok));
    map.insert("value".to_string(), val);
    map.insert("closed".to_string(), Value::Bool(closed));
    Ok(Value::dict(map))
}

fn chan_close(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("channel.close() expects 1 argument (chan)".to_string());
    }
    let id = extract_chan_id(&args[0], "channel.close()")?;
    vm.channel_close(id)?;
    Ok(Value::Unit)
}

fn chan_len(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("channel.len() expects 1 argument (chan)".to_string());
    }
    let id = extract_chan_id(&args[0], "channel.len()")?;
    Ok(Value::Int(vm.channel_len(id)? as i64))
}

fn chan_is_closed(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("channel.is_closed() expects 1 argument (chan)".to_string());
    }
    let id = extract_chan_id(&args[0], "channel.is_closed()")?;
    Ok(Value::Bool(vm.channel_is_closed(id)?))
}

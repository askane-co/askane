use std::cell::RefCell;
use std::rc::Rc;

use crate::modules::{NativeFnPtr, build_module};
use crate::value::{
    DictMap, FastMap, StructDef, StructInstance, Value, new_dict_map, new_dict_with_capacity,
};
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "collections",
        &[
            ("deque", col_deque),
            ("counter", col_counter),
            ("priority_queue", col_priority_queue),
            ("groupby", col_groupby),
            ("window", col_window),
            ("first", crate::vm::native_first),
            ("last", crate::vm::native_last),
            ("is_empty", crate::vm::native_is_empty),
            ("find_index", crate::vm::native_find_index),
            ("count", crate::vm::native_count),
            ("partition", crate::vm::native_partition),
            ("flat_map", crate::vm::native_flat_map),
            ("take_while", crate::vm::native_take_while),
            ("drop_while", crate::vm::native_drop_while),
            ("zip_longest", crate::vm::native_zip_longest),
            ("min_by", crate::vm::native_min_by),
            ("max_by", crate::vm::native_max_by),
        ],
        &[],
    )
}

fn make_struct_def(
    name: &str,
    fields: &[&str],
    methods: &[(&'static str, NativeFnPtr)],
) -> Rc<StructDef> {
    let field_names: Vec<String> = fields.iter().map(|s| (*s).to_string()).collect();
    let mut field_index = FastMap::default();
    field_index.reserve(field_names.len());
    let mut field_defaults = Vec::new();
    for (i, f) in field_names.iter().enumerate() {
        field_index.insert(f.clone(), i);
        field_defaults.push(None);
    }
    let mut method_map = new_dict_with_capacity(methods.len());
    for &(mname, func) in methods {
        method_map.insert(mname.to_string(), Value::native_fn(mname, func));
    }
    Rc::new(StructDef {
        ic_id: crate::bytecode::next_ic_id(),
        name: name.to_string(),
        field_names,
        field_index,
        field_defaults,
        methods: method_map,
    })
}

fn extract_items(vm: &mut VM, val: &Value, ctx: &str) -> Result<Vec<Value>, String> {
    match val {
        Value::List(l) => {
            let len = l.borrow().len();
            if len > 1_000_000 {
                return Err(format!("{}: list exceeds 1,000,000 items", ctx));
            }
            vm.charge_alloc(len.saturating_mul(std::mem::size_of::<Value>()))?;
            Ok(l.borrow().clone())
        }
        Value::Set(s) => {
            let len = s.borrow().items.len();
            if len > 1_000_000 {
                return Err(format!("{}: set exceeds 1,000,000 items", ctx));
            }
            vm.charge_alloc(len.saturating_mul(std::mem::size_of::<Value>()))?;
            Ok(s.borrow().items.clone())
        }
        Value::Range(r) => {
            let len = r.end.saturating_sub(r.start).max(0) as usize;
            if len > 1_000_000 {
                return Err(format!("{}: range exceeds 1,000,000 items", ctx));
            }
            vm.charge_alloc(len.saturating_mul(std::mem::size_of::<Value>()))?;
            Ok((r.start..r.end).map(Value::Int).collect())
        }
        Value::Bytes(b) => {
            if b.len() > 1_000_000 {
                return Err(format!("{}: bytes exceeds 1,000,000 items", ctx));
            }
            vm.charge_alloc(b.len().saturating_mul(std::mem::size_of::<Value>()))?;
            Ok(b.iter().map(|&x| Value::Int(x as i64)).collect())
        }
        Value::Str(s) => {
            let len = s.chars().count();
            if len > 1_000_000 {
                return Err(format!("{}: string exceeds 1,000,000 characters", ctx));
            }
            vm.charge_alloc(len.saturating_mul(std::mem::size_of::<Value>().saturating_add(24)))?;
            Ok(s.chars().map(|c| Value::from_str(c.to_string())).collect())
        }
        other => Err(format!(
            "{} expects iterable, got '{}'",
            ctx,
            other.type_name()
        )),
    }
}

fn col_deque(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() > 2 {
        return Err("collections.deque() expects 0..=2 arguments (items?, maxlen?)".to_string());
    }
    let mut items = if !args.is_empty() && !matches!(args[0], Value::Unit) {
        extract_items(vm, &args[0], "collections.deque")?
    } else {
        Vec::new()
    };
    let maxlen = if args.len() == 2 && !matches!(args[1], Value::Unit) {
        let Value::Int(m) = args[1] else {
            return Err("collections.deque() maxlen must be integer or unit".to_string());
        };
        if m < 0 {
            return Err("collections.deque() maxlen must be >= 0".to_string());
        }
        Value::Int(m)
    } else {
        Value::Unit
    };
    if let Value::Int(m) = maxlen {
        let cap = m as usize;
        if items.len() > cap {
            let drain = items.len() - cap;
            items.drain(0..drain);
        }
    }
    vm.charge_alloc(items.len().saturating_mul(std::mem::size_of::<Value>()) + 64)?;
    let def = make_struct_def(
        "Deque",
        &["items", "maxlen"],
        &[
            ("push_back", deque_push_back),
            ("push_front", deque_push_front),
            ("pop_back", deque_pop_back),
            ("pop_front", deque_pop_front),
            ("peek_front", deque_peek_front),
            ("peek_back", deque_peek_back),
            ("len", deque_len),
            ("to_list", deque_to_list),
            ("clear", deque_clear),
        ],
    );
    Ok(Value::StructInstance(Rc::new(StructInstance {
        def,
        fields: RefCell::new(vec![Value::List(Rc::new(RefCell::new(items))), maxlen]),
    })))
}

fn as_deque(v: &Value, ctx: &str) -> Result<(Rc<RefCell<Vec<Value>>>, Option<usize>), String> {
    let Value::StructInstance(inst) = v else {
        return Err(format!("{} expects Deque receiver", ctx));
    };
    let f = inst.fields.borrow();
    let Value::List(list) = f[0].clone() else {
        return Err(format!("{} invalid Deque state", ctx));
    };
    let maxlen = match f[1] {
        Value::Int(m) if m >= 0 => Some(m as usize),
        _ => None,
    };
    Ok((list, maxlen))
}

fn deque_push_back(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("Deque.push_back() expects 1 argument".to_string());
    }
    let (list_rc, maxlen) = as_deque(&args[0], "Deque.push_back")?;
    vm.charge_alloc(std::mem::size_of::<Value>())?;
    let mut list = list_rc.borrow_mut();
    list.push(args[1].clone());
    if let Some(max) = maxlen {
        while list.len() > max {
            list.remove(0);
        }
    }
    Ok(args[0].clone())
}

fn deque_push_front(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("Deque.push_front() expects 1 argument".to_string());
    }
    let (list_rc, maxlen) = as_deque(&args[0], "Deque.push_front")?;
    vm.charge_alloc(std::mem::size_of::<Value>())?;
    let mut list = list_rc.borrow_mut();
    if let Some(max) = maxlen {
        if max == 0 {
            return Ok(args[0].clone());
        }
        while list.len() >= max {
            list.pop();
        }
    }
    list.insert(0, args[1].clone());
    Ok(args[0].clone())
}

fn deque_pop_back(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("Deque.pop_back() expects 0 arguments".to_string());
    }
    let (list_rc, _) = as_deque(&args[0], "Deque.pop_back")?;
    let val = list_rc.borrow_mut().pop().unwrap_or(Value::Unit);
    Ok(val)
}

fn deque_pop_front(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("Deque.pop_front() expects 0 arguments".to_string());
    }
    let (list_rc, _) = as_deque(&args[0], "Deque.pop_front")?;
    let mut list = list_rc.borrow_mut();
    if list.is_empty() {
        Ok(Value::Unit)
    } else {
        Ok(list.remove(0))
    }
}

fn deque_peek_front(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("Deque.peek_front() expects 0 arguments".to_string());
    }
    let (list_rc, _) = as_deque(&args[0], "Deque.peek_front")?;
    let val = list_rc.borrow().first().cloned().unwrap_or(Value::Unit);
    Ok(val)
}

fn deque_peek_back(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("Deque.peek_back() expects 0 arguments".to_string());
    }
    let (list_rc, _) = as_deque(&args[0], "Deque.peek_back")?;
    let val = list_rc.borrow().last().cloned().unwrap_or(Value::Unit);
    Ok(val)
}

fn deque_len(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("Deque.len() expects 0 arguments".to_string());
    }
    let (list_rc, _) = as_deque(&args[0], "Deque.len")?;
    let len = list_rc.borrow().len() as i64;
    Ok(Value::Int(len))
}

fn deque_to_list(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("Deque.to_list() expects 0 arguments".to_string());
    }
    let (list_rc, _) = as_deque(&args[0], "Deque.to_list")?;
    let items = list_rc.borrow().clone();
    vm.charge_alloc(items.len().saturating_mul(std::mem::size_of::<Value>()))?;
    Ok(Value::List(Rc::new(RefCell::new(items))))
}

fn deque_clear(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("Deque.clear() expects 0 arguments".to_string());
    }
    let (list_rc, _) = as_deque(&args[0], "Deque.clear")?;
    list_rc.borrow_mut().clear();
    Ok(Value::Unit)
}

fn col_counter(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() > 1 {
        return Err("collections.counter() expects 0 or 1 iterable argument".to_string());
    }
    let mut counts = new_dict_map();
    if args.len() == 1 && !matches!(args[0], Value::Unit) {
        let items = extract_items(vm, &args[0], "collections.counter")?;
        for item in items {
            let k = item.to_string();
            let cur = match counts.get(&k) {
                Some(Value::Int(n)) => *n,
                _ => 0,
            };
            counts.insert(k, Value::Int(cur + 1));
        }
    }
    vm.charge_alloc(counts.len().saturating_mul(64))?;
    let def = make_struct_def(
        "Counter",
        &["counts"],
        &[
            ("add", counter_add),
            ("get", counter_get),
            ("most_common", counter_most_common),
            ("total", counter_total),
            ("to_dict", counter_to_dict),
        ],
    );
    Ok(Value::StructInstance(Rc::new(StructInstance {
        def,
        fields: RefCell::new(vec![Value::dict(counts)]),
    })))
}

fn as_counter(v: &Value, ctx: &str) -> Result<Rc<RefCell<DictMap>>, String> {
    let Value::StructInstance(inst) = v else {
        return Err(format!("{} expects Counter receiver", ctx));
    };
    let Value::Dict(d) = inst.fields.borrow()[0].clone() else {
        return Err(format!("{} invalid Counter state", ctx));
    };
    Ok(d)
}

fn counter_add(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err("Counter.add() expects (key, delta=1)".to_string());
    }
    let delta = if args.len() == 3 {
        let Value::Int(n) = args[2] else {
            return Err("Counter.add() delta must be int".to_string());
        };
        n
    } else {
        1
    };
    let d_rc = as_counter(&args[0], "Counter.add")?;
    let key = args[1].to_string();
    let mut d = d_rc.borrow_mut();
    let cur = match d.get(&key) {
        Some(Value::Int(n)) => *n,
        _ => 0,
    };
    d.insert(key, Value::Int(cur.saturating_add(delta)));
    Ok(args[0].clone())
}

fn counter_get(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("Counter.get() expects 1 argument (key)".to_string());
    }
    let d_rc = as_counter(&args[0], "Counter.get")?;
    let n = match d_rc.borrow().get(&args[1].to_string()) {
        Some(Value::Int(v)) => *v,
        _ => 0,
    };
    Ok(Value::Int(n))
}

fn counter_most_common(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() > 2 {
        return Err("Counter.most_common() expects 0 or 1 integer argument".to_string());
    }
    let limit = if args.len() == 2 {
        let Value::Int(n) = args[1] else {
            return Err("Counter.most_common() limit must be integer".to_string());
        };
        if n < 0 {
            return Err("Counter.most_common() limit must be >= 0".to_string());
        }
        Some(n as usize)
    } else {
        None
    };
    let d_rc = as_counter(&args[0], "Counter.most_common")?;
    let mut pairs: Vec<(String, i64)> = d_rc
        .borrow()
        .iter()
        .filter_map(|(k, v)| match v {
            Value::Int(n) => Some((k.clone(), *n)),
            _ => None,
        })
        .collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    if let Some(lim) = limit {
        pairs.truncate(lim);
    }
    let out: Vec<Value> = pairs
        .into_iter()
        .map(|(k, n)| {
            Value::List(Rc::new(RefCell::new(vec![
                Value::from_str(k),
                Value::Int(n),
            ])))
        })
        .collect();
    vm.charge_alloc(out.len().saturating_mul(std::mem::size_of::<Value>()))?;
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn counter_total(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("Counter.total() expects 0 arguments".to_string());
    }
    let d_rc = as_counter(&args[0], "Counter.total")?;
    let mut sum = 0i64;
    for v in d_rc.borrow().values() {
        if let Value::Int(n) = v {
            sum = sum.saturating_add(*n);
        }
    }
    Ok(Value::Int(sum))
}

fn counter_to_dict(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("Counter.to_dict() expects 0 arguments".to_string());
    }
    let d_rc = as_counter(&args[0], "Counter.to_dict")?;
    let count = d_rc.borrow().len();
    vm.charge_alloc(count.saturating_mul(64))?;
    Ok(Value::dict(d_rc.borrow().clone()))
}

fn col_priority_queue(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("collections.priority_queue() expects 0 arguments".to_string());
    }
    let def = make_struct_def(
        "PriorityQueue",
        &["heap", "seq"],
        &[
            ("push", pq_push),
            ("pop", pq_pop),
            ("peek", pq_peek),
            ("len", pq_len),
            ("is_empty", pq_is_empty),
        ],
    );
    Ok(Value::StructInstance(Rc::new(StructInstance {
        def,
        fields: RefCell::new(vec![
            Value::List(Rc::new(RefCell::new(Vec::new()))),
            Value::Int(0),
        ]),
    })))
}

fn pq_entry_key(entry: &Value) -> (f64, i64) {
    if let Value::List(l) = entry {
        let Ok(b) = l.try_borrow() else {
            return (0.0, 0);
        };
        let prio = match b.first() {
            Some(Value::Int(i)) => *i as f64,
            Some(Value::Float(f)) => *f,
            _ => 0.0,
        };
        let seq = match b.get(1) {
            Some(Value::Int(s)) => *s,
            _ => 0,
        };
        return (prio, seq);
    }
    (0.0, 0)
}

fn pq_push(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("PriorityQueue.push() expects (priority, value)".to_string());
    }
    match &args[1] {
        Value::Int(_) => {}
        Value::Float(f) if f.is_finite() => {}
        _ => return Err("PriorityQueue.push() priority must be a finite number".to_string()),
    }
    let Value::StructInstance(inst) = &args[0] else {
        return Err("PriorityQueue.push() invalid receiver".to_string());
    };
    vm.charge_alloc(std::mem::size_of::<Value>() * 4)?;
    let (heap_rc, seq) = {
        let mut f = inst.fields.borrow_mut();
        let next_seq = match f[1] {
            Value::Int(s) => s
                .checked_add(1)
                .ok_or_else(|| "PriorityQueue sequence overflow".to_string())?,
            _ => 1,
        };
        f[1] = Value::Int(next_seq);
        let Value::List(l) = f[0].clone() else {
            return Err("Invalid PriorityQueue state".to_string());
        };
        (l, next_seq)
    };
    let entry = Value::List(Rc::new(RefCell::new(vec![
        args[1].clone(),
        Value::Int(seq),
        args[2].clone(),
    ])));
    let mut heap = heap_rc.borrow_mut();
    heap.push(entry);
    let mut idx = heap.len() - 1;
    while idx > 0 {
        let parent = (idx - 1) / 2;
        if pq_entry_key(&heap[idx]) < pq_entry_key(&heap[parent]) {
            heap.swap(idx, parent);
            idx = parent;
        } else {
            break;
        }
    }
    Ok(args[0].clone())
}

fn pq_pop(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("PriorityQueue.pop() expects 0 arguments".to_string());
    }
    let Value::StructInstance(inst) = &args[0] else {
        return Err("PriorityQueue.pop() invalid receiver".to_string());
    };
    let Value::List(heap_rc) = inst.fields.borrow()[0].clone() else {
        return Err("Invalid PriorityQueue state".to_string());
    };
    let mut heap = heap_rc.borrow_mut();
    if heap.is_empty() {
        return Ok(Value::Unit);
    }
    let top = heap.swap_remove(0);
    let mut idx = 0usize;
    let n = heap.len();
    loop {
        let left = idx * 2 + 1;
        let right = left + 1;
        let mut smallest = idx;
        if left < n && pq_entry_key(&heap[left]) < pq_entry_key(&heap[smallest]) {
            smallest = left;
        }
        if right < n && pq_entry_key(&heap[right]) < pq_entry_key(&heap[smallest]) {
            smallest = right;
        }
        if smallest == idx {
            break;
        }
        heap.swap(idx, smallest);
        idx = smallest;
    }
    if let Value::List(entry_rc) = top {
        let val = entry_rc.borrow().get(2).cloned().unwrap_or(Value::Unit);
        return Ok(val);
    }
    Ok(Value::Unit)
}

fn pq_peek(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("PriorityQueue.peek() expects 0 arguments".to_string());
    }
    let Value::StructInstance(inst) = &args[0] else {
        return Err("PriorityQueue.peek() invalid receiver".to_string());
    };
    let Value::List(heap_rc) = inst.fields.borrow()[0].clone() else {
        return Err("Invalid PriorityQueue state".to_string());
    };
    let heap = heap_rc.borrow();
    if let Some(Value::List(entry_rc)) = heap.first() {
        let b = entry_rc
            .try_borrow()
            .map_err(|_| "PriorityQueue entry is currently borrowed".to_string())?;
        Ok(b.get(2).cloned().unwrap_or(Value::Unit))
    } else {
        Ok(Value::Unit)
    }
}

fn pq_len(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("PriorityQueue.len() expects 0 arguments".to_string());
    }
    let Value::StructInstance(inst) = &args[0] else {
        return Err("PriorityQueue.len() invalid receiver".to_string());
    };
    let Value::List(heap_rc) = inst.fields.borrow()[0].clone() else {
        return Err("Invalid PriorityQueue state".to_string());
    };
    let len = heap_rc.borrow().len() as i64;
    Ok(Value::Int(len))
}

fn pq_is_empty(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("PriorityQueue.is_empty() expects 0 arguments".to_string());
    }
    let Value::Int(n) = pq_len(vm, args)? else {
        unreachable!();
    };
    Ok(Value::Bool(n == 0))
}

fn col_groupby(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("collections.groupby() expects 2 arguments (iterable, key_fn)".to_string());
    }
    let items = extract_items(vm, &args[0], "collections.groupby")?;
    let key_fn = args[1].clone();
    let mut groups: FastMap<String, Vec<Value>> = FastMap::default();
    for (i, item) in items.into_iter().enumerate() {
        if (i & 1023) == 0 {
            vm.check_timeout()?;
        }
        let k = vm.call_value(key_fn.clone(), &[item.clone()])?.to_string();
        groups.entry(k).or_default().push(item);
    }
    vm.charge_alloc(groups.len().saturating_mul(64))?;
    let mut out = new_dict_with_capacity(groups.len());
    for (k, v) in groups {
        out.insert(k, Value::List(Rc::new(RefCell::new(v))));
    }
    Ok(Value::dict(out))
}

fn col_window(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err("collections.window() expects (iterable, size, step=1)".to_string());
    }
    let Value::Int(sz) = args[1] else {
        return Err("collections.window() size must be positive integer".to_string());
    };
    if sz <= 0 {
        return Err("collections.window() size must be > 0".to_string());
    }
    let step = if args.len() == 3 {
        let Value::Int(st) = args[2] else {
            return Err("collections.window() step must be positive integer".to_string());
        };
        if st <= 0 {
            return Err("collections.window() step must be > 0".to_string());
        }
        st as usize
    } else {
        1
    };
    let items = extract_items(vm, &args[0], "collections.window")?;
    let size = sz as usize;
    if items.len() < size {
        return Ok(Value::List(Rc::new(RefCell::new(Vec::new()))));
    }
    let num_windows = ((items.len() - size) / step) + 1;
    let total_elems = num_windows
        .checked_mul(size)
        .filter(|&tot| num_windows <= 1_000_000 && tot <= 5_000_000)
        .ok_or_else(|| "collections.window() result exceeds safety limit".to_string())?;
    vm.charge_alloc(
        total_elems
            .saturating_add(num_windows)
            .saturating_mul(std::mem::size_of::<Value>()),
    )?;
    vm.charge_work((total_elems / 64).max(1) as u64)?;
    let mut out = Vec::with_capacity(num_windows);
    let mut i = 0usize;
    while i + size <= items.len() {
        if (out.len() & 1023) == 0 {
            vm.check_timeout()?;
        }
        out.push(Value::List(Rc::new(RefCell::new(
            items[i..i + size].to_vec(),
        ))));
        i += step;
    }
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

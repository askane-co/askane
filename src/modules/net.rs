use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::modules::{NativeFnPtr, build_module};
use crate::value::{NativeFunction, StructDef, StructInstance, Value};
use crate::vm::VM;

const MAX_NET_READ: usize = 16 * 1024 * 1024;
const DEFAULT_ACCEPT_TIMEOUT_MS: u64 = 5000;
const DEFAULT_IO_TIMEOUT_MS: u64 = 5000;

enum NetHandle {
    TcpListener(TcpListener),
    TcpStream(TcpStream),
    UdpSocket(UdpSocket),
}

struct NetRegistry {
    next_id: u64,
    handles: HashMap<u64, NetHandle>,
    listener_def: Rc<StructDef>,
    stream_def: Rc<StructDef>,
    udp_def: Rc<StructDef>,
}

thread_local! {
    static NET_HANDLES: RefCell<NetRegistry> = RefCell::new(NetRegistry {
        next_id: 1,
        handles: HashMap::new(),
        listener_def: make_def("TcpListener", &[
            ("accept", net_tcp_accept),
            ("local_addr", net_local_addr),
            ("close", net_close),
        ]),
        stream_def: make_def("TcpStream", &[
            ("send", net_send),
            ("recv", net_recv),
            ("local_addr", net_local_addr),
            ("peer_addr", net_peer_addr),
            ("close", net_close),
        ]),
        udp_def: make_def("UdpSocket", &[
            ("send_to", net_udp_send_to),
            ("recv_from", net_udp_recv_from),
            ("local_addr", net_local_addr),
            ("close", net_close),
        ]),
    });
}

fn make_def(name: &str, methods_spec: &[(&'static str, NativeFnPtr)]) -> Rc<StructDef> {
    let field_names = vec!["__id".to_string()];
    let mut field_index = HashMap::new();
    field_index.insert("__id".to_string(), 0);
    let mut methods = HashMap::new();
    for &(mname, func) in methods_spec {
        methods.insert(
            mname.to_string(),
            Value::NativeFn(NativeFunction { name: mname, func }),
        );
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

pub fn reset_state() {
    NET_HANDLES.with(|reg| {
        reg.borrow_mut().handles.clear();
    });
}

pub fn module() -> Value {
    build_module(
        "net",
        &[
            ("tcp_listen", net_tcp_listen),
            ("tcp_accept", net_tcp_accept),
            ("tcp_connect", net_tcp_connect),
            ("send", net_send),
            ("recv", net_recv),
            ("udp_bind", net_udp_bind),
            ("udp_send_to", net_udp_send_to),
            ("udp_recv_from", net_udp_recv_from),
            ("local_addr", net_local_addr),
            ("peer_addr", net_peer_addr),
            ("close", net_close),
        ],
        &[],
    )
}

fn expect_str<'a>(v: &'a Value, ctx: &str) -> Result<&'a str, String> {
    match v {
        Value::Str(s) => Ok(s.as_str()),
        other => Err(format!(
            "{} expects a string, got '{}'",
            ctx,
            other.type_name()
        )),
    }
}

fn extract_id(v: &Value, expected: Option<&str>, ctx: &str) -> Result<u64, String> {
    let Value::StructInstance(inst) = v else {
        return Err(format!(
            "{} expects a net socket/listener handle, got '{}'",
            ctx,
            v.type_name()
        ));
    };
    let matches_def = NET_HANDLES.with(|reg| {
        let reg = reg.borrow();
        let ic = inst.def.ic_id;
        match expected {
            Some("TcpListener") => ic == reg.listener_def.ic_id,
            Some("TcpStream") => ic == reg.stream_def.ic_id,
            Some("UdpSocket") => ic == reg.udp_def.ic_id,
            _ => {
                ic == reg.listener_def.ic_id
                    || ic == reg.stream_def.ic_id
                    || ic == reg.udp_def.ic_id
            }
        }
    });
    if !matches_def {
        return Err(format!("{}: invalid net handle type", ctx));
    }
    let fields = inst.fields.borrow();
    match fields.first() {
        Some(Value::Int(id)) if *id > 0 => Ok(*id as u64),
        _ => Err(format!("{}: invalid handle id", ctx)),
    }
}

fn parse_timeout_arg(v: Option<&Value>, default_ms: u64, ctx: &str) -> Result<u64, String> {
    match v {
        None | Some(Value::Unit) => Ok(default_ms),
        Some(Value::Int(n)) if *n >= 0 && *n <= 300_000 => Ok(*n as u64),
        _ => Err(format!("{} timeout_ms must be in 0..=300000", ctx)),
    }
}

fn register_handle(handle: NetHandle) -> Value {
    NET_HANDLES.with(|reg| {
        let mut reg = reg.borrow_mut();
        let id = reg.next_id;
        reg.next_id += 1;
        let def = match &handle {
            NetHandle::TcpListener(_) => Rc::clone(&reg.listener_def),
            NetHandle::TcpStream(_) => Rc::clone(&reg.stream_def),
            NetHandle::UdpSocket(_) => Rc::clone(&reg.udp_def),
        };
        reg.handles.insert(id, handle);
        Value::StructInstance(Rc::new(StructInstance {
            def,
            fields: RefCell::new(vec![Value::Int(id as i64)]),
        }))
    })
}

fn net_tcp_listen(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("net.tcp_listen() expects 1 argument (host_port)".to_string());
    }
    let addr = expect_str(&args[0], "net.tcp_listen()")?;
    vm.check_host_port(addr)?;
    vm.charge_work(10)?;
    let listener =
        TcpListener::bind(addr).map_err(|e| format!("net.tcp_listen('{}') failed: {}", addr, e))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("net.tcp_listen() set_nonblocking failed: {}", e))?;
    Ok(register_handle(NetHandle::TcpListener(listener)))
}

fn net_tcp_accept(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("net.tcp_accept() expects (listener, [timeout_ms])".to_string());
    }
    let id = extract_id(&args[0], Some("TcpListener"), "net.tcp_accept()")?;
    let timeout_ms = parse_timeout_arg(args.get(1), DEFAULT_ACCEPT_TIMEOUT_MS, "net.tcp_accept()")?;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);

    loop {
        vm.check_timeout()?;
        let accepted = NET_HANDLES.with(|reg| {
            let reg = reg.borrow();
            let Some(NetHandle::TcpListener(listener)) = reg.handles.get(&id) else {
                return Err("net.tcp_accept(): listener is closed".to_string());
            };
            match listener.accept() {
                Ok((stream, _)) => Ok(Some(stream)),
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => Ok(None),
                Err(e) => Err(format!("net.tcp_accept() failed: {}", e)),
            }
        })?;
        if let Some(stream) = accepted {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(50)));
            return Ok(register_handle(NetHandle::TcpStream(stream)));
        }
        if Instant::now() >= deadline {
            return Ok(Value::Unit);
        }
        vm.sleep_checked(10)?;
    }
}

fn net_tcp_connect(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("net.tcp_connect() expects (host_port, [timeout_ms])".to_string());
    }
    let addr = expect_str(&args[0], "net.tcp_connect()")?;
    vm.check_host_port(addr)?;
    let timeout_ms = parse_timeout_arg(args.get(1), DEFAULT_IO_TIMEOUT_MS, "net.tcp_connect()")?;
    vm.charge_work(15)?;
    let sock_addr = addr
        .to_socket_addrs()
        .map_err(|e| format!("net.tcp_connect() address resolution failed: {}", e))?
        .next()
        .ok_or_else(|| "net.tcp_connect(): no resolved socket address".to_string())?;
    let stream = TcpStream::connect_timeout(&sock_addr, Duration::from_millis(timeout_ms))
        .map_err(|e| format!("net.tcp_connect() failed: {}", e))?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(50)));
    Ok(register_handle(NetHandle::TcpStream(stream)))
}

fn net_send(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("net.send() expects 2 arguments (stream, data)".to_string());
    }
    let id = extract_id(&args[0], Some("TcpStream"), "net.send()")?;
    let data = args[1].as_bytes_cow("net.send()")?;
    if data.len() > MAX_NET_READ {
        return Err("net.send(): payload exceeds 16 MiB limit".to_string());
    }
    vm.charge_io(data.len())?;
    vm.charge_work((data.len() as u64) / 64 + 4)?;

    let mut written = 0usize;
    while written < data.len() {
        vm.check_timeout()?;
        let n = NET_HANDLES.with(|reg| {
            let mut reg = reg.borrow_mut();
            let Some(NetHandle::TcpStream(stream)) = reg.handles.get_mut(&id) else {
                return Err("net.send(): stream is closed".to_string());
            };
            match stream.write(&data[written..]) {
                Ok(0) => Err("net.send(): connection closed cleanly".to_string()),
                Ok(n) => Ok(n),
                Err(ref e)
                    if e.kind() == ErrorKind::WouldBlock
                        || e.kind() == ErrorKind::TimedOut
                        || e.kind() == ErrorKind::Interrupted =>
                {
                    Ok(0)
                }
                Err(e) => Err(format!("net.send() failed: {}", e)),
            }
        })?;
        written += n;
    }
    Ok(Value::Int(written as i64))
}

fn net_recv(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 3 {
        return Err("net.recv() expects (stream, [max_bytes], [timeout_ms])".to_string());
    }
    let id = extract_id(&args[0], Some("TcpStream"), "net.recv()")?;
    let max_bytes = match args.get(1) {
        None | Some(Value::Unit) => 65536,
        Some(Value::Int(n)) if *n > 0 && *n <= MAX_NET_READ as i64 => *n as usize,
        _ => return Err("net.recv() max_bytes must be in 1..=16777216".to_string()),
    };
    let timeout_ms = parse_timeout_arg(args.get(2), DEFAULT_IO_TIMEOUT_MS, "net.recv()")?;
    vm.charge_alloc(max_bytes.min(65536))?;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut buf = vec![0u8; max_bytes];

    loop {
        vm.check_timeout()?;
        enum Step {
            Data(usize),
            Eof,
            Retry,
        }
        let step = NET_HANDLES.with(|reg| {
            let mut reg = reg.borrow_mut();
            let Some(NetHandle::TcpStream(stream)) = reg.handles.get_mut(&id) else {
                return Err("net.recv(): stream is closed".to_string());
            };
            match stream.read(&mut buf) {
                Ok(0) => Ok(Step::Eof),
                Ok(n) => Ok(Step::Data(n)),
                Err(ref e)
                    if e.kind() == ErrorKind::WouldBlock
                        || e.kind() == ErrorKind::TimedOut
                        || e.kind() == ErrorKind::Interrupted =>
                {
                    Ok(Step::Retry)
                }
                Err(e) => Err(format!("net.recv() failed: {}", e)),
            }
        })?;
        match step {
            Step::Data(n) => {
                buf.truncate(n);
                vm.charge_io(n)?;
                return Ok(Value::Bytes(Rc::new(buf)));
            }
            Step::Eof => return Ok(Value::Bytes(Rc::new(Vec::new()))),
            Step::Retry => {
                if Instant::now() >= deadline {
                    return Ok(Value::Unit);
                }
            }
        }
    }
}

fn net_udp_bind(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("net.udp_bind() expects 1 argument (host_port)".to_string());
    }
    let addr = expect_str(&args[0], "net.udp_bind()")?;
    vm.check_host_port(addr)?;
    let sock =
        UdpSocket::bind(addr).map_err(|e| format!("net.udp_bind('{}') failed: {}", addr, e))?;
    let _ = sock.set_read_timeout(Some(Duration::from_millis(50)));
    Ok(register_handle(NetHandle::UdpSocket(sock)))
}

fn net_udp_send_to(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("net.udp_send_to() expects 3 arguments (sock, host_port, data)".to_string());
    }
    let id = extract_id(&args[0], Some("UdpSocket"), "net.udp_send_to()")?;
    let addr = expect_str(&args[1], "net.udp_send_to()")?;
    vm.check_host_port(addr)?;
    let data = args[2].as_bytes_cow("net.udp_send_to()")?;
    if data.len() > 65507 {
        return Err("net.udp_send_to(): datagram exceeds 65507 bytes".to_string());
    }
    vm.charge_io(data.len())?;
    let n = NET_HANDLES.with(|reg| {
        let reg = reg.borrow();
        let Some(NetHandle::UdpSocket(sock)) = reg.handles.get(&id) else {
            return Err("net.udp_send_to(): socket is closed".to_string());
        };
        sock.send_to(&data, addr)
            .map_err(|e| format!("net.udp_send_to() failed: {}", e))
    })?;
    Ok(Value::Int(n as i64))
}

fn net_udp_recv_from(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 3 {
        return Err("net.udp_recv_from() expects (sock, [max_bytes], [timeout_ms])".to_string());
    }
    let id = extract_id(&args[0], Some("UdpSocket"), "net.udp_recv_from()")?;
    let max_bytes = match args.get(1) {
        None | Some(Value::Unit) => 65536,
        Some(Value::Int(n)) if *n > 0 && *n <= 65536 => *n as usize,
        _ => return Err("net.udp_recv_from() max_bytes must be in 1..=65536".to_string()),
    };
    let timeout_ms = parse_timeout_arg(args.get(2), DEFAULT_IO_TIMEOUT_MS, "net.udp_recv_from()")?;
    vm.charge_alloc(max_bytes)?;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut buf = vec![0u8; max_bytes];

    loop {
        vm.check_timeout()?;
        let res = NET_HANDLES.with(|reg| {
            let reg = reg.borrow();
            let Some(NetHandle::UdpSocket(sock)) = reg.handles.get(&id) else {
                return Err("net.udp_recv_from(): socket is closed".to_string());
            };
            match sock.recv_from(&mut buf) {
                Ok((n, peer)) => Ok(Some((n, peer.to_string()))),
                Err(ref e)
                    if e.kind() == ErrorKind::WouldBlock
                        || e.kind() == ErrorKind::TimedOut
                        || e.kind() == ErrorKind::Interrupted =>
                {
                    Ok(None)
                }
                Err(e) => Err(format!("net.udp_recv_from() failed: {}", e)),
            }
        })?;
        if let Some((n, peer)) = res {
            buf.truncate(n);
            vm.charge_io(n)?;
            let mut out = HashMap::new();
            out.insert("data".to_string(), Value::Bytes(Rc::new(buf)));
            out.insert("addr".to_string(), Value::Str(peer));
            return Ok(Value::Dict(Rc::new(RefCell::new(out))));
        }
        if Instant::now() >= deadline {
            return Ok(Value::Unit);
        }
    }
}

fn net_local_addr(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("net.local_addr() expects 1 argument (handle)".to_string());
    }
    let id = extract_id(&args[0], None, "net.local_addr()")?;
    NET_HANDLES.with(|reg| {
        let reg = reg.borrow();
        let addr = match reg.handles.get(&id) {
            Some(NetHandle::TcpListener(l)) => l.local_addr().map_err(|e| e.to_string())?,
            Some(NetHandle::TcpStream(s)) => s.local_addr().map_err(|e| e.to_string())?,
            Some(NetHandle::UdpSocket(u)) => u.local_addr().map_err(|e| e.to_string())?,
            None => return Err("net.local_addr(): handle is closed".to_string()),
        };
        Ok(Value::Str(addr.to_string()))
    })
}

fn net_peer_addr(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("net.peer_addr() expects 1 argument (stream)".to_string());
    }
    let id = extract_id(&args[0], Some("TcpStream"), "net.peer_addr()")?;
    NET_HANDLES.with(|reg| {
        let reg = reg.borrow();
        let Some(NetHandle::TcpStream(s)) = reg.handles.get(&id) else {
            return Err("net.peer_addr(): stream is closed".to_string());
        };
        let addr = s.peer_addr().map_err(|e| e.to_string())?;
        Ok(Value::Str(addr.to_string()))
    })
}

fn net_close(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("net.close() expects 1 argument (handle)".to_string());
    }
    let id = extract_id(&args[0], None, "net.close()")?;
    NET_HANDLES.with(|reg| {
        reg.borrow_mut().handles.remove(&id);
    });
    Ok(Value::Unit)
}

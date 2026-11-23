use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::modules::base64::{STD_ALPHABET, encode_bytes};
use crate::modules::random::os_random_bytes;
use crate::modules::{NativeFnPtr, build_module};
use crate::value::{NativeFunction, StructDef, StructInstance, Value};
use crate::vm::VM;

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MAX_WS_FRAME: usize = 8 * 1024 * 1024;
const MAX_HTTP_HEADER: usize = 16 * 1024;

struct WsConn {
    stream: TcpStream,
    is_client: bool,
}

enum WsHandle {
    Listener(TcpListener),
    Conn(WsConn),
}

struct WsRegistry {
    next_id: u64,
    handles: HashMap<u64, WsHandle>,
    listener_def: Rc<StructDef>,
    conn_def: Rc<StructDef>,
}

thread_local! {
    static WS_HANDLES: RefCell<WsRegistry> = RefCell::new(WsRegistry {
        next_id: 1,
        handles: HashMap::new(),
        listener_def: make_def("WsListener", &[
            ("accept", ws_accept),
            ("local_addr", ws_local_addr),
            ("close", ws_close),
        ]),
        conn_def: make_def("WsConn", &[
            ("send", ws_send),
            ("send_bytes", ws_send_bytes),
            ("recv", ws_recv),
            ("local_addr", ws_local_addr),
            ("close", ws_close),
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
    WS_HANDLES.with(|reg| {
        reg.borrow_mut().handles.clear();
    });
}

pub fn module() -> Value {
    build_module(
        "ws",
        &[
            ("listen", ws_listen),
            ("accept", ws_accept),
            ("connect", ws_connect),
            ("send", ws_send),
            ("send_bytes", ws_send_bytes),
            ("recv", ws_recv),
            ("local_addr", ws_local_addr),
            ("close", ws_close),
        ],
        &[],
    )
}

fn sha1_bytes(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity((data.len() + 9).div_ceil(64) * 64);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDCu32),
                _ => (b ^ c ^ d, 0xCA62_C1D6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn ws_accept_key(key: &str) -> String {
    let combined = format!("{}{}", key.trim(), WS_GUID);
    let digest = sha1_bytes(combined.as_bytes());
    encode_bytes(&digest, STD_ALPHABET, true)
}

fn register_ws(handle: WsHandle) -> Value {
    WS_HANDLES.with(|reg| {
        let mut reg = reg.borrow_mut();
        let id = reg.next_id;
        reg.next_id += 1;
        let def = match &handle {
            WsHandle::Listener(_) => Rc::clone(&reg.listener_def),
            WsHandle::Conn(_) => Rc::clone(&reg.conn_def),
        };
        reg.handles.insert(id, handle);
        Value::StructInstance(Rc::new(StructInstance {
            def,
            fields: RefCell::new(vec![Value::Int(id as i64)]),
        }))
    })
}

fn extract_ws_id(v: &Value, expected: Option<&str>, ctx: &str) -> Result<u64, String> {
    let Value::StructInstance(inst) = v else {
        return Err(format!(
            "{} expects a ws handle, got '{}'",
            ctx,
            v.type_name()
        ));
    };
    let ok = WS_HANDLES.with(|reg| {
        let reg = reg.borrow();
        let ic = inst.def.ic_id;
        match expected {
            Some("WsListener") => ic == reg.listener_def.ic_id,
            Some("WsConn") => ic == reg.conn_def.ic_id,
            _ => ic == reg.listener_def.ic_id || ic == reg.conn_def.ic_id,
        }
    });
    if !ok {
        return Err(format!("{}: invalid ws handle", ctx));
    }
    let fields = inst.fields.borrow();
    match fields.first() {
        Some(Value::Int(id)) if *id > 0 => Ok(*id as u64),
        _ => Err(format!("{}: invalid ws handle id", ctx)),
    }
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

fn read_http_header_block(stream: &mut TcpStream, vm: &VM) -> Result<String, String> {
    let mut raw = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        vm.check_timeout()?;
        match stream.read(&mut byte) {
            Ok(0) => return Err("ws handshake: unexpected EOF".to_string()),
            Ok(_) => {
                raw.push(byte[0]);
                if raw.len() > MAX_HTTP_HEADER {
                    return Err("ws handshake: HTTP header exceeds 16 KiB limit".to_string());
                }
                if raw.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            Err(ref e)
                if e.kind() == ErrorKind::WouldBlock
                    || e.kind() == ErrorKind::TimedOut
                    || e.kind() == ErrorKind::Interrupted =>
            {
                if Instant::now() >= deadline {
                    return Err("ws handshake: timed out".to_string());
                }
            }
            Err(e) => return Err(format!("ws handshake I/O error: {}", e)),
        }
    }
    String::from_utf8(raw).map_err(|_| "ws handshake: non-UTF8 header".to_string())
}

fn write_all_checked(stream: &mut TcpStream, mut data: &[u8], vm: &VM) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !data.is_empty() {
        vm.check_timeout()?;
        match stream.write(data) {
            Ok(0) => return Err("ws write: connection closed".to_string()),
            Ok(n) => data = &data[n..],
            Err(ref e)
                if e.kind() == ErrorKind::WouldBlock
                    || e.kind() == ErrorKind::TimedOut
                    || e.kind() == ErrorKind::Interrupted =>
            {
                if Instant::now() >= deadline {
                    return Err("ws write: timed out".to_string());
                }
            }
            Err(e) => return Err(format!("ws write failed: {}", e)),
        }
    }
    Ok(())
}

fn read_exact_checked(
    stream: &mut TcpStream,
    buf: &mut [u8],
    vm: &VM,
    deadline: Instant,
    allow_timeout_on_first: bool,
) -> Result<bool, String> {
    let mut pos = 0usize;
    while pos < buf.len() {
        vm.check_timeout()?;
        match stream.read(&mut buf[pos..]) {
            Ok(0) => {
                if pos == 0 {
                    return Ok(false);
                }
                return Err("ws frame: unexpected EOF".to_string());
            }
            Ok(n) => pos += n,
            Err(ref e)
                if e.kind() == ErrorKind::WouldBlock
                    || e.kind() == ErrorKind::TimedOut
                    || e.kind() == ErrorKind::Interrupted =>
            {
                if Instant::now() >= deadline {
                    if allow_timeout_on_first && pos == 0 {
                        return Ok(false);
                    }
                    return Err("ws frame: timed out".to_string());
                }
            }
            Err(e) => return Err(format!("ws frame read failed: {}", e)),
        }
    }
    Ok(true)
}

fn send_ws_frame(conn: &mut WsConn, opcode: u8, payload: &[u8], vm: &VM) -> Result<(), String> {
    if payload.len() > MAX_WS_FRAME {
        return Err("ws.send(): message exceeds 8 MiB limit".to_string());
    }
    let mut frame = Vec::with_capacity(14 + payload.len());
    frame.push(0x80 | (opcode & 0x0F));
    let mask_bit: u8 = if conn.is_client { 0x80 } else { 0x00 };
    if payload.len() <= 125 {
        frame.push(mask_bit | (payload.len() as u8));
    } else if payload.len() <= 65535 {
        frame.push(mask_bit | 126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        frame.push(mask_bit | 127);
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    if conn.is_client {
        let mut mask = [0u8; 4];
        os_random_bytes(&mut mask)?;
        frame.extend_from_slice(&mask);
        for (i, &b) in payload.iter().enumerate() {
            frame.push(b ^ mask[i & 3]);
        }
    } else {
        frame.extend_from_slice(payload);
    }
    write_all_checked(&mut conn.stream, &frame, vm)
}

fn ws_listen(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("ws.listen() expects 1 argument (host_port)".to_string());
    }
    let addr = expect_str(&args[0], "ws.listen()")?;
    vm.check_host_port(addr)?;
    let listener =
        TcpListener::bind(addr).map_err(|e| format!("ws.listen('{}') failed: {}", addr, e))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("ws.listen() set_nonblocking failed: {}", e))?;
    Ok(register_ws(WsHandle::Listener(listener)))
}

fn ws_accept(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("ws.accept() expects (listener, [timeout_ms])".to_string());
    }
    let id = extract_ws_id(&args[0], Some("WsListener"), "ws.accept()")?;
    let timeout_ms = match args.get(1) {
        None | Some(Value::Unit) => 5000u64,
        Some(Value::Int(n)) if *n >= 0 && *n <= 300_000 => *n as u64,
        _ => return Err("ws.accept() timeout_ms must be in 0..=300000".to_string()),
    };
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);

    let mut stream = loop {
        vm.check_timeout()?;
        let accepted = WS_HANDLES.with(|reg| {
            let reg = reg.borrow();
            let Some(WsHandle::Listener(listener)) = reg.handles.get(&id) else {
                return Err("ws.accept(): listener is closed".to_string());
            };
            match listener.accept() {
                Ok((s, _)) => Ok(Some(s)),
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => Ok(None),
                Err(e) => Err(format!("ws.accept() failed: {}", e)),
            }
        })?;
        if let Some(s) = accepted {
            break s;
        }
        if Instant::now() >= deadline {
            return Ok(Value::Unit);
        }
        vm.sleep_checked(10)?;
    };

    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(50)));
    let req = read_http_header_block(&mut stream, vm)?;
    let mut ws_key = None;
    for line in req.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("sec-websocket-key") {
                ws_key = Some(v.trim().to_string());
            }
        }
    }
    let key = ws_key.ok_or_else(|| "ws.accept(): missing Sec-WebSocket-Key".to_string())?;
    let accept = ws_accept_key(&key);
    let resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        accept
    );
    write_all_checked(&mut stream, resp.as_bytes(), vm)?;
    Ok(register_ws(WsHandle::Conn(WsConn {
        stream,
        is_client: false,
    })))
}

fn ws_connect(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("ws.connect() expects (ws_url, [timeout_ms])".to_string());
    }
    let url = expect_str(&args[0], "ws.connect()")?;
    if url.is_empty()
        || url.len() > 4096
        || url
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return Err("ws.connect(): invalid URL characters".to_string());
    }
    let rest = url
        .strip_prefix("ws://")
        .ok_or_else(|| "ws.connect(): only ws:// URLs are supported (zero-crate)".to_string())?;
    let (authority, path_part) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    vm.check_host_port(authority)?;
    let timeout_ms = match args.get(1) {
        None | Some(Value::Unit) => 5000u64,
        Some(Value::Int(n)) if *n >= 0 && *n <= 300_000 => *n as u64,
        _ => return Err("ws.connect() timeout_ms must be in 0..=300000".to_string()),
    };

    let host_port =
        if authority.contains(':') && (!authority.starts_with('[') || authority.contains("]:")) {
            authority.to_string()
        } else {
            format!("{}:80", authority)
        };
    let sock_addr = host_port
        .to_socket_addrs()
        .map_err(|e| format!("ws.connect() address resolution failed: {}", e))?
        .next()
        .ok_or_else(|| "ws.connect(): no resolved socket address".to_string())?;
    let mut stream = TcpStream::connect_timeout(&sock_addr, Duration::from_millis(timeout_ms))
        .map_err(|e| format!("ws.connect() failed: {}", e))?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(50)));

    let mut key_raw = [0u8; 16];
    os_random_bytes(&mut key_raw)?;
    let key_b64 = encode_bytes(&key_raw, STD_ALPHABET, true);
    let expected_accept = ws_accept_key(&key_b64);

    let req = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n\r\n",
        path_part, authority, key_b64
    );
    write_all_checked(&mut stream, req.as_bytes(), vm)?;
    let resp = read_http_header_block(&mut stream, vm)?;
    let mut lines = resp.lines();
    let status = lines.next().unwrap_or("");
    if !status.contains(" 101 ") {
        return Err(format!("ws.connect() handshake rejected: {}", status));
    }
    let mut got_accept = None;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("sec-websocket-accept") {
                got_accept = Some(v.trim().to_string());
            }
        }
    }
    if got_accept.as_deref() != Some(expected_accept.as_str()) {
        return Err("ws.connect(): invalid Sec-WebSocket-Accept from server".to_string());
    }
    Ok(register_ws(WsHandle::Conn(WsConn {
        stream,
        is_client: true,
    })))
}

fn ws_send(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("ws.send() expects 2 arguments (conn, text_or_bytes)".to_string());
    }
    let id = extract_ws_id(&args[0], Some("WsConn"), "ws.send()")?;
    let (opcode, payload) = match &args[1] {
        Value::Str(s) => (1u8, s.as_bytes().to_vec()),
        Value::Bytes(b) => (2u8, (**b).clone()),
        other => {
            return Err(format!(
                "ws.send() expects string or bytes, got '{}'",
                other.type_name()
            ));
        }
    };
    vm.charge_io(payload.len())?;
    vm.charge_work((payload.len() as u64) / 64 + 4)?;
    WS_HANDLES.with(|reg| {
        let mut reg = reg.borrow_mut();
        let Some(WsHandle::Conn(conn)) = reg.handles.get_mut(&id) else {
            return Err("ws.send(): connection is closed".to_string());
        };
        send_ws_frame(conn, opcode, &payload, vm)
    })?;
    Ok(Value::Unit)
}

fn ws_send_bytes(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("ws.send_bytes() expects 2 arguments (conn, bytes)".to_string());
    }
    let id = extract_ws_id(&args[0], Some("WsConn"), "ws.send_bytes()")?;
    let data = args[1].as_bytes_cow("ws.send_bytes()")?;
    vm.charge_io(data.len())?;
    vm.charge_work((data.len() as u64) / 64 + 4)?;
    WS_HANDLES.with(|reg| {
        let mut reg = reg.borrow_mut();
        let Some(WsHandle::Conn(conn)) = reg.handles.get_mut(&id) else {
            return Err("ws.send_bytes(): connection is closed".to_string());
        };
        send_ws_frame(conn, 2u8, &data, vm)
    })?;
    Ok(Value::Unit)
}

fn ws_recv(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("ws.recv() expects (conn, [timeout_ms])".to_string());
    }
    let id = extract_ws_id(&args[0], Some("WsConn"), "ws.recv()")?;
    let timeout_ms = match args.get(1) {
        None | Some(Value::Unit) => 5000u64,
        Some(Value::Int(n)) if *n >= 0 && *n <= 300_000 => *n as u64,
        _ => return Err("ws.recv() timeout_ms must be in 0..=300000".to_string()),
    };
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);

    let mut msg_buf = Vec::new();
    let mut msg_opcode: Option<u8> = None;

    loop {
        enum FrameOutcome {
            Done(Value),
            Continue,
        }
        let outcome = WS_HANDLES.with(|reg| {
            let mut reg = reg.borrow_mut();
            let Some(WsHandle::Conn(conn)) = reg.handles.get_mut(&id) else {
                return Err("ws.recv(): connection is closed".to_string());
            };
            let mut hdr = [0u8; 2];
            if !read_exact_checked(
                &mut conn.stream,
                &mut hdr,
                vm,
                deadline,
                msg_opcode.is_none(),
            )? {
                return Ok(FrameOutcome::Done(Value::Unit));
            }
            let fin = (hdr[0] & 0x80) != 0;
            let opcode = hdr[0] & 0x0F;
            let masked = (hdr[1] & 0x80) != 0;
            let mut len = (hdr[1] & 0x7F) as u64;
            if len == 126 {
                let mut ext = [0u8; 2];
                read_exact_checked(&mut conn.stream, &mut ext, vm, deadline, false)?;
                len = u16::from_be_bytes(ext) as u64;
            } else if len == 127 {
                let mut ext = [0u8; 8];
                read_exact_checked(&mut conn.stream, &mut ext, vm, deadline, false)?;
                len = u64::from_be_bytes(ext);
            }
            if len > MAX_WS_FRAME as u64 {
                return Err("ws.recv(): frame exceeds 8 MiB limit".to_string());
            }
            let mut mask = [0u8; 4];
            if masked {
                read_exact_checked(&mut conn.stream, &mut mask, vm, deadline, false)?;
            }
            let len_usize = len as usize;
            vm.charge_alloc(len_usize)?;
            let mut payload = vec![0u8; len_usize];
            if len_usize > 0 {
                read_exact_checked(&mut conn.stream, &mut payload, vm, deadline, false)?;
            }
            if masked {
                for (i, b) in payload.iter_mut().enumerate() {
                    *b ^= mask[i & 3];
                }
            }
            vm.charge_io(len as usize)?;

            match opcode {
                0x8 => {
                    let _ = send_ws_frame(conn, 0x8, &[], vm);
                    Ok(FrameOutcome::Done(Value::Unit))
                }
                0x9 => {
                    send_ws_frame(conn, 0xA, &payload, vm)?;
                    Ok(FrameOutcome::Continue)
                }
                0xA => Ok(FrameOutcome::Continue),
                0x1 | 0x2 | 0x0 => {
                    if opcode != 0x0 {
                        msg_opcode = Some(opcode);
                    }
                    if msg_buf.len().saturating_add(payload.len()) > MAX_WS_FRAME {
                        return Err("ws.recv(): assembled message exceeds 8 MiB limit".to_string());
                    }
                    msg_buf.extend_from_slice(&payload);
                    if fin {
                        let out = match msg_opcode.unwrap_or(2) {
                            1 => {
                                let text = String::from_utf8(std::mem::take(&mut msg_buf))
                                    .map_err(|_| {
                                        "ws.recv(): invalid UTF-8 text frame".to_string()
                                    })?;
                                Value::Str(text)
                            }
                            _ => Value::Bytes(Rc::new(std::mem::take(&mut msg_buf))),
                        };
                        Ok(FrameOutcome::Done(out))
                    } else {
                        Ok(FrameOutcome::Continue)
                    }
                }
                other => Err(format!("ws.recv(): unsupported opcode {}", other)),
            }
        })?;
        if let FrameOutcome::Done(v) = outcome {
            return Ok(v);
        }
    }
}

fn ws_local_addr(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("ws.local_addr() expects 1 argument (handle)".to_string());
    }
    let id = extract_ws_id(&args[0], None, "ws.local_addr()")?;
    WS_HANDLES.with(|reg| {
        let reg = reg.borrow();
        let addr = match reg.handles.get(&id) {
            Some(WsHandle::Listener(l)) => l.local_addr().map_err(|e| e.to_string())?,
            Some(WsHandle::Conn(c)) => c.stream.local_addr().map_err(|e| e.to_string())?,
            None => return Err("ws.local_addr(): handle is closed".to_string()),
        };
        Ok(Value::Str(addr.to_string()))
    })
}

fn ws_close(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("ws.close() expects 1 argument (handle)".to_string());
    }
    let id = extract_ws_id(&args[0], None, "ws.close()")?;
    let removed = WS_HANDLES.with(|reg| reg.borrow_mut().handles.remove(&id));
    if let Some(WsHandle::Conn(mut conn)) = removed {
        let _ = send_ws_frame(&mut conn, 0x8, &[], vm);
        let _ = conn.stream.shutdown(std::net::Shutdown::Both);
    }
    Ok(Value::Unit)
}

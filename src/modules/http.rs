use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module("http", &[("get", http_get), ("post", http_post)], &[])
}

#[cfg(windows)]
mod winhttp {
    use std::ffi::c_void;

    use crate::value::{Value, new_dict_with_capacity};

    pub type HInternet = *mut c_void;

    const WINHTTP_ACCESS_TYPE_DEFAULT_PROXY: u32 = 0;
    const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
    const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
    const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;

    #[link(name = "winhttp")]
    unsafe extern "system" {
        fn WinHttpOpen(
            pszAgentW: *const u16,
            dwAccessType: u32,
            pszProxyW: *const u16,
            pszProxyBypassW: *const u16,
            dwFlags: u32,
        ) -> HInternet;
        fn WinHttpConnect(
            hSession: HInternet,
            pswzServerName: *const u16,
            nServerPort: u16,
            dwReserved: u32,
        ) -> HInternet;
        fn WinHttpOpenRequest(
            hConnect: HInternet,
            pwszVerb: *const u16,
            pwszObjectName: *const u16,
            pwszVersion: *const u16,
            pwszReferrer: *const u16,
            ppwszAcceptTypes: *const *const u16,
            dwFlags: u32,
        ) -> HInternet;
        fn WinHttpSendRequest(
            hRequest: HInternet,
            lpszHeaders: *const u16,
            dwHeadersLength: u32,
            lpOptional: *const c_void,
            dwOptionalLength: u32,
            dwTotalLength: u32,
            dwContext: usize,
        ) -> i32;
        fn WinHttpReceiveResponse(hRequest: HInternet, lpReserved: *mut c_void) -> i32;
        fn WinHttpQueryHeaders(
            hRequest: HInternet,
            dwInfoLevel: u32,
            pwszName: *const u16,
            lpBuffer: *mut c_void,
            lpdwBufferLength: *mut u32,
            lpdwIndex: *mut u32,
        ) -> i32;
        fn WinHttpQueryDataAvailable(
            hRequest: HInternet,
            lpdwNumberOfBytesAvailable: *mut u32,
        ) -> i32;
        fn WinHttpReadData(
            hRequest: HInternet,
            lpBuffer: *mut c_void,
            dwNumberOfBytesToRead: u32,
            lpdwNumberOfBytesRead: *mut u32,
        ) -> i32;
        fn WinHttpCloseHandle(hInternet: HInternet) -> i32;
        fn WinHttpSetTimeouts(
            hInternet: HInternet,
            resolve: i32,
            connect: i32,
            send: i32,
            receive: i32,
        ) -> i32;
        fn WinHttpSetOption(
            hInternet: HInternet,
            option: u32,
            buffer: *const c_void,
            length: u32,
        ) -> i32;
    }

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    struct ParsedUrl {
        is_https: bool,
        host: String,
        port: u16,
        path: String,
    }

    fn parse_url(url: &str) -> Result<ParsedUrl, String> {
        if url.len() > 8192
            || url
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
        {
            return Err("Invalid URL characters or URL size".to_string());
        }
        let (is_https, rest) = if let Some(r) = url.strip_prefix("https://") {
            (true, r)
        } else {
            return Err("Only https:// URLs are allowed".to_string());
        };

        let rest = rest.split('#').next().unwrap_or(rest);
        let (host_port, path) = match rest.find(['/', '?']) {
            Some(idx) => (&rest[..idx], &rest[idx..]),
            None => (rest, "/"),
        };

        let (host, port) = match host_port.rsplit_once(':') {
            Some((h, p)) => {
                let port_num: u16 = p
                    .parse()
                    .map_err(|_| format!("Invalid port in URL: '{}'", p))?;
                (h.to_string(), port_num)
            }
            None => (host_port.to_string(), if is_https { 443 } else { 80 }),
        };

        if host.is_empty() || host.contains('@') || port == 0 {
            return Err("Empty host in URL".to_string());
        }

        Ok(ParsedUrl {
            is_https,
            host,
            port,
            path: if path.starts_with('?') {
                format!("/{}", path)
            } else {
                path.to_string()
            },
        })
    }

    pub fn request(
        method: &str,
        url: &str,
        headers: Option<&str>,
        body: Option<&[u8]>,
    ) -> Result<Value, String> {
        let parsed = parse_url(url)?;
        if body.is_some_and(|b| b.len() > 32 * 1024 * 1024)
            || headers.is_some_and(|h| h.len() > 64 * 1024)
        {
            return Err("HTTP request size limit exceeded".to_string());
        }
        let agent = to_wide("Askane/0.1");
        let host_w = to_wide(&parsed.host);
        let method_w = to_wide(method);
        let path_w = to_wide(&parsed.path);
        let headers_w = headers.map(to_wide);

        unsafe {
            let session = WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
                std::ptr::null(),
                std::ptr::null(),
                0,
            );
            if session.is_null() {
                return Err("WinHttpOpen failed".to_string());
            }
            if WinHttpSetTimeouts(session, 5000, 5000, 10000, 10000) == 0 {
                WinHttpCloseHandle(session);
                return Err("Could not configure HTTP timeouts".to_string());
            }

            let connect = WinHttpConnect(session, host_w.as_ptr(), parsed.port, 0);
            if connect.is_null() {
                WinHttpCloseHandle(session);
                return Err(format!("WinHttpConnect failed for '{}'", parsed.host));
            }

            let flags = if parsed.is_https {
                WINHTTP_FLAG_SECURE
            } else {
                0
            };
            let req = WinHttpOpenRequest(
                connect,
                method_w.as_ptr(),
                path_w.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                flags,
            );
            if req.is_null() {
                WinHttpCloseHandle(connect);
                WinHttpCloseHandle(session);
                return Err("WinHttpOpenRequest failed".to_string());
            }
            // Never forward credentials to a redirect target or use ambient Windows credentials.
            let redirect_policy: u32 = 0; // WINHTTP_OPTION_REDIRECT_POLICY_NEVER
            let autologon_policy: u32 = 2; // WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH
            if WinHttpSetOption(req, 88, (&redirect_policy as *const u32).cast(), 4) == 0
                || WinHttpSetOption(req, 77, (&autologon_policy as *const u32).cast(), 4) == 0
            {
                WinHttpCloseHandle(req);
                WinHttpCloseHandle(connect);
                WinHttpCloseHandle(session);
                return Err("Could not configure HTTP security policy".to_string());
            }

            let (hdr_ptr, hdr_len) = match &headers_w {
                Some(h) => (h.as_ptr(), (h.len() - 1) as u32),
                None => (std::ptr::null(), 0),
            };
            let (body_ptr, body_len) = match body {
                Some(b) if !b.is_empty() => (b.as_ptr() as *const c_void, b.len() as u32),
                _ => (std::ptr::null(), 0),
            };

            let sent = WinHttpSendRequest(req, hdr_ptr, hdr_len, body_ptr, body_len, body_len, 0);
            if sent == 0 || WinHttpReceiveResponse(req, std::ptr::null_mut()) == 0 {
                WinHttpCloseHandle(req);
                WinHttpCloseHandle(connect);
                WinHttpCloseHandle(session);
                return Err("HTTP request failed".to_string());
            }

            let mut status_code: u32 = 0;
            let mut status_size: u32 = std::mem::size_of::<u32>() as u32;
            WinHttpQueryHeaders(
                req,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                std::ptr::null(),
                (&mut status_code as *mut u32) as *mut c_void,
                &mut status_size,
                std::ptr::null_mut(),
            );

            const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;
            let mut resp_bytes = Vec::new();
            loop {
                let mut avail: u32 = 0;
                if WinHttpQueryDataAvailable(req, &mut avail) == 0 || avail == 0 {
                    break;
                }
                if resp_bytes.len().saturating_add(avail as usize) > MAX_BODY_BYTES {
                    WinHttpCloseHandle(req);
                    WinHttpCloseHandle(connect);
                    WinHttpCloseHandle(session);
                    return Err("HTTP response exceeded 32 MB limit".to_string());
                }
                let mut chunk = vec![0u8; avail as usize];
                let mut read: u32 = 0;
                if WinHttpReadData(req, chunk.as_mut_ptr() as *mut c_void, avail, &mut read) == 0
                    || read == 0
                {
                    break;
                }
                resp_bytes.extend_from_slice(&chunk[..read as usize]);
            }

            WinHttpCloseHandle(req);
            WinHttpCloseHandle(connect);
            WinHttpCloseHandle(session);

            let body_str = String::from_utf8_lossy(&resp_bytes).to_string();
            let mut map = new_dict_with_capacity(2);
            map.insert("status".to_string(), Value::Int(status_code as i64));
            map.insert("body".to_string(), Value::from_str(body_str));
            Ok(Value::dict(map))
        }
    }
}

#[cfg(windows)]
fn format_headers_dict(val: &Value, fn_name: &str) -> Result<String, String> {
    match val {
        Value::Dict(d) => {
            let mut out = String::new();
            for (k, v) in d.borrow().iter() {
                if k.is_empty()
                    || !k
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                {
                    return Err(format!("http.{}(): invalid header name {:?}", fn_name, k));
                }
                let vs = format!("{}", v);
                if vs.contains('\r') || vs.contains('\n') || vs.contains('\0') {
                    return Err(format!(
                        "http.{}(): invalid header value for {:?}",
                        fn_name, k
                    ));
                }
                out.push_str(&format!("{}: {}\r\n", k, vs));
            }
            Ok(out)
        }
        other => Err(format!(
            "http.{}() expects a headers dict, got '{}'",
            fn_name,
            other.type_name()
        )),
    }
}

#[cfg(windows)]
fn http_get(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("http.get() expects (url) or (url, headers_dict)".to_string());
    }
    let url = match &args[0] {
        Value::Str(s) => s.as_str(),
        other => {
            return Err(format!(
                "http.get() expects a string url, got '{}'",
                other.type_name()
            ));
        }
    };
    if !url.starts_with("https://") {
        return Err("Only https:// URLs are allowed".to_string());
    }
    vm.check_net(url)?;
    vm.check_timeout()?;
    let headers = if args.len() == 2 {
        Some(format_headers_dict(&args[1], "get")?)
    } else {
        None
    };
    winhttp::request("GET", url, headers.as_deref(), None)
}

#[cfg(windows)]
fn http_post(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err(
            "http.post() expects (url, body) or (url, body, headers_dict | content_type)"
                .to_string(),
        );
    }
    let url = match &args[0] {
        Value::Str(s) => s.as_str(),
        other => {
            return Err(format!(
                "http.post() expects a string url, got '{}'",
                other.type_name()
            ));
        }
    };
    if !url.starts_with("https://") {
        return Err("Only https:// URLs are allowed".to_string());
    }
    vm.check_net(url)?;
    vm.check_timeout()?;
    let body = match &args[1] {
        Value::Str(s) => s.as_bytes(),
        other => {
            return Err(format!(
                "http.post() expects a string body, got '{}'",
                other.type_name()
            ));
        }
    };
    let header = if args.len() == 3 {
        match &args[2] {
            Value::Str(s) => {
                if s.chars().any(char::is_control) {
                    return Err("Invalid Content-Type".to_string());
                }
                format!("Content-Type: {}\r\n", s)
            }
            Value::Dict(_) => format_headers_dict(&args[2], "post")?,
            other => {
                return Err(format!(
                    "http.post() expects string content_type or headers dict, got '{}'",
                    other.type_name()
                ));
            }
        }
    } else {
        "Content-Type: application/json\r\n".to_string()
    };
    winhttp::request("POST", url, Some(&header), Some(body))
}

#[cfg(not(windows))]
fn http_get(_vm: &mut VM, _args: &[Value]) -> Result<Value, String> {
    Err("http module is currently only supported on Windows".to_string())
}

#[cfg(not(windows))]
fn http_post(_vm: &mut VM, _args: &[Value]) -> Result<Value, String> {
    Err("http module is currently only supported on Windows".to_string())
}

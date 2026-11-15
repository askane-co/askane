use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module("http", &[("get", http_get), ("post", http_post)], &[])
}

#[cfg(windows)]
mod winhttp {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::rc::Rc;

    use crate::value::Value;

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
        if url.contains('\0') {
            return Err("URL cannot contain null bytes".to_string());
        }
        let (is_https, rest) = if let Some(r) = url.strip_prefix("https://") {
            (true, r)
        } else {
            return Err("Only https:// URLs are allowed".to_string());
        };

        let (host_port, path) = match rest.find('/') {
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

        if host.is_empty() {
            return Err("Empty host in URL".to_string());
        }

        Ok(ParsedUrl {
            is_https,
            host,
            port,
            path: path.to_string(),
        })
    }

    pub fn request(
        method: &str,
        url: &str,
        headers: Option<&str>,
        body: Option<&[u8]>,
    ) -> Result<Value, String> {
        let parsed = parse_url(url)?;
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
                return Err(format!("HTTP request to '{}' failed", url));
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
            let mut map = HashMap::new();
            map.insert("status".to_string(), Value::Int(status_code as i64));
            map.insert("body".to_string(), Value::Str(body_str));
            Ok(Value::Dict(Rc::new(RefCell::new(map))))
        }
    }
}

#[cfg(windows)]
fn http_get(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("http.get() expects 1 argument (url)".to_string());
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
    winhttp::request("GET", url, None, None)
}

#[cfg(windows)]
fn http_post(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err("http.post() expects (url, body) or (url, body, content_type)".to_string());
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
    let body = match &args[1] {
        Value::Str(s) => s.as_bytes(),
        other => {
            return Err(format!(
                "http.post() expects a string body, got '{}'",
                other.type_name()
            ));
        }
    };
    let content_type = if args.len() == 3 {
        match &args[2] {
            Value::Str(s) => s.as_str(),
            other => {
                return Err(format!(
                    "http.post() expects a string content_type, got '{}'",
                    other.type_name()
                ));
            }
        }
    } else {
        "application/json"
    };
    let header = format!("Content-Type: {}\r\n", content_type);
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

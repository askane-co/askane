#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedHttpsUrl {
    pub host: String,
    pub port: u16,
    pub path: String,
}

pub fn parse_https_url(url: &str) -> Result<ParsedHttpsUrl, String> {
    if url.len() > 8192 || url.chars().any(|c| c.is_control() || c.is_whitespace() || c == '\\') {
        return Err("Invalid URL characters or URL size".to_string());
    }
    let Some(rest) = url.strip_prefix("https://") else {
        return Err("Only https:// URLs are allowed".to_string());
    };
    let rest = rest.split('#').next().unwrap_or(rest);
    let (host_port, path) = match rest.find(['/', '?']) {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    let (host, port) = if let Some(bracketed) = host_port.strip_prefix('[') {
        let Some((h, tail)) = bracketed.split_once(']') else {
            return Err("Invalid IPv6 host in URL".to_string());
        };
        let p = if let Some(ps) = tail.strip_prefix(':') {
            ps.parse::<u16>()
                .map_err(|_| format!("Invalid port in URL: '{}'", ps))?
        } else {
            443
        };
        (h.to_string(), p)
    } else if let Some((h, ps)) = host_port.rsplit_once(':') {
        let p = ps
            .parse::<u16>()
            .map_err(|_| format!("Invalid port in URL: '{}'", ps))?;
        (h.to_string(), p)
    } else {
        (host_port.to_string(), 443)
    };
    if host.is_empty() || host.contains('@') || port == 0 {
        return Err("Empty host or port in URL".to_string());
    }
    let clean_path = if path.starts_with('?') {
        format!("/{}", path)
    } else {
        path.to_string()
    };
    Ok(ParsedHttpsUrl {
        host,
        port,
        path: clean_path,
    })
}

pub fn build_http1_request(
    method: &str,
    host: &str,
    port: u16,
    path: &str,
    extra_headers: Option<&str>,
    body: Option<&[u8]>,
) -> Vec<u8> {
    let host_hdr = if port == 443 {
        host.to_string()
    } else {
        format!("{}:{}", host, port)
    };
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: Askane/0.20\r\nAccept: */*\r\nConnection: close\r\n",
        method, path, host_hdr
    );
    if let Some(h) = extra_headers {
        head.push_str(h);
    }
    if let Some(b) = body {
        head.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    if let Some(b) = body {
        out.extend_from_slice(b);
    }
    out
}

pub fn decode_chunked_body(raw: &[u8], max_bytes: usize) -> Result<Vec<u8>, String> {
    let mut pos = 0usize;
    let mut out = Vec::new();
    while pos < raw.len() {
        let Some(nl_rel) = raw[pos..].windows(2).position(|w| w == b"\r\n") else {
            return Err("Incomplete HTTP chunked header".to_string());
        };
        let line = std::str::from_utf8(&raw[pos..pos + nl_rel])
            .map_err(|_| "Invalid HTTP chunk size encoding".to_string())?;
        let hex_part = line.split(';').next().unwrap_or("").trim();
        let sz = usize::from_str_radix(hex_part, 16)
            .map_err(|_| format!("Invalid HTTP chunk size '{}'", hex_part))?;
        pos += nl_rel + 2;
        if sz == 0 {
            return Ok(out);
        }
        if out.len().saturating_add(sz) > max_bytes {
            return Err("HTTP response exceeded 32 MB limit".to_string());
        }
        if pos + sz + 2 > raw.len() {
            return Err("Truncated HTTP chunked body".to_string());
        }
        out.extend_from_slice(&raw[pos..pos + sz]);
        pos += sz + 2;
    }
    Ok(out)
}

#[cfg(target_os = "linux")]
pub mod linux_https {
    use std::ffi::{c_char, c_int, c_long, c_void};
    use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::platform::dl::DynLib;

    const SSL_CTRL_OPTIONS: c_int = 32;
    const SSL_CTRL_SET_TLSEXT_HOSTNAME: c_int = 55;
    const SSL_CTRL_SET_MIN_PROTO_VERSION: c_int = 123;
    const TLS1_2_VERSION: c_long = 0x0303;
    const TLSEXT_NAMETYPE_HOST_NAME: c_long = 0;
    const SSL_VERIFY_PEER: c_int = 1;
    const SSL_OP_NO_COMPRESSION: c_long = 0x0002_0000;
    const SSL_ERROR_WANT_READ: c_int = 2;
    const SSL_ERROR_WANT_WRITE: c_int = 3;

    type FnSslCtxNew = unsafe extern "C" fn(*const c_void) -> *mut c_void;
    type FnSslCtxFree = unsafe extern "C" fn(*mut c_void);
    type FnTlsClientMethod = unsafe extern "C" fn() -> *const c_void;
    type FnSslCtxCtrl = unsafe extern "C" fn(*mut c_void, c_int, c_long, *mut c_void) -> c_long;
    type FnSslCtxSetVerifyPaths = unsafe extern "C" fn(*mut c_void) -> c_int;
    type FnSslCtxSetVerify = unsafe extern "C" fn(*mut c_void, c_int, *const c_void);
    type FnSslNew = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
    type FnSslFree = unsafe extern "C" fn(*mut c_void);
    type FnSslSetFd = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
    type FnSslCtrl = unsafe extern "C" fn(*mut c_void, c_int, c_long, *mut c_void) -> c_long;
    type FnSslGet0Param = unsafe extern "C" fn(*const c_void) -> *mut c_void;
    type FnParamSetHost = unsafe extern "C" fn(*mut c_void, *const c_char, usize) -> c_int;
    type FnSslConnect = unsafe extern "C" fn(*mut c_void) -> c_int;
    type FnSslGetVerifyResult = unsafe extern "C" fn(*const c_void) -> c_long;
    type FnSslWrite = unsafe extern "C" fn(*mut c_void, *const c_void, c_int) -> c_int;
    type FnSslRead = unsafe extern "C" fn(*mut c_void, *mut c_void, c_int) -> c_int;
    type FnSslGetError = unsafe extern "C" fn(*const c_void, c_int) -> c_int;
    type FnSslShutdown = unsafe extern "C" fn(*mut c_void) -> c_int;

    pub fn probe_openssl() -> Result<(), String> {
        let _ssl = DynLib::open(&["libssl.so.3", "libssl.so.1.1"])?;
        let _crypto = DynLib::open(&["libcrypto.so.3", "libcrypto.so.1.1"])?;
        Ok(())
    }

    pub fn request(
        method: &str,
        initial_url: &str,
        extra_headers: Option<&str>,
        body: Option<&[u8]>,
        mut check_url_allowed: impl FnMut(&str) -> Result<(), String>,
    ) -> Result<(u16, String), String> {
        let mut cur_url = initial_url.to_string();
        for hop in 0..=5usize {
            check_url_allowed(&cur_url)?;
            let parsed = parse_https_url(&cur_url)?;
            let addr: SocketAddr = format!("{}:{}", parsed.host, parsed.port)
                .to_socket_addrs()
                .map_err(|e| format!("DNS resolution failed: {}", e))?
                .next()
                .ok_or_else(|| "DNS resolution returned no address".to_string())?;

            let req_bytes = if hop == 0 {
                build_http1_request(method, &parsed.host, parsed.port, &parsed.path, extra_headers, body)
            } else {
                build_http1_request("GET", &parsed.host, parsed.port, &parsed.path, None, None)
            };

            let (status, location, resp_body) =
                https_one_shot(&parsed.host, addr, &req_bytes, Duration::from_secs(10))?;

            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                let Some(loc) = location else {
                    return Ok((status, resp_body));
                };
                if loc.starts_with("https://") {
                    cur_url = loc;
                    continue;
                }
                return Err("HTTPS redirect target rejected (non-https scheme)".to_string());
            }
            return Ok((status, resp_body));
        }
        Err("Too many HTTP redirects (> 5)".to_string())
    }

    fn https_one_shot(
        host: &str,
        addr: SocketAddr,
        req_bytes: &[u8],
        timeout: Duration,
    ) -> Result<(u16, Option<String>, String), String> {
        let ssl_lib = DynLib::open(&["libssl.so.3", "libssl.so.1.1"])?;
        let crypto_lib = DynLib::open(&["libcrypto.so.3", "libcrypto.so.1.1"])?;

        // SAFETY: Symbol signatures follow OpenSSL 1.1 / 3.x C ABI.
        let (
            tls_method,
            ctx_new,
            ctx_free,
            ctx_ctrl,
            ctx_verify_paths,
            ctx_set_verify,
            ssl_new,
            ssl_free,
            ssl_set_fd,
            ssl_ctrl,
            ssl_get0_param,
            param_set_host,
            ssl_connect,
            ssl_verify_res,
            ssl_write,
            ssl_read,
            ssl_get_error,
            ssl_shutdown,
        ): (
            FnTlsClientMethod,
            FnSslCtxNew,
            FnSslCtxFree,
            FnSslCtxCtrl,
            FnSslCtxSetVerifyPaths,
            FnSslCtxSetVerify,
            FnSslNew,
            FnSslFree,
            FnSslSetFd,
            FnSslCtrl,
            FnSslGet0Param,
            FnParamSetHost,
            FnSslConnect,
            FnSslGetVerifyResult,
            FnSslWrite,
            FnSslRead,
            FnSslGetError,
            FnSslShutdown,
        ) = unsafe {
            (
                std::mem::transmute(ssl_lib.sym("TLS_client_method")?),
                std::mem::transmute(ssl_lib.sym("SSL_CTX_new")?),
                std::mem::transmute(ssl_lib.sym("SSL_CTX_free")?),
                std::mem::transmute(ssl_lib.sym("SSL_CTX_ctrl")?),
                std::mem::transmute(ssl_lib.sym("SSL_CTX_set_default_verify_paths")?),
                std::mem::transmute(ssl_lib.sym("SSL_CTX_set_verify")?),
                std::mem::transmute(ssl_lib.sym("SSL_new")?),
                std::mem::transmute(ssl_lib.sym("SSL_free")?),
                std::mem::transmute(ssl_lib.sym("SSL_set_fd")?),
                std::mem::transmute(ssl_lib.sym("SSL_ctrl")?),
                std::mem::transmute(ssl_lib.sym("SSL_get0_param")?),
                std::mem::transmute(crypto_lib.sym("X509_VERIFY_PARAM_set1_host")?),
                std::mem::transmute(ssl_lib.sym("SSL_connect")?),
                std::mem::transmute(ssl_lib.sym("SSL_get_verify_result")?),
                std::mem::transmute(ssl_lib.sym("SSL_write")?),
                std::mem::transmute(ssl_lib.sym("SSL_read")?),
                std::mem::transmute(ssl_lib.sym("SSL_get_error")?),
                std::mem::transmute(ssl_lib.sym("SSL_shutdown")?),
            )
        };

        let tcp = TcpStream::connect_timeout(&addr, timeout)
            .map_err(|e| format!("HTTPS TCP connect failed: {}", e))?;
        let _ = tcp.set_read_timeout(Some(timeout));
        let _ = tcp.set_write_timeout(Some(timeout));

        let deadline = Instant::now() + timeout;
        // SAFETY: OpenSSL handles are created, configured, checked for null, and freed on exit.
        unsafe {
            let ctx = ctx_new(tls_method());
            if ctx.is_null() {
                return Err("SSL_CTX_new failed".to_string());
            }
            ctx_ctrl(ctx, SSL_CTRL_SET_MIN_PROTO_VERSION, TLS1_2_VERSION, std::ptr::null_mut());
            ctx_ctrl(ctx, SSL_CTRL_OPTIONS, SSL_OP_NO_COMPRESSION, std::ptr::null_mut());
            if ctx_verify_paths(ctx) != 1 {
                ctx_free(ctx);
                return Err("SSL_CTX_set_default_verify_paths failed".to_string());
            }
            ctx_set_verify(ctx, SSL_VERIFY_PEER, std::ptr::null());

            let ssl = ssl_new(ctx);
            if ssl.is_null() {
                ctx_free(ctx);
                return Err("SSL_new failed".to_string());
            }

            let c_host: Vec<u8> = host.bytes().chain(std::iter::once(0)).collect();
            ssl_ctrl(
                ssl,
                SSL_CTRL_SET_TLSEXT_HOSTNAME,
                TLSEXT_NAMETYPE_HOST_NAME,
                c_host.as_ptr() as *mut c_void,
            );
            let param = ssl_get0_param(ssl);
            if param.is_null()
                || param_set_host(param, c_host.as_ptr() as *const c_char, host.len()) != 1
            {
                ssl_free(ssl);
                ctx_free(ctx);
                return Err("X509_VERIFY_PARAM_set1_host failed".to_string());
            }
            if ssl_set_fd(ssl, tcp.as_raw_fd()) != 1 {
                ssl_free(ssl);
                ctx_free(ctx);
                return Err("SSL_set_fd failed".to_string());
            }

            loop {
                if Instant::now() > deadline {
                    ssl_free(ssl);
                    ctx_free(ctx);
                    return Err("HTTPS TLS handshake timeout".to_string());
                }
                let rc = ssl_connect(ssl);
                if rc == 1 {
                    break;
                }
                let err = ssl_get_error(ssl, rc);
                if err == SSL_ERROR_WANT_READ || err == SSL_ERROR_WANT_WRITE {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                ssl_free(ssl);
                ctx_free(ctx);
                return Err("TLS handshake failed".to_string());
            }

            if ssl_verify_res(ssl) != 0 {
                ssl_shutdown(ssl);
                ssl_free(ssl);
                ctx_free(ctx);
                return Err("TLS certificate verification failed".to_string());
            }

            let mut written = 0usize;
            while written < req_bytes.len() {
                if Instant::now() > deadline {
                    ssl_shutdown(ssl);
                    ssl_free(ssl);
                    ctx_free(ctx);
                    return Err("HTTPS write timeout".to_string());
                }
                let n = ssl_write(
                    ssl,
                    req_bytes[written..].as_ptr() as *const c_void,
                    (req_bytes.len() - written).min(65536) as c_int,
                );
                if n > 0 {
                    written += n as usize;
                } else {
                    let err = ssl_get_error(ssl, n);
                    if err == SSL_ERROR_WANT_WRITE || err == SSL_ERROR_WANT_READ {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    ssl_shutdown(ssl);
                    ssl_free(ssl);
                    ctx_free(ctx);
                    return Err("HTTPS TLS write failed".to_string());
                }
            }

            const MAX_RESP: usize = 32 * 1024 * 1024 + 65536;
            let mut raw_resp = Vec::new();
            let mut chunk = [0u8; 16384];
            loop {
                if Instant::now() > deadline {
                    ssl_shutdown(ssl);
                    ssl_free(ssl);
                    ctx_free(ctx);
                    return Err("HTTPS read timeout".to_string());
                }
                let n = ssl_read(ssl, chunk.as_mut_ptr() as *mut c_void, chunk.len() as c_int);
                if n > 0 {
                    if raw_resp.len().saturating_add(n as usize) > MAX_RESP {
                        ssl_shutdown(ssl);
                        ssl_free(ssl);
                        ctx_free(ctx);
                        return Err("HTTP response exceeded 32 MB limit".to_string());
                    }
                    raw_resp.extend_from_slice(&chunk[..n as usize]);
                } else {
                    let err = ssl_get_error(ssl, n);
                    if err == SSL_ERROR_WANT_READ || err == SSL_ERROR_WANT_WRITE {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    break;
                }
            }

            ssl_shutdown(ssl);
            ssl_free(ssl);
            ctx_free(ctx);

            let Some(hdr_end) = raw_resp.windows(4).position(|w| w == b"\r\n\r\n") else {
                return Err("Malformed HTTP response headers".to_string());
            };
            let hdr_str = String::from_utf8_lossy(&raw_resp[..hdr_end]);
            let mut lines = hdr_str.split("\r\n");
            let status_line = lines.next().unwrap_or("");
            let status: u16 = status_line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| "Malformed HTTP status line".to_string())?;
            let mut is_chunked = false;
            let mut location: Option<String> = None;
            for line in lines {
                if let Some((k, v)) = line.split_once(':') {
                    if k.trim().eq_ignore_ascii_case("transfer-encoding")
                        && v.to_ascii_lowercase().contains("chunked")
                    {
                        is_chunked = true;
                    } else if k.trim().eq_ignore_ascii_case("location") {
                        location = Some(v.trim().to_string());
                    }
                }
            }
            let body_slice = &raw_resp[hdr_end + 4..];
            let body_bytes = if is_chunked {
                decode_chunked_body(body_slice, 32 * 1024 * 1024)?
            } else {
                if body_slice.len() > 32 * 1024 * 1024 {
                    return Err("HTTP response exceeded 32 MB limit".to_string());
                }
                body_slice.to_vec()
            };
            let body_str = String::from_utf8_lossy(&body_bytes).to_string();
            Ok((status, location, body_str))
        }
    }
}

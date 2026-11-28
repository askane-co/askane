pub mod dl;
pub mod linux_tls;
pub mod linux_x11;

use std::env;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendStatus {
    Ready,
    Unavailable,
}

impl BackendStatus {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Clone, Debug)]
pub struct BackendProbe {
    pub subsystem: &'static str,
    pub backend: &'static str,
    pub status: BackendStatus,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct BackendReport {
    pub os: &'static str,
    pub arch: &'static str,
    pub probes: Vec<BackendProbe>,
}

pub fn probe_backends() -> BackendReport {
    let mut probes = Vec::new();

    #[cfg(windows)]
    {
        let has_winmm = dl::DynLib::open(&["winmm.dll"]).is_ok();
        let has_winhttp = dl::DynLib::open(&["winhttp.dll"]).is_ok();
        let has_user32 = dl::DynLib::open(&["user32.dll"]).is_ok();
        probes.push(BackendProbe {
            subsystem: "gfx",
            backend: "win32_gdi",
            status: if has_user32 {
                BackendStatus::Ready
            } else {
                BackendStatus::Unavailable
            },
            detail: "Software Surface + Win32 StretchDIBits + Headless".to_string(),
        });
        probes.push(BackendProbe {
            subsystem: "audio",
            backend: if has_winmm { "waveout" } else { "null" },
            status: BackendStatus::Ready,
            detail: if has_winmm {
                "Win32 waveOut CALLBACK_EVENT + NullSink".to_string()
            } else {
                "NullSink only (winmm.dll missing)".to_string()
            },
        });
        probes.push(BackendProbe {
            subsystem: "http",
            backend: "winhttp",
            status: if has_winhttp {
                BackendStatus::Ready
            } else {
                BackendStatus::Unavailable
            },
            detail: "WinHTTP TLS + --allow-net gating".to_string(),
        });
        probes.push(BackendProbe {
            subsystem: "clipboard",
            backend: "win32_clipboard",
            status: if has_user32 {
                BackendStatus::Ready
            } else {
                BackendStatus::Unavailable
            },
            detail: "CF_UNICODETEXT Win32 clipboard".to_string(),
        });
    }

    #[cfg(target_os = "linux")]
    {
        let display_ok = env::var("DISPLAY")
            .ok()
            .is_some_and(|d| !d.trim().is_empty() && linux_x11::parse_display(&d).is_ok());
        probes.push(BackendProbe {
            subsystem: "gfx",
            backend: if display_ok { "raw_x11" } else { "headless" },
            status: if display_ok {
                BackendStatus::Ready
            } else {
                BackendStatus::Unavailable
            },
            detail: if display_ok {
                "Pure-Rust raw X11 UnixStream + Headless Surface".to_string()
            } else {
                "DISPLAY not set; gfx.headless(true) works without display server".to_string()
            },
        });

        let has_alsa = dl::DynLib::open(&["libasound.so.2"]).is_ok();
        probes.push(BackendProbe {
            subsystem: "audio",
            backend: if has_alsa { "alsa_dl" } else { "null" },
            status: BackendStatus::Ready,
            detail: if has_alsa {
                "ALSA (libasound.so.2 via dlopen) + NullSink".to_string()
            } else {
                "NullSink + render_offline (libasound.so.2 not found)".to_string()
            },
        });

        let tls_ok = linux_tls::linux_https::probe_openssl().is_ok();
        probes.push(BackendProbe {
            subsystem: "http",
            backend: "openssl_dl",
            status: if tls_ok {
                BackendStatus::Ready
            } else {
                BackendStatus::Unavailable
            },
            detail: if tls_ok {
                "OpenSSL (libssl.so via dlopen) with DNS pinning & SNI".to_string()
            } else {
                "libssl.so.3 / libcrypto.so.3 not found".to_string()
            },
        });

        probes.push(BackendProbe {
            subsystem: "clipboard",
            backend: "x11_selection",
            status: BackendStatus::Unavailable,
            detail: if display_ok {
                "X11 clipboard selection requires active window/INCR-free cap".to_string()
            } else {
                "DISPLAY not set".to_string()
            },
        });
    }

    #[cfg(target_os = "macos")]
    {
        probes.push(BackendProbe {
            subsystem: "gfx",
            backend: "headless",
            status: BackendStatus::Unavailable,
            detail: "macOS window backend deferred to 0.21; gfx.headless(true) ready".to_string(),
        });
        probes.push(BackendProbe {
            subsystem: "audio",
            backend: "null",
            status: BackendStatus::Ready,
            detail: "Deterministic Mixer + NullSink + render_offline".to_string(),
        });
        probes.push(BackendProbe {
            subsystem: "http",
            backend: "unavailable",
            status: BackendStatus::Unavailable,
            detail: "macOS Security framework TLS deferred to 0.21".to_string(),
        });
        let has_pbcopy = std::path::Path::new("/usr/bin/pbcopy").is_file()
            && std::path::Path::new("/usr/bin/pbpaste").is_file();
        probes.push(BackendProbe {
            subsystem: "clipboard",
            backend: "pbcopy",
            status: if has_pbcopy {
                BackendStatus::Ready
            } else {
                BackendStatus::Unavailable
            },
            detail: "/usr/bin/pbcopy & /usr/bin/pbpaste (256 KiB cap)".to_string(),
        });
    }

    BackendReport {
        os: env::consts::OS,
        arch: env::consts::ARCH,
        probes,
    }
}

#[cfg(target_os = "macos")]
pub mod macos_clip {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const MAX_MAC_CLIP_BYTES: usize = 256 * 1024;

    pub fn get_text() -> Result<String, String> {
        let mut child = Command::new("/usr/bin/pbpaste")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("E_BACKEND_UNAVAILABLE: /usr/bin/pbpaste: {}", e))?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                if !status.success() {
                    return Err("E_BACKEND_UNAVAILABLE: pbpaste failed".to_string());
                }
                let out = child.wait_with_output().map_err(|e| e.to_string())?;
                if out.stdout.len() > MAX_MAC_CLIP_BYTES {
                    return Err("Clipboard text exceeds 256 KiB safety limit".to_string());
                }
                return Ok(String::from_utf8_lossy(&out.stdout).to_string());
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err("E_BACKEND_UNAVAILABLE: pbpaste timed out".to_string());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn set_text(text: &str) -> Result<(), String> {
        if text.len() > MAX_MAC_CLIP_BYTES {
            return Err("Clipboard text exceeds 256 KiB safety limit".to_string());
        }
        let mut child = Command::new("/usr/bin/pbcopy")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("E_BACKEND_UNAVAILABLE: /usr/bin/pbcopy: {}", e))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| e.to_string())?;
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                if !status.success() {
                    return Err("E_BACKEND_UNAVAILABLE: pbcopy failed".to_string());
                }
                return Ok(());
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err("E_BACKEND_UNAVAILABLE: pbcopy timed out".to_string());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(lp_lib_file_name: *const u16) -> *mut c_void;
        fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const u8) -> *const c_void;
        fn FreeLibrary(h_lib_module: *mut c_void) -> i32;
    }

    pub struct DynLib {
        handle: *mut c_void,
    }

    impl DynLib {
        pub fn open(candidates: &[&str]) -> Result<Self, String> {
            for &name in candidates {
                if name.is_empty() || name.contains('\0') {
                    continue;
                }
                let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
                // SAFETY: `wide` is null-terminated UTF-16 valid for the call duration.
                let h = unsafe { LoadLibraryW(wide.as_ptr()) };
                if !h.is_null() {
                    return Ok(Self { handle: h });
                }
            }
            Err(format!(
                "E_BACKEND_UNAVAILABLE: failed to load library ({})",
                candidates.join(", ")
            ))
        }

        #[cfg_attr(windows, allow(dead_code))]
        pub fn sym(&self, symbol: &str) -> Result<*mut c_void, String> {
            if symbol.contains('\0') {
                return Err("E_BACKEND_UNAVAILABLE: symbol name contains NUL".to_string());
            }
            let mut c_name = Vec::with_capacity(symbol.len() + 1);
            c_name.extend_from_slice(symbol.as_bytes());
            c_name.push(0);
            // SAFETY: `self.handle` is an open HMODULE and `c_name` is NUL-terminated.
            let p = unsafe { GetProcAddress(self.handle, c_name.as_ptr()) };
            if p.is_null() {
                Err(format!(
                    "E_BACKEND_UNAVAILABLE: symbol '{}' not found",
                    symbol
                ))
            } else {
                Ok(p as *mut c_void)
            }
        }
    }

    impl Drop for DynLib {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                // SAFETY: `self.handle` was returned by `LoadLibraryW`.
                unsafe {
                    FreeLibrary(self.handle);
                }
                self.handle = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(unix)]
mod imp {
    use std::ffi::{c_char, c_int, c_void};

    #[cfg(target_os = "macos")]
    const RTLD_FLAGS: c_int = 0x2 | 0x4; // RTLD_NOW | RTLD_LOCAL
    #[cfg(not(target_os = "macos"))]
    const RTLD_FLAGS: c_int = 0x2; // RTLD_NOW | RTLD_LOCAL (0 on glibc)

    #[cfg_attr(target_env = "gnu", link(name = "dl"))]
    unsafe extern "C" {
        fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn dlclose(handle: *mut c_void) -> c_int;
        fn dlerror() -> *const c_char;
    }

    pub struct DynLib {
        handle: *mut c_void,
    }

    impl DynLib {
        pub fn open(candidates: &[&str]) -> Result<Self, String> {
            for &name in candidates {
                if name.is_empty() || name.contains('\0') {
                    continue;
                }
                let mut c_name = Vec::with_capacity(name.len() + 1);
                c_name.extend_from_slice(name.as_bytes());
                c_name.push(0);
                // SAFETY: `c_name` is NUL-terminated valid C string.
                unsafe {
                    let _ = dlerror();
                    let h = dlopen(c_name.as_ptr() as *const c_char, RTLD_FLAGS);
                    if !h.is_null() {
                        return Ok(Self { handle: h });
                    }
                }
            }
            Err(format!(
                "E_BACKEND_UNAVAILABLE: failed to load shared library ({})",
                candidates.join(", ")
            ))
        }

        #[cfg_attr(windows, allow(dead_code))]
        pub fn sym(&self, symbol: &str) -> Result<*mut c_void, String> {
            if symbol.contains('\0') {
                return Err("E_BACKEND_UNAVAILABLE: symbol name contains NUL".to_string());
            }
            let mut c_name = Vec::with_capacity(symbol.len() + 1);
            c_name.extend_from_slice(symbol.as_bytes());
            c_name.push(0);
            // SAFETY: `self.handle` is a valid dlopen handle and `c_name` is NUL-terminated.
            unsafe {
                let _ = dlerror();
                let ptr = dlsym(self.handle, c_name.as_ptr() as *const c_char);
                let err = dlerror();
                if ptr.is_null() || !err.is_null() {
                    Err(format!(
                        "E_BACKEND_UNAVAILABLE: symbol '{}' not resolved",
                        symbol
                    ))
                } else {
                    Ok(ptr)
                }
            }
        }
    }

    impl Drop for DynLib {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                // SAFETY: `self.handle` was returned by `dlopen`.
                unsafe {
                    dlclose(self.handle);
                }
                self.handle = std::ptr::null_mut();
            }
        }
    }
}

pub use imp::DynLib;

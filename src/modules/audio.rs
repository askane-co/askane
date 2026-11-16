use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module("audio", &[("play", audio_play), ("stop", audio_stop)], &[])
}

#[cfg(windows)]
mod win32_audio {
    use std::ptr::null_mut;

    const SND_FILENAME: u32 = 0x0002_0000;
    const SND_ASYNC: u32 = 0x0000_0001;
    const SND_NODEFAULT: u32 = 0x0000_0002;

    #[link(name = "winmm")]
    unsafe extern "system" {
        fn PlaySoundW(pszSound: *const u16, hmod: *mut std::ffi::c_void, fdwSound: u32) -> i32;
    }

    pub fn play_wav(path: &std::path::Path) -> Result<(), String> {
        let wide: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let ok = unsafe {
            PlaySoundW(
                wide.as_ptr(),
                null_mut(),
                SND_FILENAME | SND_ASYNC | SND_NODEFAULT,
            )
        };
        if ok == 0 {
            Err(format!("audio.play(): failed to play '{}'", path.display()))
        } else {
            Ok(())
        }
    }

    pub fn stop() {
        unsafe {
            PlaySoundW(std::ptr::null(), null_mut(), 0);
        }
    }
}

fn audio_play(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("audio.play() expects 1 string argument (wav path)".to_string());
    }
    let path = match &args[0] {
        Value::Str(s) => s.as_str(),
        other => {
            return Err(format!(
                "audio.play() expects a string path, got '{}'",
                other.type_name()
            ));
        }
    };
    let resolved = vm.resolve_read_path(path)?;
    if !resolved.exists() {
        return Err(format!("audio.play('{}'): file not found", path));
    }
    #[cfg(windows)]
    {
        win32_audio::play_wav(&resolved)?;
        Ok(Value::Unit)
    }
    #[cfg(not(windows))]
    {
        let _ = resolved;
        Err("audio.play() is currently supported on Windows only".to_string())
    }
}

fn audio_stop(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("audio.stop() expects 0 arguments".to_string());
    }
    #[cfg(windows)]
    win32_audio::stop();
    Ok(Value::Unit)
}

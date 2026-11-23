#[cfg(windows)]
use std::io::{self, Write};

pub(crate) fn add_dir_to_path_list(existing: &str, dir: &str) -> (String, bool) {
    let norm_target = normalize_dir_for_cmp(dir);
    let mut parts: Vec<String> = existing
        .split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(ToString::to_string)
        .collect();
    if parts
        .iter()
        .any(|p| normalize_dir_for_cmp(p) == norm_target)
    {
        return (parts.join(";"), false);
    }
    parts.push(dir.trim().to_string());
    (parts.join(";"), true)
}

pub(crate) fn remove_dir_from_path_list(existing: &str, dir: &str) -> (String, bool) {
    let norm_target = normalize_dir_for_cmp(dir);
    let mut changed = false;
    let mut kept = Vec::new();
    for raw in existing.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        if normalize_dir_for_cmp(raw) == norm_target {
            changed = true;
        } else {
            kept.push(raw.to_string());
        }
    }
    (kept.join(";"), changed)
}

fn normalize_dir_for_cmp(p: &str) -> String {
    let trimmed = p.trim().trim_matches('"').trim_end_matches(['\\', '/']);
    #[cfg(windows)]
    {
        trimmed.replace('/', "\\").to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        trimmed.to_string()
    }
}

#[cfg(windows)]
pub(crate) mod win32_setup {
    use super::{add_dir_to_path_list, normalize_dir_for_cmp, remove_dir_from_path_list};
    use std::ffi::c_void;
    use std::path::Path;
    use std::ptr::{null, null_mut};

    type Hkey = *mut c_void;

    const HKEY_CURRENT_USER: Hkey = 0x8000_0001usize as Hkey;
    const KEY_READ: u32 = 0x0002_0019;
    const KEY_WRITE: u32 = 0x0002_0006;
    const KEY_ALL_ACCESS: u32 = 0x000F_003F;
    const REG_SZ: u32 = 1;
    const REG_EXPAND_SZ: u32 = 2;
    const ERROR_SUCCESS: i32 = 0;
    const HWND_BROADCAST: *mut c_void = 0xFFFFusize as *mut c_void;
    const WM_SETTINGCHANGE: u32 = 0x001A;
    const SMTO_ABORTIFHUNG: u32 = 0x0002;
    const SHCNE_ASSOCCHANGED: i32 = 0x0800_0000;

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegOpenKeyExW(
            h_key: Hkey,
            sub_key: *const u16,
            ul_options: u32,
            sam_desired: u32,
            phk_result: *mut Hkey,
        ) -> i32;
        fn RegCreateKeyExW(
            h_key: Hkey,
            sub_key: *const u16,
            reserved: u32,
            lp_class: *const u16,
            dw_options: u32,
            sam_desired: u32,
            lp_sec: *const c_void,
            phk_result: *mut Hkey,
            lpdw_disposition: *mut u32,
        ) -> i32;
        fn RegQueryValueExW(
            h_key: Hkey,
            val_name: *const u16,
            reserved: *mut u32,
            lp_type: *mut u32,
            lp_data: *mut u8,
            lpcb_data: *mut u32,
        ) -> i32;
        fn RegSetValueExW(
            h_key: Hkey,
            val_name: *const u16,
            reserved: u32,
            dw_type: u32,
            lp_data: *const u8,
            cb_data: u32,
        ) -> i32;
        fn RegDeleteValueW(h_key: Hkey, val_name: *const u16) -> i32;
        fn RegDeleteTreeW(h_key: Hkey, sub_key: *const u16) -> i32;
        fn RegCloseKey(h_key: Hkey) -> i32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn SendMessageTimeoutW(
            hwnd: *mut c_void,
            msg: u32,
            w_param: usize,
            l_param: isize,
            fu_flags: u32,
            u_timeout: u32,
            lpdw_result: *mut usize,
        ) -> isize;
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHChangeNotify(
            w_event_id: i32,
            u_flags: u32,
            dw_item1: *const c_void,
            dw_item2: *const c_void,
        );
    }

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn reg_read_string(subkey: &str, name: Option<&str>) -> Option<(String, u32)> {
        let sub_w = to_wide(subkey);
        let mut hkey: Hkey = null_mut();
        let rc =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub_w.as_ptr(), 0, KEY_READ, &mut hkey) };
        if rc != ERROR_SUCCESS || hkey.is_null() {
            return None;
        }
        let val_w = name.map(to_wide);
        let val_ptr = val_w.as_ref().map_or(null(), |v| v.as_ptr());
        let mut ty: u32 = 0;
        let mut bytes_len: u32 = 0;
        let q1 = unsafe {
            RegQueryValueExW(
                hkey,
                val_ptr,
                null_mut(),
                &mut ty,
                null_mut(),
                &mut bytes_len,
            )
        };
        if q1 != ERROR_SUCCESS || (ty != REG_SZ && ty != REG_EXPAND_SZ) || bytes_len == 0 {
            unsafe {
                RegCloseKey(hkey);
            }
            return None;
        }
        let mut buf = vec![0u8; bytes_len as usize];
        let q2 = unsafe {
            RegQueryValueExW(
                hkey,
                val_ptr,
                null_mut(),
                &mut ty,
                buf.as_mut_ptr(),
                &mut bytes_len,
            )
        };
        unsafe {
            RegCloseKey(hkey);
        }
        if q2 != ERROR_SUCCESS {
            return None;
        }
        let u16s: Vec<u16> = buf[..(bytes_len as usize).min(buf.len())]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let end = u16s.iter().position(|&c| c == 0).unwrap_or(u16s.len());
        Some((String::from_utf16_lossy(&u16s[..end]), ty))
    }

    fn reg_write_string(
        subkey: &str,
        name: Option<&str>,
        value: &str,
        reg_type: u32,
    ) -> Result<(), String> {
        let sub_w = to_wide(subkey);
        let mut hkey: Hkey = null_mut();
        let mut disp: u32 = 0;
        let rc = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                sub_w.as_ptr(),
                0,
                null(),
                0,
                KEY_WRITE,
                null(),
                &mut hkey,
                &mut disp,
            )
        };
        if rc != ERROR_SUCCESS || hkey.is_null() {
            return Err(format!(
                "Failed to open/create registry key '{}': error {}",
                subkey, rc
            ));
        }
        let val_w = name.map(to_wide);
        let val_ptr = val_w.as_ref().map_or(null(), |v| v.as_ptr());
        let data_w = to_wide(value);
        let byte_len = (data_w.len() * 2) as u32;
        let set_rc = unsafe {
            RegSetValueExW(
                hkey,
                val_ptr,
                0,
                reg_type,
                data_w.as_ptr() as *const u8,
                byte_len,
            )
        };
        unsafe {
            RegCloseKey(hkey);
        }
        if set_rc != ERROR_SUCCESS {
            return Err(format!(
                "Failed to write registry value in '{}': error {}",
                subkey, set_rc
            ));
        }
        Ok(())
    }

    fn reg_delete_value(subkey: &str, name: Option<&str>) {
        let sub_w = to_wide(subkey);
        let mut hkey: Hkey = null_mut();
        if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub_w.as_ptr(), 0, KEY_WRITE, &mut hkey) }
            == ERROR_SUCCESS
        {
            let val_w = name.map(to_wide);
            let val_ptr = val_w.as_ref().map_or(null(), |v| v.as_ptr());
            unsafe {
                RegDeleteValueW(hkey, val_ptr);
                RegCloseKey(hkey);
            }
        }
    }

    fn reg_delete_tree(subkey: &str) {
        let sub_w = to_wide(subkey);
        let mut hkey: Hkey = null_mut();
        if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, null(), 0, KEY_ALL_ACCESS, &mut hkey) }
            == ERROR_SUCCESS
        {
            unsafe {
                RegDeleteTreeW(hkey, sub_w.as_ptr());
                RegCloseKey(hkey);
            }
        }
    }

    fn broadcast_env_change() {
        let env_w = to_wide("Environment");
        let mut res: usize = 0;
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                env_w.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                1500,
                &mut res,
            );
        }
    }

    pub struct SetupStatus {
        pub exe_path: String,
        pub exe_dir: String,
        pub in_path: bool,
        pub assoc_registered: bool,
        pub assoc_mode: String,
        pub assoc_command: Option<String>,
    }

    pub fn current_exe_clean() -> Result<std::path::PathBuf, String> {
        let raw_exe =
            std::env::current_exe().map_err(|e| format!("Failed to locate askane.exe: {}", e))?;
        Ok(std::fs::canonicalize(&raw_exe)
            .map(|p| {
                let s = p.to_string_lossy();
                if let Some(stripped) = s.strip_prefix("\\\\?\\") {
                    std::path::PathBuf::from(stripped)
                } else {
                    p
                }
            })
            .unwrap_or(raw_exe))
    }

    pub fn current_status(exe_path: &Path) -> SetupStatus {
        let exe_str = exe_path.to_string_lossy().to_string();
        let exe_dir = exe_path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());
        let norm_dir = normalize_dir_for_cmp(&exe_dir);
        let in_path = reg_read_string("Environment", Some("Path"))
            .map(|(val, _)| {
                val.split(';').any(|entry| {
                    !entry.trim().is_empty() && normalize_dir_for_cmp(entry) == norm_dir
                })
            })
            .unwrap_or(false);
        let dot_ask = reg_read_string("Software\\Classes\\.ask", None).map(|(v, _)| v);
        let assoc_cmd = reg_read_string(
            "Software\\Classes\\Askane.Script\\shell\\open\\command",
            None,
        )
        .map(|(v, _)| v);
        let assoc_registered = dot_ask.as_deref() == Some("Askane.Script") && assoc_cmd.is_some();
        let assoc_mode = if !assoc_registered {
            "Not registered".to_string()
        } else if assoc_cmd.as_deref().is_some_and(|c| c.contains("--gui")) {
            "Edit in Askane Studio".to_string()
        } else {
            "Run with Askane".to_string()
        };
        SetupStatus {
            exe_path: exe_str,
            exe_dir,
            in_path,
            assoc_registered,
            assoc_mode,
            assoc_command: assoc_cmd,
        }
    }

    pub fn install_path(exe_path: &Path) -> Result<bool, String> {
        let exe_dir = exe_path
            .parent()
            .ok_or_else(|| "Cannot determine executable directory".to_string())?
            .to_string_lossy()
            .to_string();
        let (existing, ty) = reg_read_string("Environment", Some("Path"))
            .unwrap_or_else(|| (String::new(), REG_EXPAND_SZ));
        let (updated, changed) = add_dir_to_path_list(&existing, &exe_dir);
        if changed {
            let target_ty = if updated.contains('%') {
                REG_EXPAND_SZ
            } else {
                ty
            };
            reg_write_string("Environment", Some("Path"), &updated, target_ty)?;
            broadcast_env_change();
        }
        Ok(changed)
    }

    pub fn install_assoc_with_default(exe_path: &Path, studio_default: bool) -> Result<(), String> {
        let exe_str = exe_path.to_string_lossy().to_string();
        let icon_spec = format!("\"{}\",0", exe_str);
        let run_cmd = format!("\"{}\" \"%1\"", exe_str);
        let studio_cmd = format!("\"{}\" --gui \"%1\"", exe_str);

        reg_write_string("Software\\Classes\\.ask", None, "Askane.Script", REG_SZ)?;
        reg_write_string(
            "Software\\Classes\\.ask",
            Some("Content Type"),
            "text/plain",
            REG_SZ,
        )?;
        reg_write_string(
            "Software\\Classes\\.ask",
            Some("PerceivedType"),
            "text",
            REG_SZ,
        )?;
        reg_write_string(
            "Software\\Classes\\.ask\\OpenWithProgids",
            Some("Askane.Script"),
            "",
            REG_SZ,
        )?;

        reg_write_string(
            "Software\\Classes\\Askane.Script",
            None,
            "Askane Script",
            REG_SZ,
        )?;
        reg_write_string(
            "Software\\Classes\\Askane.Script",
            Some("FriendlyTypeName"),
            "Askane Script",
            REG_SZ,
        )?;
        reg_write_string(
            "Software\\Classes\\Askane.Script\\DefaultIcon",
            None,
            &icon_spec,
            REG_SZ,
        )?;
        let (open_lbl, open_cmd, sec_key, sec_lbl, sec_cmd) = if studio_default {
            (
                "Edit in Askane Studio",
                &studio_cmd,
                "Software\\Classes\\Askane.Script\\shell\\run",
                "Run with Askane",
                &run_cmd,
            )
        } else {
            (
                "Run with Askane",
                &run_cmd,
                "Software\\Classes\\Askane.Script\\shell\\edit",
                "Edit in Askane Studio",
                &studio_cmd,
            )
        };
        reg_delete_tree("Software\\Classes\\Askane.Script\\shell");
        reg_write_string(
            "Software\\Classes\\Askane.Script\\shell\\open",
            None,
            open_lbl,
            REG_SZ,
        )?;
        reg_write_string(
            "Software\\Classes\\Askane.Script\\shell\\open\\command",
            None,
            open_cmd,
            REG_SZ,
        )?;
        reg_write_string(sec_key, None, sec_lbl, REG_SZ)?;
        reg_write_string(&format!("{}\\command", sec_key), None, sec_cmd, REG_SZ)?;

        reg_write_string(
            "Software\\Classes\\Applications\\askane.exe",
            Some("FriendlyAppName"),
            "Askane",
            REG_SZ,
        )?;
        reg_write_string(
            "Software\\Classes\\Applications\\askane.exe\\SupportedTypes",
            Some(".ask"),
            "",
            REG_SZ,
        )?;
        reg_write_string(
            "Software\\Classes\\Applications\\askane.exe\\DefaultIcon",
            None,
            &icon_spec,
            REG_SZ,
        )?;
        reg_write_string(
            "Software\\Classes\\Applications\\askane.exe\\shell\\open\\command",
            None,
            open_cmd,
            REG_SZ,
        )?;

        unsafe {
            SHChangeNotify(SHCNE_ASSOCCHANGED, 0, null(), null());
        }
        Ok(())
    }

    pub fn remove_path(exe_path: &Path) -> Result<bool, String> {
        let exe_dir = exe_path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());
        if let Some((existing, ty)) = reg_read_string("Environment", Some("Path")) {
            let (updated, changed) = remove_dir_from_path_list(&existing, &exe_dir);
            if changed {
                reg_write_string("Environment", Some("Path"), &updated, ty)?;
                broadcast_env_change();
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn uninstall_assoc() -> bool {
        let had_assoc = reg_read_string("Software\\Classes\\Askane.Script", None).is_some()
            || reg_read_string("Software\\Classes\\.ask", None).is_some();
        if reg_read_string("Software\\Classes\\.ask", None)
            .is_some_and(|(v, _)| v == "Askane.Script")
        {
            reg_delete_value("Software\\Classes\\.ask", None);
        }
        reg_delete_value(
            "Software\\Classes\\.ask\\OpenWithProgids",
            Some("Askane.Script"),
        );
        reg_delete_tree("Software\\Classes\\Askane.Script");
        reg_delete_tree("Software\\Classes\\Applications\\askane.exe");
        if had_assoc {
            unsafe {
                SHChangeNotify(SHCNE_ASSOCCHANGED, 0, null(), null());
            }
        }
        had_assoc
    }

    pub fn uninstall_all(exe_path: &Path) -> Result<(bool, bool), String> {
        let path_removed = remove_path(exe_path)?;
        let had_assoc = uninstall_assoc();
        Ok((path_removed, had_assoc))
    }
}

#[cfg(windows)]
pub fn run_setup_cli(args: &[String]) -> Result<(), String> {
    let exe_path = win32_setup::current_exe_clean()?;

    let mut do_status = false;
    let mut do_path = false;
    let mut do_assoc = false;
    let mut do_uninstall = false;
    let mut studio_default = false;

    for arg in args {
        match arg.as_str() {
            "--status" => do_status = true,
            "--path" => do_path = true,
            "--assoc" => do_assoc = true,
            "--default=run" => studio_default = false,
            "--default=studio" => studio_default = true,
            "--all" | "--install" => {
                do_path = true;
                do_assoc = true;
            }
            "--uninstall" | "--remove" => do_uninstall = true,
            "--help" | "-h" => {
                println!(
                    "Usage: askane setup [--status | --all | --path | --assoc | --default=run|studio | --uninstall]"
                );
                return Ok(());
            }
            other => return Err(format!("Unknown setup option: '{}'", other)),
        }
    }

    if !do_status && !do_path && !do_assoc && !do_uninstall {
        let st = win32_setup::current_status(&exe_path);
        println!("Askane Windows Integration Setup");
        println!("  Executable : {}", st.exe_path);
        println!(
            "  User PATH  : {}",
            if st.in_path {
                "Installed"
            } else {
                "Not in PATH"
            }
        );
        println!("  .ask Open  : {}", st.assoc_mode);
        println!();
        println!("Choose an option:");
        println!("  1) Add Askane folder to User PATH");
        println!("  2) Register .ask file association (default: Run + Edit in Studio)");
        println!("  3) Register .ask file association (default: Edit in Studio)");
        println!("  4) Install PATH + .ask (Run default) [Recommended]");
        println!("  5) Remove PATH & .ask integration");
        println!("  0) Cancel");
        print!("> ");
        let _ = io::stdout().flush();
        let mut choice = String::new();
        io::stdin()
            .read_line(&mut choice)
            .map_err(|e| format!("Failed to read input: {}", e))?;
        match choice.trim() {
            "1" => do_path = true,
            "2" => do_assoc = true,
            "3" => {
                do_assoc = true;
                studio_default = true;
            }
            "4" => {
                do_path = true;
                do_assoc = true;
            }
            "5" => do_uninstall = true,
            _ => {
                println!("Cancelled.");
                return Ok(());
            }
        }
    }

    if do_status {
        let st = win32_setup::current_status(&exe_path);
        println!("Executable      : {}", st.exe_path);
        println!("Directory       : {}", st.exe_dir);
        println!(
            "User PATH       : {}",
            if st.in_path { "yes" } else { "no" }
        );
        println!(
            ".ask registered : {} ({})",
            if st.assoc_registered { "yes" } else { "no" },
            st.assoc_mode
        );
        if let Some(cmd) = st.assoc_command {
            println!(".ask command    : {}", cmd);
        }
        return Ok(());
    }

    if do_uninstall {
        let (p_rem, a_rem) = win32_setup::uninstall_all(&exe_path)?;
        println!(
            "Removed PATH entry: {} | Removed .ask association: {}",
            p_rem, a_rem
        );
        return Ok(());
    }

    if do_path {
        let added = win32_setup::install_path(&exe_path)?;
        println!(
            "{}",
            if added {
                "Added Askane directory to User PATH (restart new terminals to pick it up)."
            } else {
                "Askane directory is already in User PATH."
            }
        );
    }
    if do_assoc {
        win32_setup::install_assoc_with_default(&exe_path, studio_default)?;
        println!(
            "Registered .ask file association (default: {}).",
            if studio_default {
                "Edit in Askane Studio"
            } else {
                "Run with Askane"
            }
        );
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn run_setup_cli(_args: &[String]) -> Result<(), String> {
    Err(
        "askane setup (Windows PATH & .ask file association) is only supported on Windows"
            .to_string(),
    )
}

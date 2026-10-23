//! "Start with Windows": a value under the current user's Run key, pointing at
//! this exe with `--minimized` so it starts in the tray.

#[cfg(windows)]
mod imp {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const NAME: &str = "freezetime";

    fn command() -> Option<String> {
        let exe = std::env::current_exe().ok()?;
        Some(format!("\"{}\" --minimized", exe.display()))
    }

    pub fn enabled() -> bool {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(RUN, KEY_READ)
            .and_then(|k| k.get_value::<String, _>(NAME))
            .is_ok_and(|v| Some(v) == command())
    }

    pub fn set(on: bool) -> Result<(), String> {
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(RUN, KEY_READ | KEY_WRITE)
            .map_err(|e| format!("couldn't change startup apps: {e}"))?;
        if on {
            let cmd = command().ok_or("couldn't find freezetime's own location")?;
            key.set_value(NAME, &cmd).map_err(|e| format!("couldn't add freezetime to startup apps: {e}"))
        } else {
            match key.delete_value(NAME) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("couldn't remove freezetime from startup apps: {e}")),
                _ => Ok(()),
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn enabled() -> bool {
        false
    }

    pub fn set(_on: bool) -> Result<(), String> {
        Err("starting with the system is only available on Windows.".into())
    }
}

pub use imp::{enabled, set};

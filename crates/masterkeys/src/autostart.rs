//! "Start with Windows": a value under the user's Run key that launches MasterKeys in the tray.

use std::path::Path;

/// The Run value's command line: the exe, quoted, starting in the tray.
pub fn run_command(exe: &Path) -> String {
    format!("\"{}\" --tray", exe.display())
}

#[cfg(windows)]
mod registry {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };
    use windows_sys::w;

    const RUN: windows_sys::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    const VALUE: windows_sys::core::PCWSTR = w!("MasterKeys");

    /// Whether MasterKeys starts with Windows.
    pub fn is_enabled() -> bool {
        let mut size = 0u32;
        // SAFETY: size query only; no buffer is written.
        let status = unsafe {
            RegGetValueW(HKEY_CURRENT_USER, RUN, VALUE, RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut size)
        };
        status == ERROR_SUCCESS
    }

    /// Start (or stop starting) MasterKeys with Windows, from where this exe is now.
    pub fn set_enabled(on: bool) -> Result<(), String> {
        // SAFETY: the data is a NUL-terminated UTF-16 string whose byte length is passed with it.
        let status = unsafe {
            if on {
                let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
                let data: Vec<u16> = super::run_command(&exe).encode_utf16().chain([0]).collect();
                RegSetKeyValueW(HKEY_CURRENT_USER, RUN, VALUE, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32)
            } else {
                match RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN, VALUE) {
                    2 => ERROR_SUCCESS, // ERROR_FILE_NOT_FOUND: already off
                    s => s,
                }
            }
        };
        if status == ERROR_SUCCESS { Ok(()) } else { Err(format!("cannot change Start with Windows (error {status})")) }
    }
}
#[cfg(windows)]
pub use registry::{is_enabled, set_enabled};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_command_quotes_the_path() {
        assert_eq!(
            run_command(Path::new(r"C:\Users\Jo Smith\masterkeys.exe")),
            r#""C:\Users\Jo Smith\masterkeys.exe" --tray"#
        );
    }
}

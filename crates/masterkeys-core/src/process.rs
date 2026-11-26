//! Running programs, for switching profiles when a linked game starts.

/// One of the user's running processes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    /// Start time in clock ticks since boot (`/proc/<pid>/stat` field 22); orders starts and tells a
    /// reused pid apart.
    pub start: u64,
    /// Target of `/proc/<pid>/exe`.
    pub exe: Option<String>,
    pub cmdline: Vec<String>,
}

/// Last path component, splitting on both `/` and `\` (Wine paths).
fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Whether `p` is the program `link` names: its executable's file name, or (for Proton/Wine games, whose
/// executable is the Wine loader) the file name of any command-line argument. Case-insensitive.
pub fn matches(p: &ProcessInfo, link: &str) -> bool {
    let link = link.trim();
    if link.is_empty() {
        return false;
    }
    let is = |path: &str| basename(path).eq_ignore_ascii_case(link);
    p.exe.as_deref().is_some_and(is) || p.cmdline.iter().any(|arg| is(arg))
}

/// The name to offer for linking: a Windows `.exe` from the command line if there is one, else the
/// executable's file name.
pub fn display_name(p: &ProcessInfo) -> Option<String> {
    p.cmdline
        .iter()
        .map(|arg| basename(arg))
        .find(|name| name.to_ascii_lowercase().ends_with(".exe"))
        .or_else(|| p.exe.as_deref().map(basename))
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// The processes this user can read; others are skipped.
#[cfg(windows)]
pub fn running() -> Vec<ProcessInfo> {
    windows::running()
}

/// The processes this user can read; others are skipped.
#[cfg(not(windows))]
pub fn running() -> Vec<ProcessInfo> {
    let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
    entries.flatten().filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok()).filter_map(read).collect()
}

#[cfg(not(windows))]
fn read(pid: u32) -> Option<ProcessInfo> {
    let base = format!("/proc/{pid}");
    let stat = std::fs::read_to_string(format!("{base}/stat")).ok()?;
    // Fields after the command name, which is in parentheses and may contain spaces or ')'.
    let rest = &stat[stat.rfind(')')? + 2..];
    // `rest` starts at field 3, so field 22 is index 19.
    let start = rest.split_whitespace().nth(19)?.parse().ok()?;
    let exe = std::fs::read_link(format!("{base}/exe")).ok().map(|p| p.to_string_lossy().into_owned());
    let cmdline = std::fs::read(format!("{base}/cmdline"))
        .ok()?
        .split(|&b| b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    Some(ProcessInfo { pid, start, exe, cmdline })
}

#[cfg(windows)]
mod windows {
    use super::ProcessInfo;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };

    pub fn running() -> Vec<ProcessInfo> {
        let mut out = Vec::new();
        // SAFETY: Toolhelp snapshot walk with a correctly sized entry; the handle is closed after.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return out;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snapshot, &mut entry) != 0;
            while more {
                if let Some(p) = read(entry.th32ProcessID) {
                    out.push(p);
                }
                more = Process32NextW(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
        }
        out
    }

    /// Exe path and start time, if this user may query the process.
    fn read(pid: u32) -> Option<ProcessInfo> {
        // SAFETY: the handle is checked and closed; buffers are sized for the calls.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let exe = (QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0)
                .then(|| String::from_utf16_lossy(&buf[..len as usize]));
            let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
            let start = if GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) != 0 {
                (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime)
            } else {
                0
            };
            CloseHandle(handle);
            Some(ProcessInfo { pid, start, exe, cmdline: Vec::new() })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(exe: &str, cmd: &[&str]) -> ProcessInfo {
        ProcessInfo { pid: 1, start: 0, exe: Some(exe.into()), cmdline: cmd.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn native_exe_matches_case_insensitively() {
        assert!(matches(&p("/usr/bin/factorio", &["factorio"]), "Factorio"));
    }

    #[test]
    fn proton_game_matches_by_cmdline() {
        let g = p("/home/u/.steam/proton/bin/wine64-preloader", &["Z:\\games\\ELDEN RING\\Game\\eldenring.exe"]);
        assert!(matches(&g, "eldenring.exe"));
        assert_eq!(display_name(&g).as_deref(), Some("eldenring.exe"));
    }

    #[test]
    fn partial_names_do_not_match() {
        assert!(!matches(&p("/usr/bin/factorio-server", &[]), "factorio"));
        assert!(!matches(&p("/usr/bin/factorio", &[]), ""));
    }

    #[test]
    fn display_name_falls_back_to_exe() {
        assert_eq!(display_name(&p("/usr/bin/factorio", &["factorio", "--x"])).as_deref(), Some("factorio"));
    }

    #[test]
    fn running_includes_this_test_process() {
        let me = std::process::id();
        assert!(running().iter().any(|p| p.pid == me && p.start > 0));
    }
}

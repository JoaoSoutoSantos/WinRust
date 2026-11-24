//! Process helpers on plain Win32, so nobody has to start PowerShell (30-60 MB and 0.3 s) just to list or end a process.
use std::{ffi::c_void, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS},
        Threading::{OpenProcess, TerminateProcess},
    },
};

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(h: *mut c_void, class: u32, out: *mut c_void, len: u32, ret: *mut u32) -> i32;
}

/// `(pid, exe name)` of every running process.
pub fn list() -> Vec<(u32, String)> {
    let mut v = vec![];
    unsafe {
        let s = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if s == INVALID_HANDLE_VALUE {
            return v;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(s, &mut e) != 0;
        while ok {
            let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            v.push((e.th32ProcessID, String::from_utf16_lossy(&e.szExeFile[..n])));
            ok = Process32NextW(s, &mut e) != 0;
        }
        CloseHandle(s);
    }
    v
}

/// Pids of the processes with this exe name (case-insensitive, e.g. `explorer.exe`).
pub fn pids(name: &str) -> Vec<u32> {
    list().into_iter().filter(|(_, n)| n.eq_ignore_ascii_case(name)).map(|p| p.0).collect()
}

/// The command line of a process via NtQueryInformationProcess(ProcessCommandLineInformation, 60).
/// Uses PROCESS_QUERY_LIMITED_INFORMATION (0x1000) and avoids hardcoded PEB offsets.
pub fn cmdline(pid: u32) -> Option<String> {
    #[repr(C)]
    struct UnicodeString {
        len: u16,
        max: u16,
        buf: *const u16,
    }
    const PROCESS_COMMAND_LINE_INFO: u32 = 60;
    unsafe {
        let h = OpenProcess(0x1000, 0, pid); // PROCESS_QUERY_LIMITED_INFORMATION
        if h.is_null() {
            return None;
        }
        let r = (|| {
            let mut need = 0u32;
            let _ = NtQueryInformationProcess(h, PROCESS_COMMAND_LINE_INFO, null_mut(), 0, &mut need);
            if (need as usize) < size_of::<UnicodeString>() {
                return None;
            }
            let words = (need as usize).div_ceil(size_of::<usize>());
            let mut buf = vec![0usize; words];
            if NtQueryInformationProcess(h, PROCESS_COMMAND_LINE_INFO, buf.as_mut_ptr().cast(), (words * size_of::<usize>()) as u32, &mut need) != 0 {
                return None;
            }
            let us = &*(buf.as_ptr() as *const UnicodeString);
            if us.buf.is_null() || us.len == 0 {
                return Some(String::new());
            }
            let slice = std::slice::from_raw_parts(us.buf, (us.len / 2) as usize);
            Some(String::from_utf16_lossy(slice))
        })();
        CloseHandle(h);
        r
    }
}

pub fn kill(pid: u32) {
    unsafe {
        let h = OpenProcess(1, 0, pid); // PROCESS_TERMINATE
        if !h.is_null() {
            TerminateProcess(h, 1);
            CloseHandle(h);
        }
    }
}

/// An explorer.exe started with no path: the one that runs as the shell. Folder windows (started with a path) and the COM host
/// (`/factory ... -Embedding`) are other explorer.exe processes and must stay.
fn is_shell_cmdline(c: &str) -> bool {
    let mut s = c.trim_end().to_ascii_lowercase();
    if let Some(r) = s.strip_suffix("/nouaccheck") {
        if !r.ends_with(char::is_whitespace) {
            return false;
        }
        s = r.trim_end().to_string();
    }
    s.strip_suffix('"').unwrap_or(&s).ends_with("explorer.exe")
}

/// Pids of the explorer.exe processes that are a shell.
pub fn shell_explorers() -> Vec<u32> {
    pids("explorer.exe").into_iter().filter(|&p| cmdline(p).is_some_and(|c| is_shell_cmdline(&c))).collect()
}

pub fn kill_shell_explorers() {
    shell_explorers().into_iter().for_each(kill);
}

#[cfg(test)]
mod tests {
    use super::is_shell_cmdline;

    #[test]
    fn only_a_bare_explorer_is_the_shell() {
        assert!(is_shell_cmdline(r"C:\Windows\explorer.exe"));
        assert!(is_shell_cmdline(r#""C:\Windows\explorer.exe""#));
        assert!(is_shell_cmdline(r#""C:\Windows\EXPLORER.EXE" /NoUACCheck "#));
        assert!(!is_shell_cmdline(r"C:\Windows\explorer.exe C:\Users"));
        assert!(!is_shell_cmdline(r"C:\Windows\explorer.exe /factory,{5BD95610-9434-43C2-886C-57852CC8A120} -Embedding"));
        assert!(!is_shell_cmdline(r"C:\Windows\explorer.exe ms-settings:"));
    }

    #[test]
    fn lists_own_process() {
        let me = std::process::id();
        assert!(super::list().iter().any(|(p, _)| *p == me));
        assert!(super::cmdline(me).is_some_and(|c| !c.is_empty()));
    }
}

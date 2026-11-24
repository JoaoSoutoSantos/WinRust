//! rs-notepad and rs-calc: replace notepad.exe and calc.exe via IFEO Debugger (system files stay untouched).
use crate::*;

const NP_KEY: &str = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\notepad.exe";
const NP_STATE: &str = r"HKLM\SOFTWARE\plugin-launcher\rs-notepad";
const NP_EXTS: [&str; 7] = ["txt", "log", "ini", "md", "csv", "json", "xml"];
const NP_PROG: &str = "WinRustNotepad";
const NP_APP: &str = "WinRust Notepad";

const CALC_KEY: &str = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\calc.exe";
const CALC_STATE: &str = r"HKLM\SOFTWARE\plugin-launcher\rs-calc";

pub(crate) fn np_dir() -> PathBuf {
    program_files().join("rs-notepad")
}

pub(crate) fn calc_dir() -> PathBuf {
    program_files().join("rs-calc")
}

/// Ours only: a Debugger set by another program (e.g. Notepad2/Notepad3) is not this plugin.
pub(crate) fn np_installed() -> bool {
    reg_str(NP_KEY, "Debugger").is_some_and(|d| d.to_lowercase().contains("rs-notepad.exe"))
}

pub(crate) fn calc_installed() -> bool {
    reg_str(CALC_KEY, "Debugger").is_some_and(|d| d.to_lowercase().contains("rs-calc.exe"))
}

fn ifeo_install(exe: &str, dir: &Path, lnk: &str, key: &str, state: &str, ours: fn() -> bool) -> Result<(), String> {
    if let Some(old) = reg_str(key, "Debugger").filter(|_| !ours() && reg_str(state, "Debugger").is_none()) {
        run("reg", &["add", state, "/v", "Debugger", "/t", "REG_SZ", "/d", &old, "/f"])?;
    }
    tool_install(exe, dir, Some(lnk))?;
    let dst = dir.join(exe);
    run("reg", &["add", key, "/v", "Debugger", "/t", "REG_SZ", "/d", &format!("\"{}\"", dst.display()), "/f"])
}

fn ifeo_uninstall(exe: &str, dir: &Path, lnk: &str, key: &str, state: &str, ours: fn() -> bool) -> Result<(), String> {
    // Registry first: with the exe gone, the stock binary would not start at all.
    match reg_str(state, "Debugger") {
        Some(old) => run("reg", &["add", key, "/v", "Debugger", "/t", "REG_SZ", "/d", &old, "/f"])?,
        None if ours() => run("reg", &["delete", key, "/v", "Debugger", "/f"])?,
        None => {}
    }
    let _ = run("reg", &["delete", state, "/f"]);
    tool_remove(exe, dir, Some(lnk));
    Ok(())
}

pub(crate) fn np_install() -> Result<(), String> {
    ifeo_install("rs-notepad.exe", &np_dir(), "WinRust Notepad", NP_KEY, NP_STATE, np_installed)?;
    let exe = np_dir().join("rs-notepad.exe").display().to_string().replace('\\', r"\\");
    let mut reg = format!(
        "Windows Registry Editor Version 5.00\r\n\r\n[HKEY_LOCAL_MACHINE\\SOFTWARE\\Classes\\{NP_PROG}]\r\n@=\"Text Document\"\r\n\r\n[HKEY_LOCAL_MACHINE\\SOFTWARE\\Classes\\{NP_PROG}\\shell\\open\\command]\r\n@=\"\\\"{exe}\\\" \\\"%1\\\"\"\r\n\r\n\
         [HKEY_LOCAL_MACHINE\\SOFTWARE\\WinRust\\Notepad\\Capabilities]\r\n\"ApplicationName\"=\"{NP_APP}\"\r\n\"ApplicationDescription\"=\"Fast text editor\"\r\n\r\n[HKEY_LOCAL_MACHINE\\SOFTWARE\\WinRust\\Notepad\\Capabilities\\FileAssociations]\r\n"
    );
    for e in NP_EXTS {
        reg += &format!("\".{e}\"=\"{NP_PROG}\"\r\n");
    }
    reg += &format!("\r\n[HKEY_LOCAL_MACHINE\\SOFTWARE\\RegisteredApplications]\r\n\"{NP_APP}\"=\"SOFTWARE\\\\WinRust\\\\Notepad\\\\Capabilities\"\r\n");
    for e in NP_EXTS {
        reg += &format!("\r\n[HKEY_LOCAL_MACHINE\\SOFTWARE\\Classes\\.{e}\\OpenWithProgids]\r\n\"{NP_PROG}\"=hex(0):\r\n");
    }
    let f = np_dir().join("register.reg");
    fs::write(&f, reg).map_err(|e| format!("{}: {e}", f.display()))?;
    let r = run("reg", &["import", &f.display().to_string()]);
    let _ = fs::remove_file(&f);
    r
}

pub(crate) fn np_uninstall() -> Result<(), String> {
    for e in NP_EXTS {
        let _ = run("reg", &["delete", &format!(r"HKLM\SOFTWARE\Classes\.{e}\OpenWithProgids"), "/v", NP_PROG, "/f"]);
    }
    let _ = run("reg", &["delete", r"HKLM\SOFTWARE\RegisteredApplications", "/v", NP_APP, "/f"]);
    let _ = run("reg", &["delete", &format!(r"HKLM\SOFTWARE\Classes\{NP_PROG}"), "/f"]);
    let _ = run("reg", &["delete", r"HKLM\SOFTWARE\WinRust\Notepad", "/f"]);
    ifeo_uninstall("rs-notepad.exe", &np_dir(), "WinRust Notepad", NP_KEY, NP_STATE, np_installed)
}

pub(crate) fn calc_install() -> Result<(), String> {
    ifeo_install("rs-calc.exe", &calc_dir(), "WinRust Calculator", CALC_KEY, CALC_STATE, calc_installed)
}

pub(crate) fn calc_uninstall() -> Result<(), String> {
    ifeo_uninstall("rs-calc.exe", &calc_dir(), "WinRust Calculator", CALC_KEY, CALC_STATE, calc_installed)
}

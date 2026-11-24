//! WinRust's own Calculator and Notepad in place of the Store ones.
//!
//! A Store (UWP) app needs Explorer as the shell: with rs-shell replacing Explorer, the Store Calculator opens as a
//! black frame that never draws and takes the focus (the screen looks frozen). calc.exe / notepad.exe are redirected by
//! the installer (IFEO), but the Start tile / pin activates the app id directly, and `ms-calculator:` is a protocol:
//! the bar and the launcher map those here before starting anything.
use std::path::PathBuf;

/// (program files folder, exe, what it replaces: app id prefixes / bare commands / protocols, all lower-case)
/// Task Manager is here too: the IFEO redirect does not catch every way of starting taskmgr.exe (explorer's COM host
/// hands the auto-elevated exe to AppInfo, which opened the Windows one), so the bar starts ours by path.
const OWN: [(&str, &str, &[&str]); 3] = [
    ("rs-calc", "rs-calc.exe", &["microsoft.windowscalculator_8wekyb3d8bbwe!", "calc", "calc.exe", "calculatorapp.exe", "ms-calculator:"]),
    ("rs-notepad", "rs-notepad.exe", &["microsoft.windowsnotepad_8wekyb3d8bbwe!", "notepad", "notepad.exe"]),
    ("rtaskmgr", "rtaskmgr-win32.exe", &["taskmgr", "taskmgr.exe"]),
];

/// Which own app replaces `target` (an app id, `shell:AppsFolder\<id>`, a bare command, a path or a protocol URI),
/// as (folder, exe). None for anything else.
pub fn replacement(target: &str) -> Option<(&'static str, &'static str)> {
    let t = target.trim().trim_matches('"').to_lowercase();
    let t = t.strip_prefix(r"shell:appsfolder\").unwrap_or(&t);
    let name = t.rsplit(['\\', '/']).next().unwrap_or(t);
    OWN.iter().find(|(_, _, keys)| {
        keys.iter().any(|k| if k.ends_with('!') || k.ends_with(':') { t.starts_with(k) } else { name == *k })
    }).map(|(dir, exe, _)| (*dir, *exe))
}

/// The installed own app for `target` (see `replacement`), None when it does not apply or that app is not installed.
pub fn own_app(target: &str) -> Option<PathBuf> {
    let (dir, exe) = replacement(target)?;
    let p = PathBuf::from(std::env::var_os("ProgramFiles")?).join(dir).join(exe);
    p.is_file().then_some(p)
}

/// The calculator to start for the keyboard's Calculator key: ours when installed, else calc.exe.
pub fn calculator() -> PathBuf {
    own_app("calc").unwrap_or_else(|| PathBuf::from("calc.exe"))
}

#[cfg(test)]
mod tests {
    use super::replacement;

    #[test]
    fn maps_store_ids_commands_and_protocols() {
        assert_eq!(replacement("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"), Some(("rs-calc", "rs-calc.exe")));
        assert_eq!(replacement(r"shell:AppsFolder\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"), Some(("rs-calc", "rs-calc.exe")));
        assert_eq!(replacement("calc"), Some(("rs-calc", "rs-calc.exe")));
        assert_eq!(replacement(r"C:\Windows\System32\calc.exe"), Some(("rs-calc", "rs-calc.exe")));
        assert_eq!(replacement("ms-calculator:"), Some(("rs-calc", "rs-calc.exe")));
        assert_eq!(replacement("Microsoft.WindowsNotepad_8wekyb3d8bbwe!App"), Some(("rs-notepad", "rs-notepad.exe")));
        assert_eq!(replacement(r"C:\Windows\notepad.exe"), Some(("rs-notepad", "rs-notepad.exe")));
        assert_eq!(replacement(r"C:\Windows\System32\Taskmgr.exe"), Some(("rtaskmgr", "rtaskmgr-win32.exe")));
        assert_eq!(replacement("taskmgr"), Some(("rtaskmgr", "rtaskmgr-win32.exe")));
    }

    #[test]
    fn leaves_everything_else() {
        assert_eq!(replacement(r"C:\tools\calculator.exe"), None);
        assert_eq!(replacement(r"C:\notes\calc.txt"), None);
        assert_eq!(replacement("Microsoft.WindowsTerminal_8wekyb3d8bbwe!App"), None);
        assert_eq!(replacement("ms-settings:"), None);
        assert_eq!(replacement(""), None);
    }
}

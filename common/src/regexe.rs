//! The reg.exe commands the installer uses (`query`, `add`, `delete`, `export`, `import` with `/v`, `/ve`, `/t`, `/d`, `/f`,
//! `/y`), done in-process with the registry API instead of starting reg.exe.
//!
//! Why: an unsigned program that starts reg.exe to set an IFEO "Debugger" or the shell's folder verbs is exactly what
//! antivirus behaviour engines look for. Kaspersky blocked the child process ("Access is denied") and rolled back the
//! taskmgr.exe IFEO value on this machine. The output keeps reg.exe's text shape, so `reg::value` / `reg::dword` read it unchanged.
//!
//! Always the 64-bit view of the registry (what 64-bit reg.exe uses). `.reg` files are written as UTF-16 with a BOM, like
//! reg.exe, and read in UTF-16 or UTF-8.
use std::ptr::{null, null_mut};
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, WIN32_ERROR},
    Storage::FileSystem::DELETE,
    System::Registry::*,
};

const NOT_FOUND: &str = "ERROR: The system was unable to find the specified registry key or value.";

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn err_text(code: WIN32_ERROR) -> String {
    if code == ERROR_FILE_NOT_FOUND {
        return NOT_FOUND.into();
    }
    format!("ERROR: {}", std::io::Error::from_raw_os_error(code as i32))
}

/// `HKLM\SOFTWARE\X` -> (root handle, "SOFTWARE\X", the root's long name).
fn split_key(key: &str) -> Result<(HKEY, String, &'static str), String> {
    let (root, rest) = key.split_once('\\').unwrap_or((key, ""));
    let (h, long) = match root.to_ascii_uppercase().as_str() {
        "HKLM" | "HKEY_LOCAL_MACHINE" => (HKEY_LOCAL_MACHINE, "HKEY_LOCAL_MACHINE"),
        "HKCU" | "HKEY_CURRENT_USER" => (HKEY_CURRENT_USER, "HKEY_CURRENT_USER"),
        "HKU" | "HKEY_USERS" => (HKEY_USERS, "HKEY_USERS"),
        "HKCR" | "HKEY_CLASSES_ROOT" => (HKEY_CLASSES_ROOT, "HKEY_CLASSES_ROOT"),
        "HKCC" | "HKEY_CURRENT_CONFIG" => (HKEY_CURRENT_CONFIG, "HKEY_CURRENT_CONFIG"),
        _ => return Err(format!("ERROR: Invalid key name: {key}")),
    };
    Ok((h, rest.trim_end_matches('\\').to_string(), long))
}

fn full_name(long: &str, sub: &str) -> String {
    if sub.is_empty() { long.to_string() } else { format!("{long}\\{sub}") }
}

/// An open key, closed on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

fn open(root: HKEY, sub: &str, access: REG_SAM_FLAGS) -> Result<Key, WIN32_ERROR> {
    let mut h: HKEY = null_mut();
    let r = unsafe { RegOpenKeyExW(root, w(sub).as_ptr(), 0, access | KEY_WOW64_64KEY, &mut h) };
    if r == ERROR_SUCCESS { Ok(Key(h)) } else { Err(r) }
}

fn create(root: HKEY, sub: &str) -> Result<Key, WIN32_ERROR> {
    let mut h: HKEY = null_mut();
    let r = unsafe { RegCreateKeyExW(root, w(sub).as_ptr(), 0, null(), REG_OPTION_NON_VOLATILE, KEY_ALL_ACCESS | KEY_WOW64_64KEY, null(), &mut h, null_mut()) };
    if r == ERROR_SUCCESS { Ok(Key(h)) } else { Err(r) }
}

/// (type, raw bytes) of value `name` ("" = the default value).
fn get(k: &Key, name: &str) -> Result<(u32, Vec<u8>), WIN32_ERROR> {
    let n = w(name);
    let mut ty = 0u32;
    let mut len = 0u32;
    loop {
        let mut buf = vec![0u8; len as usize];
        let r = unsafe { RegQueryValueExW(k.0, n.as_ptr(), null(), &mut ty, if len == 0 { null_mut() } else { buf.as_mut_ptr() }, &mut len) };
        if r == ERROR_SUCCESS && (len as usize) <= buf.len() {
            buf.truncate(len as usize);
            return Ok((ty, buf));
        }
        if r != ERROR_SUCCESS && r != ERROR_MORE_DATA {
            return Err(r);
        }
    }
}

fn set(k: &Key, name: &str, ty: u32, data: &[u8]) -> Result<(), WIN32_ERROR> {
    let r = unsafe { RegSetValueExW(k.0, w(name).as_ptr(), 0, ty, data.as_ptr(), data.len() as u32) };
    if r == ERROR_SUCCESS { Ok(()) } else { Err(r) }
}

/// Every value of the key: (name, type, bytes).
fn values(k: &Key) -> Vec<(String, u32, Vec<u8>)> {
    let mut out = vec![];
    for i in 0.. {
        let mut name = vec![0u16; 16384];
        let mut nlen = name.len() as u32;
        let r = unsafe { RegEnumValueW(k.0, i, name.as_mut_ptr(), &mut nlen, null(), null_mut(), null_mut(), null_mut()) };
        if r == ERROR_NO_MORE_ITEMS {
            break;
        }
        if r != ERROR_SUCCESS {
            continue;
        }
        let n = String::from_utf16_lossy(&name[..nlen as usize]);
        if let Ok((ty, data)) = get(k, &n) {
            out.push((n, ty, data));
        }
    }
    out
}

fn subkeys(k: &Key) -> Vec<String> {
    let mut out = vec![];
    for i in 0.. {
        let mut name = vec![0u16; 512];
        let mut nlen = name.len() as u32;
        let r = unsafe { RegEnumKeyExW(k.0, i, name.as_mut_ptr(), &mut nlen, null(), null_mut(), null_mut(), null_mut()) };
        if r != ERROR_SUCCESS {
            break;
        }
        out.push(String::from_utf16_lossy(&name[..nlen as usize]));
    }
    out
}

// ---------- text forms ----------

fn type_name(ty: u32) -> String {
    match ty {
        REG_SZ => "REG_SZ".into(),
        REG_EXPAND_SZ => "REG_EXPAND_SZ".into(),
        REG_BINARY => "REG_BINARY".into(),
        REG_DWORD => "REG_DWORD".into(),
        REG_MULTI_SZ => "REG_MULTI_SZ".into(),
        REG_QWORD => "REG_QWORD".into(),
        REG_NONE => "REG_NONE".into(),
        t => format!("REG_UNKNOWN_{t}"),
    }
}

fn type_of(name: &str) -> Option<u32> {
    Some(match name.to_ascii_uppercase().as_str() {
        "REG_SZ" => REG_SZ,
        "REG_EXPAND_SZ" => REG_EXPAND_SZ,
        "REG_BINARY" => REG_BINARY,
        "REG_DWORD" => REG_DWORD,
        "REG_MULTI_SZ" => REG_MULTI_SZ,
        "REG_QWORD" => REG_QWORD,
        "REG_NONE" => REG_NONE,
        _ => return None,
    })
}

fn utf16(data: &[u8]) -> Vec<u16> {
    data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
}

fn sz(data: &[u8]) -> String {
    let v = utf16(data);
    let end = v.iter().position(|&c| c == 0).unwrap_or(v.len());
    String::from_utf16_lossy(&v[..end])
}

/// The data column of `reg query`: text, 0x-hex numbers, hex bytes, `\0`-joined strings.
pub(crate) fn show(ty: u32, data: &[u8]) -> String {
    match ty {
        REG_SZ | REG_EXPAND_SZ => sz(data),
        REG_DWORD if data.len() >= 4 => format!("0x{:x}", u32::from_le_bytes(data[..4].try_into().unwrap())),
        REG_QWORD if data.len() >= 8 => format!("0x{:x}", u64::from_le_bytes(data[..8].try_into().unwrap())),
        REG_MULTI_SZ => String::from_utf16_lossy(&utf16(data)).trim_end_matches('\0').split('\0').collect::<Vec<_>>().join("\\0"),
        _ => data.iter().map(|b| format!("{b:02X}")).collect(),
    }
}

fn wide_bytes(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(|c| c.to_le_bytes()).collect()
}

/// `reg add /t <ty> /d <data>` -> the bytes to store.
pub(crate) fn parse_data(ty: u32, d: &str) -> Result<Vec<u8>, String> {
    let num = |d: &str| -> Option<u64> {
        let d = d.trim();
        match d.strip_prefix("0x").or_else(|| d.strip_prefix("0X")) {
            Some(h) => u64::from_str_radix(h, 16).ok(),
            None => d.parse().ok(),
        }
    };
    Ok(match ty {
        REG_SZ | REG_EXPAND_SZ => wide_bytes(d),
        REG_DWORD => (num(d).filter(|&v| v <= u32::MAX as u64).ok_or(format!("ERROR: Invalid DWORD: {d}"))? as u32).to_le_bytes().to_vec(),
        REG_QWORD => num(d).ok_or(format!("ERROR: Invalid QWORD: {d}"))?.to_le_bytes().to_vec(),
        REG_MULTI_SZ => {
            let mut v: Vec<u8> = d.split("\\0").filter(|s| !s.is_empty()).flat_map(wide_bytes).collect();
            v.extend([0, 0]);
            v
        }
        _ => {
            let h: String = d.chars().filter(|c| !c.is_whitespace()).collect();
            if h.len() % 2 != 0 {
                return Err(format!("ERROR: Invalid binary data: {d}"));
            }
            (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).map_err(|_| format!("ERROR: Invalid binary data: {d}"))).collect::<Result<_, _>>()?
        }
    })
}

fn query_line(name: &str, ty: u32, data: &[u8]) -> String {
    format!("    {}    {}    {}\r\n", if name.is_empty() { "(Default)" } else { name }, type_name(ty), show(ty, data))
}

// ---------- .reg files ----------

fn reg_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// One value as a `.reg` line (without the line end).
pub(crate) fn reg_line(name: &str, ty: u32, data: &[u8]) -> String {
    let lhs = if name.is_empty() { "@".to_string() } else { format!("\"{}\"", reg_escape(name)) };
    let hex = |d: &[u8]| d.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(",");
    let rhs = match ty {
        REG_SZ if data.len() % 2 == 0 && utf16(data).iter().position(|&c| c == 0).is_none_or(|p| p + 1 >= data.len() / 2) => format!("\"{}\"", reg_escape(&sz(data))),
        REG_DWORD if data.len() == 4 => format!("dword:{:08x}", u32::from_le_bytes(data.try_into().unwrap())),
        REG_BINARY => format!("hex:{}", hex(data)),
        t => format!("hex({t:x}):{}", hex(data)),
    };
    format!("{lhs}={rhs}")
}

/// What a `.reg` file says to do, in order.
#[derive(Debug, PartialEq)]
pub(crate) enum RegOp {
    Key(String),                          // [key]: create, and the values below go into it
    DeleteKey(String),                    // [-key]
    Set(String, u32, Vec<u8>),            // "name"=... (name "" = @)
    DeleteValue(String),                  // "name"=-
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            if let Some(n) = it.next() { out.push(n) }
        } else {
            out.push(c);
        }
    }
    out
}

/// The closing quote of a quoted string starting at `s[0] == '"'`, skipping `\"`.
fn close_quote(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

pub(crate) fn parse_reg_file(text: &str) -> Result<Vec<RegOp>, String> {
    // join continuation lines (a trailing backslash on hex data)
    let mut lines: Vec<String> = vec![];
    let mut cur = String::new();
    for l in text.lines() {
        let l = l.trim_end_matches('\r');
        if cur.is_empty() && (l.trim_start().starts_with(';') || l.trim().is_empty()) {
            continue;
        }
        let t = if cur.is_empty() { l.trim_end() } else { l.trim() };
        if let Some(head) = t.strip_suffix('\\').filter(|_| !cur.is_empty() || (!t.starts_with('[') && t.contains("=hex"))) {
            cur.push_str(head);
        } else {
            cur.push_str(t);
            lines.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    let mut ops = vec![];
    let mut it = lines.into_iter();
    match it.next() {
        Some(h) if h.starts_with("Windows Registry Editor Version") || h == "REGEDIT4" => {}
        _ => return Err("ERROR: not a registry file".into()),
    }
    for l in it {
        if let Some(k) = l.strip_prefix("[-").and_then(|r| r.strip_suffix(']')) {
            ops.push(RegOp::DeleteKey(k.to_string()));
            continue;
        }
        if let Some(k) = l.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            ops.push(RegOp::Key(k.to_string()));
            continue;
        }
        let (name, rhs) = if let Some(r) = l.strip_prefix("@=") {
            (String::new(), r.to_string())
        } else if l.starts_with('"') {
            let q = close_quote(&l).ok_or(format!("ERROR: bad line: {l}"))?;
            let rest = l[q + 1..].trim_start().strip_prefix('=').ok_or(format!("ERROR: bad line: {l}"))?;
            (unescape(&l[1..q]), rest.trim().to_string())
        } else {
            return Err(format!("ERROR: bad line: {l}"));
        };
        let bytes = |h: &str| -> Result<Vec<u8>, String> {
            h.split(',').map(str::trim).filter(|x| !x.is_empty()).map(|x| u8::from_str_radix(x, 16).map_err(|_| format!("ERROR: bad hex in: {l}"))).collect()
        };
        let op = if rhs == "-" {
            RegOp::DeleteValue(name)
        } else if rhs.starts_with('"') {
            let q = close_quote(&rhs).ok_or(format!("ERROR: bad line: {l}"))?;
            RegOp::Set(name, REG_SZ, wide_bytes(&unescape(&rhs[1..q])))
        } else if let Some(d) = rhs.strip_prefix("dword:") {
            RegOp::Set(name, REG_DWORD, u32::from_str_radix(d.trim(), 16).map_err(|_| format!("ERROR: bad dword in: {l}"))?.to_le_bytes().to_vec())
        } else if let Some(h) = rhs.strip_prefix("hex:") {
            RegOp::Set(name, REG_BINARY, bytes(h)?)
        } else if let Some(r) = rhs.strip_prefix("hex(") {
            let (t, h) = r.split_once("):").ok_or(format!("ERROR: bad line: {l}"))?;
            RegOp::Set(name, u32::from_str_radix(t, 16).map_err(|_| format!("ERROR: bad type in: {l}"))?, bytes(h)?)
        } else {
            return Err(format!("ERROR: bad line: {l}"));
        };
        ops.push(op);
    }
    Ok(ops)
}

fn read_text(path: &str) -> Result<String, String> {
    let b = std::fs::read(path).map_err(|e| format!("ERROR: {path}: {e}"))?;
    Ok(if b.starts_with(&[0xFF, 0xFE]) {
        String::from_utf16_lossy(&utf16(&b[2..]))
    } else {
        String::from_utf8_lossy(b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&b)).into_owned()
    })
}

fn export_key(root: HKEY, sub: &str, long: &str, out: &mut String) -> Result<(), WIN32_ERROR> {
    let k = open(root, sub, KEY_READ)?;
    out.push_str(&format!("[{}]\r\n", full_name(long, sub)));
    for (n, ty, d) in values(&k) {
        out.push_str(&reg_line(&n, ty, &d));
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    for s in subkeys(&k) {
        let child = if sub.is_empty() { s } else { format!("{sub}\\{s}") };
        export_key(root, &child, long, out)?;
    }
    Ok(())
}

// ---------- the commands ----------

struct Args<'a> {
    pos: Vec<&'a str>,
    v: Option<&'a str>,
    ve: bool,
    t: Option<&'a str>,
    d: Option<&'a str>,
}

fn parse_args<'a>(a: &[&'a str]) -> Result<Args<'a>, String> {
    let mut r = Args { pos: vec![], v: None, ve: false, t: None, d: None };
    let mut i = 0;
    while i < a.len() {
        let next = || a.get(i + 1).copied().ok_or(format!("ERROR: {} needs a value", a[i]));
        match a[i].to_ascii_lowercase().as_str() {
            "/v" => { r.v = Some(next()?); i += 1; }
            "/t" => { r.t = Some(next()?); i += 1; }
            "/d" => { r.d = Some(next()?); i += 1; }
            "/ve" => r.ve = true,
            "/f" | "/y" | "/reg:64" => {}
            o if o.starts_with('/') => return Err(format!("ERROR: unsupported option {o}")),
            _ => r.pos.push(a[i]),
        }
        i += 1;
    }
    Ok(r)
}

/// Runs a reg.exe command line (without "reg" itself) in-process. Ok = what reg.exe would print on success.
pub fn exec<S: AsRef<str>>(args: &[S]) -> Result<String, String> {
    let a: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    let op = a.first().map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    let p = parse_args(&a[1.min(a.len())..])?;
    let key = p.pos.first().copied().ok_or("ERROR: no key")?;
    let value = p.v.or(if p.ve { Some("") } else { None });
    match op.as_str() {
        "query" => {
            let (root, sub, long) = split_key(key)?;
            let k = open(root, &sub, KEY_READ).map_err(err_text)?;
            let mut out = format!("\r\n{}\r\n", full_name(long, &sub));
            match value {
                Some(n) => {
                    let (ty, d) = get(&k, n).map_err(err_text)?;
                    out.push_str(&query_line(n, ty, &d));
                }
                None => values(&k).iter().for_each(|(n, ty, d)| out.push_str(&query_line(n, *ty, d))),
            }
            Ok(out + "\r\n")
        }
        "add" => {
            let (root, sub, _) = split_key(key)?;
            let k = create(root, &sub).map_err(err_text)?;
            if let Some(n) = value {
                let ty = match p.t { Some(t) => type_of(t).ok_or(format!("ERROR: unsupported type {t}"))?, None => REG_SZ };
                set(&k, n, ty, &parse_data(ty, p.d.unwrap_or(""))?).map_err(err_text)?;
            }
            Ok("The operation completed successfully.\r\n".into())
        }
        "delete" => {
            let (root, sub, _) = split_key(key)?;
            match value {
                Some(n) => {
                    let k = open(root, &sub, KEY_SET_VALUE).map_err(err_text)?;
                    let r = unsafe { RegDeleteValueW(k.0, w(n).as_ptr()) };
                    if r != ERROR_SUCCESS { return Err(err_text(r)); }
                }
                None => {
                    if sub.is_empty() {
                        return Err("ERROR: refusing to delete a root key".into());
                    }
                    // open the target key itself (not the root hive, where HKLM denies KEY_ALL_ACCESS)
                    let k = open(root, &sub, DELETE | KEY_ENUMERATE_SUB_KEYS | KEY_QUERY_VALUE | KEY_SET_VALUE).map_err(err_text)?;
                    let r = unsafe { RegDeleteTreeW(k.0, null()) };
                    drop(k);
                    if r != ERROR_SUCCESS && r != ERROR_FILE_NOT_FOUND { return Err(err_text(r)); }
                    // RegDeleteTree empties the key; take the key itself away as well
                    let r = unsafe { RegDeleteKeyExW(root, w(&sub).as_ptr(), KEY_WOW64_64KEY, 0) };
                    if r != ERROR_SUCCESS && r != ERROR_FILE_NOT_FOUND { return Err(err_text(r)); }
                }
            }
            Ok("The operation completed successfully.\r\n".into())
        }
        "export" => {
            let file = p.pos.get(1).ok_or("ERROR: no file")?;
            let (root, sub, long) = split_key(key)?;
            let mut text = String::from("Windows Registry Editor Version 5.00\r\n\r\n");
            export_key(root, &sub, long, &mut text).map_err(err_text)?;
            let mut bytes = vec![0xFF, 0xFE];
            bytes.extend(text.encode_utf16().flat_map(|c| c.to_le_bytes()));
            std::fs::write(file, bytes).map_err(|e| format!("ERROR: {file}: {e}"))?;
            Ok("The operation completed successfully.\r\n".into())
        }
        "import" => {
            let ops = parse_reg_file(&read_text(key)?)?;
            let mut cur: Option<Key> = None;
            for op in ops {
                match op {
                    RegOp::Key(k) => {
                        let (root, sub, _) = split_key(&k)?;
                        cur = Some(create(root, &sub).map_err(err_text)?);
                    }
                    RegOp::DeleteKey(k) => {
                        cur = None;
                        let _ = exec(&["delete", &k, "/f"]);
                    }
                    RegOp::Set(n, ty, d) => set(cur.as_ref().ok_or("ERROR: value before any key")?, &n, ty, &d).map_err(err_text)?,
                    RegOp::DeleteValue(n) => {
                        let r = unsafe { RegDeleteValueW(cur.as_ref().ok_or("ERROR: value before any key")?.0, w(&n).as_ptr()) };
                        if r != ERROR_SUCCESS && r != ERROR_FILE_NOT_FOUND { return Err(err_text(r)); }
                    }
                }
            }
            Ok("The operation completed successfully.\r\n".into())
        }
        _ => Err(format!("ERROR: unsupported reg command {op}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reg::{dword, value};

    #[test]
    fn data_forms() {
        assert_eq!(parse_data(REG_DWORD, "26").unwrap(), 26u32.to_le_bytes());
        assert_eq!(parse_data(REG_DWORD, "0x1a").unwrap(), 26u32.to_le_bytes());
        assert!(parse_data(REG_DWORD, "x").is_err() && parse_data(REG_DWORD, "4294967296").is_err());
        assert_eq!(show(REG_DWORD, &parse_data(REG_DWORD, "26").unwrap()), "0x1a");
        assert_eq!(show(REG_SZ, &parse_data(REG_SZ, r#""C:\a b.exe" /x"#).unwrap()), r#""C:\a b.exe" /x"#);
        assert_eq!(show(REG_MULTI_SZ, &parse_data(REG_MULTI_SZ, r"a\0b c").unwrap()), r"a\0b c");
        assert_eq!(show(REG_BINARY, &parse_data(REG_BINARY, "00ff10").unwrap()), "00FF10");
        assert_eq!(show(REG_QWORD, &parse_data(REG_QWORD, "0x100000000").unwrap()), "0x100000000");
        assert_eq!(dword(&query_line("V", REG_DWORD, &7u32.to_le_bytes())), Some(7));
        assert_eq!(value(&query_line("", REG_SZ, &wide_bytes(""))), Some(("REG_SZ", "")));
    }

    #[test]
    fn reg_files_round_trip() {
        let vals = [
            ("", REG_SZ, wide_bytes(r#""C:\x.exe" "%1""#)),
            ("DelegateExecute", REG_SZ, wide_bytes("")),
            ("N", REG_DWORD, 5u32.to_le_bytes().to_vec()),
            ("Bin", REG_BINARY, vec![1, 2, 255]),
            ("Path", REG_EXPAND_SZ, wide_bytes("%SystemRoot%")),
            ("Multi", REG_MULTI_SZ, parse_data(REG_MULTI_SZ, r"a\0b").unwrap()),
            (r#"we"ird\name"#, REG_SZ, wide_bytes("q\"uote")),
        ];
        let mut text = "Windows Registry Editor Version 5.00\r\n\r\n[HKEY_CURRENT_USER\\Software\\X]\r\n".to_string();
        for (n, t, d) in &vals {
            text += &reg_line(n, *t, d);
            text += "\r\n";
        }
        text += "\"Gone\"=-\r\n\r\n[-HKEY_CURRENT_USER\\Software\\Old]\r\n";
        let ops = parse_reg_file(&text).unwrap();
        assert_eq!(ops[0], RegOp::Key(r"HKEY_CURRENT_USER\Software\X".into()));
        for (i, (n, t, d)) in vals.iter().enumerate() {
            assert_eq!(ops[1 + i], RegOp::Set(n.to_string(), *t, d.clone()), "value {n}");
        }
        assert_eq!(ops[vals.len() + 1], RegOp::DeleteValue("Gone".into()));
        assert_eq!(ops[vals.len() + 2], RegOp::DeleteKey(r"HKEY_CURRENT_USER\Software\Old".into()));
        // what reg.exe itself writes: long hex wrapped over 2+ lines with a trailing backslash
        let wrapped = "Windows Registry Editor Version 5.00\r\n\r\n[HKEY_CURRENT_USER\\Software\\Y]\r\n\"B\"=hex:01,02,\\\r\n  03,04,\\\r\n  05\r\n";
        assert_eq!(parse_reg_file(wrapped).unwrap()[1], RegOp::Set("B".into(), REG_BINARY, vec![1, 2, 3, 4, 5]));
        assert!(parse_reg_file("not a reg file").is_err());
    }

    /// The live round trip, in a scratch key of the current user.
    #[test]
    fn live_commands() {
        let key = format!(r"HKCU\Software\WinRust-test-{}", std::process::id());
        let sub = format!(r"{key}\shell\open\command");
        exec(&["add", &sub, "/ve", "/t", "REG_SZ", "/d", r#""C:\x.exe" "%1""#, "/f"]).unwrap();
        exec(&["add", &sub, "/v", "DelegateExecute", "/t", "REG_SZ", "/d", "", "/f"]).unwrap();
        exec(&["add", &key, "/v", "N", "/t", "REG_DWORD", "/d", "26", "/f"]).unwrap();
        assert_eq!(dword(&exec(&["query", &key, "/v", "N"]).unwrap()), Some(26));
        assert_eq!(value(&exec(&["query", &sub, "/ve"]).unwrap()), Some(("REG_SZ", r#""C:\x.exe" "%1""#)));
        assert!(exec(&["query", &key, "/v", "Missing"]).is_err());
        // export, change, import: the old values come back
        let file = std::env::temp_dir().join(format!("winrust-test-{}.reg", std::process::id()));
        let f = file.to_string_lossy().into_owned();
        exec(&["export", &key, &f, "/y"]).unwrap();
        exec(&["add", &sub, "/ve", "/t", "REG_SZ", "/d", "changed", "/f"]).unwrap();
        exec(&["import", &f]).unwrap();
        assert_eq!(value(&exec(&["query", &sub, "/ve"]).unwrap()), Some(("REG_SZ", r#""C:\x.exe" "%1""#)));
        let _ = std::fs::remove_file(&file);
        exec(&["delete", &key, "/v", "N", "/f"]).unwrap();
        assert!(exec(&["query", &key, "/v", "N"]).is_err());
        exec(&["delete", &key, "/f"]).unwrap();
        assert!(exec(&["query", &key]).is_err());
        assert!(exec(&["delete", &key, "/f"]).is_err()); // like reg.exe: a missing key is an error
    }
}

//! The notepad's logic without windows: encodings (detect, decode, encode), line endings, the command line, the recent
//! files list and the settings file. The editor control keeps paragraphs apart with a lone CR, so text goes in with
//! `to_editor` and comes out with `from_editor` in the file's own line ending.
use windows_sys::Win32::Globalization::{CP_ACP, GetACP, MultiByteToWideChar, WideCharToMultiByte};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Ansi,
}

pub const ENCODINGS: [Encoding; 5] = [Encoding::Utf8, Encoding::Utf8Bom, Encoding::Utf16Le, Encoding::Utf16Be, Encoding::Ansi];

impl Encoding {
    pub fn name(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf8Bom => "UTF-8 BOM",
            Encoding::Utf16Le => "UTF-16 LE",
            Encoding::Utf16Be => "UTF-16 BE",
            Encoding::Ansi => "ANSI",
        }
    }
    fn bom(self) -> &'static [u8] {
        match self {
            Encoding::Utf8Bom => &[0xEF, 0xBB, 0xBF],
            Encoding::Utf16Le => &[0xFF, 0xFE],
            Encoding::Utf16Be => &[0xFE, 0xFF],
            _ => &[],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Eol {
    Crlf,
    Lf,
    Cr,
}

pub const EOLS: [Eol; 3] = [Eol::Crlf, Eol::Lf, Eol::Cr];

impl Eol {
    pub fn name(self) -> &'static str {
        match self {
            Eol::Crlf => "Windows (CRLF)",
            Eol::Lf => "Unix (LF)",
            Eol::Cr => "Macintosh (CR)",
        }
    }
    fn units(self) -> &'static [u16] {
        match self {
            Eol::Crlf => &[13, 10],
            Eol::Lf => &[10],
            Eol::Cr => &[13],
        }
    }
}

/// The encoding of a file's bytes: a BOM decides; else valid UTF-8 is UTF-8; else text with a zero in every other byte is
/// UTF-16 without a BOM; else the system ANSI code page.
pub fn detect(b: &[u8]) -> Encoding {
    if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Encoding::Utf8Bom;
    }
    if b.starts_with(&[0xFF, 0xFE]) {
        return Encoding::Utf16Le;
    }
    if b.starts_with(&[0xFE, 0xFF]) {
        return Encoding::Utf16Be;
    }
    // UTF-16 first: ASCII text in UTF-16 is valid UTF-8 too (the zeros are NUL characters)
    let s = &b[..b.len().min(1 << 16) & !1];
    if s.len() >= 4 {
        let pairs = s.len() / 2;
        let even = s.iter().step_by(2).filter(|&&c| c == 0).count();
        let odd = s.iter().skip(1).step_by(2).filter(|&&c| c == 0).count();
        if odd * 10 > pairs * 4 && even * 20 < pairs {
            return Encoding::Utf16Le;
        }
        if even * 10 > pairs * 4 && odd * 20 < pairs {
            return Encoding::Utf16Be;
        }
    }
    if std::str::from_utf8(b).is_ok() { Encoding::Utf8 } else { Encoding::Ansi }
}

/// The file's text as UTF-16, without the BOM. Broken sequences become U+FFFD.
pub fn decode(b: &[u8], enc: Encoding) -> Vec<u16> {
    let b = b.strip_prefix(enc.bom()).unwrap_or(b);
    match enc {
        Encoding::Utf8 | Encoding::Utf8Bom => {
            let s = String::from_utf8_lossy(b);
            let mut v = Vec::with_capacity(s.len());
            v.extend(s.encode_utf16());
            v
        }
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let mut v: Vec<u16> = b.chunks_exact(2).map(|c| if enc == Encoding::Utf16Le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
            if b.len() % 2 == 1 {
                v.push(0xFFFD);
            }
            v
        }
        Encoding::Ansi => {
            let mut v = Vec::new();
            for part in b.chunks(1 << 30) {
                let n = unsafe { MultiByteToWideChar(CP_ACP, 0, part.as_ptr(), part.len() as i32, std::ptr::null_mut(), 0) };
                let at = v.len();
                v.resize(at + n.max(0) as usize, 0);
                unsafe { MultiByteToWideChar(CP_ACP, 0, part.as_ptr(), part.len() as i32, v.as_mut_ptr().add(at), n) };
            }
            v
        }
    }
}

/// The file's bytes in `enc` (with its BOM). The flag is true when some characters could not be written (ANSI, or a
/// lone surrogate in UTF-8): they were replaced.
pub fn encode(u: &[u16], enc: Encoding) -> (Vec<u8>, bool) {
    let mut out = enc.bom().to_vec();
    match enc {
        Encoding::Utf8 | Encoding::Utf8Bom => {
            let mut lossy = false;
            out.reserve(u.len());
            let mut buf = [0u8; 4];
            for c in char::decode_utf16(u.iter().copied()) {
                let c = c.unwrap_or_else(|_| {
                    lossy = true;
                    '\u{FFFD}'
                });
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
            (out, lossy)
        }
        Encoding::Utf16Le => {
            out.reserve(u.len() * 2);
            u.iter().for_each(|c| out.extend_from_slice(&c.to_le_bytes()));
            (out, false)
        }
        Encoding::Utf16Be => {
            out.reserve(u.len() * 2);
            u.iter().for_each(|c| out.extend_from_slice(&c.to_be_bytes()));
            (out, false)
        }
        Encoding::Ansi if unsafe { GetACP() } == 65001 => encode(u, Encoding::Utf8), // "Beta: UTF-8 for worldwide language support"
        Encoding::Ansi => {
            let mut lossy = false;
            for part in u.chunks(1 << 28) {
                let mut used = 0i32;
                let n = unsafe { WideCharToMultiByte(CP_ACP, 0, part.as_ptr(), part.len() as i32, std::ptr::null_mut(), 0, std::ptr::null(), std::ptr::null_mut()) };
                let at = out.len();
                out.resize(at + n.max(0) as usize, 0);
                unsafe { WideCharToMultiByte(CP_ACP, 0, part.as_ptr(), part.len() as i32, out.as_mut_ptr().add(at), n, std::ptr::null(), &mut used) };
                lossy |= used != 0;
            }
            (out, lossy)
        }
    }
}

/// The line ending most lines of `u` use (CRLF when there is none).
pub fn detect_eol(u: &[u16]) -> Eol {
    let (mut crlf, mut lf, mut cr) = (0usize, 0usize, 0usize);
    let mut i = 0;
    while i < u.len() {
        match u[i] {
            13 if u.get(i + 1) == Some(&10) => {
                crlf += 1;
                i += 1;
            }
            13 => cr += 1,
            10 => lf += 1,
            _ => {}
        }
        i += 1;
    }
    if lf > crlf && lf >= cr {
        Eol::Lf
    } else if cr > crlf && cr > lf {
        Eol::Cr
    } else {
        Eol::Crlf
    }
}

/// Every CRLF, LF and CR as the lone CR the editor control uses for a line break (in place, no second copy of a big file).
pub fn to_editor(mut u: Vec<u16>) -> Vec<u16> {
    let (mut r, mut w) = (0, 0);
    while r < u.len() {
        let c = u[r];
        if c == 13 && u.get(r + 1) == Some(&10) {
            r += 1;
        }
        u[w] = if c == 10 { 13 } else { c };
        w += 1;
        r += 1;
    }
    u.truncate(w);
    u
}

/// The editor's text with its CR line breaks written as `eol`.
pub fn from_editor(u: &[u16], eol: Eol) -> Vec<u16> {
    if eol == Eol::Cr {
        return u.to_vec();
    }
    let breaks = u.iter().filter(|&&c| c == 13).count();
    let mut v = Vec::with_capacity(u.len() + breaks);
    for &c in u {
        if c == 13 {
            v.extend_from_slice(eol.units());
        } else {
            v.push(c);
        }
    }
    v
}

/// The file to open from the command line. Started through the IFEO Debugger value the line is
/// `rs-notepad.exe C:\Windows\System32\notepad.exe <file>`, so the notepad.exe part goes; Notepad's switches (/A, /W, /P)
/// go too; a path with spaces may come unquoted, as Notepad accepts it.
pub fn file_arg(args: &[String]) -> Option<String> {
    let mut rest = args;
    if let Some(first) = rest.first() {
        let f = first.trim_matches('"').to_lowercase();
        let name = f.rsplit(['\\', '/']).next().unwrap_or("");
        if name == "notepad.exe" || name == "notepad" {
            rest = &rest[1..];
        }
    }
    while rest.first().is_some_and(|a| a.starts_with('/') && a.len() <= 3) {
        rest = &rest[1..];
    }
    let p = rest.join(" ");
    let p = p.trim().trim_matches('"').trim();
    (!p.is_empty()).then(|| p.to_string())
}

pub const RECENT_MAX: usize = 10;

/// Puts `path` on top of the recent files (once: case does not matter on Windows), at most `RECENT_MAX`.
pub fn push_recent(list: &mut Vec<String>, path: &str) {
    let low = path.to_lowercase();
    list.retain(|p| p.to_lowercase() != low);
    list.insert(0, path.to_string());
    list.truncate(RECENT_MAX);
}

pub fn parse_recent(s: &str) -> Vec<String> {
    s.lines().map(str::trim).filter(|l| !l.is_empty()).take(RECENT_MAX).map(String::from).collect()
}

/// Zoom steps in percent, like the browsers'.
const ZOOMS: [u32; 17] = [10, 25, 50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300, 400, 500];

/// The next zoom step up or down from `cur` (which may lie between steps after a Ctrl+wheel zoom).
pub fn zoom_step(cur: u32, up: bool) -> u32 {
    if up {
        ZOOMS.iter().copied().find(|&z| z > cur).unwrap_or(500)
    } else {
        ZOOMS.iter().rev().copied().find(|&z| z < cur).unwrap_or(10)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    pub wrap: bool,
    pub zoom: u32,
    pub status: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { wrap: true, zoom: 100, status: true }
    }
}

/// `key=value` lines; unknown keys and bad values keep the defaults.
pub fn parse_settings(s: &str) -> Settings {
    let mut st = Settings::default();
    for l in s.lines() {
        let Some((k, v)) = l.split_once('=') else { continue };
        let v = v.trim();
        match k.trim() {
            "wrap" => st.wrap = v != "0",
            "status" => st.status = v != "0",
            "zoom" => st.zoom = v.parse().ok().filter(|z| (10..=500).contains(z)).unwrap_or(100),
            _ => {}
        }
    }
    st
}

pub fn settings_text(s: Settings) -> String {
    format!("wrap={}\r\nzoom={}\r\nstatus={}\r\n", s.wrap as u8, s.zoom, s.status as u8)
}

/// "Ln 3, Col 7" in the UI language.
pub fn position_text(line: usize, col: usize) -> String {
    tf!("Стр {}, стлб {}", "Ln {}, Col {}", ja: "{} 行、{} 列", pt: "Ln {}, Col {}", line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn detects_boms_utf8_utf16_and_ansi() {
        assert_eq!(detect(b"\xEF\xBB\xBFhi"), Encoding::Utf8Bom);
        assert_eq!(detect(b"\xFF\xFEh\0"), Encoding::Utf16Le);
        assert_eq!(detect(b"\xFE\xFF\0h"), Encoding::Utf16Be);
        assert_eq!(detect("привет".as_bytes()), Encoding::Utf8);
        assert_eq!(detect(b""), Encoding::Utf8);
        assert_eq!(detect(b"h\0e\0l\0l\0o\0"), Encoding::Utf16Le);
        assert_eq!(detect(b"\0h\0e\0l\0l\0o"), Encoding::Utf16Be);
        assert_eq!(detect(b"caf\xE9 \xF0\xF0"), Encoding::Ansi); // cp1252 é: not UTF-8
    }

    #[test]
    fn decodes_and_encodes_round_trip() {
        let text = "Ёлка 🎄 tree";
        for enc in [Encoding::Utf8, Encoding::Utf8Bom, Encoding::Utf16Le, Encoding::Utf16Be] {
            let (bytes, lossy) = encode(&w(text), enc);
            assert!(!lossy);
            assert_eq!(detect(&bytes), enc, "{enc:?}");
            assert_eq!(decode(&bytes, enc), w(text), "{enc:?}");
        }
        // ASCII is the same in every ANSI code page
        let (bytes, lossy) = encode(&w("plain ascii"), Encoding::Ansi);
        assert!(!lossy);
        assert_eq!(bytes, b"plain ascii");
        assert_eq!(decode(b"plain ascii", Encoding::Ansi), w("plain ascii"));
    }

    #[test]
    fn broken_input_is_replaced_not_lost() {
        assert_eq!(decode(b"a\xFFb", Encoding::Utf8), w("a\u{FFFD}b"));
        assert_eq!(decode(b"a\0b", Encoding::Utf16Le), vec![b'a' as u16, 0xFFFD]);
        let (bytes, lossy) = encode(&[0xD800, b'x' as u16], Encoding::Utf8); // lone surrogate
        assert!(lossy);
        assert_eq!(bytes, "\u{FFFD}x".as_bytes());
    }

    #[test]
    fn line_endings() {
        assert_eq!(detect_eol(&w("a\r\nb\r\nc")), Eol::Crlf);
        assert_eq!(detect_eol(&w("a\nb\nc\r\n")), Eol::Lf);
        assert_eq!(detect_eol(&w("a\rb\rc")), Eol::Cr);
        assert_eq!(detect_eol(&w("one line")), Eol::Crlf);
        let ed = to_editor(w("a\r\nb\nc\rd"));
        assert_eq!(ed, w("a\rb\rc\rd"));
        assert_eq!(from_editor(&ed, Eol::Crlf), w("a\r\nb\r\nc\r\nd"));
        assert_eq!(from_editor(&ed, Eol::Lf), w("a\nb\nc\nd"));
        assert_eq!(from_editor(&ed, Eol::Cr), ed);
    }

    #[test]
    fn command_line_from_ifeo_and_shell() {
        let a = |v: &[&str]| file_arg(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&[r"C:\WINDOWS\system32\NOTEPAD.EXE", r"C:\notes\a.txt"]).as_deref(), Some(r"C:\notes\a.txt"));
        assert_eq!(a(&["notepad", r"C:\My", r"notes\b.txt"]).as_deref(), Some(r"C:\My notes\b.txt"));
        assert_eq!(a(&[r"C:\Windows\notepad.exe", "/A", r"C:\x.log"]).as_deref(), Some(r"C:\x.log"));
        assert_eq!(a(&[r"D:\plain.txt"]).as_deref(), Some(r"D:\plain.txt"));
        assert_eq!(a(&[r"C:\Windows\System32\notepad.exe"]), None);
        assert_eq!(a(&[]), None);
    }

    #[test]
    fn recent_files_stay_unique_and_short() {
        let mut l = parse_recent("C:\\a.txt\r\n\r\nC:\\b.txt\n");
        assert_eq!(l, ["C:\\a.txt", "C:\\b.txt"]);
        push_recent(&mut l, "c:\\B.TXT");
        assert_eq!(l, ["c:\\B.TXT", "C:\\a.txt"]);
        for i in 0..20 {
            push_recent(&mut l, &format!("C:\\{i}.txt"));
        }
        assert_eq!(l.len(), RECENT_MAX);
        assert_eq!(l[0], "C:\\19.txt");
    }

    #[test]
    fn zoom_steps_and_settings() {
        assert_eq!(zoom_step(100, true), 110);
        assert_eq!(zoom_step(100, false), 90);
        assert_eq!(zoom_step(113, false), 110);
        assert_eq!(zoom_step(500, true), 500);
        assert_eq!(zoom_step(10, false), 10);
        let s = Settings { wrap: false, zoom: 150, status: false };
        assert_eq!(parse_settings(&settings_text(s)), s);
        assert_eq!(parse_settings("zoom=9000\njunk\nwrap=0"), Settings { wrap: false, ..Settings::default() });
    }

    #[test]
    fn status_text_is_translated() {
        assert_eq!(position_text(3, 7), "Стр 3, стлб 7");
    }
}

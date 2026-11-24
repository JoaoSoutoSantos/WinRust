#![windows_subsystem = "windows"]
#![allow(unsafe_op_in_unsafe_fn)]
//! rs-notepad: a lightweight Win32 Notepad replacement using RICHEDIT50W (Msftedit.dll).
//! Opens 100 MB+ files without freezing via background decode + chunked EM_STREAMIN.
#[macro_use]
extern crate winrust_common;

mod re;
mod text;

use re::*;
use std::{
    cell::RefCell,
    mem::{size_of, zeroed},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::atomic::{AtomicU32, Ordering::Relaxed},
    time::Instant,
};
use text::{ENCODINGS, EOLS, Encoding, Eol, RECENT_MAX, Settings};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::DwmSetWindowAttribute, Gdi::*},
    System::LibraryLoader::{GetModuleHandleW, LoadLibraryW},
    UI::{
        Controls::{
            EM_EMPTYUNDOBUFFER, EM_GETMODIFY, EM_LINEINDEX, EM_REPLACESEL, EM_SCROLLCARET,
            EM_SETMODIFY, EM_SETREADONLY, EM_UNDO, NMHDR, SetWindowTheme,
        },
        Controls::Dialogs::{
            FINDREPLACEW, FR_DIALOGTERM, FR_DOWN, FR_FINDNEXT, FR_MATCHCASE, FR_REPLACE,
            FR_REPLACEALL, FR_WHOLEWORD, FindTextW, GetOpenFileNameW, GetSaveFileNameW,
            OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST,
            OPENFILENAMEW, ReplaceTextW,
        },
        Input::KeyboardAndMouse::{
            SetFocus, VK_ADD, VK_F3, VK_OEM_MINUS, VK_OEM_PLUS, VK_SUBTRACT,
        },
        Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, HDROP},
        WindowsAndMessaging::*,
    },
};
use winrust_common::{frames, style};

const WM_LOADED: u32 = WM_APP + 1;
const WM_CHUNK: u32 = WM_APP + 2;
const WM_SAVED: u32 = WM_APP + 3;
const TIMER_NOTE: usize = 1;
const CHUNK_UNITS: usize = 1_000_000;

// Menu / accelerator command IDs
const ID_NEW: u16 = 101;
const ID_NEW_WIN: u16 = 102;
const ID_OPEN: u16 = 103;
const ID_SAVE: u16 = 104;
const ID_SAVE_AS: u16 = 105;
const ID_EXIT: u16 = 106;
const ID_UNDO: u16 = 201;
const ID_REDO: u16 = 202;
const ID_CUT: u16 = 203;
const ID_COPY: u16 = 204;
const ID_PASTE: u16 = 205;
const ID_DELETE: u16 = 206;
const ID_FIND: u16 = 207;
const ID_FIND_NEXT: u16 = 208;
const ID_FIND_PREV: u16 = 209;
const ID_REPLACE: u16 = 210;
const ID_SELECT_ALL: u16 = 211;
const ID_WRAP: u16 = 301;
const ID_ZOOM_IN: u16 = 401;
const ID_ZOOM_OUT: u16 = 402;
const ID_ZOOM_RESET: u16 = 403;
const ID_STATUS: u16 = 404;
const ID_ENC_BASE: u16 = 500; // 500..505
const ID_EOL_BASE: u16 = 510; // 510..513
const ID_RECENT_BASE: u16 = 600; // 600..610

static LOAD_GEN: AtomicU32 = AtomicU32::new(0);
static FIND_MSG: AtomicU32 = AtomicU32::new(0);

struct LoadResult {
    seq: u32,
    path: PathBuf,
    res: Result<(Vec<u16>, Encoding, Eol), String>,
}

struct SaveResult {
    path: PathBuf,
    lossy: bool,
    res: Result<(), String>,
}

struct FindState {
    fr: *mut FINDREPLACEW,
    find_buf: *mut [u16; 256],
    repl_buf: *mut [u16; 256],
}

struct App {
    edit: HWND,
    menu_recent: HMENU,
    menu_format: HMENU,
    menu_view: HMENU,
    path: Option<PathBuf>,
    enc: Encoding,
    eol: Eol,
    modified: bool,
    /// just opened, no edit yet: a late change event from loading the text is not an edit
    fresh: bool,
    settings: Settings,
    recent: Vec<String>,
    loading: bool,
    load_gen: u32,
    stream_buf: Option<Vec<u16>>,
    stream_pos: usize,
    saving: bool,
    close_after_save: bool,
    find_dlg: HWND,
    find_state: Option<FindState>,
    last_find: String,
    last_flags: u32,
    line: usize,
    col: usize,
    note: String,
    note_since: Option<Instant>,
    note_sticky: bool,
    dpi: i32,
    font: HFONT,
}

impl Default for App {
    fn default() -> Self {
        Self {
            edit: null_mut(),
            menu_recent: null_mut(),
            menu_format: null_mut(),
            menu_view: null_mut(),
            path: None,
            enc: Encoding::Utf8,
            eol: Eol::Crlf,
            modified: false,
            fresh: false,
            settings: Settings::default(),
            recent: Vec::new(),
            loading: false,
            load_gen: 0,
            stream_buf: None,
            stream_pos: 0,
            saving: false,
            close_after_save: false,
            find_dlg: null_mut(),
            find_state: None,
            last_find: String::new(),
            last_flags: FR_DOWN,
            line: 1,
            col: 1,
            note: String::new(),
            note_since: None,
            note_sticky: false,
            dpi: 96,
            font: null_mut(),
        }
    }
}

thread_local! {
    static APP: RefCell<App> = RefCell::new(App::default());
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn from_wide_z(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

fn config_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("WinRust").join("notepad"))
}

fn load_config() -> (Settings, Vec<String>) {
    let Some(dir) = config_dir() else {
        return (Settings::default(), Vec::new());
    };
    let st = std::fs::read_to_string(dir.join("settings.txt"))
        .map(|s| text::parse_settings(&s))
        .unwrap_or_default();
    let rec = std::fs::read_to_string(dir.join("recent.txt"))
        .map(|s| text::parse_recent(&s))
        .unwrap_or_default();
    (st, rec)
}

fn save_settings(st: Settings) {
    if let Some(dir) = config_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("settings.txt"), text::settings_text(st));
    }
}

fn save_recent(list: &[String]) {
    if let Some(dir) = config_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("recent.txt"), list.join("\r\n"));
    }
}

fn px(v: i32, dpi: i32) -> i32 {
    (v * dpi + 48) / 96
}

fn status_h(st: Settings, dpi: i32) -> i32 {
    if st.status { px(26, dpi) } else { 0 }
}

// ---------- RichEdit streaming & text access ----------

struct StreamBuf<'a> {
    bytes: &'a [u8],
    pos: usize,
}

unsafe extern "system" fn stream_in_cb(
    cookie: usize,
    buf: *mut u8,
    cb: i32,
    pcb: *mut i32,
) -> u32 {
    let st = &mut *(cookie as *mut StreamBuf);
    let rem = st.bytes.len().saturating_sub(st.pos);
    // Keep UTF-16 code units intact (always even byte count)
    let n = rem.min(cb.max(0) as usize) & !1;
    if n > 0 {
        std::ptr::copy_nonoverlapping(st.bytes.as_ptr().add(st.pos), buf, n);
        st.pos += n;
    }
    *pcb = n as i32;
    0
}

unsafe fn stream_in(edit: HWND, units: &[u16], append: bool) {
    if append {
        let end = CHARRANGE { cpMin: -1, cpMax: -1 };
        SendMessageW(edit, EM_EXSETSEL, 0, &end as *const _ as isize);
    }
    let bytes = std::slice::from_raw_parts(units.as_ptr() as *const u8, units.len() * 2);
    let mut sb = StreamBuf { bytes, pos: 0 };
    let mut es = EDITSTREAM {
        dwCookie: &mut sb as *mut _ as usize,
        dwError: 0,
        pfnCallback: Some(stream_in_cb),
    };
    let flags = SF_TEXT | SF_UNICODE | if append { SFF_SELECTION } else { 0 };
    SendMessageW(edit, EM_STREAMIN, flags as usize, &mut es as *mut _ as isize);
}

unsafe fn read_editor_text(edit: HWND) -> Vec<u16> {
    let gtl = GETTEXTLENGTHEX { flags: GTL_PRECISE | GTL_NUMCHARS, codepage: CP_UNICODE };
    let len = SendMessageW(edit, EM_GETTEXTLENGTHEX, &gtl as *const _ as usize, 0);
    if len <= 0 {
        return Vec::new();
    }
    let mut buf = vec![0u16; len as usize + 2];
    let gt = GETTEXTEX {
        cb: (buf.len() * 2) as u32,
        flags: GT_DEFAULT,
        codepage: CP_UNICODE,
        lpDefaultChar: null(),
        lpUsedDefChar: null_mut(),
    };
    let n = SendMessageW(edit, EM_GETTEXTEX, &gt as *const _ as usize, buf.as_mut_ptr() as isize);
    let n = (n.max(0) as usize).min(buf.len());
    buf.truncate(n);
    buf
}

unsafe fn init_richedit(edit: HWND) {
    SendMessageW(
        edit,
        EM_SETTEXTMODE,
        (TM_PLAINTEXT | TM_MULTILEVELUNDO | TM_MULTICODEPAGE) as usize,
        0,
    );
    SendMessageW(edit, EM_EXLIMITTEXT, 0, 0x7FFF_FFFE);
    SendMessageW(
        edit,
        EM_SETEVENTMASK,
        0,
        (ENM_CHANGE | ENM_SELCHANGE | ENM_DROPFILES) as isize,
    );
}

// ---------- Theme, layout, title, menus ----------

unsafe fn apply_theme(hwnd: HWND) {
    let (edit, dpi) = APP.with_borrow(|a| (a.edit, a.dpi));
    let p = style::pal();
    let dark = !style::style().light();
    let dark_i: i32 = if dark { 1 } else { 0 };
    DwmSetWindowAttribute(hwnd, 20, &dark_i as *const _ as *const _, 4);
    DwmSetWindowAttribute(hwnd, 35, &p.bg as *const _ as *const _, 4);
    if !edit.is_null() {
        let theme = wide(if dark { "DarkMode_Explorer" } else { "Explorer" });
        unchanged(edit, || {
            SetWindowTheme(edit, theme.as_ptr(), null());
            SendMessageW(edit, EM_SETBKGNDCOLOR, 0, p.bg as isize);
        });
        apply_char_format(edit, p.text);
    }
    let old = APP.with_borrow_mut(|a| {
        let f = CreateFontW(
            -px(12, dpi),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1,
            0,
            0,
            5,
            0,
            wide(style::ui_face()).as_ptr(),
        );
        std::mem::replace(&mut a.font, f)
    });
    if !old.is_null() {
        DeleteObject(old);
    }
    InvalidateRect(hwnd, null(), 1);
}

unsafe fn apply_char_format(edit: HWND, color: u32) {
    let mut cf: CHARFORMATW = zeroed();
    cf.cbSize = size_of::<CHARFORMATW>() as u32;
    cf.dwMask = CFM_FACE | CFM_SIZE | CFM_COLOR | CFM_CHARSET;
    cf.yHeight = 220; // 11 pt in twips
    cf.crTextColor = color;
    cf.bCharSet = DEFAULT_CHARSET as u8;
    for (dst, src) in cf.szFaceName.iter_mut().zip("Consolas".encode_utf16()) {
        *dst = src;
    }
    unchanged(edit, || {
        SendMessageW(edit, EM_SETCHARFORMAT, SCF_DEFAULT as usize, &cf as *const _ as isize);
        SendMessageW(edit, EM_SETCHARFORMAT, SCF_ALL as usize, &cf as *const _ as isize);
    });
}

/// Runs a look-only change (colour, font, wrap, zoom): RichEdit counts a format change as an edit, which marked a file
/// as modified ("*name") right after opening it or on a theme change. The modified flag and the change events are kept,
/// and so is an empty undo stack: `EM_SETCHARFORMAT` leaves an undo step, and a theme broadcast arriving just after a
/// load used to leave one behind. The EN_CHANGE check ("a real edit always leaves something to undo") then took the
/// next stray change notification for a real edit and showed "*" on a file nobody touched (a few opens in 30).
/// It also kept Ctrl+Z from undoing a theme change.
unsafe fn unchanged(edit: HWND, f: impl FnOnce()) {
    const EM_GETEVENTMASK: u32 = 0x043B; // WM_USER + 59
    const EM_CANUNDO: u32 = 0x00C6;
    const EM_EMPTYUNDOBUFFER: u32 = 0x00CD;
    let mask = SendMessageW(edit, EM_GETEVENTMASK, 0, 0);
    let modified = SendMessageW(edit, EM_GETMODIFY, 0, 0) != 0;
    let undo = SendMessageW(edit, EM_CANUNDO, 0, 0) != 0;
    SendMessageW(edit, EM_SETEVENTMASK, 0, 0);
    f();
    if !undo {
        SendMessageW(edit, EM_EMPTYUNDOBUFFER, 0, 0);
    }
    SendMessageW(edit, EM_SETMODIFY, modified as usize, 0);
    SendMessageW(edit, EM_SETEVENTMASK, 0, mask);
}

unsafe fn apply_wrap(edit: HWND, wrap: bool) {
    unchanged(edit, || {
        SendMessageW(edit, EM_SETTARGETDEVICE, 0, if wrap { 0 } else { 1 });
    });
}

unsafe fn apply_zoom(edit: HWND, zoom: u32) {
    unchanged(edit, || {
        if zoom == 100 {
            SendMessageW(edit, EM_SETZOOM, 0, 0);
        } else {
            SendMessageW(edit, EM_SETZOOM, zoom as usize, 100);
        }
    });
}

unsafe fn read_zoom(edit: HWND) -> u32 {
    let (mut num, mut den) = (0u32, 0u32);
    if SendMessageW(edit, EM_GETZOOM, &mut num as *mut _ as usize, &mut den as *mut _ as isize) != 0
        && num > 0
        && den > 0
    {
        ((num as u64 * 100 + den as u64 / 2) / den as u64).clamp(10, 500) as u32
    } else {
        100
    }
}

unsafe fn layout(hwnd: HWND) {
    let (edit, st, dpi) = APP.with_borrow(|a| (a.edit, a.settings, a.dpi));
    if edit.is_null() {
        return;
    }
    let mut rc: RECT = zeroed();
    GetClientRect(hwnd, &mut rc);
    let sh = status_h(st, dpi);
    let eh = (rc.bottom - sh).max(0);
    MoveWindow(edit, 0, 0, rc.right, eh, 1);
    let sr = RECT { left: 0, top: eh, right: rc.right, bottom: rc.bottom };
    InvalidateRect(hwnd, &sr, 0);
}

unsafe fn update_title(hwnd: HWND) {
    let (name, modified) = APP.with_borrow(|a| {
        let n = a
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                t!("Безымянный", "Untitled", ja: "無題", pt: "Sem título").to_string()
            });
        (n, a.modified)
    });
    let mark = if modified { "*" } else { "" };
    let app = t!("Блокнот", "Notepad", ja: "メモ帳", pt: "Bloco de Notas");
    let title = wide(&format!("{mark}{name} — {app}"));
    SetWindowTextW(hwnd, title.as_ptr());
}

unsafe fn update_caret(hwnd: HWND) {
    let edit = APP.with_borrow(|a| a.edit);
    if edit.is_null() {
        return;
    }
    let mut sel = CHARRANGE::default();
    SendMessageW(edit, EM_EXGETSEL, 0, &mut sel as *mut _ as isize);
    let line = SendMessageW(edit, EM_EXLINEFROMCHAR, 0, sel.cpMax as isize).max(0) as usize;
    let line_start = SendMessageW(edit, EM_LINEINDEX, line, 0).max(0) as usize;
    let col = (sel.cpMax.max(0) as usize).saturating_sub(line_start);
    let zoom = read_zoom(edit);
    let changed = APP.with_borrow_mut(|a| {
        let c = a.line != line + 1 || a.col != col + 1 || a.settings.zoom != zoom;
        a.line = line + 1;
        a.col = col + 1;
        a.settings.zoom = zoom;
        c
    });
    if changed {
        invalidate_status(hwnd);
    }
}

unsafe fn invalidate_status(hwnd: HWND) {
    let (st, dpi) = APP.with_borrow(|a| (a.settings, a.dpi));
    if !st.status {
        return;
    }
    let mut rc: RECT = zeroed();
    GetClientRect(hwnd, &mut rc);
    let sr = RECT { left: 0, top: (rc.bottom - status_h(st, dpi)).max(0), right: rc.right, bottom: rc.bottom };
    InvalidateRect(hwnd, &sr, 0);
}

unsafe fn show_note(hwnd: HWND, msg: impl Into<String>, sticky: bool) {
    APP.with_borrow_mut(|a| {
        a.note = msg.into();
        a.note_since = Some(Instant::now());
        a.note_sticky = sticky;
    });
    if !sticky {
        frames::start(hwnd, TIMER_NOTE);
    } else {
        frames::stop(hwnd, TIMER_NOTE);
    }
    invalidate_status(hwnd);
}

unsafe fn clear_note(hwnd: HWND) {
    APP.with_borrow_mut(|a| {
        a.note.clear();
        a.note_since = None;
        a.note_sticky = false;
    });
    frames::stop(hwnd, TIMER_NOTE);
    invalidate_status(hwnd);
}

unsafe fn build_menus(hwnd: HWND) {
    let bar = CreateMenu();

    let file = CreatePopupMenu();
    let recent = CreatePopupMenu();
    AppendMenuW(file, MF_STRING, ID_NEW as usize, wide(t!("Создать\tCtrl+N", "New\tCtrl+N", ja: "新規\tCtrl+N", pt: "Novo\tCtrl+N")).as_ptr());
    AppendMenuW(file, MF_STRING, ID_NEW_WIN as usize, wide(t!("Новое окно\tCtrl+Shift+N", "New window\tCtrl+Shift+N", ja: "新しいウィンドウ\tCtrl+Shift+N", pt: "Nova janela\tCtrl+Shift+N")).as_ptr());
    AppendMenuW(file, MF_STRING, ID_OPEN as usize, wide(t!("Открыть...\tCtrl+O", "Open...\tCtrl+O", ja: "開く...\tCtrl+O", pt: "Abrir...\tCtrl+O")).as_ptr());
    AppendMenuW(file, MF_STRING, ID_SAVE as usize, wide(t!("Сохранить\tCtrl+S", "Save\tCtrl+S", ja: "保存\tCtrl+S", pt: "Salvar\tCtrl+S")).as_ptr());
    AppendMenuW(file, MF_STRING, ID_SAVE_AS as usize, wide(t!("Сохранить как...\tCtrl+Shift+S", "Save as...\tCtrl+Shift+S", ja: "名前を付けて保存...\tCtrl+Shift+S", pt: "Salvar como...\tCtrl+Shift+S")).as_ptr());
    AppendMenuW(file, MF_POPUP, recent as usize, wide(t!("Недавние", "Recent", ja: "最近使ったファイル", pt: "Recentes")).as_ptr());
    AppendMenuW(file, MF_SEPARATOR, 0, null());
    AppendMenuW(file, MF_STRING, ID_EXIT as usize, wide(t!("Выход\tAlt+F4", "Exit\tAlt+F4", ja: "終了\tAlt+F4", pt: "Sair\tAlt+F4")).as_ptr());
    AppendMenuW(bar, MF_POPUP, file as usize, wide(t!("Файл", "File", ja: "ファイル", pt: "Arquivo")).as_ptr());

    let edit = CreatePopupMenu();
    AppendMenuW(edit, MF_STRING, ID_UNDO as usize, wide(t!("Отменить\tCtrl+Z", "Undo\tCtrl+Z", ja: "元に戻す\tCtrl+Z", pt: "Desfazer\tCtrl+Z")).as_ptr());
    AppendMenuW(edit, MF_STRING, ID_REDO as usize, wide(t!("Повторить\tCtrl+Y", "Redo\tCtrl+Y", ja: "やり直し\tCtrl+Y", pt: "Refazer\tCtrl+Y")).as_ptr());
    AppendMenuW(edit, MF_SEPARATOR, 0, null());
    AppendMenuW(edit, MF_STRING, ID_CUT as usize, wide(t!("Вырезать\tCtrl+X", "Cut\tCtrl+X", ja: "切り取り\tCtrl+X", pt: "Recortar\tCtrl+X")).as_ptr());
    AppendMenuW(edit, MF_STRING, ID_COPY as usize, wide(t!("Копировать\tCtrl+C", "Copy\tCtrl+C", ja: "コピー\tCtrl+C", pt: "Copiar\tCtrl+C")).as_ptr());
    AppendMenuW(edit, MF_STRING, ID_PASTE as usize, wide(t!("Вставить\tCtrl+V", "Paste\tCtrl+V", ja: "貼り付け\tCtrl+V", pt: "Colar\tCtrl+V")).as_ptr());
    AppendMenuW(edit, MF_STRING, ID_DELETE as usize, wide(t!("Удалить\tDel", "Delete\tDel", ja: "削除\tDel", pt: "Excluir\tDel")).as_ptr());
    AppendMenuW(edit, MF_SEPARATOR, 0, null());
    AppendMenuW(edit, MF_STRING, ID_FIND as usize, wide(t!("Найти...\tCtrl+F", "Find...\tCtrl+F", ja: "検索...\tCtrl+F", pt: "Localizar...\tCtrl+F")).as_ptr());
    AppendMenuW(edit, MF_STRING, ID_FIND_NEXT as usize, wide(t!("Найти далее\tF3", "Find next\tF3", ja: "次を検索\tF3", pt: "Localizar próxima\tF3")).as_ptr());
    AppendMenuW(edit, MF_STRING, ID_FIND_PREV as usize, wide(t!("Найти ранее\tShift+F3", "Find previous\tShift+F3", ja: "前を検索\tShift+F3", pt: "Localizar anterior\tShift+F3")).as_ptr());
    AppendMenuW(edit, MF_STRING, ID_REPLACE as usize, wide(t!("Заменить...\tCtrl+H", "Replace...\tCtrl+H", ja: "置換...\tCtrl+H", pt: "Substituir...\tCtrl+H")).as_ptr());
    AppendMenuW(edit, MF_SEPARATOR, 0, null());
    AppendMenuW(edit, MF_STRING, ID_SELECT_ALL as usize, wide(t!("Выделить всё\tCtrl+A", "Select all\tCtrl+A", ja: "すべて選択\tCtrl+A", pt: "Selecionar tudo\tCtrl+A")).as_ptr());
    AppendMenuW(bar, MF_POPUP, edit as usize, wide(t!("Правка", "Edit", ja: "編集", pt: "Editar")).as_ptr());

    let fmt = CreatePopupMenu();
    let enc_m = CreatePopupMenu();
    for (i, e) in ENCODINGS.iter().enumerate() {
        AppendMenuW(enc_m, MF_STRING, (ID_ENC_BASE + i as u16) as usize, wide(e.name()).as_ptr());
    }
    let eol_m = CreatePopupMenu();
    for (i, e) in EOLS.iter().enumerate() {
        AppendMenuW(eol_m, MF_STRING, (ID_EOL_BASE + i as u16) as usize, wide(e.name()).as_ptr());
    }
    AppendMenuW(fmt, MF_STRING, ID_WRAP as usize, wide(t!("Перенос по словам", "Word wrap", ja: "右端で折り返す", pt: "Quebra automática de linha")).as_ptr());
    AppendMenuW(fmt, MF_POPUP, enc_m as usize, wide(t!("Кодировка", "Encoding", ja: "エンコーディング", pt: "Codificação")).as_ptr());
    AppendMenuW(fmt, MF_POPUP, eol_m as usize, wide(t!("Концы строк", "Line endings", ja: "改行コード", pt: "Quebras de linha")).as_ptr());
    AppendMenuW(bar, MF_POPUP, fmt as usize, wide(t!("Формат", "Format", ja: "書式", pt: "Formatar")).as_ptr());

    let view = CreatePopupMenu();
    AppendMenuW(view, MF_STRING, ID_ZOOM_IN as usize, wide(t!("Увеличить\tCtrl++", "Zoom in\tCtrl++", ja: "拡大\tCtrl++", pt: "Ampliar\tCtrl++")).as_ptr());
    AppendMenuW(view, MF_STRING, ID_ZOOM_OUT as usize, wide(t!("Уменьшить\tCtrl+-", "Zoom out\tCtrl+-", ja: "縮小\tCtrl+-", pt: "Reduzir\tCtrl+-")).as_ptr());
    AppendMenuW(view, MF_STRING, ID_ZOOM_RESET as usize, wide(t!("100%\tCtrl+0", "100%\tCtrl+0", ja: "100%\tCtrl+0", pt: "100%\tCtrl+0")).as_ptr());
    AppendMenuW(view, MF_SEPARATOR, 0, null());
    AppendMenuW(view, MF_STRING, ID_STATUS as usize, wide(t!("Строка состояния", "Status bar", ja: "ステータス バー", pt: "Barra de status")).as_ptr());
    AppendMenuW(bar, MF_POPUP, view as usize, wide(t!("Вид", "View", ja: "表示", pt: "Exibir")).as_ptr());

    SetMenu(hwnd, bar);
    APP.with_borrow_mut(|a| {
        a.menu_recent = recent;
        a.menu_format = fmt;
        a.menu_view = view;
    });
    sync_menus();
}

unsafe fn sync_menus() {
    let (rec_m, fmt_m, view_m, rec, st, enc, eol) = APP.with_borrow(|a| {
        (a.menu_recent, a.menu_format, a.menu_view, a.recent.clone(), a.settings, a.enc, a.eol)
    });
    if !fmt_m.is_null() {
        CheckMenuItem(fmt_m, ID_WRAP as u32, MF_BYCOMMAND | if st.wrap { MF_CHECKED } else { MF_UNCHECKED });
        for (i, e) in ENCODINGS.iter().enumerate() {
            CheckMenuItem(fmt_m, (ID_ENC_BASE + i as u16) as u32, MF_BYCOMMAND | if *e == enc { MF_CHECKED } else { MF_UNCHECKED });
        }
        for (i, e) in EOLS.iter().enumerate() {
            CheckMenuItem(fmt_m, (ID_EOL_BASE + i as u16) as u32, MF_BYCOMMAND | if *e == eol { MF_CHECKED } else { MF_UNCHECKED });
        }
    }
    if !view_m.is_null() {
        CheckMenuItem(view_m, ID_STATUS as u32, MF_BYCOMMAND | if st.status { MF_CHECKED } else { MF_UNCHECKED });
    }
    if !rec_m.is_null() {
        while GetMenuItemCount(rec_m) > 0 {
            DeleteMenu(rec_m, 0, MF_BYPOSITION);
        }
        if rec.is_empty() {
            AppendMenuW(rec_m, MF_STRING | MF_GRAYED, 0, wide(t!("(пусто)", "(empty)", ja: "(なし)", pt: "(vazio)")).as_ptr());
        } else {
            for (i, path) in rec.iter().take(RECENT_MAX).enumerate() {
                // Escape '&' so menu doesn't treat it as mnemonic
                let label = format!("&{} {}", (i + 1) % 10, path.replace('&', "&&"));
                AppendMenuW(rec_m, MF_STRING, (ID_RECENT_BASE + i as u16) as usize, wide(&label).as_ptr());
            }
        }
    }
}

unsafe fn make_accel() -> HACCEL {
    let mut a = [
        ACCEL { fVirt: (FVIRTKEY | FCONTROL) as u8, key: b'N' as u16, cmd: ID_NEW },
        ACCEL { fVirt: (FVIRTKEY | FCONTROL | FSHIFT) as u8, key: b'N' as u16, cmd: ID_NEW_WIN },
        ACCEL { fVirt: (FVIRTKEY | FCONTROL) as u8, key: b'O' as u16, cmd: ID_OPEN },
        ACCEL { fVirt: (FVIRTKEY | FCONTROL) as u8, key: b'S' as u16, cmd: ID_SAVE },
        ACCEL { fVirt: (FVIRTKEY | FCONTROL | FSHIFT) as u8, key: b'S' as u16, cmd: ID_SAVE_AS },
        ACCEL { fVirt: (FVIRTKEY | FCONTROL) as u8, key: b'F' as u16, cmd: ID_FIND },
        ACCEL { fVirt: FVIRTKEY as u8, key: VK_F3, cmd: ID_FIND_NEXT },
        ACCEL { fVirt: (FVIRTKEY | FSHIFT) as u8, key: VK_F3, cmd: ID_FIND_PREV },
        ACCEL { fVirt: (FVIRTKEY | FCONTROL) as u8, key: b'H' as u16, cmd: ID_REPLACE },
        ACCEL { fVirt: (FVIRTKEY | FCONTROL) as u8, key: VK_OEM_PLUS, cmd: ID_ZOOM_IN },
        ACCEL { fVirt: (FVIRTKEY | FCONTROL) as u8, key: VK_ADD, cmd: ID_ZOOM_IN },
        ACCEL { fVirt: (FVIRTKEY | FCONTROL) as u8, key: VK_OEM_MINUS, cmd: ID_ZOOM_OUT },
        ACCEL { fVirt: (FVIRTKEY | FCONTROL) as u8, key: VK_SUBTRACT, cmd: ID_ZOOM_OUT },
        ACCEL { fVirt: (FVIRTKEY | FCONTROL) as u8, key: b'0' as u16, cmd: ID_ZOOM_RESET },
    ];
    CreateAcceleratorTableW(a.as_mut_ptr(), a.len() as i32)
}

// ---------- File load & save ----------

unsafe fn start_load(hwnd: HWND, path: PathBuf) {
    let seq = LOAD_GEN.fetch_add(1, Relaxed) + 1;
    let edit = APP.with_borrow_mut(|a| {
        a.loading = true;
        a.load_gen = seq;
        a.stream_buf = None;
        a.stream_pos = 0;
        a.edit
    });
    if !edit.is_null() {
        SendMessageW(edit, EM_SETEVENTMASK, 0, 0);
        SendMessageW(edit, EM_SETREADONLY, 1, 0);
        SendMessageW(edit, EM_SETUNDOLIMIT, 0, 0);
        SetWindowTextW(edit, wide("").as_ptr());
    }
    show_note(hwnd, t!("Загрузка...", "Loading...", ja: "読み込み中...", pt: "Carregando..."), true);
    let h = hwnd as isize;
    std::thread::spawn(move || {
        let res = std::fs::read(&path)
            .map(|bytes| {
                let enc = text::detect(&bytes);
                let dec = text::decode(&bytes, enc);
                let eol = text::detect_eol(&dec);
                let ed = text::to_editor(dec);
                (ed, enc, eol)
            })
            .map_err(|e| format!("{}: {e}", path.display()));
        let payload = Box::into_raw(Box::new(LoadResult { seq, path, res }));
        PostMessageW(h as HWND, WM_LOADED, 0, payload as isize);
    });
}

unsafe fn on_loaded(hwnd: HWND, lparam: isize) {
    let res = *Box::from_raw(lparam as *mut LoadResult);
    let (cur_gen, edit) = APP.with_borrow(|a| (a.load_gen, a.edit));
    if res.seq != cur_gen {
        return;
    }
    match res.res {
        Err(err) => {
            // start_load already emptied the editor: forget the previous file too, or Save would write the empty text
            // over it
            APP.with_borrow_mut(|a| {
                a.loading = false;
                a.path = None;
                a.enc = Encoding::Utf8;
                a.eol = Eol::Crlf;
                a.modified = false;
                a.stream_buf = None;
            });
            finish_edit_unlock(edit);
            clear_note(hwnd);
            sync_menus();
            update_title(hwnd);
            MessageBoxW(hwnd, wide(&err).as_ptr(), wide(t!("Ошибка", "Error", ja: "エラー", pt: "Erro")).as_ptr(), MB_ICONERROR);
        }
        Ok((units, enc, eol)) => {
            let rec_copy = APP.with_borrow_mut(|a| {
                a.path = Some(res.path.clone());
                a.enc = enc;
                a.eol = eol;
                a.modified = false;
                a.stream_buf = Some(units);
                a.stream_pos = 0;
                text::push_recent(&mut a.recent, &res.path.to_string_lossy());
                a.recent.clone()
            });
            save_recent(&rec_copy);
            sync_menus();
            update_title(hwnd);
            PostMessageW(hwnd, WM_CHUNK, res.seq as usize, 0);
        }
    }
}

unsafe fn on_chunk(hwnd: HWND, wparam: usize) {
    let seq = wparam as u32;
    let taken = APP.with_borrow_mut(|a| {
        if a.load_gen != seq {
            return None;
        }
        let buf = a.stream_buf.take()?;
        Some((a.edit, buf, a.stream_pos))
    });
    let Some((edit, buf, pos)) = taken else {
        return;
    };
    let total = buf.len();
    let mut end = (pos + CHUNK_UNITS).min(total);
    // Do not split a UTF-16 surrogate pair across chunks
    if end < total && (0xD800..=0xDBFF).contains(&buf[end - 1]) {
        end -= 1;
    }
    if end > pos {
        stream_in(edit, &buf[pos..end], pos > 0);
    }
    if end < total {
        let pct = (end as u64 * 100) / total.max(1) as u64;
        APP.with_borrow_mut(|a| {
            if a.load_gen == seq {
                a.stream_buf = Some(buf);
                a.stream_pos = end;
            }
        });
        show_note(hwnd, tf!("Загрузка... {}%", "Loading... {}%", ja: "読み込み中... {}%", pt: "Carregando... {}%", pct), true);
        PostMessageW(hwnd, WM_CHUNK, seq as usize, 0);
    } else {
        drop(buf);
        let (color, zoom, wrap) = APP.with_borrow_mut(|a| {
            a.loading = false;
            a.fresh = true;
            a.modified = false;
            (style::pal().text, a.settings.zoom, a.settings.wrap)
        });
        apply_char_format(edit, color);
        apply_wrap(edit, wrap);
        apply_zoom(edit, zoom);
        finish_edit_unlock(edit);
        let zero = CHARRANGE { cpMin: 0, cpMax: 0 };
        SendMessageW(edit, EM_EXSETSEL, 0, &zero as *const _ as isize);
        clear_note(hwnd);
        update_caret(hwnd);
        update_title(hwnd);
        invalidate_status(hwnd);
    }
}

unsafe fn finish_edit_unlock(edit: HWND) {
    if edit.is_null() {
        return;
    }
    SendMessageW(edit, EM_SETUNDOLIMIT, 100, 0);
    SendMessageW(edit, EM_EMPTYUNDOBUFFER, 0, 0);
    SendMessageW(edit, EM_SETMODIFY, 0, 0);
    SendMessageW(edit, EM_SETREADONLY, 0, 0);
    SendMessageW(
        edit,
        EM_SETEVENTMASK,
        0,
        (ENM_CHANGE | ENM_SELCHANGE | ENM_DROPFILES) as isize,
    );
}

unsafe fn save_async(hwnd: HWND, path: PathBuf) {
    let (edit, enc, eol, busy) = APP.with_borrow(|a| (a.edit, a.enc, a.eol, a.saving || a.loading));
    if busy || edit.is_null() {
        return;
    }
    let units = read_editor_text(edit);
    APP.with_borrow_mut(|a| a.saving = true);
    show_note(hwnd, t!("Сохранение...", "Saving...", ja: "保存中...", pt: "Salvando..."), true);
    let h = hwnd as isize;
    std::thread::spawn(move || {
        let out_u = text::from_editor(&units, eol);
        let (bytes, lossy) = text::encode(&out_u, enc);
        let res = std::fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()));
        let payload = Box::into_raw(Box::new(SaveResult { path, lossy, res }));
        PostMessageW(h as HWND, WM_SAVED, 0, payload as isize);
    });
}

unsafe fn on_saved(hwnd: HWND, lparam: isize) {
    let r = *Box::from_raw(lparam as *mut SaveResult);
    let (edit, close) = APP.with_borrow_mut(|a| {
        a.saving = false;
        (a.edit, a.close_after_save)
    });
    match r.res {
        Ok(()) => {
            if !edit.is_null() {
                SendMessageW(edit, EM_SETMODIFY, 0, 0);
            }
            let rec_copy = APP.with_borrow_mut(|a| {
                a.path = Some(r.path.clone());
                a.modified = false;
                text::push_recent(&mut a.recent, &r.path.to_string_lossy());
                a.recent.clone()
            });
            save_recent(&rec_copy);
            sync_menus();
            update_title(hwnd);
            if close {
                DestroyWindow(hwnd);
                return;
            }
            let note = if r.lossy {
                t!("Сохранено (часть символов заменена)", "Saved (some characters replaced)", ja: "保存しました (一部の文字を置換)", pt: "Salvo (alguns caracteres substituídos)")
            } else {
                t!("Сохранено", "Saved", ja: "保存しました", pt: "Salvo")
            };
            show_note(hwnd, note, false);
        }
        Err(err) => {
            APP.with_borrow_mut(|a| a.close_after_save = false);
            clear_note(hwnd);
            MessageBoxW(hwnd, wide(&err).as_ptr(), wide(t!("Ошибка", "Error", ja: "エラー", pt: "Erro")).as_ptr(), MB_ICONERROR);
        }
    }
}

unsafe fn save_sync(hwnd: HWND, path: &Path) -> bool {
    let (edit, enc, eol) = APP.with_borrow(|a| (a.edit, a.enc, a.eol));
    if edit.is_null() {
        return false;
    }
    let units = read_editor_text(edit);
    let out_u = text::from_editor(&units, eol);
    let (bytes, _) = text::encode(&out_u, enc);
    match std::fs::write(path, &bytes) {
        Ok(()) => {
            SendMessageW(edit, EM_SETMODIFY, 0, 0);
            let rec_copy = APP.with_borrow_mut(|a| {
                a.path = Some(path.to_path_buf());
                a.modified = false;
                text::push_recent(&mut a.recent, &path.to_string_lossy());
                a.recent.clone()
            });
            save_recent(&rec_copy);
            sync_menus();
            update_title(hwnd);
            true
        }
        Err(e) => {
            let err = format!("{}: {e}", path.display());
            MessageBoxW(hwnd, wide(&err).as_ptr(), wide(t!("Ошибка", "Error", ja: "エラー", pt: "Erro")).as_ptr(), MB_ICONERROR);
            false
        }
    }
}

unsafe fn file_dialog(hwnd: HWND, save: bool) -> Option<PathBuf> {
    let mut buf = [0u16; 1024];
    if save {
        if let Some(p) = APP.with_borrow(|a| a.path.clone()) {
            for (d, s) in buf.iter_mut().zip(p.to_string_lossy().encode_utf16()) {
                *d = s;
            }
        }
    }
    let filter_str = t!(
        "Текстовые файлы (*.txt)\0*.txt\0Все файлы (*.*)\0*.*\0\0",
        "Text files (*.txt)\0*.txt\0All files (*.*)\0*.*\0\0",
        ja: "テキスト ファイル (*.txt)\0*.txt\0すべてのファイル (*.*)\0*.*\0\0",
        pt: "Arquivos de texto (*.txt)\0*.txt\0Todos os arquivos (*.*)\0*.*\0\0"
    );
    let filter: Vec<u16> = filter_str.encode_utf16().collect();
    let def_ext = wide("txt");
    let mut ofn: OPENFILENAMEW = zeroed();
    ofn.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = hwnd;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = buf.as_mut_ptr();
    ofn.nMaxFile = buf.len() as u32;
    ofn.lpstrDefExt = def_ext.as_ptr();
    ofn.Flags = if save {
        OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST
    } else {
        OFN_FILEMUSTEXIST | OFN_HIDEREADONLY
    };
    let ok = if save { GetSaveFileNameW(&mut ofn) } else { GetOpenFileNameW(&mut ofn) };
    (ok != 0).then(|| PathBuf::from(from_wide_z(&buf)))
}

unsafe fn confirm_discard(hwnd: HWND) -> bool {
    let (modified, path) = APP.with_borrow(|a| (a.modified, a.path.clone()));
    if !modified {
        return true;
    }
    let prompt = t!(
        "Сохранить изменения?",
        "Save changes?",
        ja: "変更を保存しますか？",
        pt: "Salvar alterações?"
    );
    let title = t!("Блокнот", "Notepad", ja: "メモ帳", pt: "Bloco de Notas");
    match MessageBoxW(hwnd, wide(prompt).as_ptr(), wide(title).as_ptr(), MB_YESNOCANCEL | MB_ICONWARNING) {
        IDYES => match path.or_else(|| file_dialog(hwnd, true)) {
            Some(p) => save_sync(hwnd, &p),
            None => false,
        },
        IDNO => true,
        _ => false,
    }
}

unsafe fn cmd_new(hwnd: HWND) {
    if !confirm_discard(hwnd) {
        return;
    }
    LOAD_GEN.fetch_add(1, Relaxed);
    let edit = APP.with_borrow_mut(|a| {
        a.path = None;
        a.enc = Encoding::Utf8;
        a.eol = Eol::Crlf;
        a.modified = false;
        a.loading = false;
        a.stream_buf = None;
        a.edit
    });
    if !edit.is_null() {
        SendMessageW(edit, EM_SETEVENTMASK, 0, 0);
        SetWindowTextW(edit, wide("").as_ptr());
        finish_edit_unlock(edit);
    }
    clear_note(hwnd);
    sync_menus();
    update_title(hwnd);
    update_caret(hwnd);
    invalidate_status(hwnd);
}

// ---------- Find & Replace ----------

unsafe fn ensure_find_state() -> (*mut FINDREPLACEW, *mut [u16; 256], *mut [u16; 256]) {
    APP.with_borrow_mut(|a| {
        let fs = a.find_state.get_or_insert_with(|| FindState {
            // Leaked once per process: comdlg keeps the pointer for the lifetime of the modeless dialog.
            fr: Box::into_raw(Box::new(zeroed::<FINDREPLACEW>())),
            find_buf: Box::into_raw(Box::new([0u16; 256])),
            repl_buf: Box::into_raw(Box::new([0u16; 256])),
        });
        (fs.fr, fs.find_buf, fs.repl_buf)
    })
}

unsafe fn open_find_dialog(hwnd: HWND, replace: bool) {
    let old = APP.with_borrow_mut(|a| std::mem::replace(&mut a.find_dlg, null_mut()));
    if !old.is_null() {
        DestroyWindow(old);
    }
    let (fr, find_buf, repl_buf) = ensure_find_state();
    // Pre-fill with selected text if short and single-line
    let edit = APP.with_borrow(|a| a.edit);
    let mut sel = CHARRANGE::default();
    SendMessageW(edit, EM_EXGETSEL, 0, &mut sel as *mut _ as isize);
    let sel_len = (sel.cpMax - sel.cpMin).max(0) as usize;
    if sel_len > 0 && sel_len < 200 {
        let mut tmp = vec![0u16; sel_len + 2];
        let gt = GETTEXTEX {
            cb: (tmp.len() * 2) as u32,
            flags: 2, // GT_SELECTION
            codepage: CP_UNICODE,
            lpDefaultChar: null(),
            lpUsedDefChar: null_mut(),
        };
        let n = SendMessageW(edit, EM_GETTEXTEX, &gt as *const _ as usize, tmp.as_mut_ptr() as isize).max(0) as usize;
        let s = &tmp[..n.min(tmp.len())];
        if !s.contains(&13) && !s.contains(&10) {
            let fb = &mut *find_buf;
            fb.fill(0);
            for (d, &c) in fb.iter_mut().take(255).zip(s.iter()) {
                *d = c;
            }
        }
    }
    let flags = APP.with_borrow(|a| a.last_flags);
    *fr = zeroed();
    (*fr).lStructSize = size_of::<FINDREPLACEW>() as u32;
    (*fr).hwndOwner = hwnd;
    (*fr).Flags = flags;
    (*fr).lpstrFindWhat = (*find_buf).as_mut_ptr();
    (*fr).wFindWhatLen = 256;
    (*fr).lpstrReplaceWith = (*repl_buf).as_mut_ptr();
    (*fr).wReplaceWithLen = 256;
    let dlg = if replace { ReplaceTextW(fr) } else { FindTextW(fr) };
    APP.with_borrow_mut(|a| a.find_dlg = dlg);
}

unsafe fn find_in_edit(edit: HWND, needle: &[u16], flags: u32, from: i32, to: i32) -> Option<CHARRANGE> {
    let mut ft = FINDTEXTEXW {
        chrg: CHARRANGE { cpMin: from, cpMax: to },
        lpstrText: needle.as_ptr(),
        chrgText: CHARRANGE::default(),
    };
    let pos = SendMessageW(edit, EM_FINDTEXTEXW, flags as usize, &mut ft as *mut _ as isize);
    (pos >= 0).then_some(ft.chrgText)
}

unsafe fn do_find(hwnd: HWND, down: bool) -> bool {
    let (edit, needle_str, base_flags) = APP.with_borrow(|a| (a.edit, a.last_find.clone(), a.last_flags));
    if edit.is_null() || needle_str.is_empty() {
        open_find_dialog(hwnd, false);
        return false;
    }
    let needle = wide(&needle_str);
    let flags = (base_flags & (FR_MATCHCASE | FR_WHOLEWORD)) | if down { FR_DOWN } else { 0 };
    let mut sel = CHARRANGE::default();
    SendMessageW(edit, EM_EXGETSEL, 0, &mut sel as *mut _ as isize);
    let found = if down {
        find_in_edit(edit, &needle, flags, sel.cpMax, -1)
            .or_else(|| find_in_edit(edit, &needle, flags, 0, sel.cpMax))
    } else {
        find_in_edit(edit, &needle, flags, sel.cpMin, 0)
            .or_else(|| find_in_edit(edit, &needle, flags, -1, sel.cpMin))
    };
    match found {
        Some(r) => {
            SendMessageW(edit, EM_EXSETSEL, 0, &r as *const _ as isize);
            SendMessageW(edit, EM_SCROLLCARET, 0, 0);
            update_caret(hwnd);
            true
        }
        None => {
            show_note(hwnd, t!("Не найдено", "Not found", ja: "見つかりません", pt: "Não encontrado"), false);
            false
        }
    }
}

unsafe fn on_find_msg(hwnd: HWND, lparam: isize) {
    let fr = &*(lparam as *const FINDREPLACEW);
    if fr.Flags & FR_DIALOGTERM != 0 {
        APP.with_borrow_mut(|a| a.find_dlg = null_mut());
        return;
    }
    let find_s = from_wide_z(std::slice::from_raw_parts(fr.lpstrFindWhat, fr.wFindWhatLen as usize));
    let repl_s = if !fr.lpstrReplaceWith.is_null() {
        from_wide_z(std::slice::from_raw_parts(fr.lpstrReplaceWith, fr.wReplaceWithLen as usize))
    } else {
        String::new()
    };
    let flags = fr.Flags & (FR_DOWN | FR_MATCHCASE | FR_WHOLEWORD);
    let edit = APP.with_borrow_mut(|a| {
        a.last_find = find_s.clone();
        a.last_flags = flags;
        a.edit
    });
    if fr.Flags & FR_FINDNEXT != 0 {
        do_find(hwnd, fr.Flags & FR_DOWN != 0);
    } else if fr.Flags & FR_REPLACE != 0 {
        // If current selection matches needle, replace it first, then advance
        let mut sel = CHARRANGE::default();
        SendMessageW(edit, EM_EXGETSEL, 0, &mut sel as *mut _ as isize);
        let needle = wide(&find_s);
        let search_flags = (flags & (FR_MATCHCASE | FR_WHOLEWORD)) | FR_DOWN;
        if sel.cpMax > sel.cpMin {
            if let Some(m) = find_in_edit(edit, &needle, search_flags, sel.cpMin, sel.cpMax) {
                if m.cpMin == sel.cpMin && m.cpMax == sel.cpMax {
                    let rep = wide(&repl_s);
                    SendMessageW(edit, EM_REPLACESEL, 1, rep.as_ptr() as isize);
                }
            }
        }
        do_find(hwnd, true);
    } else if fr.Flags & FR_REPLACEALL != 0 && !find_s.is_empty() {
        let needle = wide(&find_s);
        let rep = wide(&repl_s);
        let rep_units = (rep.len() - 1) as i32;
        let search_flags = (flags & (FR_MATCHCASE | FR_WHOLEWORD)) | FR_DOWN;
        SendMessageW(edit, WM_SETREDRAW, 0, 0);
        let mut pos = 0i32;
        let mut count = 0usize;
        while let Some(m) = find_in_edit(edit, &needle, search_flags, pos, -1) {
            SendMessageW(edit, EM_EXSETSEL, 0, &m as *const _ as isize);
            SendMessageW(edit, EM_REPLACESEL, 1, rep.as_ptr() as isize);
            pos = m.cpMin + rep_units;
            count += 1;
        }
        SendMessageW(edit, WM_SETREDRAW, 1, 0);
        InvalidateRect(edit, null(), 1);
        update_caret(hwnd);
        show_note(
            hwnd,
            tf!("Заменено: {}", "Replaced: {}", ja: "置換: {}", pt: "Substituído: {}", count),
            false,
        );
    }
}

// ---------- Status bar painting ----------

unsafe fn paint_status(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let dc = BeginPaint(hwnd, &mut ps);
    let (st, dpi, font, line, col, eol, enc, note, since, sticky) = APP.with_borrow(|a| {
        (a.settings, a.dpi, a.font, a.line, a.col, a.eol, a.enc, a.note.clone(), a.note_since, a.note_sticky)
    });
    if st.status {
        let mut rc: RECT = zeroed();
        GetClientRect(hwnd, &mut rc);
        let sh = status_h(st, dpi);
        let top = (rc.bottom - sh).max(0);
        let p = style::pal();
        let bg_br = CreateSolidBrush(style::card());
        let bar_rc = RECT { left: 0, top, right: rc.right, bottom: rc.bottom };
        FillRect(dc, &bar_rc, bg_br);
        DeleteObject(bg_br);

        let ln_br = CreateSolidBrush(p.line);
        let ln_rc = RECT { left: 0, top, right: rc.right, bottom: top + 1 };
        FillRect(dc, &ln_rc, ln_br);
        DeleteObject(ln_br);

        let of = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as i32);
        let pad = px(10, dpi);

        if !note.is_empty() {
            let alpha = if sticky {
                1.0
            } else {
                let elapsed = since.map_or(3.0, |t| t.elapsed().as_secs_f32());
                if elapsed < 1.5 { 1.0 } else { (1.0 - (elapsed - 1.5)).clamp(0.0, 1.0) }
            };
            if alpha > 0.01 {
                SetTextColor(dc, style::mix(style::card(), p.accent, alpha));
                let mut nr = RECT { left: pad, top: top + 1, right: rc.right / 2, bottom: rc.bottom };
                DrawTextW(dc, wide(&note).as_ptr(), -1, &mut nr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            }
        }

        let right_text = format!(
            "{}   |   {}%   |   {}   |   {}",
            text::position_text(line, col),
            st.zoom,
            eol.name(),
            enc.name()
        );
        SetTextColor(dc, p.dim);
        let mut rr = RECT { left: pad, top: top + 1, right: rc.right - pad, bottom: rc.bottom };
        DrawTextW(dc, wide(&right_text).as_ptr(), -1, &mut rr, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(dc, of);
    }
    EndPaint(hwnd, &ps);
}

// ---------- Command & message dispatch ----------

unsafe fn on_command(hwnd: HWND, id: u16, code: u16) {
    if code == EN_CHANGE as u16 {
        let edit = APP.with_borrow(|a| a.edit);
        const EM_CANUNDO: u32 = 0x00C6;
        if APP.with_borrow(|a| a.loading) {
            return; // streaming the file in: not an edit (the load ends with the flag cleared)
        }
        let mut m = SendMessageW(edit, EM_GETMODIFY, 0, 0) != 0;
        // RichEdit can report the loaded text as a change after the load already cleared the modified flag ("*name"
        // right after opening). A real edit always leaves something to undo: the load emptied the undo buffer.
        if m && APP.with_borrow(|a| a.fresh) {
            if SendMessageW(edit, EM_CANUNDO, 0, 0) == 0 {
                SendMessageW(edit, EM_SETMODIFY, 0, 0);
                m = false;
            } else {
                APP.with_borrow_mut(|a| a.fresh = false);
            }
        }
        let changed = APP.with_borrow_mut(|a| {
            let c = a.modified != m;
            a.modified = m;
            c
        });
        if changed {
            update_title(hwnd);
        }
        return;
    }
    let edit = APP.with_borrow(|a| a.edit);
    match id {
        ID_NEW => cmd_new(hwnd),
        ID_NEW_WIN => {
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).spawn();
            }
        }
        ID_OPEN => {
            if confirm_discard(hwnd) {
                if let Some(p) = file_dialog(hwnd, false) {
                    start_load(hwnd, p);
                }
            }
        }
        ID_SAVE => {
            let path = APP.with_borrow(|a| a.path.clone()).or_else(|| file_dialog(hwnd, true));
            if let Some(p) = path {
                save_async(hwnd, p);
            }
        }
        ID_SAVE_AS => {
            if let Some(p) = file_dialog(hwnd, true) {
                save_async(hwnd, p);
            }
        }
        ID_EXIT => {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
        ID_UNDO => {
            SendMessageW(edit, EM_UNDO, 0, 0);
        }
        ID_REDO => {
            SendMessageW(edit, EM_REDO, 0, 0);
        }
        ID_CUT => {
            SendMessageW(edit, WM_CUT, 0, 0);
        }
        ID_COPY => {
            SendMessageW(edit, WM_COPY, 0, 0);
        }
        ID_PASTE => {
            SendMessageW(edit, WM_PASTE, 0, 0);
        }
        ID_DELETE => {
            SendMessageW(edit, WM_CLEAR, 0, 0);
        }
        ID_FIND => open_find_dialog(hwnd, false),
        ID_FIND_NEXT => {
            do_find(hwnd, true);
        }
        ID_FIND_PREV => {
            do_find(hwnd, false);
        }
        ID_REPLACE => open_find_dialog(hwnd, true),
        ID_SELECT_ALL => {
            let all = CHARRANGE { cpMin: 0, cpMax: -1 };
            SendMessageW(edit, EM_EXSETSEL, 0, &all as *const _ as isize);
        }
        ID_WRAP => {
            let st = APP.with_borrow_mut(|a| {
                a.settings.wrap = !a.settings.wrap;
                a.settings
            });
            apply_wrap(edit, st.wrap);
            save_settings(st);
            sync_menus();
        }
        ID_ZOOM_IN | ID_ZOOM_OUT | ID_ZOOM_RESET => {
            let cur = read_zoom(edit);
            let next = match id {
                ID_ZOOM_IN => text::zoom_step(cur, true),
                ID_ZOOM_OUT => text::zoom_step(cur, false),
                _ => 100,
            };
            let st = APP.with_borrow_mut(|a| {
                a.settings.zoom = next;
                a.settings
            });
            apply_zoom(edit, next);
            save_settings(st);
            invalidate_status(hwnd);
        }
        ID_STATUS => {
            let st = APP.with_borrow_mut(|a| {
                a.settings.status = !a.settings.status;
                a.settings
            });
            save_settings(st);
            sync_menus();
            layout(hwnd);
        }
        x if (ID_ENC_BASE..ID_ENC_BASE + ENCODINGS.len() as u16).contains(&x) => {
            let enc = ENCODINGS[(x - ID_ENC_BASE) as usize];
            APP.with_borrow_mut(|a| {
                if a.enc != enc {
                    a.enc = enc;
                    a.modified = true;
                }
            });
            sync_menus();
            update_title(hwnd);
            invalidate_status(hwnd);
        }
        x if (ID_EOL_BASE..ID_EOL_BASE + EOLS.len() as u16).contains(&x) => {
            let eol = EOLS[(x - ID_EOL_BASE) as usize];
            APP.with_borrow_mut(|a| {
                if a.eol != eol {
                    a.eol = eol;
                    a.modified = true;
                }
            });
            sync_menus();
            update_title(hwnd);
            invalidate_status(hwnd);
        }
        x if (ID_RECENT_BASE..ID_RECENT_BASE + RECENT_MAX as u16).contains(&x) => {
            let p = APP.with_borrow(|a| a.recent.get((x - ID_RECENT_BASE) as usize).cloned());
            if let Some(p) = p {
                if confirm_discard(hwnd) {
                    start_load(hwnd, PathBuf::from(p));
                }
            }
        }
        _ => {}
    }
}

unsafe fn open_hdrop(hwnd: HWND, hdrop: HDROP) {
    let mut buf = [0u16; 1024];
    let n = DragQueryFileW(hdrop, 0, buf.as_mut_ptr(), buf.len() as u32);
    DragFinish(hdrop);
    if n > 0 && confirm_discard(hwnd) {
        start_load(hwnd, PathBuf::from(from_wide_z(&buf)));
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg != 0 && msg == FIND_MSG.load(Relaxed) {
        on_find_msg(hwnd, lparam);
        return 0;
    }
    if msg != 0 && msg == style::msg() {
        style::reload();
        apply_theme(hwnd);
        return 0;
    }
    match msg {
        WM_CREATE => {
            let dc = GetDC(hwnd);
            let dpi = GetDeviceCaps(dc, LOGPIXELSX as i32).max(96);
            ReleaseDC(hwnd, dc);
            let (st, rec) = load_config();
            let hi = GetModuleHandleW(null());
            let edit = CreateWindowExW(
                0,
                wide("RICHEDIT50W").as_ptr(),
                null(),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_VSCROLL
                    | WS_HSCROLL
                    | ES_MULTILINE as u32
                    | ES_AUTOVSCROLL as u32
                    | ES_AUTOHSCROLL as u32
                    | ES_NOHIDESEL as u32
                    | ES_WANTRETURN as u32,
                0,
                0,
                100,
                100,
                hwnd,
                1 as HMENU,
                hi,
                null(),
            );
            init_richedit(edit);
            APP.with_borrow_mut(|a| {
                a.edit = edit;
                a.dpi = dpi;
                a.settings = st;
                a.recent = rec;
            });
            build_menus(hwnd);
            apply_theme(hwnd);
            apply_wrap(edit, st.wrap);
            apply_zoom(edit, st.zoom);
            DragAcceptFiles(hwnd, 1);
            update_title(hwnd);
            0
        }
        WM_SIZE => {
            layout(hwnd);
            0
        }
        WM_SETFOCUS => {
            let edit = APP.with_borrow(|a| a.edit);
            if !edit.is_null() {
                SetFocus(edit);
            }
            0
        }
        WM_PAINT => {
            paint_status(hwnd);
            0
        }
        WM_COMMAND => {
            on_command(hwnd, (wparam & 0xFFFF) as u16, ((wparam >> 16) & 0xFFFF) as u16);
            0
        }
        WM_INITMENUPOPUP => {
            let edit = APP.with_borrow(|a| a.edit);
            if !edit.is_null() {
                let can_redo = SendMessageW(edit, EM_CANREDO, 0, 0) != 0;
                EnableMenuItem(wparam as HMENU, ID_REDO as u32, MF_BYCOMMAND | if can_redo { MF_ENABLED } else { MF_GRAYED });
            }
            0
        }
        WM_NOTIFY => {
            let nm = &*(lparam as *const NMHDR);
            if nm.code == EN_SELCHANGE {
                update_caret(hwnd);
            } else if nm.code == EN_DROPFILES {
                let df = &*(lparam as *const ENDROPFILES);
                open_hdrop(hwnd, df.hDrop);
                return 1; // handled, do not insert file object
            }
            0
        }
        WM_DROPFILES => {
            open_hdrop(hwnd, wparam as HDROP);
            0
        }
        WM_TIMER if wparam == TIMER_NOTE => {
            let done = APP.with_borrow(|a| {
                a.note_sticky || a.note_since.is_none_or(|t| t.elapsed().as_secs_f32() >= 2.5)
            });
            if done {
                clear_note(hwnd);
            } else {
                invalidate_status(hwnd);
            }
            0
        }
        WM_LOADED => {
            on_loaded(hwnd, lparam);
            0
        }
        WM_CHUNK => {
            on_chunk(hwnd, wparam);
            0
        }
        WM_SAVED => {
            on_saved(hwnd, lparam);
            0
        }
        WM_CLOSE => {
            let saving = APP.with_borrow_mut(|a| {
                if a.saving {
                    a.close_after_save = true;
                    true
                } else {
                    false
                }
            });
            if saving {
                return 0;
            }
            if confirm_discard(hwnd) {
                let st = APP.with_borrow(|a| a.settings);
                save_settings(st);
                frames::stop(hwnd, TIMER_NOTE);
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            frames::stop(hwnd, TIMER_NOTE);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn main() {
    unsafe {
        LoadLibraryW(wide("Msftedit.dll").as_ptr());
        FIND_MSG.store(RegisterWindowMessageW(wide("commdlg_FindReplace").as_ptr()), Relaxed);
        let hi = GetModuleHandleW(null());
        let cls = wide("WinRustNotepad");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hi,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: cls.as_ptr(),
            ..zeroed()
        };
        RegisterClassW(&wc);
        let dc = GetDC(null_mut());
        let dpi = GetDeviceCaps(dc, LOGPIXELSX as i32).max(96);
        ReleaseDC(null_mut(), dc);
        let hwnd = CreateWindowExW(
            0,
            cls.as_ptr(),
            wide(t!("Блокнот", "Notepad", ja: "メモ帳", pt: "Bloco de Notas")).as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            px(960, dpi),
            px(680, dpi),
            null_mut(),
            null_mut(),
            hi,
            null(),
        );
        if hwnd.is_null() {
            return;
        }
        let args: Vec<String> = std::env::args().skip(1).collect();
        if let Some(p) = text::file_arg(&args) {
            let path = PathBuf::from(p);
            if path.is_file() {
                start_load(hwnd, path);
            } else {
                APP.with_borrow_mut(|a| a.path = Some(path));
                update_title(hwnd);
            }
        }
        let accel = make_accel();
        let mut m: MSG = zeroed();
        while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
            let dlg = APP.with_borrow(|a| a.find_dlg);
            if !dlg.is_null() && IsDialogMessageW(dlg, &m) != 0 {
                continue;
            }
            if TranslateAcceleratorW(hwnd, accel, &m) == 0 {
                TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        }
        DestroyAcceleratorTable(accel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn richedit_stream_in_and_read_back_has_no_extra_trailing_cr() {
        unsafe {
            assert!(!LoadLibraryW(wide("Msftedit.dll").as_ptr()).is_null());
            let edit = CreateWindowExW(
                0,
                wide("RICHEDIT50W").as_ptr(),
                null(),
                WS_CHILD | ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | ES_AUTOHSCROLL as u32,
                0,
                0,
                200,
                100,
                HWND_MESSAGE,
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            assert!(!edit.is_null());
            init_richedit(edit);

            for sample in ["hello", "line1\rline2\rline3", "trailing\r", "Ёлка 🎄\rsecond"] {
                let u: Vec<u16> = sample.encode_utf16().collect();
                SetWindowTextW(edit, wide("").as_ptr());
                stream_in(edit, &u, false);
                assert_eq!(read_editor_text(edit), u, "single chunk mismatch for {sample:?}");
            }

            // Multi-chunk append test (as used for large files)
            SetWindowTextW(edit, wide("").as_ptr());
            let p1: Vec<u16> = "first\rsecond".encode_utf16().collect();
            let p2: Vec<u16> = "\rthird".encode_utf16().collect();
            stream_in(edit, &p1, false);
            stream_in(edit, &p2, true);
            let expected: Vec<u16> = "first\rsecond\rthird".encode_utf16().collect();
            assert_eq!(read_editor_text(edit), expected);

            DestroyWindow(edit);
        }
    }
}

#![windows_subsystem = "windows"]
#![allow(unsafe_op_in_unsafe_fn)]
//! rs-calc: a fast native Win32 calculator with Standard (f64) and Programmer (i64, hex/dec/oct/bin) modes,
//! history overlay, keyboard input, clipboard copy/paste, and animated custom-drawn buttons.
#[macro_use]
extern crate winrust_common;

mod calc;

use calc::{BASES, Base};
use std::{
    cell::RefCell,
    mem::zeroed,
    ptr::{null, null_mut},
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::DwmSetWindowAttribute, Gdi::*},
    System::{
        DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData},
        LibraryLoader::GetModuleHandleW,
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
    },
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};
use winrust_common::{frames, style};

const TIMER_ANIM: usize = 1;
const WM_MOUSELEAVE_MSG: u32 = 0x02A3;
const CF_UNICODETEXT: u32 = 13;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Std,
    Prog,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BtnKind {
    Digit,
    Op,
    Accent,
    Muted,
}

#[derive(Clone, Copy)]
struct BtnDef {
    label: &'static str,
    action: Action,
    kind: BtnKind,
    col: i32,
    row: i32,
    span: i32,
}

#[derive(Clone, Copy)]
enum Action {
    Ins(&'static str),
    Dot,
    Neg,
    Back,
    Clear,
    ClearEntry,
    Eval,
}

struct HistItem {
    expr: String,
    result: String,
    mode: Mode,
    base: Base,
}

struct App {
    mode: Mode,
    base: Base,
    expr: String,
    last_expr: String,
    result: String,
    prog_val: i64,
    err: Option<String>,
    just_evaled: bool,
    history: Vec<HistItem>,
    show_hist: bool,
    hover_btn: Option<usize>,
    hover_hist: Option<usize>,
    hover_base: Option<usize>,
    hover_tab: Option<usize>, // 0 = std, 1 = prog, 2 = hist
    btn_t: Vec<f32>,          // hover fade 0..1 per button
    last_tick: Instant,
    copied_until: Option<Instant>,
    tracking_mouse: bool,
    dpi: i32,
}

impl Default for App {
    fn default() -> Self {
        Self {
            mode: Mode::Std,
            base: Base::Dec,
            expr: String::new(),
            last_expr: String::new(),
            result: "0".into(),
            prog_val: 0,
            err: None,
            just_evaled: false,
            history: Vec::new(),
            show_hist: false,
            hover_btn: None,
            hover_hist: None,
            hover_base: None,
            hover_tab: None,
            btn_t: vec![0.0; 36],
            last_tick: Instant::now(),
            copied_until: None,
            tracking_mouse: false,
            dpi: 96,
        }
    }
}

thread_local! {
    static APP: RefCell<App> = RefCell::new(App::default());
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn px(v: i32, dpi: i32) -> i32 {
    (v * dpi + 48) / 96
}

const STD_BTNS: &[BtnDef] = &[
    BtnDef { label: "C", action: Action::Clear, kind: BtnKind::Muted, col: 0, row: 0, span: 1 },
    BtnDef { label: "⌫", action: Action::Back, kind: BtnKind::Muted, col: 1, row: 0, span: 1 },
    BtnDef { label: "(", action: Action::Ins("("), kind: BtnKind::Op, col: 2, row: 0, span: 1 },
    BtnDef { label: ")", action: Action::Ins(")"), kind: BtnKind::Op, col: 3, row: 0, span: 1 },
    BtnDef { label: "%", action: Action::Ins("%"), kind: BtnKind::Op, col: 0, row: 1, span: 1 },
    BtnDef { label: "√", action: Action::Ins("√"), kind: BtnKind::Op, col: 1, row: 1, span: 1 },
    BtnDef { label: "xʸ", action: Action::Ins("^"), kind: BtnKind::Op, col: 2, row: 1, span: 1 },
    BtnDef { label: "÷", action: Action::Ins("/"), kind: BtnKind::Op, col: 3, row: 1, span: 1 },
    BtnDef { label: "7", action: Action::Ins("7"), kind: BtnKind::Digit, col: 0, row: 2, span: 1 },
    BtnDef { label: "8", action: Action::Ins("8"), kind: BtnKind::Digit, col: 1, row: 2, span: 1 },
    BtnDef { label: "9", action: Action::Ins("9"), kind: BtnKind::Digit, col: 2, row: 2, span: 1 },
    BtnDef { label: "×", action: Action::Ins("*"), kind: BtnKind::Op, col: 3, row: 2, span: 1 },
    BtnDef { label: "4", action: Action::Ins("4"), kind: BtnKind::Digit, col: 0, row: 3, span: 1 },
    BtnDef { label: "5", action: Action::Ins("5"), kind: BtnKind::Digit, col: 1, row: 3, span: 1 },
    BtnDef { label: "6", action: Action::Ins("6"), kind: BtnKind::Digit, col: 2, row: 3, span: 1 },
    BtnDef { label: "−", action: Action::Ins("-"), kind: BtnKind::Op, col: 3, row: 3, span: 1 },
    BtnDef { label: "1", action: Action::Ins("1"), kind: BtnKind::Digit, col: 0, row: 4, span: 1 },
    BtnDef { label: "2", action: Action::Ins("2"), kind: BtnKind::Digit, col: 1, row: 4, span: 1 },
    BtnDef { label: "3", action: Action::Ins("3"), kind: BtnKind::Digit, col: 2, row: 4, span: 1 },
    BtnDef { label: "+", action: Action::Ins("+"), kind: BtnKind::Op, col: 3, row: 4, span: 1 },
    BtnDef { label: "±", action: Action::Neg, kind: BtnKind::Digit, col: 0, row: 5, span: 1 },
    BtnDef { label: "0", action: Action::Ins("0"), kind: BtnKind::Digit, col: 1, row: 5, span: 1 },
    BtnDef { label: ".", action: Action::Dot, kind: BtnKind::Digit, col: 2, row: 5, span: 1 },
    BtnDef { label: "=", action: Action::Eval, kind: BtnKind::Accent, col: 3, row: 5, span: 1 },
];

const PROG_BTNS: &[BtnDef] = &[
    BtnDef { label: "AND", action: Action::Ins(" AND "), kind: BtnKind::Op, col: 0, row: 0, span: 1 },
    BtnDef { label: "OR", action: Action::Ins(" OR "), kind: BtnKind::Op, col: 1, row: 0, span: 1 },
    BtnDef { label: "XOR", action: Action::Ins(" XOR "), kind: BtnKind::Op, col: 2, row: 0, span: 1 },
    BtnDef { label: "NOT", action: Action::Ins("NOT "), kind: BtnKind::Op, col: 3, row: 0, span: 1 },
    BtnDef { label: "MOD", action: Action::Ins(" MOD "), kind: BtnKind::Op, col: 4, row: 0, span: 1 },
    BtnDef { label: "A", action: Action::Ins("A"), kind: BtnKind::Digit, col: 0, row: 1, span: 1 },
    BtnDef { label: "<<", action: Action::Ins("<<"), kind: BtnKind::Op, col: 1, row: 1, span: 1 },
    BtnDef { label: ">>", action: Action::Ins(">>"), kind: BtnKind::Op, col: 2, row: 1, span: 1 },
    BtnDef { label: "AC", action: Action::Clear, kind: BtnKind::Muted, col: 3, row: 1, span: 1 },
    BtnDef { label: "⌫", action: Action::Back, kind: BtnKind::Muted, col: 4, row: 1, span: 1 },
    BtnDef { label: "B", action: Action::Ins("B"), kind: BtnKind::Digit, col: 0, row: 2, span: 1 },
    BtnDef { label: "(", action: Action::Ins("("), kind: BtnKind::Op, col: 1, row: 2, span: 1 },
    BtnDef { label: ")", action: Action::Ins(")"), kind: BtnKind::Op, col: 2, row: 2, span: 1 },
    BtnDef { label: "CE", action: Action::ClearEntry, kind: BtnKind::Muted, col: 3, row: 2, span: 1 },
    BtnDef { label: "÷", action: Action::Ins("/"), kind: BtnKind::Op, col: 4, row: 2, span: 1 },
    BtnDef { label: "C", action: Action::Ins("C"), kind: BtnKind::Digit, col: 0, row: 3, span: 1 },
    BtnDef { label: "7", action: Action::Ins("7"), kind: BtnKind::Digit, col: 1, row: 3, span: 1 },
    BtnDef { label: "8", action: Action::Ins("8"), kind: BtnKind::Digit, col: 2, row: 3, span: 1 },
    BtnDef { label: "9", action: Action::Ins("9"), kind: BtnKind::Digit, col: 3, row: 3, span: 1 },
    BtnDef { label: "×", action: Action::Ins("*"), kind: BtnKind::Op, col: 4, row: 3, span: 1 },
    BtnDef { label: "D", action: Action::Ins("D"), kind: BtnKind::Digit, col: 0, row: 4, span: 1 },
    BtnDef { label: "4", action: Action::Ins("4"), kind: BtnKind::Digit, col: 1, row: 4, span: 1 },
    BtnDef { label: "5", action: Action::Ins("5"), kind: BtnKind::Digit, col: 2, row: 4, span: 1 },
    BtnDef { label: "6", action: Action::Ins("6"), kind: BtnKind::Digit, col: 3, row: 4, span: 1 },
    BtnDef { label: "−", action: Action::Ins("-"), kind: BtnKind::Op, col: 4, row: 4, span: 1 },
    BtnDef { label: "E", action: Action::Ins("E"), kind: BtnKind::Digit, col: 0, row: 5, span: 1 },
    BtnDef { label: "1", action: Action::Ins("1"), kind: BtnKind::Digit, col: 1, row: 5, span: 1 },
    BtnDef { label: "2", action: Action::Ins("2"), kind: BtnKind::Digit, col: 2, row: 5, span: 1 },
    BtnDef { label: "3", action: Action::Ins("3"), kind: BtnKind::Digit, col: 3, row: 5, span: 1 },
    BtnDef { label: "+", action: Action::Ins("+"), kind: BtnKind::Op, col: 4, row: 5, span: 1 },
    BtnDef { label: "F", action: Action::Ins("F"), kind: BtnKind::Digit, col: 0, row: 6, span: 1 },
    BtnDef { label: "±", action: Action::Neg, kind: BtnKind::Digit, col: 1, row: 6, span: 1 },
    BtnDef { label: "0", action: Action::Ins("0"), kind: BtnKind::Digit, col: 2, row: 6, span: 2 },
    BtnDef { label: "=", action: Action::Eval, kind: BtnKind::Accent, col: 4, row: 6, span: 1 },
];

fn buttons(mode: Mode) -> (&'static [BtnDef], i32, i32) {
    match mode {
        Mode::Std => (STD_BTNS, 4, 6),
        Mode::Prog => (PROG_BTNS, 5, 7),
    }
}

fn btn_enabled(b: &BtnDef, mode: Mode, base: Base) -> bool {
    if mode == Mode::Std {
        return true;
    }
    if let Action::Ins(s) = b.action {
        if s.len() == 1 {
            let c = s.chars().next().unwrap();
            if c.is_ascii_hexdigit() {
                return base.is_digit(c);
            }
        }
    }
    true
}

// ---------- Layout geometry ----------

struct Layout {
    tabs: [RECT; 3],
    expr_rc: RECT,
    val_rc: RECT,
    bases: [RECT; 4],
    grid_rc: RECT,
    btns: Vec<RECT>,
    hist_clear_rc: RECT,
}

fn compute_layout(w: i32, h: i32, mode: Mode, dpi: i32) -> Layout {
    let pad = px(12, dpi);
    let gap = px(6, dpi);
    let top_h = px(32, dpi);
    let hist_w = px(36, dpi);
    let tab_w = ((w - pad * 2 - hist_w - gap * 2) / 2).max(40);
    let tabs = [
        RECT { left: pad, top: pad, right: pad + tab_w, bottom: pad + top_h },
        RECT { left: pad + tab_w + gap, top: pad, right: pad + tab_w * 2 + gap, bottom: pad + top_h },
        RECT { left: w - pad - hist_w, top: pad, right: w - pad, bottom: pad + top_h },
    ];
    let mut y = pad + top_h + px(8, dpi);
    let expr_h = px(22, dpi);
    let val_h = px(48, dpi);
    let expr_rc = RECT { left: pad, top: y, right: w - pad, bottom: y + expr_h };
    y += expr_h;
    let val_rc = RECT { left: pad, top: y, right: w - pad, bottom: y + val_h };
    y += val_h + px(6, dpi);

    let mut bases = [RECT { left: 0, top: 0, right: 0, bottom: 0 }; 4];
    if mode == Mode::Prog {
        let row_h = px(24, dpi);
        for r in &mut bases {
            *r = RECT { left: pad, top: y, right: w - pad, bottom: y + row_h };
            y += row_h + px(2, dpi);
        }
        y += px(4, dpi);
    }

    let grid_rc = RECT { left: pad, top: y, right: w - pad, bottom: h - pad };
    let (defs, cols, rows) = buttons(mode);
    let gw = (grid_rc.right - grid_rc.left).max(1);
    let gh = (grid_rc.bottom - grid_rc.top).max(1);
    let cw = (gw - gap * (cols - 1)) / cols;
    let ch = (gh - gap * (rows - 1)) / rows;
    let mut btns = Vec::with_capacity(defs.len());
    for b in defs {
        let x0 = grid_rc.left + b.col * (cw + gap);
        let y0 = grid_rc.top + b.row * (ch + gap);
        let x1 = if b.col + b.span == cols {
            grid_rc.right
        } else {
            x0 + b.span * cw + (b.span - 1) * gap
        };
        let y1 = if b.row + 1 == rows { grid_rc.bottom } else { y0 + ch };
        btns.push(RECT { left: x0, top: y0, right: x1, bottom: y1 });
    }
    let clear_h = px(32, dpi);
    let hist_clear_rc = RECT {
        left: grid_rc.right - px(100, dpi),
        top: grid_rc.bottom - clear_h - px(6, dpi),
        right: grid_rc.right - px(8, dpi),
        bottom: grid_rc.bottom - px(6, dpi),
    };
    Layout { tabs, expr_rc, val_rc, bases, grid_rc, btns, hist_clear_rc }
}

fn pt_in(r: RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

// ---------- Calculator state operations ----------

impl App {
    fn preview_eval(&mut self) {
        self.err = None;
        if self.expr.trim().is_empty() {
            self.result = "0".into();
            self.prog_val = 0;
            return;
        }
        match self.mode {
            Mode::Std => match calc::eval_std(&self.expr) {
                Ok(v) => self.result = calc::format_std(v),
                Err(_) => {} // Keep previous preview while typing an incomplete expression
            },
            Mode::Prog => match calc::eval_prog(&self.expr, self.base) {
                Ok(v) => {
                    self.prog_val = v;
                    self.result = calc::format_prog_grouped(v, self.base);
                }
                Err(_) => {}
            },
        }
    }

    fn apply_action(&mut self, act: Action) {
        self.err = None;
        match act {
            Action::Clear => {
                self.expr.clear();
                self.last_expr.clear();
                self.result = "0".into();
                self.prog_val = 0;
                self.just_evaled = false;
            }
            Action::ClearEntry => {
                self.expr.clear();
                self.just_evaled = false;
                self.preview_eval();
            }
            Action::Back => {
                if self.just_evaled {
                    self.just_evaled = false;
                }
                // Remove trailing keyword token cleanly
                let trimmed = self.expr.trim_end();
                let mut removed = false;
                for kw in ["AND", "OR", "XOR", "NOT", "MOD", "<<", ">>"] {
                    if trimmed.ends_with(kw) {
                        let new_len = trimmed.len() - kw.len();
                        self.expr.truncate(new_len);
                        self.expr = self.expr.trim_end().to_string();
                        removed = true;
                        break;
                    }
                }
                if !removed {
                    self.expr.pop();
                }
                self.preview_eval();
            }
            Action::Dot => {
                if self.mode == Mode::Std {
                    if self.just_evaled {
                        self.expr.clear();
                        self.just_evaled = false;
                    }
                    let sep = if dot!() { '.' } else { ',' };
                    if self.expr.is_empty() || !self.expr.chars().last().unwrap().is_ascii_digit() {
                        self.expr.push('0');
                    }
                    self.expr.push(sep);
                    self.preview_eval();
                }
            }
            Action::Neg => {
                if self.expr.is_empty() {
                    self.expr.push('-');
                } else if self.expr.starts_with('-') && self.just_evaled {
                    self.expr.remove(0);
                } else if self.just_evaled {
                    self.expr.insert(0, '-');
                } else {
                    self.expr = format!("-({})", self.expr);
                }
                self.just_evaled = false;
                self.preview_eval();
            }
            Action::Ins(s) => {
                let starts_op = s.trim_start().starts_with(|c: char| "+-*/^%&|<>".contains(c))
                    || matches!(s.trim(), "AND" | "OR" | "XOR" | "MOD");
                if self.just_evaled {
                    if !starts_op {
                        self.expr.clear();
                    }
                    self.just_evaled = false;
                }
                self.expr.push_str(s);
                self.preview_eval();
            }
            Action::Eval => {
                let e = self.expr.trim().to_string();
                if e.is_empty() {
                    return;
                }
                match self.mode {
                    Mode::Std => match calc::eval_std(&e) {
                        Ok(v) => {
                            let res = calc::format_std(v);
                            self.history.insert(0, HistItem { expr: e.clone(), result: res.clone(), mode: Mode::Std, base: self.base });
                            self.history.truncate(50);
                            self.last_expr = format!("{e} =");
                            self.result = res.clone();
                            self.expr = res;
                            self.just_evaled = true;
                        }
                        Err(msg) => self.err = Some(msg),
                    },
                    Mode::Prog => match calc::eval_prog(&e, self.base) {
                        Ok(v) => {
                            let disp = calc::format_prog_grouped(v, self.base);
                            let raw = calc::format_prog_raw(v, self.base);
                            self.history.insert(0, HistItem { expr: e.clone(), result: disp.clone(), mode: Mode::Prog, base: self.base });
                            self.history.truncate(50);
                            self.prog_val = v;
                            self.last_expr = format!("{e} =");
                            self.result = disp;
                            self.expr = raw;
                            self.just_evaled = true;
                        }
                        Err(msg) => self.err = Some(msg),
                    },
                }
            }
        }
    }

    fn set_mode(&mut self, mode: Mode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.expr.clear();
        self.last_expr.clear();
        self.result = "0".into();
        self.prog_val = 0;
        self.err = None;
        self.just_evaled = false;
        self.hover_btn = None;
        self.btn_t.fill(0.0);
    }

    fn set_base(&mut self, next: Base) {
        if self.base == next {
            return;
        }
        self.expr = calc::rebase_expr(&self.expr, self.base, next);
        self.base = next;
        self.err = None;
        self.preview_eval();
    }
}

// ---------- Clipboard ----------

unsafe fn copy_text(hwnd: HWND, s: &str) {
    if OpenClipboard(hwnd) == 0 {
        return;
    }
    EmptyClipboard();
    let u = wide(s);
    let h = GlobalAlloc(GMEM_MOVEABLE, u.len() * 2);
    if !h.is_null() {
        let ptr = GlobalLock(h) as *mut u16;
        if !ptr.is_null() {
            std::ptr::copy_nonoverlapping(u.as_ptr(), ptr, u.len());
            GlobalUnlock(h);
            SetClipboardData(CF_UNICODETEXT, h);
        }
    }
    CloseClipboard();
}

unsafe fn paste_text(hwnd: HWND) -> Option<String> {
    if OpenClipboard(hwnd) == 0 {
        return None;
    }
    let h = GetClipboardData(CF_UNICODETEXT);
    let res = if !h.is_null() {
        let ptr = GlobalLock(h) as *const u16;
        if !ptr.is_null() {
            let mut len = 0usize;
            while *ptr.add(len) != 0 && len < 4096 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            GlobalUnlock(h);
            Some(s)
        } else {
            None
        }
    } else {
        None
    };
    CloseClipboard();
    res
}

// ---------- Painting ----------

unsafe fn apply_dwm(hwnd: HWND) {
    let p = style::pal();
    let dark: i32 = if style::style().light() { 0 } else { 1 };
    DwmSetWindowAttribute(hwnd, 20, &dark as *const _ as *const _, 4);
    DwmSetWindowAttribute(hwnd, 35, &p.bg as *const _ as *const _, 4);
}

unsafe fn fill_round(dc: HDC, r: RECT, rad: i32, fill: u32, border: u32) {
    let br = CreateSolidBrush(fill);
    let pen = CreatePen(PS_SOLID as i32, 1, border);
    let ob = SelectObject(dc, br);
    let op = SelectObject(dc, pen);
    if rad <= 0 {
        Rectangle(dc, r.left, r.top, r.right, r.bottom);
    } else {
        RoundRect(dc, r.left, r.top, r.right, r.bottom, rad * 2, rad * 2);
    }
    SelectObject(dc, ob);
    SelectObject(dc, op);
    DeleteObject(br);
    DeleteObject(pen);
}

unsafe fn draw_text(dc: HDC, s: &str, r: RECT, size: i32, weight: i32, color: u32, flags: u32, dpi: i32) {
    let f = CreateFontW(
        -px(size, dpi),
        0,
        0,
        0,
        weight,
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
    let of = SelectObject(dc, f);
    SetTextColor(dc, color);
    SetBkMode(dc, TRANSPARENT as i32);
    let mut rc = r;
    DrawTextW(dc, wide(s).as_ptr(), -1, &mut rc, flags | DT_NOPREFIX);
    SelectObject(dc, of);
    DeleteObject(f);
}

unsafe fn paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc: RECT = zeroed();
    GetClientRect(hwnd, &mut rc);
    let (w, h) = (rc.right.max(1), rc.bottom.max(1));

    let mem = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, w, h);
    let old_bmp = SelectObject(mem, bmp);

    let p = style::pal();
    let rad = if style::style().square() { 0 } else { px(6, APP.with_borrow(|a| a.dpi)) };

    let bg_br = CreateSolidBrush(p.bg);
    FillRect(mem, &rc, bg_br);
    DeleteObject(bg_br);

    APP.with_borrow(|a| {
        let lay = compute_layout(w, h, a.mode, a.dpi);

        // Header tabs: Standard | Programmer | History
        let tab_labels = [
            t!("Обычный", "Standard", ja: "標準", pt: "Padrão"),
            t!("Программист", "Programmer", ja: "プログラマー", pt: "Programador"),
            "⏱",
        ];
        for (i, (&tr, &lbl)) in lay.tabs.iter().zip(&tab_labels).enumerate() {
            let active = (i == 0 && a.mode == Mode::Std)
                || (i == 1 && a.mode == Mode::Prog)
                || (i == 2 && a.show_hist);
            let hovered = a.hover_tab == Some(i);
            let fill = if active {
                p.active
            } else if hovered {
                p.hover
            } else {
                style::card()
            };
            let border = if active { p.accent } else { p.line };
            fill_round(mem, tr, rad, fill, border);
            let tc = if active { p.text } else { p.dim };
            draw_text(mem, lbl, tr, 12, if active { 600 } else { 400 }, tc, DT_CENTER | DT_VCENTER | DT_SINGLELINE, a.dpi);
        }

        // Secondary expression line (or "Copied" badge)
        let top_line = if a.copied_until.is_some_and(|t| Instant::now() < t) {
            t!("Скопировано", "Copied", ja: "コピーしました", pt: "Copiado").to_string()
        } else if !a.last_expr.is_empty() && a.just_evaled {
            a.last_expr.clone()
        } else {
            a.expr.clone()
        };
        draw_text(mem, &top_line, lay.expr_rc, 13, 400, p.dim, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS, a.dpi);

        // Primary value or error
        let (main_str, main_col, main_sz) = match &a.err {
            Some(e) => (e.as_str(), style::BAD, 18),
            None => {
                let s = if a.expr.is_empty() || a.just_evaled { a.result.as_str() } else { a.result.as_str() };
                let sz = if s.len() > 22 { 20 } else if s.len() > 14 { 26 } else { 34 };
                (s, p.text, sz)
            }
        };
        draw_text(mem, main_str, lay.val_rc, main_sz, 600, main_col, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS, a.dpi);

        // Programmer base readouts
        if a.mode == Mode::Prog {
            for (i, (&br, &b)) in lay.bases.iter().zip(&BASES).enumerate() {
                let active = a.base == b;
                if active || a.hover_base == Some(i) {
                    fill_round(mem, br, rad / 2, if active { p.active } else { p.hover }, if active { p.active } else { p.hover });
                }
                if active {
                    let bar = RECT { left: br.left + px(2, a.dpi), top: br.top + px(4, a.dpi), right: br.left + px(5, a.dpi), bottom: br.bottom - px(4, a.dpi) };
                    let ab = CreateSolidBrush(p.accent);
                    FillRect(mem, &bar, ab);
                    DeleteObject(ab);
                }
                let lbl_rc = RECT { left: br.left + px(10, a.dpi), top: br.top, right: br.left + px(46, a.dpi), bottom: br.bottom };
                let val_rc = RECT { left: br.left + px(48, a.dpi), top: br.top, right: br.right - px(6, a.dpi), bottom: br.bottom };
                draw_text(mem, b.label(), lbl_rc, 11, if active { 700 } else { 500 }, if active { p.accent } else { p.dim }, DT_LEFT | DT_VCENTER | DT_SINGLELINE, a.dpi);
                let val_s = calc::format_prog_grouped(a.prog_val, b);
                draw_text(mem, &val_s, val_rc, 12, 400, if active { p.text } else { p.dim }, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS, a.dpi);
            }
        }

        if a.show_hist {
            // History overlay over the button grid
            fill_round(mem, lay.grid_rc, rad, style::card(), p.line);
            if a.history.is_empty() {
                draw_text(
                    mem,
                    t!("История пуста", "No history yet", ja: "履歴はありません", pt: "Sem histórico"),
                    lay.grid_rc,
                    13,
                    400,
                    p.dim,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                    a.dpi,
                );
            } else {
                let row_h = px(52, a.dpi);
                let max_y = lay.hist_clear_rc.top - px(4, a.dpi);
                for (i, item) in a.history.iter().enumerate() {
                    let y0 = lay.grid_rc.top + px(6, a.dpi) + i as i32 * row_h;
                    if y0 + row_h > max_y {
                        break;
                    }
                    let rr = RECT { left: lay.grid_rc.left + px(6, a.dpi), top: y0, right: lay.grid_rc.right - px(6, a.dpi), bottom: y0 + row_h - px(4, a.dpi) };
                    if a.hover_hist == Some(i) {
                        fill_round(mem, rr, rad, p.hover, p.hover);
                    }
                    let r1 = RECT { left: rr.left + px(8, a.dpi), top: rr.top + px(4, a.dpi), right: rr.right - px(8, a.dpi), bottom: rr.top + px(22, a.dpi) };
                    let r2 = RECT { left: rr.left + px(8, a.dpi), top: rr.top + px(22, a.dpi), right: rr.right - px(8, a.dpi), bottom: rr.bottom - px(2, a.dpi) };
                    draw_text(mem, &format!("{} =", item.expr), r1, 11, 400, p.dim, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS, a.dpi);
                    draw_text(mem, &item.result, r2, 16, 600, p.text, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS, a.dpi);
                }
                fill_round(mem, lay.hist_clear_rc, rad, p.hover, p.line);
                draw_text(mem, t!("Очистить", "Clear", ja: "クリア", pt: "Limpar"), lay.hist_clear_rc, 12, 500, p.text, DT_CENTER | DT_VCENTER | DT_SINGLELINE, a.dpi);
            }
        } else {
            // Custom-drawn buttons with hover fade
            let (defs, _, _) = buttons(a.mode);
            for (i, (def, &br)) in defs.iter().zip(&lay.btns).enumerate() {
                let enabled = btn_enabled(def, a.mode, a.base);
                let t = a.btn_t.get(i).copied().unwrap_or(0.0);
                let (base_fill, hover_fill, text_col) = match def.kind {
                    BtnKind::Accent => (p.accent, style::mix(p.accent, p.text, 0.18), style::on_accent()),
                    BtnKind::Digit => (style::card(), p.hover, if enabled { p.text } else { style::mix(p.bg, p.dim, 0.45) }),
                    BtnKind::Op => (style::mix(p.bg, p.text, 0.03), p.hover, p.text),
                    BtnKind::Muted => (style::mix(p.bg, p.text, 0.03), p.hover, p.dim),
                };
                let fill = if enabled { style::mix(base_fill, hover_fill, t) } else { p.bg };
                let border = if def.kind == BtnKind::Accent { fill } else { p.line };
                fill_round(mem, br, rad, fill, border);
                let lbl = if matches!(def.action, Action::Dot) {
                    if dot!() { "." } else { "," }
                } else {
                    def.label
                };
                let sz = if lbl.len() >= 3 { 12 } else { 16 };
                draw_text(mem, lbl, br, sz, if def.kind == BtnKind::Digit { 500 } else { 600 }, text_col, DT_CENTER | DT_VCENTER | DT_SINGLELINE, a.dpi);
            }
        }
    });

    BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old_bmp);
    DeleteObject(bmp);
    DeleteDC(mem);
    EndPaint(hwnd, &ps);
}

// ---------- Animation & Input ----------

unsafe fn kick_anim(hwnd: HWND) {
    APP.with_borrow_mut(|a| a.last_tick = Instant::now());
    frames::start(hwnd, TIMER_ANIM);
}

unsafe fn on_anim_tick(hwnd: HWND) {
    let keep = APP.with_borrow_mut(|a| {
        let dt = a.last_tick.elapsed().as_secs_f32().clamp(0.001, 0.05);
        a.last_tick = Instant::now();
        let speed = 10.0 * dt;
        let mut active = false;
        let n = buttons(a.mode).0.len();
        for i in 0..n {
            let target = if !a.show_hist && a.hover_btn == Some(i) { 1.0 } else { 0.0 };
            let cur = a.btn_t[i];
            if (cur - target).abs() > 0.01 {
                a.btn_t[i] = if cur < target { (cur + speed).min(1.0) } else { (cur - speed).max(0.0) };
                active = true;
            } else {
                a.btn_t[i] = target;
            }
        }
        if let Some(until) = a.copied_until {
            if Instant::now() < until {
                active = true;
            } else {
                a.copied_until = None;
            }
        }
        active
    });
    InvalidateRect(hwnd, null(), 0);
    if !keep {
        frames::stop(hwnd, TIMER_ANIM);
    }
}

unsafe fn on_mouse_move(hwnd: HWND, x: i32, y: i32) {
    let mut rc: RECT = zeroed();
    GetClientRect(hwnd, &mut rc);
    let changed = APP.with_borrow_mut(|a| {
        if !a.tracking_mouse {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            TrackMouseEvent(&mut tme);
            a.tracking_mouse = true;
        }
        let lay = compute_layout(rc.right, rc.bottom, a.mode, a.dpi);
        let tab = lay.tabs.iter().position(|&r| pt_in(r, x, y));
        let base = if a.mode == Mode::Prog {
            lay.bases.iter().position(|&r| pt_in(r, x, y))
        } else {
            None
        };
        let (btn, hist) = if a.show_hist {
            let row_h = px(52, a.dpi);
            let h = if pt_in(lay.grid_rc, x, y) && y < lay.hist_clear_rc.top {
                let idx = ((y - lay.grid_rc.top - px(6, a.dpi)) / row_h) as isize;
                (idx >= 0 && (idx as usize) < a.history.len()).then_some(idx as usize)
            } else {
                None
            };
            (None, h)
        } else {
            (lay.btns.iter().position(|&r| pt_in(r, x, y)), None)
        };
        let c = a.hover_tab != tab || a.hover_base != base || a.hover_btn != btn || a.hover_hist != hist;
        a.hover_tab = tab;
        a.hover_base = base;
        a.hover_btn = btn;
        a.hover_hist = hist;
        c
    });
    if changed {
        kick_anim(hwnd);
        InvalidateRect(hwnd, null(), 0);
    }
}

unsafe fn on_click(hwnd: HWND, x: i32, y: i32) {
    let mut rc: RECT = zeroed();
    GetClientRect(hwnd, &mut rc);
    APP.with_borrow_mut(|a| {
        let lay = compute_layout(rc.right, rc.bottom, a.mode, a.dpi);
        if pt_in(lay.tabs[0], x, y) {
            a.show_hist = false;
            a.set_mode(Mode::Std);
            return;
        }
        if pt_in(lay.tabs[1], x, y) {
            a.show_hist = false;
            a.set_mode(Mode::Prog);
            return;
        }
        if pt_in(lay.tabs[2], x, y) {
            a.show_hist = !a.show_hist;
            return;
        }
        if a.mode == Mode::Prog {
            for (i, &br) in lay.bases.iter().enumerate() {
                if pt_in(br, x, y) {
                    a.set_base(BASES[i]);
                    return;
                }
            }
        }
        if a.show_hist {
            if !a.history.is_empty() && pt_in(lay.hist_clear_rc, x, y) {
                a.history.clear();
                return;
            }
            let row_h = px(52, a.dpi);
            if pt_in(lay.grid_rc, x, y) && y < lay.hist_clear_rc.top {
                let idx = ((y - lay.grid_rc.top - px(6, a.dpi)) / row_h) as isize;
                if idx >= 0 && (idx as usize) < a.history.len() {
                    let idx = idx as usize;
                    let (expr, mode, base) = (a.history[idx].expr.clone(), a.history[idx].mode, a.history[idx].base);
                    a.set_mode(mode);
                    a.base = base;
                    a.expr = expr;
                    a.just_evaled = false;
                    a.show_hist = false;
                    a.preview_eval();
                }
            }
            return;
        }
        let (defs, _, _) = buttons(a.mode);
        for (def, &br) in defs.iter().zip(&lay.btns) {
            if pt_in(br, x, y) {
                if btn_enabled(def, a.mode, a.base) {
                    a.apply_action(def.action);
                }
                return;
            }
        }
    });
    InvalidateRect(hwnd, null(), 0);
}

unsafe fn on_keydown(hwnd: HWND, vk: u16) -> bool {
    let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
    let alt = GetKeyState(VK_MENU as i32) < 0;
    if alt {
        match vk {
            0x31 => {
                APP.with_borrow_mut(|a| {
                    a.show_hist = false;
                    a.set_mode(Mode::Std);
                });
                InvalidateRect(hwnd, null(), 0);
                return true;
            }
            0x33 => {
                APP.with_borrow_mut(|a| {
                    a.show_hist = false;
                    a.set_mode(Mode::Prog);
                });
                InvalidateRect(hwnd, null(), 0);
                return true;
            }
            _ => return false,
        }
    }
    if ctrl {
        match vk {
            0x43 => {
                // Ctrl+C: copy result
                let text = APP.with_borrow_mut(|a| {
                    a.copied_until = Some(Instant::now() + std::time::Duration::from_millis(1200));
                    a.result.clone()
                });
                copy_text(hwnd, &text);
                kick_anim(hwnd);
                InvalidateRect(hwnd, null(), 0);
                return true;
            }
            0x56 => {
                // Ctrl+V: paste sanitized
                if let Some(raw) = paste_text(hwnd) {
                    APP.with_borrow_mut(|a| {
                        let clean = calc::sanitize_paste(&raw, a.mode == Mode::Prog, a.base);
                        if !clean.is_empty() {
                            if a.just_evaled {
                                a.expr.clear();
                                a.just_evaled = false;
                            }
                            a.expr.push_str(&clean);
                            a.preview_eval();
                        }
                    });
                    InvalidateRect(hwnd, null(), 0);
                }
                return true;
            }
            _ => return false,
        }
    }
    match vk {
        VK_RETURN => {
            APP.with_borrow_mut(|a| a.apply_action(Action::Eval));
            InvalidateRect(hwnd, null(), 0);
            true
        }
        VK_BACK => {
            APP.with_borrow_mut(|a| a.apply_action(Action::Back));
            InvalidateRect(hwnd, null(), 0);
            true
        }
        VK_ESCAPE => {
            APP.with_borrow_mut(|a| {
                if a.show_hist {
                    a.show_hist = false;
                } else {
                    a.apply_action(Action::Clear);
                }
            });
            InvalidateRect(hwnd, null(), 0);
            true
        }
        VK_DELETE => {
            APP.with_borrow_mut(|a| a.apply_action(Action::ClearEntry));
            InvalidateRect(hwnd, null(), 0);
            true
        }
        _ => false,
    }
}

unsafe fn on_char(hwnd: HWND, ch: u32) {
    let Some(c) = char::from_u32(ch) else { return };
    if c.is_control() {
        return;
    }
    APP.with_borrow_mut(|a| {
        if c == '=' {
            a.apply_action(Action::Eval);
            return;
        }
        match a.mode {
            Mode::Std => match c {
                '0'..='9' => {
                    let s = c.to_string();
                    if a.just_evaled {
                        a.expr.clear();
                        a.just_evaled = false;
                    }
                    a.expr.push_str(&s);
                    a.preview_eval();
                }
                '.' | ',' => a.apply_action(Action::Dot),
                '+' => a.apply_action(Action::Ins("+")),
                '-' => a.apply_action(Action::Ins("-")),
                '*' => a.apply_action(Action::Ins("*")),
                '/' => a.apply_action(Action::Ins("/")),
                '^' => a.apply_action(Action::Ins("^")),
                '%' => a.apply_action(Action::Ins("%")),
                '(' => a.apply_action(Action::Ins("(")),
                ')' => a.apply_action(Action::Ins(")")),
                _ => {}
            },
            Mode::Prog => {
                let up = c.to_ascii_uppercase();
                if a.base.is_digit(up) {
                    if a.just_evaled {
                        a.expr.clear();
                        a.just_evaled = false;
                    }
                    a.expr.push(up);
                    a.preview_eval();
                } else {
                    match c {
                        '+' => a.apply_action(Action::Ins("+")),
                        '-' => a.apply_action(Action::Ins("-")),
                        '*' => a.apply_action(Action::Ins("*")),
                        '/' => a.apply_action(Action::Ins("/")),
                        '%' => a.apply_action(Action::Ins(" MOD ")),
                        '&' => a.apply_action(Action::Ins(" AND ")),
                        '|' => a.apply_action(Action::Ins(" OR ")),
                        '^' => a.apply_action(Action::Ins(" XOR ")),
                        '~' => a.apply_action(Action::Ins("NOT ")),
                        '<' => a.apply_action(Action::Ins("<<")),
                        '>' => a.apply_action(Action::Ins(">>")),
                        '(' => a.apply_action(Action::Ins("(")),
                        ')' => a.apply_action(Action::Ins(")")),
                        _ => {}
                    }
                }
            }
        }
    });
    InvalidateRect(hwnd, null(), 0);
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg != 0 && msg == style::msg() {
        style::reload();
        apply_dwm(hwnd);
        InvalidateRect(hwnd, null(), 0);
        return 0;
    }
    match msg {
        WM_CREATE => {
            let dc = GetDC(hwnd);
            let dpi = GetDeviceCaps(dc, LOGPIXELSX as i32).max(96);
            ReleaseDC(hwnd, dc);
            APP.with_borrow_mut(|a| a.dpi = dpi);
            apply_dwm(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_MOUSEMOVE => {
            let x = (lparam & 0xFFFF) as i16 as i32;
            let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            on_mouse_move(hwnd, x, y);
            0
        }
        WM_MOUSELEAVE_MSG => {
            APP.with_borrow_mut(|a| {
                a.tracking_mouse = false;
                a.hover_btn = None;
                a.hover_base = None;
                a.hover_tab = None;
                a.hover_hist = None;
            });
            kick_anim(hwnd);
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_LBUTTONDOWN => {
            let x = (lparam & 0xFFFF) as i16 as i32;
            let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            on_click(hwnd, x, y);
            0
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if on_keydown(hwnd, wparam as u16) {
                0
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        WM_CHAR => {
            on_char(hwnd, wparam as u32);
            0
        }
        WM_TIMER if wparam == TIMER_ANIM => {
            on_anim_tick(hwnd);
            0
        }
        WM_GETMINMAXINFO => {
            let mmi = &mut *(lparam as *mut MINMAXINFO);
            let dpi = APP.with_borrow(|a| a.dpi);
            mmi.ptMinTrackSize.x = px(320, dpi);
            mmi.ptMinTrackSize.y = px(480, dpi);
            0
        }
        WM_DESTROY => {
            frames::stop(hwnd, TIMER_ANIM);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn main() {
    unsafe {
        let hi = GetModuleHandleW(null());
        let cls = wide("WinRustCalc");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
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
        let title = wide(t!("Калькулятор", "Calculator", ja: "電卓", pt: "Calculadora"));
        let hwnd = CreateWindowExW(
            0,
            cls.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            px(380, dpi),
            px(580, dpi),
            null_mut(),
            null_mut(),
            hi,
            null(),
        );
        if hwnd.is_null() {
            return;
        }
        let mut m: MSG = zeroed();
        while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
}

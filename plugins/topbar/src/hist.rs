//! Win+V without Explorer (its shell hosts Windows' clipboard history, so the history died with it): our own. The bar listens
//! to clipboard changes and keeps the last texts and images (screenshots and snips too) in memory only, like Windows with the
//! history on. Win+V opens the list at the text cursor (else the mouse); a click or Enter puts the item back on the clipboard
//! and pastes it into the window that was in front. Data that apps mark as private (password managers) is not kept.
use super::*;
use windows_sys::Win32::System::{DataExchange::*, Memory::*};

pub(crate) const CLIPS: u32 = WM_APP + 19; // keyboard hook -> bar: Win+V
pub(crate) const WM_CLIPBOARDUPDATE: u32 = 0x031D;

const MAX_ITEMS: usize = 25;
const MAX_IMAGES: usize = 8; // a screenshot is ~8 MB as a DIB: the oldest images go first
const MAX_TEXT: usize = 200_000; // chars kept of one text
const MAX_DIB: usize = 64 << 20;
const CF_TEXT_U: u32 = 13; // CF_UNICODETEXT
const CF_DIB: u32 = 8;

// panel geometry, logical px
const W: i32 = 360;
const H: i32 = 480;
const HEAD: i32 = 56;
const TEXT_ROW: i32 = 66;
const IMG_ROW: i32 = 132;
const GAP: i32 = 6;
const DEL: i32 = 28; // the remove button in a row's corner

enum Data {
    Text(String),
    Dib(Vec<u8>),
}

struct Item {
    data: Data,
    thumb: HBITMAP, // images: made on first paint
    tsize: (i32, i32),
}

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    Row(usize),
    Del(usize),
    Clear,
}

thread_local! {
    static ITEMS: RefCell<Vec<Item>> = const { RefCell::new(Vec::new()) };
    static WIN: Cell<HWND> = const { Cell::new(null_mut()) };
    static PREV: Cell<HWND> = const { Cell::new(null_mut()) }; // where the chosen item is pasted
    static SCROLL: Cell<i32> = const { Cell::new(0) };
    static SEL: Cell<usize> = const { Cell::new(0) };
    static HOT: Cell<Option<Hit>> = const { Cell::new(None) };
}

fn fmt(name: &str) -> u32 {
    unsafe { RegisterClipboardFormatW(wide(name).as_ptr()) }
}

/// Starts listening (once, with the bar window).
pub(crate) unsafe fn listen(bar: HWND) {
    AddClipboardFormatListener(bar);
}

/// The clipboard changed: keep its text or image at the top of the history.
pub(crate) unsafe fn on_change(owner: HWND) {
    // the formats apps use to say "do not record this" (KeePass, 1Password, Bitwarden, browsers' password fields)
    if ["ExcludeClipboardContentFromMonitorProcessing", "Clipboard Viewer Ignore"].iter().any(|f| IsClipboardFormatAvailable(fmt(f)) != 0) {
        return;
    }
    let (text, dib) = (IsClipboardFormatAvailable(CF_TEXT_U) != 0, IsClipboardFormatAvailable(CF_DIB) != 0);
    if !text && !dib {
        return;
    }
    // the app that just wrote may still hold it open for a moment
    if !(0..6).any(|i| {
        if i > 0 {
            windows_sys::Win32::System::Threading::Sleep(12);
        }
        OpenClipboard(owner) != 0
    }) {
        return;
    }
    let data = (|| {
        let can = fmt("CanIncludeInClipboardHistory");
        if IsClipboardFormatAvailable(can) != 0 {
            let h = GetClipboardData(can);
            let p = GlobalLock(h) as *const u32;
            if !p.is_null() {
                let no = *p == 0;
                GlobalUnlock(h);
                if no {
                    return None;
                }
            }
        }
        if text {
            let h = GetClipboardData(CF_TEXT_U);
            let p = GlobalLock(h) as *const u16;
            if p.is_null() {
                return None;
            }
            let n = GlobalSize(h) / 2;
            let s = std::slice::from_raw_parts(p, n);
            let s = String::from_utf16_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(n)]);
            GlobalUnlock(h);
            (!s.trim().is_empty()).then(|| Data::Text(s.chars().take(MAX_TEXT).collect()))
        } else {
            let h = GetClipboardData(CF_DIB);
            let p = GlobalLock(h) as *const u8;
            if p.is_null() {
                return None;
            }
            let n = GlobalSize(h);
            let v = (n > size_of::<BITMAPINFOHEADER>() && n <= MAX_DIB).then(|| std::slice::from_raw_parts(p, n).to_vec());
            GlobalUnlock(h);
            v.map(Data::Dib)
        }
    })();
    CloseClipboard();
    if let Some(d) = data {
        add(d);
    }
}

fn same(a: &Data, b: &Data) -> bool {
    match (a, b) {
        (Data::Text(x), Data::Text(y)) => x == y,
        (Data::Dib(x), Data::Dib(y)) => x == y,
        _ => false,
    }
}

unsafe fn add(d: Data) {
    ITEMS.with_borrow_mut(|v| {
        // copied again (or picked from the history): it moves to the top, with its thumbnail
        let item = match v.iter().position(|i| same(&i.data, &d)) {
            Some(p) => v.remove(p),
            None => Item { data: d, thumb: null_mut(), tsize: (0, 0) },
        };
        v.insert(0, item);
        while v.len() > MAX_ITEMS || v.iter().filter(|i| matches!(i.data, Data::Dib(_))).count() > MAX_IMAGES {
            let at = if v.len() > MAX_ITEMS { v.len() - 1 } else { v.iter().rposition(|i| matches!(i.data, Data::Dib(_))).unwrap() };
            free(v.remove(at));
        }
    });
    repaint();
}

unsafe fn free(i: Item) {
    if !i.thumb.is_null() {
        DeleteObject(i.thumb);
    }
}

unsafe fn repaint() {
    let w = WIN.get();
    if !w.is_null() {
        InvalidateRect(w, null(), 0);
    }
}

/// The DIB's header, its pixels and its size.
/// Any program can put bytes on the clipboard and the bar runs elevated, so the header is checked against the buffer before
/// GDI reads the pixels: None for anything odd or too short.
unsafe fn dib_parts(b: &[u8]) -> Option<(*const BITMAPINFO, *const u8, i32, i32)> {
    dib_layout(b).map(|(off, w, h)| (b.as_ptr() as *const BITMAPINFO, b.as_ptr().add(off), w, h))
}

/// Offset of the pixels and the image size, when the header is sane and the buffer holds every row.
fn dib_layout(b: &[u8]) -> Option<(usize, i32, i32)> {
    if b.len() < size_of::<BITMAPINFOHEADER>() {
        return None;
    }
    let h = unsafe { std::ptr::read_unaligned(b.as_ptr() as *const BITMAPINFOHEADER) };
    if h.biSize < 40 || h.biSize as usize > b.len() || !(1..=32768).contains(&h.biWidth) || h.biHeight == i32::MIN || !(1..=32768).contains(&h.biHeight.abs()) {
        return None;
    }
    if !matches!(h.biCompression, 0 | 3) || !matches!(h.biBitCount, 1 | 4 | 8 | 16 | 24 | 32) {
        return None; // BI_RGB / BI_BITFIELDS only: compressed rows have no fixed size to check
    }
    let colors = if h.biClrUsed > 0 { (h.biClrUsed as usize).min(256) } else if h.biBitCount <= 8 { 1 << h.biBitCount } else { 0 };
    let masks = if h.biCompression == 3 && h.biSize == 40 { 12 } else { 0 }; // BI_BITFIELDS after a plain header
    let off = (h.biSize as usize).checked_add(masks)?.checked_add(colors * 4)?;
    let stride = (h.biWidth as usize).checked_mul(h.biBitCount as usize)?.checked_add(31)? / 32 * 4;
    let end = off.checked_add(stride.checked_mul(h.biHeight.unsigned_abs() as usize)?)?;
    (end <= b.len()).then_some((off, h.biWidth, h.biHeight.abs()))
}

/// Makes the thumbnail of image `i` (fit in `maxw` x `maxh` physical px) once.
unsafe fn thumb(dc: HDC, i: &mut Item, maxw: i32, maxh: i32) {
    if !i.thumb.is_null() {
        return;
    }
    let Data::Dib(b) = &i.data else { return };
    let Some((bmi, bits, w, h)) = dib_parts(b) else { return };
    let k = (maxw as f32 / w as f32).min(maxh as f32 / h as f32).min(1.0);
    let (tw, th) = (((w as f32 * k) as i32).max(1), ((h as f32 * k) as i32).max(1));
    let mdc = CreateCompatibleDC(dc);
    let bmp = CreateCompatibleBitmap(dc, tw, th);
    let old = SelectObject(mdc, bmp);
    SetStretchBltMode(mdc, HALFTONE);
    SetBrushOrgEx(mdc, 0, 0, null_mut());
    StretchDIBits(mdc, 0, 0, tw, th, 0, 0, w, h, bits as _, bmi, DIB_RGB_COLORS, SRCCOPY);
    SelectObject(mdc, old);
    DeleteDC(mdc);
    i.thumb = bmp;
    i.tsize = (tw, th);
}

/// (top, bottom) of each row in logical px, before scrolling, and the bottom of the last one.
fn rows() -> (Vec<(i32, i32)>, i32) {
    let mut y = HEAD;
    let v = ITEMS.with_borrow(|v| {
        v.iter()
            .map(|i| {
                let h = if matches!(i.data, Data::Dib(_)) { IMG_ROW } else { TEXT_ROW };
                y += h + GAP;
                (y - h - GAP, y - GAP)
            })
            .collect()
    });
    (v, y)
}

fn r(l: i32, t: i32, rr: i32, b: i32) -> RECT {
    RECT { left: sc(l), top: sc(t), right: sc(rr), bottom: sc(b) }
}

fn clear_rect() -> RECT {
    r(W - 132, 14, W - 14, 42)
}

fn hit(x: i32, y: i32) -> Option<Hit> {
    let inside = |rc: RECT| x >= rc.left && x < rc.right && y >= rc.top && y < rc.bottom;
    if ITEMS.with_borrow(|v| !v.is_empty()) && inside(clear_rect()) {
        return Some(Hit::Clear);
    }
    if y < sc(HEAD - 4) {
        return None;
    }
    let s = SCROLL.get();
    rows().0.iter().enumerate().find_map(|(i, &(t, b))| {
        let rc = r(12, t - s, W - 12, b - s);
        if !inside(rc) {
            return None;
        }
        Some(if inside(r(W - 12 - DEL - 6, t - s + 6, W - 18, t - s + 6 + DEL)) { Hit::Del(i) } else { Hit::Row(i) })
    })
}

fn max_scroll() -> i32 {
    (rows().1 - H + 8).max(0)
}

/// Scrolls so that row `i` is in view.
fn show_row(i: usize) {
    let Some(&(t, b)) = rows().0.get(i) else { return };
    let s = SCROLL.get();
    if t - s < HEAD {
        SCROLL.set(t - HEAD);
    } else if b - s > H - 8 {
        SCROLL.set(b - H + 8);
    }
    SCROLL.set(SCROLL.get().clamp(0, max_scroll()));
}

/// Win+V: opens the history (or closes it when it is open).
pub(crate) unsafe fn open() {
    if !WIN.get().is_null() {
        close();
        return;
    }
    let fg = GetForegroundWindow();
    PREV.set(fg);
    SCROLL.set(0);
    SEL.set(0);
    HOT.set(None);
    static CLASS: std::sync::Once = std::sync::Once::new();
    let class = wide("topbar_clips");
    CLASS.call_once(|| {
        let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: GetModuleHandleW(null()), lpszClassName: class.as_ptr(), hCursor: LoadCursorW(null_mut(), IDC_ARROW), ..zeroed() };
        RegisterClassW(&wc);
    });
    // at the text cursor of the window in front, like Windows; else at the mouse
    let mut at: POINT = zeroed();
    GetCursorPos(&mut at);
    let mut gi: GUITHREADINFO = zeroed();
    gi.cbSize = size_of::<GUITHREADINFO>() as u32;
    if GetGUIThreadInfo(GetWindowThreadProcessId(fg, null_mut()), &mut gi) != 0 && !gi.hwndCaret.is_null() {
        let mut p = POINT { x: gi.rcCaret.left, y: gi.rcCaret.bottom };
        ClientToScreen(gi.hwndCaret, &mut p);
        at = p;
    }
    let mon = MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST);
    let mut mi: MONITORINFO = zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(mon, &mut mi);
    let wa = mi.rcWork;
    let (w, h) = (sc(W), sc(H));
    let x = (at.x - sc(24)).clamp(wa.left + sc(8), (wa.right - w - sc(8)).max(wa.left));
    let y = if at.y + sc(8) + h <= wa.bottom - sc(8) { at.y + sc(8) } else { (at.y - h - sc(28)).max(wa.top + sc(8)) };
    let hw = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST, class.as_ptr(), wide("Clipboard").as_ptr(), WS_POPUP, x, y, w, h, null_mut(), null_mut(), GetModuleHandleW(null()), null());
    if hw.is_null() {
        return;
    }
    SetWindowRgn(hw, CreateRoundRectRgn(0, 0, w + 1, h + 1, rr(16), rr(16)), 1);
    WIN.set(hw);
    ShowWindow(hw, SW_SHOW);
    force_fg(hw);
    SetFocus(hw);
}

unsafe fn close() {
    let w = WIN.replace(null_mut());
    if !w.is_null() {
        DestroyWindow(w);
    }
}

/// Puts item `i` on the clipboard, closes the panel and pastes it into the window that was in front.
unsafe fn choose(i: usize) {
    let owner = WIN.get();
    let ok = ITEMS.with_borrow(|v| {
        let Some(it) = v.get(i) else { return false };
        let (bytes, f): (Vec<u8>, u32) = match &it.data {
            Data::Text(s) => (s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect(), CF_TEXT_U),
            Data::Dib(b) => (b.clone(), CF_DIB),
        };
        if OpenClipboard(owner) == 0 {
            return false;
        }
        EmptyClipboard();
        let g = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
        let p = GlobalLock(g) as *mut u8;
        let mut done = false;
        if !p.is_null() {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
            GlobalUnlock(g);
            done = !SetClipboardData(f, g as _).is_null(); // the clipboard owns the memory from here on
        }
        if !done && !g.is_null() {
            GlobalFree(g);
        }
        CloseClipboard();
        done
    });
    let prev = PREV.get();
    close();
    if ok && !prev.is_null() && IsWindow(prev) != 0 {
        force_fg(prev);
        let target = prev as usize;
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(70)); // let the window take the focus back first
            if GetForegroundWindow() as usize == target {
                keys(&[VK_CONTROL, b'V' as u16]);
            }
        });
    }
}

unsafe fn remove(i: usize) {
    ITEMS.with_borrow_mut(|v| {
        if i < v.len() {
            free(v.remove(i));
        }
    });
    let n = ITEMS.with_borrow(|v| v.len());
    SEL.set(SEL.get().min(n.saturating_sub(1)));
    SCROLL.set(SCROLL.get().clamp(0, max_scroll()));
    repaint();
}

unsafe fn clear_all() {
    ITEMS.with_borrow_mut(|v| v.drain(..).for_each(|i| free(i)));
    SEL.set(0);
    SCROLL.set(0);
    repaint();
}

/// The first lines of a text, tabs and runs of blank lines tamed, for its row.
fn preview(s: &str) -> String {
    let mut out = String::new();
    for l in s.lines().map(|l| l.replace('\t', "    ")).filter(|l| !l.trim().is_empty()).take(4) {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&l.chars().take(160).collect::<String>());
    }
    out
}

unsafe fn paint(hw: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let wdc = BeginPaint(hw, &mut ps);
    let mut cr: RECT = zeroed();
    GetClientRect(hw, &mut cr);
    let (dc, bmp) = (CreateCompatibleDC(wdc), CreateCompatibleBitmap(wdc, cr.right, cr.bottom));
    let old = SelectObject(dc, bmp);
    let inset = |rc: RECT, d: i32| RECT { left: rc.left + d, top: rc.top + d, right: rc.right - d, bottom: rc.bottom - d };
    fill(dc, cr, if style().light() { mix(pal().bg, pal().dim, 0.3) } else { mix(pal().bg, pal().text, 0.12) }, rr(16));
    fill(dc, inset(cr, 1), pal().bg, rr(16));
    winrust_common::style::backdrop(dc, inset(cr, 1), sc(100) as f32 / 100.0);
    SetBkMode(dc, TRANSPARENT as i32);
    let face = wide(ui_face());
    let gface = wide(winrust_common::style::icon_face());
    let font = |f: &[u16], px: i32, wt: i32| CreateFontW(-sc(px), 0, 0, 0, wt, 0, 0, 0, 1, 0, 0, 5, 0, f.as_ptr());
    let (f_title, f_txt, f_small, f_glyph) = (font(&face, 15, 600), font(&face, 13, 400), font(&face, 12, 400), font(&gface, 12, 400));
    let of = SelectObject(dc, f_title);
    let hot = HOT.get();
    let card = card();
    let empty = ITEMS.with_borrow(|v| v.is_empty());

    text(dc, t!("Буфер обмена", "Clipboard", ja: "クリップボード", pt: "Área de transferência"), r(18, 14, W - 140, 42), pal().text, DT_LEFT);
    if !empty {
        let c = clear_rect();
        fill(dc, c, if hot == Some(Hit::Clear) { mix(card, pal().text, 0.1) } else { card }, sc(14));
        SelectObject(dc, f_small);
        text(dc, t!("Очистить всё", "Clear all", ja: "すべてクリア", pt: "Limpar tudo"), c, pal().text, DT_CENTER);
    }

    // the list, clipped below the header
    let clip = CreateRectRgn(0, sc(HEAD - 4), cr.right, cr.bottom - sc(4));
    SelectClipRgn(dc, clip);
    let s = SCROLL.get();
    let (rws, _) = rows();
    let sel = SEL.get();
    ITEMS.with_borrow_mut(|v| {
        for (i, it) in v.iter_mut().enumerate() {
            let (t, b) = rws[i];
            if b - s < HEAD - 8 || t - s > H {
                continue;
            }
            let rc = r(12, t - s, W - 12, b - s);
            let over = matches!(hot, Some(Hit::Row(j) | Hit::Del(j)) if j == i);
            fill(dc, rc, if over { mix(card, pal().text, 0.07) } else { card }, sc(10));
            if i == sel {
                // the keyboard choice: an accent edge
                fill(dc, RECT { left: rc.left, top: rc.top + sc(10), right: rc.left + sc(3), bottom: rc.bottom - sc(10) }, pal().accent, sc(2));
            }
            match &it.data {
                Data::Text(txt) => {
                    SelectObject(dc, f_txt);
                    SetTextColor(dc, pal().text);
                    let mut tr = RECT { left: rc.left + sc(14), top: rc.top + sc(10), right: rc.right - sc(14 + DEL), bottom: rc.bottom - sc(8) };
                    let w = wide(&preview(txt));
                    DrawTextW(dc, w.as_ptr(), -1, &mut tr, DT_LEFT | DT_WORDBREAK | DT_EDITCONTROL | DT_END_ELLIPSIS | DT_NOPREFIX);
                }
                Data::Dib(bytes) => {
                    let (iw, ih) = dib_parts(bytes).map_or((0, 0), |p| (p.2, p.3));
                    thumb(dc, it, rc.right - rc.left - sc(28 + DEL), rc.bottom - rc.top - sc(36));
                    if !it.thumb.is_null() {
                        let mdc = CreateCompatibleDC(dc);
                        let o = SelectObject(mdc, it.thumb);
                        BitBlt(dc, rc.left + sc(14), rc.top + sc(10), it.tsize.0, it.tsize.1, mdc, 0, 0, SRCCOPY);
                        SelectObject(mdc, o);
                        DeleteDC(mdc);
                    }
                    SelectObject(dc, f_small);
                    let lab = tf!("Изображение · {}×{}", "Image · {}×{}", ja: "画像 · {}×{}", pt: "Imagem · {}×{}", iw, ih);
                    text(dc, &lab, RECT { left: rc.left + sc(14), top: rc.bottom - sc(26), right: rc.right - sc(14), bottom: rc.bottom - sc(6) }, pal().dim, DT_LEFT);
                }
            }
            if over {
                let d = r(W - 12 - DEL - 6, t - s + 6, W - 18, t - s + 6 + DEL);
                fill(dc, d, if hot == Some(Hit::Del(i)) { mix(card, BAD, 0.35) } else { mix(card, pal().text, 0.1) }, sc(DEL / 2));
                SelectObject(dc, f_glyph);
                text(dc, "\u{E711}", d, pal().text, DT_CENTER);
            }
        }
    });
    SelectClipRgn(dc, null_mut());
    DeleteObject(clip);
    if empty {
        SelectObject(dc, f_txt);
        SetTextColor(dc, pal().dim);
        let mut er = r(24, HEAD + 40, W - 24, H - 40);
        let w = wide(t!(
            "Здесь пока пусто.\nСкопируйте текст или сделайте снимок экрана (PrtSc, Win+Shift+S): он появится здесь.",
            "Nothing here yet.\nCopy some text or take a screenshot (PrtSc, Win+Shift+S) and it shows up here.",
            ja: "まだ何もありません。\nテキストをコピーするか、スクリーンショット (PrtSc、Win+Shift+S) を撮るとここに表示されます。",
            pt: "Nada aqui ainda.\nCopie um texto ou tire uma captura de tela (PrtSc, Win+Shift+S) e ela aparece aqui."
        ));
        DrawTextW(dc, w.as_ptr(), -1, &mut er, DT_CENTER | DT_WORDBREAK | DT_NOPREFIX);
    }
    BitBlt(wdc, 0, 0, cr.right, cr.bottom, dc, 0, 0, SRCCOPY);
    SelectObject(dc, of);
    [f_title, f_txt, f_small, f_glyph].into_iter().for_each(|f| _ = DeleteObject(f));
    SelectObject(dc, old);
    DeleteObject(bmp);
    DeleteDC(dc);
    EndPaint(hw, &ps);
}

unsafe extern "system" fn proc(hw: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let xy = || ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
    match msg {
        WM_PAINT => paint(hw),
        WM_ERASEBKGND => return 1,
        WM_ACTIVATE if wp & 0xFFFF == 0 => _ = PostMessageW(hw, WM_CLOSE, 0, 0), // clicked elsewhere
        WM_CLOSE => close(),
        WM_MOUSEMOVE => {
            let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hw, dwHoverTime: 0 };
            TrackMouseEvent(&mut t);
            let (x, y) = xy();
            let h = hit(x, y);
            if h != HOT.get() {
                HOT.set(h);
                InvalidateRect(hw, null(), 0);
            }
        }
        WM_MOUSELEAVE => {
            HOT.set(None);
            InvalidateRect(hw, null(), 0);
        }
        WM_LBUTTONUP => {
            let (x, y) = xy();
            match hit(x, y) {
                Some(Hit::Row(i)) => choose(i),
                Some(Hit::Del(i)) => remove(i),
                Some(Hit::Clear) => clear_all(),
                None => {}
            }
        }
        WM_MOUSEWHEEL => {
            let d = ((wp >> 16) & 0xFFFF) as i16 as i32;
            SCROLL.set((SCROLL.get() - d * 70 / 120).clamp(0, max_scroll()));
            let mut p = POINT { x: (lp & 0xFFFF) as i16 as i32, y: ((lp >> 16) & 0xFFFF) as i16 as i32 };
            ScreenToClient(hw, &mut p);
            HOT.set(hit(p.x, p.y));
            InvalidateRect(hw, null(), 0);
        }
        WM_KEYDOWN => {
            let n = ITEMS.with_borrow(|v| v.len());
            match wp as u16 {
                VK_ESCAPE => close(),
                VK_RETURN if n > 0 => choose(SEL.get()),
                VK_DELETE if n > 0 => remove(SEL.get()),
                VK_DOWN | VK_UP if n > 0 => {
                    let s = SEL.get();
                    SEL.set(if wp as u16 == VK_DOWN { (s + 1).min(n - 1) } else { s.saturating_sub(1) });
                    show_row(SEL.get());
                    InvalidateRect(hw, null(), 0);
                }
                _ => {}
            }
        }
        _ => return DefWindowProcW(hw, msg, wp, lp),
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_keeps_the_first_lines() {
        assert_eq!(preview("a\n\n\tb\nc\nd\ne"), "a\n    b\nc\nd");
        assert_eq!(preview(&"x".repeat(300)).chars().count(), 160);
    }

    fn dib(w: i32, h: i32, bits: u16, extra: usize) -> Vec<u8> {
        let hd = BITMAPINFOHEADER { biSize: 40, biWidth: w, biHeight: h, biPlanes: 1, biBitCount: bits, ..unsafe { zeroed() } };
        let mut v = unsafe { std::slice::from_raw_parts(&hd as *const _ as *const u8, 40) }.to_vec();
        v.resize(40 + extra, 0);
        v
    }

    #[test]
    fn dib_layout_rejects_short_or_odd_buffers() {
        assert_eq!(dib_layout(&dib(2, 2, 32, 16)), Some((40, 2, 2)));
        assert_eq!(dib_layout(&dib(2, -2, 32, 16)), Some((40, 2, 2))); // top-down
        assert_eq!(dib_layout(&dib(2, 2, 32, 15)), None); // last row cut short
        assert_eq!(dib_layout(&dib(30000, 30000, 32, 16)), None); // header claims far more than the buffer
        assert_eq!(dib_layout(&dib(2, i32::MIN, 32, 16)), None);
        assert_eq!(dib_layout(&dib(2, 2, 7, 16)), None);
        assert_eq!(dib_layout(&[0u8; 10]), None);
    }

    #[test]
    fn same_compares_kind_and_content() {
        assert!(same(&Data::Text("a".into()), &Data::Text("a".into())));
        assert!(!same(&Data::Text("a".into()), &Data::Dib(vec![b'a'])));
    }
}

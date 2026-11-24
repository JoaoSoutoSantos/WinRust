//! topbar: StartAllBack-style settings window: taskbar, personalization, tray, widgets, Explorer, look, language.
use super::*;
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::UI::Controls::Dialogs::{CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW};

const WIN_W: i32 = 760;
const WIN_H: i32 = 540;
const SIDE_W: i32 = 212;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Appearance,
    Taskbar,
    Explorer,
    Advanced,
    Tweaks,
    Personalize,
    Tray,
    Widgets,
    Language,
}

/// The sidebar, top to bottom (`Hit::Tab(i)` is an index into it).
const TABS: [Tab; 9] = [Tab::Taskbar, Tab::Personalize, Tab::Tray, Tab::Widgets, Tab::Explorer, Tab::Appearance, Tab::Tweaks, Tab::Language, Tab::Advanced];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hit {
    Tab(usize),
    Theme(usize),
    Opacity(u8),
    Color(usize),
    ResetColors,
    SaveTemplate,
    ApplyTemplate(usize),
    DelTemplate(usize),
    Side(u8),
    BarSize(u8),
    AlignLeft(bool),
    ToggleAutoHide,
    ToggleSearchBtn,
    ToggleAltTab,
    TileWindows,
    ToggleUseRustExplorer,
    ExplorerStart(u8),
    ToggleHidden,
    ToggleGroupDate,
    OpenExplorer,
    ResetAll,
    Flag(usize), // index into i18n::style::FLAGS
    Num(usize, u8), // index into i18n::style::NUMS, the value to set
    Lang(u8),       // 0 en, 1 ru, 2 ja, 3 pt
    TrayApp(usize), // index into TRAY_KEYS: keep that app's icon on the bar or in the chevron menu
    OpenWidgets,
}

thread_local! {
    static WIN: Cell<HWND> = const { Cell::new(null_mut()) };
    static TAB: Cell<Tab> = const { Cell::new(Tab::Taskbar) };
    static HOVER: Cell<Option<Hit>> = const { Cell::new(None) };
    static HITS: RefCell<Vec<(RECT, Hit)>> = const { RefCell::new(Vec::new()) };
    static CONFIRM_RESET: Cell<bool> = const { Cell::new(false) };
    static TRAY_KEYS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) }; // the tray apps listed on the Tray page
}

/// Opens the settings on the Widgets page (right click on the widgets button, the gear in their panel).
pub(crate) unsafe fn open_widgets_page() {
    TAB.set(Tab::Widgets);
    style_menu();
    let w = WIN.get();
    if !w.is_null() {
        InvalidateRect(w, null(), 0);
    }
}

fn settings_title() -> &'static str {
    t!("WinRust — Настройки", "WinRust — Settings", ja: "WinRust — 設定", pt: "WinRust — Configurações")
}

/// A sample of the clock or date in picture format `pic` (empty = Windows' own short format), for the format buttons.
unsafe fn now_pic(time: bool, pic: &str) -> String {
    let mut t: SYSTEMTIME = zeroed();
    GetLocalTime(&mut t);
    let mut b = [0u16; 64];
    let p = wide(pic);
    let fmt = if pic.is_empty() { null() } else { p.as_ptr() };
    let n = if time {
        GetTimeFormatW(0x0400, if pic.is_empty() { 2 } else { 0 }, &t, fmt, b.as_mut_ptr(), 64)
    } else {
        GetDateFormatW(0x0400, if pic.is_empty() { 1 } else { 0 }, &t, fmt, b.as_mut_ptr(), 64)
    };
    String::from_utf16_lossy(&b[..(n.max(1) as usize - 1)])
}

/// Opens (or brings to front) the StartAllBack-style WinRust settings window.
pub(crate) unsafe fn style_menu() {
    let cur = WIN.get();
    if !cur.is_null() && IsWindow(cur) != 0 {
        winrust_common::fg::raise(cur);
        return;
    }
    static REG: std::sync::Once = std::sync::Once::new();
    let inst = GetModuleHandleW(null());
    let cls = wide("WinRustSettings");
    REG.call_once(|| {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(settings_proc),
            hInstance: inst,
            lpszClassName: cls.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..zeroed()
        };
        RegisterClassW(&wc);
    });
    let (w, h) = (sc(WIN_W), sc(WIN_H));
    let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
    let title = wide(settings_title());
    let style_flags = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
    let mut wr = RECT { left: 0, top: 0, right: w, bottom: h };
    AdjustWindowRect(&mut wr, style_flags, 0);
    let (ww, wh) = (wr.right - wr.left, wr.bottom - wr.top);
    let hw = CreateWindowExW(0, cls.as_ptr(), title.as_ptr(), style_flags, (sw - ww) / 2, (sh - wh) / 2, ww, wh, null_mut(), null_mut(), inst, null());
    if hw.is_null() {
        return;
    }
    WIN.set(hw);
    CONFIRM_RESET.set(false);
    apply_frame(hw);
    ShowWindow(hw, SW_SHOW);
    winrust_common::fg::force(hw);
}

unsafe fn apply_frame(hw: HWND) {
    let set = |attr: u32, v: i32| {
        DwmSetWindowAttribute(hw, attr, &v as *const _ as _, 4);
    };
    set(20, !style().light() as i32); // DWMWA_USE_IMMERSIVE_DARK_MODE
    set(33, if style().square() { 1 } else { 2 }); // DWMWA_WINDOW_CORNER_PREFERENCE
    set(35, pal().bg as i32); // DWMWA_CAPTION_COLOR
    set(36, pal().text as i32); // DWMWA_TEXT_COLOR
    set(34, pal().line as i32); // DWMWA_BORDER_COLOR
}

pub(crate) fn style_name(s: i18n::style::Style) -> &'static str {
    use i18n::style::Style::*;
    match s {
        Normal => t!("Обычный", "Normal", ja: "通常", pt: "Normal", zh: "正常"),
        Terminal => t!("Терминал", "Terminal", ja: "ターミナル", pt: "Terminal", zh: "终端"),
        Dock => t!("Плавающий док", "Floating dock", ja: "フローティング ドック", pt: "Dock flutuante", zh: "悬浮停靠"),
        Tiling => t!("Тайлинг", "Tiling", ja: "タイル表示", pt: "Lado a lado", zh: "平铺"),
        Road => t!("Белая дорога", "White road", ja: "白い道", pt: "Estrada branca", zh: "白色道路"),
        Ruby => t!("Рубин", "Ruby", ja: "ルビー", pt: "Rubi", zh: "红宝石"),
        Win98 => "Windows 98",
        Win7 => "Windows 7",
    }
}

/// Repaints the bar in the new look; `moved` = its place or size may have changed too.
pub(crate) unsafe fn relook(moved: bool) {
    let bar = BAR.load(Relaxed) as HWND;
    place(bar);
    if moved {
        bar_anim(bar);
    }
    apply_opacity(bar);
    InvalidateRect(bar, null(), 0);
    let w = WIN.get();
    if !w.is_null() && IsWindow(w) != 0 {
        apply_frame(w);
        InvalidateRect(w, null(), 0);
    }
}

/// Bar transparency from the colours page (the window is layered; 100 % = opaque).
pub(crate) unsafe fn apply_opacity(hwnd: HWND) {
    SetLayeredWindowAttributes(hwnd, 0, (255 * i18n::style::bar_opacity() as u32 / 100) as u8, LWA_ALPHA);
}

fn group_marker() -> PathBuf {
    PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join(r"rust-explorer\group-by-date")
}

fn group_by_date() -> bool {
    group_marker().exists()
}

fn set_group_by_date(on: bool) {
    let f = group_marker();
    if on {
        let _ = std::fs::create_dir_all(f.parent().unwrap_or(&f));
        let _ = std::fs::write(f, "1");
    } else {
        let _ = std::fs::remove_file(f);
    }
}

unsafe fn draw_toggle(dc: HDC, r: RECT, on: bool, hot: bool) {
    let h = r.bottom - r.top;
    let track = if on {
        if hot { mix(pal().accent, pal().text, 0.14) } else { pal().accent }
    } else {
        if hot { mix(pal().line, pal().text, 0.18) } else { pal().line }
    };
    fill(dc, r, track, h);
    let pad = sc(3);
    let d = (h - 2 * pad).max(4);
    let kx = if on { r.right - pad - d } else { r.left + pad };
    let knob = if on { on_accent() } else { pal().text };
    fill(dc, RECT { left: kx, top: r.top + pad, right: kx + d, bottom: r.top + pad + d }, knob, d);
}

unsafe fn draw_pill(dc: HDC, r: RECT, label: &str, active: bool, hot: bool, f_txt: HFONT, swatch: Option<u32>, danger: bool) {
    let bg = if active {
        pal().accent
    } else if danger {
        if hot { mix(card(), rgb(255, 69, 58), 0.25) } else { mix(card(), rgb(255, 69, 58), 0.12) }
    } else if hot {
        mix(card(), pal().text, 0.10)
    } else {
        card()
    };
    let fg = if active {
        on_accent()
    } else if danger {
        rgb(255, 69, 58)
    } else {
        pal().text
    };
    fill(dc, r, if active { bg } else { pal().line }, rr(8));
    fill(dc, RECT { left: r.left + 1, top: r.top + 1, right: r.right - 1, bottom: r.bottom - 1 }, bg, rr(7));
    let mut lx = r.left + sc(10);
    if let Some(c) = swatch {
        let cy = (r.top + r.bottom) / 2;
        let d = sc(6);
        fill(dc, RECT { left: lx, top: cy - d, right: lx + 2 * d, bottom: cy + d }, pal().line, 2 * d);
        fill(dc, RECT { left: lx + 1, top: cy - d + 1, right: lx + 2 * d - 1, bottom: cy + d - 1 }, c, 2 * d);
        lx += sc(18);
    }
    SelectObject(dc, f_txt);
    let align = if swatch.is_some() { DT_LEFT } else { DT_CENTER };
    text(dc, label, RECT { left: if swatch.is_some() { lx } else { r.left + sc(6) }, right: r.right - sc(6), ..r }, fg, align);
}

unsafe fn draw_toggle_row(dc: HDC, hits: &mut Vec<(RECT, Hit)>, x: i32, y: i32, w: i32, title: &str, sub: &str, on: bool, hit: Hit, hot: Option<Hit>, f_txt: HFONT, f_small: HFONT) -> i32 {
    let h = sc(52);
    let r = RECT { left: x, top: y, right: x + w, bottom: y + h };
    let is_hot = hot == Some(hit);
    let bg = if is_hot { mix(card(), pal().text, 0.05) } else { card() };
    fill(dc, r, pal().line, rr(10));
    fill(dc, RECT { left: r.left + 1, top: r.top + 1, right: r.right - 1, bottom: r.bottom - 1 }, bg, rr(9));
    SelectObject(dc, f_txt);
    text(dc, title, RECT { left: r.left + sc(14), top: r.top + sc(8), right: r.right - sc(70), bottom: r.top + sc(28) }, pal().text, DT_LEFT);
    SelectObject(dc, f_small);
    text(dc, sub, RECT { left: r.left + sc(14), top: r.top + sc(28), right: r.right - sc(70), bottom: r.bottom - sc(6) }, pal().dim, DT_LEFT);
    let tr = RECT { left: r.right - sc(54), top: r.top + sc(15), right: r.right - sc(14), bottom: r.top + sc(37) };
    draw_toggle(dc, tr, on, is_hot);
    hits.push((r, hit));
    y + h + sc(8)
}

unsafe fn make_font(px: i32, weight: i32, face: &str) -> HFONT {
    CreateFontW(-sc(px), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, wide(face).as_ptr())
}

unsafe fn paint_settings(hw: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let hdc = BeginPaint(hw, &mut ps);
    let mut cr: RECT = zeroed();
    GetClientRect(hw, &mut cr);
    let (cw, ch) = (cr.right, cr.bottom);
    let dc = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, cw, ch);
    let old = SelectObject(dc, bmp as HGDIOBJ);
    SetBkMode(dc, TRANSPARENT as i32);

    let f_title = make_font(18, 600, ui_face());
    let f_sec = make_font(12, 600, ui_face());
    let f_txt = make_font(13, 400, ui_face());
    let f_semi = make_font(13, 600, ui_face());
    let f_small = make_font(11, 400, ui_face());
    let f_glyph = make_font(15, 400, i18n::style::icon_face());

    // Main background + left sidebar surface (StartAllBack-style two-pane layout)
    let side_w = sc(SIDE_W);
    fill(dc, cr, pal().bg, 0);
    let side_bg = if style().light() { mix(pal().bg, pal().line, 0.35) } else { mix(pal().bg, 0, 0.25) };
    fill(dc, RECT { left: 0, top: 0, right: side_w, bottom: ch }, side_bg, 0);
    fill(dc, RECT { left: side_w, top: 0, right: side_w + 1, bottom: ch }, pal().line, 0);

    // Sidebar header
    SelectObject(dc, f_title);
    text(dc, "WinRust", RECT { left: sc(18), top: sc(18), right: side_w - sc(12), bottom: sc(44) }, pal().text, DT_LEFT);
    SelectObject(dc, f_small);
    text(dc, t!("Панель управления", "Control Panel", ja: "コントロール パネル", pt: "Painel de Controle"), RECT { left: sc(18), top: sc(42), right: side_w - sc(12), bottom: sc(60) }, pal().dim, DT_LEFT);

    let mut hits: Vec<(RECT, Hit)> = Vec::new();
    let cur_tab = TAB.get();
    let hot = HOVER.get();

    let tabs = TABS.map(|tb| match tb {
        Tab::Taskbar => (tb, "\u{E8A1}", t!("Панель задач", "Taskbar", ja: "タスクバー", pt: "Barra de Tarefas")),
        Tab::Personalize => (tb, "\u{E771}", t!("Персонализация", "Personalize", ja: "個人用設定", pt: "Personalizar")),
        Tab::Tray => (tb, "\u{E7E7}", t!("Значки в трее", "Tray icons", ja: "トレイ アイコン", pt: "Ícones da bandeja")),
        Tab::Widgets => (tb, "\u{E9D2}", t!("Виджеты", "Widgets", ja: "ウィジェット", pt: "Widgets")),
        Tab::Explorer => (tb, "\u{E8B7}", t!("Проводник", "Explorer", ja: "エクスプローラー", pt: "Explorer")),
        Tab::Appearance => (tb, "\u{E790}", t!("Внешний вид", "Appearance", ja: "外観", pt: "Aparência")),
        Tab::Tweaks => (tb, "\u{E9E9}", t!("Настройки поведения", "Tweaks", ja: "微調整", pt: "Ajustes")),
        Tab::Language => (tb, "\u{E774}", t!("Язык", "Language", ja: "言語", pt: "Idioma")),
        Tab::Advanced => (tb, "\u{E713}", t!("Дополнительно", "Advanced", ja: "詳細設定", pt: "Avançado")),
    });

    for (idx, (tb, glyph, label)) in tabs.iter().enumerate() {
        let y = sc(72) + idx as i32 * sc(50);
        let r = RECT { left: sc(10), top: y, right: side_w - sc(10), bottom: y + sc(40) };
        let sel = cur_tab == *tb;
        let is_hot = hot == Some(Hit::Tab(idx));
        if sel || is_hot {
            let bg = if sel { mix(side_bg, pal().accent, 0.16) } else { mix(side_bg, pal().text, 0.06) };
            fill(dc, r, bg, rr(8));
        }
        if sel {
            fill(dc, RECT { left: r.left + sc(2), top: r.top + sc(9), right: r.left + sc(5), bottom: r.bottom - sc(9) }, pal().accent, sc(3));
        }
        SelectObject(dc, f_glyph);
        text(dc, glyph, RECT { left: r.left + sc(10), top: r.top, right: r.left + sc(38), bottom: r.bottom }, if sel { pal().accent } else { pal().dim }, DT_CENTER);
        SelectObject(dc, if sel { f_semi } else { f_txt });
        text(dc, label, RECT { left: r.left + sc(42), top: r.top, right: r.right - sc(8), bottom: r.bottom }, if sel { pal().text } else { mix(pal().text, pal().dim, 0.25) }, DT_LEFT);
        hits.push((r, Hit::Tab(idx)));
    }

    // Content area
    let cx = side_w + sc(24);
    let pw = cw - cx - sc(24);
    let mut y = sc(22);

    let section = |dc: HDC, y: i32, label: &str| -> i32 {
        SelectObject(dc, f_sec);
        text(dc, label, RECT { left: cx, top: y, right: cx + pw, bottom: y + sc(20) }, pal().dim, DT_LEFT);
        y + sc(24)
    };

    match cur_tab {
        Tab::Taskbar => {
            SelectObject(dc, f_title);
            text(dc, t!("Панель задач", "Taskbar", ja: "タスクバー", pt: "Barra de Tarefas"), RECT { left: cx, top: y, right: cx + pw, bottom: y + sc(28) }, pal().text, DT_LEFT);
            y += sc(38);

            // Position
            y = section(dc, y, t!("ПОЛОЖЕНИЕ НА ЭКРАНЕ", "POSITION", ja: "位置", pt: "POSIÇÃO NA TELA"));
            let sides = [
                (0u8, i18n::style::Side::Bottom, t!("Снизу", "Bottom", ja: "下", pt: "Embaixo")),
                (1u8, i18n::style::Side::Left, t!("Слева", "Left", ja: "左", pt: "Esquerda")),
                (2u8, i18n::style::Side::Right, t!("Справа", "Right", ja: "右", pt: "Direita")),
            ];
            let bw = (pw - sc(16)) / 3;
            for (k, (id, s, lbl)) in sides.iter().enumerate() {
                let r = RECT { left: cx + k as i32 * (bw + sc(8)), top: y, right: cx + k as i32 * (bw + sc(8)) + bw, bottom: y + sc(34) };
                draw_pill(dc, r, lbl, i18n::style::side() == *s, hot == Some(Hit::Side(*id)), f_txt, None, false);
                hits.push((r, Hit::Side(*id)));
            }
            y += sc(46);

            // Size + Alignment row
            let half = (pw - sc(16)) / 2;
            SelectObject(dc, f_sec);
            text(dc, t!("РАЗМЕР ПАНЕЛИ", "BAR SIZE", ja: "サイズ", pt: "TAMANHO"), RECT { left: cx, top: y, right: cx + half, bottom: y + sc(20) }, pal().dim, DT_LEFT);
            text(dc, t!("ВЫРАВНИВАНИЕ", "ALIGNMENT", ja: "配置", pt: "ALINHAMENTO"), RECT { left: cx + half + sc(16), top: y, right: cx + pw, bottom: y + sc(20) }, pal().dim, DT_LEFT);
            y += sc(24);
            let sz_labels = [t!("Мал.", "Small", ja: "小", pt: "Pequena"), t!("Обычн.", "Normal", ja: "通常", pt: "Normal"), t!("Больш.", "Large", ja: "大", pt: "Grande")];
            let sbw = (half - sc(12)) / 3;
            for (k, lbl) in sz_labels.iter().enumerate() {
                let r = RECT { left: cx + k as i32 * (sbw + sc(6)), top: y, right: cx + k as i32 * (sbw + sc(6)) + sbw, bottom: y + sc(34) };
                draw_pill(dc, r, lbl, i18n::style::bar_size() == k as u8, hot == Some(Hit::BarSize(k as u8)), f_txt, None, false);
                hits.push((r, Hit::BarSize(k as u8)));
            }
            let abw = (half - sc(8)) / 2;
            let align_x = cx + half + sc(16);
            for (k, (left_on, lbl)) in [(false, t!("По центру", "Center", ja: "中央", pt: "Centro")), (true, t!("Слева (Win 10)", "Left (Win 10)", ja: "左 (Win 10)", pt: "Esquerda (Win 10)"))].iter().enumerate() {
                let r = RECT { left: align_x + k as i32 * (abw + sc(8)), top: y, right: align_x + k as i32 * (abw + sc(8)) + abw, bottom: y + sc(34) };
                draw_pill(dc, r, lbl, i18n::style::align_left() == *left_on, hot == Some(Hit::AlignLeft(*left_on)), f_txt, None, false);
                hits.push((r, Hit::AlignLeft(*left_on)));
            }
            y += sc(48);

            // Toggles
            y = section(dc, y, t!("ПОВЕДЕНИЕ", "BEHAVIOR", ja: "動作", pt: "COMPORTAMENTO"));
            y = draw_toggle_row(dc, &mut hits, cx, y, pw,
                t!("Скрывать панель автоматически", "Auto-hide taskbar", ja: "タスクバーを自動的に隠す", pt: "Ocultar barra automaticamente"),
                t!("Появляется при наведении курсора на край экрана", "Slides back in when the pointer reaches the edge", ja: "画面端にカーソルを当てると表示されます", pt: "Aparece ao encostar o mouse na borda da tela"),
                i18n::style::autohide(), Hit::ToggleAutoHide, hot, f_semi, f_small);
            y = draw_toggle_row(dc, &mut hits, cx, y, pw,
                t!("Показывать кнопку поиска", "Show search button", ja: "検索ボタンを表示", pt: "Mostrar botão de pesquisa"),
                t!("Значок лупы рядом с кнопкой «Пуск»", "Magnifying glass button next to Start", ja: "スタートの横の虫眼鏡ボタン", pt: "Ícone da lupa ao lado do botão Iniciar"),
                i18n::style::show_search_btn(), Hit::ToggleSearchBtn, hot, f_semi, f_small);
            y = draw_toggle_row(dc, &mut hits, cx, y, pw,
                t!("Alt+Tab: только этот монитор", "Alt+Tab: current monitor only", ja: "Alt+Tab: このモニターのみ", pt: "Alt+Tab: somente este monitor"),
                t!("Переключать окна только на активном экране", "Only cycle windows on the active display", ja: "アクティブなモニターのウィンドウのみ切り替え", pt: "Alternar apenas janelas do monitor atual"),
                i18n::style::alttab_per_monitor(), Hit::ToggleAltTab, hot, f_semi, f_small);

            let tr = RECT { left: cx, top: y + sc(4), right: cx + sc(220), bottom: y + sc(38) };
            draw_pill(dc, tr, t!("Разложить окна плиткой", "Tile open windows", ja: "ウィンドウを並べて表示", pt: "Organizar janelas lado a lado"), false, hot == Some(Hit::TileWindows), f_txt, None, false);
            hits.push((tr, Hit::TileWindows));
        }

        Tab::Explorer => {
            SelectObject(dc, f_title);
            text(dc, t!("Проводник (Rust Explorer)", "Explorer (Rust Explorer)", ja: "エクスプローラー (Rust Explorer)", pt: "Explorer (Rust Explorer)"), RECT { left: cx, top: y, right: cx + pw, bottom: y + sc(28) }, pal().text, DT_LEFT);
            y += sc(38);

            y = section(dc, y, t!("ИНТЕГРАЦИЯ", "INTEGRATION", ja: "統合", pt: "INTEGRAÇÃO"));
            y = draw_toggle_row(dc, &mut hits, cx, y, pw,
                t!("Открывать Rust Explorer с панели задач", "Use Rust Explorer from the taskbar icon", ja: "タスクバーアイコンから Rust Explorer を開く", pt: "Usar Rust Explorer no ícone da barra"),
                t!("Один значок проводника без дубликатов (иначе открывает проводник Windows)", "Single clean Explorer icon without duplicates (off = Windows Explorer)", ja: "重複なしの単一アイコン（オフ = Windows エクスプローラー）", pt: "Ícone único sem duplicação (desligado = Explorer do Windows)"),
                i18n::style::use_rust_explorer(), Hit::ToggleUseRustExplorer, hot, f_semi, f_small);

            y += sc(4);
            y = section(dc, y, t!("НАЧАЛЬНАЯ ПАПКА", "DEFAULT START FOLDER", ja: "初期フォルダー", pt: "PASTA INICIAL AO ABRIR"));
            let starts = [
                (0u8, t!("Домашняя (Home)", "Home", ja: "ホーム", pt: "Usuário (Home)")),
                (1u8, t!("Рабочий стол", "Desktop", ja: "デスクトップ", pt: "Área de Trabalho")),
                (2u8, t!("Загрузки", "Downloads", ja: "ダウンロード", pt: "Downloads")),
                (3u8, t!("Диск C:\\", "Drive C:\\", ja: "C:\\ ドライブ", pt: "Disco C:\\")),
            ];
            let bw = (pw - sc(24)) / 4;
            for (k, (id, lbl)) in starts.iter().enumerate() {
                let r = RECT { left: cx + k as i32 * (bw + sc(8)), top: y, right: cx + k as i32 * (bw + sc(8)) + bw, bottom: y + sc(34) };
                draw_pill(dc, r, lbl, i18n::style::explorer_start() == *id, hot == Some(Hit::ExplorerStart(*id)), f_txt, None, false);
                hits.push((r, Hit::ExplorerStart(*id)));
            }
            y += sc(48);

            y = section(dc, y, t!("ОТОБРАЖЕНИЕ", "VIEW OPTIONS", ja: "表示オプション", pt: "EXIBIÇÃO"));
            // two columns: six switches, all applied live to open Rust Explorer windows
            {
                use i18n::style::Flag;
                let flag_row = |f: Flag| (i18n::style::flag(f), Hit::Flag(i18n::style::FLAGS.iter().position(|&x| x == f).unwrap_or(0)));
                let rows: [(&str, &str, (bool, Hit)); 6] = [
                    (t!("Скрытые файлы", "Hidden files", ja: "隠しファイル", pt: "Arquivos ocultos"), t!("Также Ctrl+H", "Also Ctrl+H", ja: "Ctrl+H でも切替", pt: "Também Ctrl+H"), (i18n::style::explorer_hidden(), Hit::ToggleHidden)),
                    (t!("Группировать по дате", "Group by date", ja: "日付でグループ化", pt: "Agrupar por data"), t!("Сегодня, Вчера…", "Today, Yesterday…", ja: "今日、昨日…", pt: "Hoje, Ontem…"), (group_by_date(), Hit::ToggleGroupDate)),
                    (t!("Расширения имён", "File extensions", ja: "拡張子を表示", pt: "Extensões de arquivo"), t!("Выкл.: «отчёт» вместо «отчёт.pdf»", "Off: \"report\" instead of \"report.pdf\"", ja: "オフ: 拡張子を隠す", pt: "Desligado: \"relatorio\" em vez de \"relatorio.pdf\""), flag_row(Flag::ExpExtensions)),
                    (t!("Папки сверху", "Folders first", ja: "フォルダーを先頭に", pt: "Pastas primeiro"), t!("Выкл.: папки вперемешку с файлами", "Off: folders mixed with files", ja: "オフ: ファイルと混在", pt: "Desligado: pastas misturadas aos arquivos"), flag_row(Flag::ExpFoldersFirst)),
                    (t!("Открывать одним щелчком", "Single-click to open", ja: "シングルクリックで開く", pt: "Abrir com um clique"), t!("Наведение выделяет", "Hovering selects", ja: "ポイントで選択", pt: "Passar o mouse seleciona"), flag_row(Flag::ExpSingleClick)),
                    (t!("Эскизы в новых окнах", "Thumbnails in new windows", ja: "新しいウィンドウは縮小表示", pt: "Miniaturas em novas janelas"), t!("Иначе таблица", "Otherwise details", ja: "オフ: 詳細表示", pt: "Senão, detalhes"), flag_row(Flag::ExpThumbs)),
                ];
                let cw = (pw - sc(8)) / 2;
                let mut bottom = y;
                for (k, (title, sub, (on, hit))) in rows.iter().enumerate() {
                    let x = cx + (k as i32 % 2) * (cw + sc(8));
                    let ry = y + (k as i32 / 2) * sc(60);
                    bottom = draw_toggle_row(dc, &mut hits, x, ry, cw, title, sub, *on, *hit, hot, f_semi, f_small);
                }
                y = bottom;
            }

            let or = RECT { left: cx, top: y + sc(6), right: cx + sc(210), bottom: y + sc(40) };
            draw_pill(dc, or, t!("Открыть Rust Explorer", "Open Rust Explorer", ja: "Rust Explorer を開く", pt: "Abrir Rust Explorer"), false, hot == Some(Hit::OpenExplorer), f_semi, None, false);
            hits.push((or, Hit::OpenExplorer));
        }

        Tab::Appearance => {
            SelectObject(dc, f_title);
            text(dc, t!("Внешний вид и темы", "Appearance & Themes", ja: "外観とテーマ", pt: "Aparência e Temas"), RECT { left: cx, top: y, right: cx + pw, bottom: y + sc(28) }, pal().text, DT_LEFT);
            y += sc(38);

            y = section(dc, y, t!("ТЕМА", "THEME", ja: "テーマ", pt: "TEMA"));
            let tw = (pw - sc(16)) / 3;
            for (k, &s) in i18n::style::ALL.iter().enumerate() {
                let col = (k % 3) as i32;
                let row = (k / 3) as i32;
                let r = RECT { left: cx + col * (tw + sc(8)), top: y + row * sc(40), right: cx + col * (tw + sc(8)) + tw, bottom: y + row * sc(40) + sc(34) };
                draw_pill(dc, r, style_name(s), s == style(), hot == Some(Hit::Theme(k)), f_txt, Some(i18n::style::PALS[k].accent), false);
                hits.push((r, Hit::Theme(k)));
            }
            y += sc(90);

            y = section(dc, y, t!("НЕПРОЗРАЧНОСТЬ ПАНЕЛИ", "TASKBAR OPACITY", ja: "タスクバーの不透明度", pt: "OPACIDADE DA BARRA"));
            const PCT: [u8; 5] = [100, 85, 70, 55, 40];
            let ow = (pw - sc(32)) / 5;
            for (k, &v) in PCT.iter().enumerate() {
                let r = RECT { left: cx + k as i32 * (ow + sc(8)), top: y, right: cx + k as i32 * (ow + sc(8)) + ow, bottom: y + sc(32) };
                let lbl = format!("{v}%");
                draw_pill(dc, r, &lbl, i18n::style::bar_opacity() == v, hot == Some(Hit::Opacity(v)), f_txt, None, false);
                hits.push((r, Hit::Opacity(v)));
            }
            y += sc(44);

            y = section(dc, y, t!("СВОИ ЦВЕТА (КЛИК — ВЫБОР ЦВЕТА)", "CUSTOM COLORS (CLICK TO PICK)", ja: "カスタム色（クリックで選択）", pt: "CORES PERSONALIZADAS (CLIQUE PARA ESCOLHER)"));
            let cnames = [
                t!("Фон", "Background", ja: "背景", pt: "Fundo"),
                t!("Рамки", "Borders", ja: "枠線", pt: "Bordas"),
                t!("Наведение", "Hover", ja: "ホバー", pt: "Hover"),
                t!("Выбрано", "Active", ja: "選択中", pt: "Ativo"),
                t!("Акцент", "Accent", ja: "アクセント", pt: "Destaque"),
                t!("Текст", "Text", ja: "テキスト", pt: "Texto"),
                t!("Вторичный", "Muted", ja: "補助", pt: "Secundário"),
            ];
            let p = pal();
            let now = [p.bg, p.line, p.hover, p.active, p.accent, p.text, p.dim];
            let cw_pill = (pw - sc(24)) / 4;
            for (k, lbl) in cnames.iter().enumerate() {
                let col = (k % 4) as i32;
                let row = (k / 4) as i32;
                let r = RECT { left: cx + col * (cw_pill + sc(8)), top: y + row * sc(38), right: cx + col * (cw_pill + sc(8)) + cw_pill, bottom: y + row * sc(38) + sc(32) };
                draw_pill(dc, r, lbl, false, hot == Some(Hit::Color(k)), f_small, Some(now[k]), false);
                hits.push((r, Hit::Color(k)));
            }
            let rr_btn = RECT { left: cx + 3 * (cw_pill + sc(8)), top: y + sc(38), right: cx + pw, bottom: y + sc(70) };
            draw_pill(dc, rr_btn, t!("Сбросить", "Reset", ja: "リセット", pt: "Limpar"), false, hot == Some(Hit::ResetColors), f_small, None, true);
            hits.push((rr_btn, Hit::ResetColors));
            y += sc(86);

            y = section(dc, y, t!("СОХРАНЁННЫЕ ШАБЛОНЫ", "SAVED TEMPLATES", ja: "保存済みテンプレート", pt: "MODELOS SALVOS"));
            let list = i18n::style::themes();
            let now_th = i18n::style::Theme::current("");
            let mut tx = cx;
            for (k, th) in list.iter().take(3).enumerate() {
                let same = (th.style, th.colors, th.opacity) == (now_th.style, now_th.colors, now_th.opacity);
                let bw = sc(126);
                let r = RECT { left: tx, top: y, right: tx + bw - sc(28), bottom: y + sc(32) };
                draw_pill(dc, r, &th.name, same, hot == Some(Hit::ApplyTemplate(k)), f_small, None, false);
                hits.push((r, Hit::ApplyTemplate(k)));
                let dr = RECT { left: tx + bw - sc(26), top: y, right: tx + bw, bottom: y + sc(32) };
                draw_pill(dc, dr, "×", false, hot == Some(Hit::DelTemplate(k)), f_small, None, true);
                hits.push((dr, Hit::DelTemplate(k)));
                tx += bw + sc(8);
            }
            let sr = RECT { left: tx, top: y, right: (tx + sc(130)).min(cx + pw), bottom: y + sc(32) };
            draw_pill(dc, sr, t!("+ Сохранить", "+ Save look", ja: "+ 保存", pt: "+ Salvar"), false, hot == Some(Hit::SaveTemplate), f_small, None, false);
            hits.push((sr, Hit::SaveTemplate));
        }

        Tab::Tweaks => {
            use i18n::style::Flag;
            SelectObject(dc, f_title);
            text(dc, t!("Настройки поведения", "Tweaks", ja: "微調整", pt: "Ajustes"), RECT { left: cx, top: y, right: cx + pw, bottom: y + sc(28) }, pal().text, DT_LEFT);
            y += sc(38);
            y = section(dc, y, t!("ЧАСЫ, МЫШЬ И АНИМАЦИЯ", "CLOCK, MOUSE AND MOTION", ja: "時計・マウス・アニメーション", pt: "RELÓGIO, MOUSE E ANIMAÇÕES"));
            for (i, &f) in i18n::style::FLAGS.iter().enumerate().take(i18n::style::TASKBAR_FLAGS) {
                let (title, sub) = match f {
                    Flag::ClockSeconds => (
                        t!("Секунды на часах", "Show seconds on the clock", ja: "時計に秒を表示", pt: "Mostrar segundos no relógio"),
                        t!("Часы становятся немного шире", "The clock gets a few pixels wider", ja: "時計の幅が少し広がります", pt: "O relógio fica alguns pixels mais largo"),
                    ),
                    Flag::ClockDate => (
                        t!("Дата под временем", "Date under the time", ja: "時刻の下に日付", pt: "Data abaixo da hora"),
                        t!("Выкл.: только время, по центру", "Off: the time alone, centered", ja: "オフ: 時刻のみ中央に表示", pt: "Desligado: só a hora, centralizada"),
                    ),
                    Flag::EmptyDesktop => (
                        t!("Щелчок по пустому месту: рабочий стол", "Click an empty spot: show desktop", ja: "空白をクリック: デスクトップを表示", pt: "Clique no vazio: mostrar área de trabalho"),
                        t!("Свернуть все окна и вернуть их", "Minimizes every window, and back", ja: "すべてのウィンドウを最小化/復元", pt: "Minimiza todas as janelas e as traz de volta"),
                    ),
                    Flag::WheelWindows => (
                        t!("Колесо над кнопкой: переключать окна", "Wheel over a button: switch its windows", ja: "ボタン上でホイール: ウィンドウ切り替え", pt: "Roda sobre um botão: alterna suas janelas"),
                        t!("В остальных местах колесо меняет громкость", "Elsewhere on the bar the wheel stays volume", ja: "それ以外の場所では音量を変更します", pt: "No resto da barra a roda continua sendo o volume"),
                    ),
                    Flag::Animations => (
                        t!("Анимация", "Animations", ja: "アニメーション", pt: "Animações"),
                        t!("Выкл.: панели и подсветка появляются сразу", "Off: panels and highlights appear at once", ja: "オフ: パネルや強調表示が即座に表示", pt: "Desligado: painéis e destaques aparecem na hora"),
                    ),
                    _ => continue, // Explorer switches live on the Explorer page
                };
                y = draw_toggle_row(dc, &mut hits, cx, y, pw, title, sub, i18n::style::flag(f), Hit::Flag(i), hot, f_semi, f_small);
            }
        }

        Tab::Personalize => {
            use i18n::style::{NUMS, Num, num};
            SelectObject(dc, f_title);
            text(dc, t!("Персонализация панели", "Personalize the bar", ja: "バーの個人用設定", pt: "Personalizar a barra"), RECT { left: cx, top: y, right: cx + pw, bottom: y + sc(28) }, pal().text, DT_LEFT);
            y += sc(38);
            let ni = |n: Num| NUMS.iter().position(|&x| x == n).unwrap_or(0);
            // a row of pills for choice `n` in [x, x + w)
            let pills = |dc: HDC, hits: &mut Vec<(RECT, Hit)>, n: Num, x: i32, y: i32, w: i32, labels: &[&str]| {
                let k = labels.len() as i32;
                let bw = (w - sc(6) * (k - 1)) / k;
                for (v, lbl) in labels.iter().enumerate() {
                    let r = RECT { left: x + v as i32 * (bw + sc(6)), top: y, right: x + v as i32 * (bw + sc(6)) + bw, bottom: y + sc(34) };
                    let hit = Hit::Num(ni(n), v as u8);
                    draw_pill(dc, r, lbl, num(n) == v as u8, hot == Some(hit), f_txt, None, false);
                    hits.push((r, hit));
                }
            };
            y = section(dc, y, t!("ШРИФТ ПАНЕЛИ", "BAR FONT", ja: "バーのフォント", pt: "FONTE DA BARRA"));
            let mut fonts: Vec<&str> = vec![t!("Как в теме", "Theme's", ja: "テーマ既定", pt: "Do tema")];
            fonts.extend(i18n::style::BAR_FONTS.iter().skip(1));
            pills(dc, &mut hits, Num::Font, cx, y, pw, &fonts);
            y += sc(46);
            let half = (pw - sc(16)) / 2;
            let x2 = cx + half + sc(16);
            SelectObject(dc, f_sec);
            text(dc, t!("РАЗМЕР ШРИФТА", "FONT SIZE", ja: "文字サイズ", pt: "TAMANHO DA FONTE"), RECT { left: cx, top: y, right: cx + half, bottom: y + sc(20) }, pal().dim, DT_LEFT);
            text(dc, t!("УГЛЫ КНОПОК", "BUTTON CORNERS", ja: "ボタンの角", pt: "CANTOS DOS BOTÕES"), RECT { left: x2, top: y, right: cx + pw, bottom: y + sc(20) }, pal().dim, DT_LEFT);
            y += sc(24);
            pills(dc, &mut hits, Num::FontSize, cx, y, half, &["A−", "A", "A+", "A++"]);
            pills(dc, &mut hits, Num::Radius, x2, y, half, &[t!("Тема", "Theme", ja: "テーマ", pt: "Tema"), t!("Прямые", "Square", ja: "四角", pt: "Retos"), t!("Малые", "Small", ja: "小", pt: "Leves"), t!("Круглые", "Round", ja: "丸", pt: "Redondos")]);
            y += sc(48);
            SelectObject(dc, f_sec);
            text(dc, t!("ЧАСЫ", "CLOCK", ja: "時計", pt: "RELÓGIO"), RECT { left: cx, top: y, right: cx + half, bottom: y + sc(20) }, pal().dim, DT_LEFT);
            text(dc, t!("ДАТА", "DATE", ja: "日付", pt: "DATA"), RECT { left: x2, top: y, right: cx + pw, bottom: y + sc(20) }, pal().dim, DT_LEFT);
            y += sc(24);
            let (c0, c1, c2) = (now_pic(true, ""), now_pic(true, "HH':'mm"), now_pic(true, "h':'mm tt"));
            pills(dc, &mut hits, Num::ClockFmt, cx, y, half, &[&c0, &c1, &c2]);
            let (d0, d1, d2, d3) = (now_pic(false, ""), now_pic(false, "d MMM"), now_pic(false, "yyyy'-'MM'-'dd"), now_pic(false, "ddd d"));
            pills(dc, &mut hits, Num::DateFmt, x2, y, half, &[&d0, &d1, &d2, &d3]);
            y += sc(40);
            SelectObject(dc, f_small);
            text(dc, t!("Первый вариант — формат Windows. Дату и секунды включают на странице «Настройки поведения».", "The first choice is Windows' own format. Date and seconds are switched on the Tweaks page.", ja: "最初の選択肢は Windows の形式です。日付と秒は「微調整」で切り替えます。", pt: "A primeira opção é o formato do Windows. Data e segundos ficam na página Ajustes."), RECT { left: cx, top: y, right: cx + pw, bottom: y + sc(18) }, pal().dim, DT_LEFT);
            y += sc(30);
            y = section(dc, y, t!("РАССТОЯНИЕ МЕЖДУ ЗНАЧКАМИ", "ICON SPACING", ja: "アイコンの間隔", pt: "ESPAÇAMENTO DOS ÍCONES"));
            pills(dc, &mut hits, Num::IconGap, cx, y, half, &[t!("Плотно", "Tight", ja: "狭い", pt: "Justo"), t!("Обычно", "Normal", ja: "標準", pt: "Normal"), t!("Широко", "Wide", ja: "広い", pt: "Largo")]);
        }

        Tab::Tray => {
            use i18n::style::Flag;
            SelectObject(dc, f_title);
            text(dc, t!("Значки в трее", "Tray icons", ja: "トレイ アイコン", pt: "Ícones da bandeja"), RECT { left: cx, top: y, right: cx + pw, bottom: y + sc(28) }, pal().text, DT_LEFT);
            y += sc(38);
            y = section(dc, y, t!("СИСТЕМНЫЕ ЗНАЧКИ", "SYSTEM ICONS", ja: "システム アイコン", pt: "ÍCONES DO SISTEMA"));
            let fi = |f: Flag| Hit::Flag(i18n::style::FLAGS.iter().position(|&x| x == f).unwrap_or(0));
            let rows: [(&str, &str, Flag); 4] = [
                (t!("Язык ввода", "Input language", ja: "入力言語", pt: "Idioma de entrada"), t!("Щелчок — выбор раскладки", "Click to pick a layout", ja: "クリックでレイアウト選択", pt: "Clique para trocar o layout"), Flag::ShowLang),
                (t!("Сеть", "Network", ja: "ネットワーク", pt: "Rede"), t!("Wi‑Fi или кабель", "Wi‑Fi or cable", ja: "Wi‑Fi または有線", pt: "Wi‑Fi ou cabo"), Flag::ShowNet),
                (t!("Громкость", "Volume", ja: "音量", pt: "Volume"), t!("Колесо над панелью тоже меняет её", "The wheel over the bar changes it too", ja: "バー上のホイールでも変更", pt: "A roda sobre a barra também muda"), Flag::ShowVol),
                (t!("Батарея", "Battery", ja: "バッテリー", pt: "Bateria"), t!("Только на ноутбуке", "Only on a laptop", ja: "ノート PC のみ", pt: "Só em notebook"), Flag::ShowBat),
            ];
            let cw2 = (pw - sc(8)) / 2;
            let mut bottom = y;
            for (k, (title, sub, f)) in rows.iter().enumerate() {
                let x = cx + (k as i32 % 2) * (cw2 + sc(8));
                let ry = y + (k as i32 / 2) * sc(60);
                bottom = draw_toggle_row(dc, &mut hits, x, ry, cw2, title, sub, i18n::style::flag(*f), fi(*f), hot, f_semi, f_small);
            }
            y = bottom + sc(4);
            y = section(dc, y, t!("ЗНАЧКИ ПРИЛОЖЕНИЙ (ВКЛ. — НА ПАНЕЛИ, ВЫКЛ. — В МЕНЮ ˄)", "APP ICONS (ON = ON THE BAR, OFF = IN THE ˄ MENU)", ja: "アプリ アイコン（オン = バー、オフ = ˄ メニュー）", pt: "ÍCONES DE APPS (LIGADO = NA BARRA, DESLIGADO = NO MENU ˄)"));
            let mut keys: Vec<String> = TAPPS.with_borrow(|t| t.iter().filter(|x| !x.hidden).map(|x| x.key.clone()).collect());
            keys.sort();
            keys.dedup();
            let on_bar: Vec<String> = TAPPS.with_borrow(|t| bar_idx().into_iter().map(|i| t[i].key.clone()).collect());
            if keys.is_empty() {
                SelectObject(dc, f_small);
                text(dc, t!("Сейчас в трее нет значков приложений", "No app has a tray icon right now", ja: "現在トレイにアプリのアイコンはありません", pt: "Nenhum app tem ícone na bandeja agora"), RECT { left: cx, top: y, right: cx + pw, bottom: y + sc(20) }, pal().dim, DT_LEFT);
            }
            let bw = (pw - sc(16)) / 3;
            for (k, key) in keys.iter().enumerate().take(12) {
                let (col, row) = (k as i32 % 3, k as i32 / 3);
                let r = RECT { left: cx + col * (bw + sc(8)), top: y + row * sc(40), right: cx + col * (bw + sc(8)) + bw, bottom: y + row * sc(40) + sc(34) };
                let name = key.strip_suffix(".exe").unwrap_or(key);
                draw_pill(dc, r, name, on_bar.contains(key), hot == Some(Hit::TrayApp(k)), f_txt, None, false);
                hits.push((r, Hit::TrayApp(k)));
            }
            TRAY_KEYS.set(keys);
        }

        Tab::Widgets => {
            use i18n::style::Flag;
            SelectObject(dc, f_title);
            text(dc, t!("Виджеты", "Widgets", ja: "ウィジェット", pt: "Widgets"), RECT { left: cx, top: y, right: cx + pw, bottom: y + sc(28) }, pal().text, DT_LEFT);
            y += sc(38);
            let fi = |f: Flag| Hit::Flag(i18n::style::FLAGS.iter().position(|&x| x == f).unwrap_or(0));
            let cw2 = (pw - sc(8)) / 2;
            let grid = |dc: HDC, hits: &mut Vec<(RECT, Hit)>, y: i32, rows: &[(&str, &str, Flag)]| -> i32 {
                let mut bottom = y;
                for (k, (title, sub, f)) in rows.iter().enumerate() {
                    let x = cx + (k as i32 % 2) * (cw2 + sc(8));
                    let ry = y + (k as i32 / 2) * sc(60);
                    bottom = draw_toggle_row(dc, hits, x, ry, cw2, title, sub, i18n::style::flag(*f), fi(*f), hot, f_semi, f_small);
                }
                bottom
            };
            y = section(dc, y, t!("НА ПАНЕЛИ", "ON THE BAR", ja: "バー上", pt: "NA BARRA"));
            y = grid(dc, &mut hits, y, &[
                (t!("Кнопка виджетов", "Widgets button", ja: "ウィジェット ボタン", pt: "Botão de widgets"), t!("Рядом с шестерёнкой", "Next to the gear", ja: "歯車の隣", pt: "Ao lado da engrenagem"), Flag::WidgetsBtn),
                (t!("Мини-виджет ЦП", "Mini CPU widget", ja: "ミニ CPU", pt: "Mini widget de CPU"), t!("Загрузка процессора на панели", "Processor load on the bar", ja: "バーに CPU 使用率", pt: "Uso do processador na barra"), Flag::MiniCpu),
                (t!("Мини-виджет ОЗУ", "Mini RAM widget", ja: "ミニ メモリ", pt: "Mini widget de RAM"), t!("Занятая память на панели", "Memory in use on the bar", ja: "バーにメモリ使用量", pt: "Memória em uso na barra"), Flag::MiniRam),
            ]);
            y += sc(4);
            y = section(dc, y, t!("В ПАНЕЛИ ВИДЖЕТОВ", "IN THE WIDGETS PANEL", ja: "ウィジェット パネル内", pt: "NO PAINEL DE WIDGETS"));
            y = grid(dc, &mut hits, y, &[
                (t!("Процессор", "CPU", ja: "CPU", pt: "CPU"), t!("График за минуту", "A minute of history", ja: "1 分間のグラフ", pt: "Gráfico de um minuto"), Flag::WCpu),
                (t!("Память", "Memory", ja: "メモリ", pt: "Memória"), t!("Занято из всего", "Used of the total", ja: "使用量 / 合計", pt: "Usada do total"), Flag::WRam),
                (t!("Диск", "Disk", ja: "ディスク", pt: "Disco"), t!("Чтение и запись, место на C:", "Reads and writes, space on C:", ja: "読み書きと C: の容量", pt: "Leitura e escrita, espaço no C:"), Flag::WDisk),
                (t!("Сеть", "Network", ja: "ネットワーク", pt: "Rede"), t!("Приём и передача", "Download and upload", ja: "受信と送信", pt: "Download e upload"), Flag::WNet),
                (t!("Календарь", "Calendar", ja: "カレンダー", pt: "Calendário"), t!("Текущий месяц", "The current month", ja: "今月", pt: "O mês atual"), Flag::WCal),
                (t!("Мультимедиа", "Media", ja: "メディア", pt: "Mídia"), t!("Что играет: пауза, следующий", "What's playing: pause, next", ja: "再生中: 一時停止、次へ", pt: "O que está tocando: pausar, próxima"), Flag::WMedia),
            ]);
            let or = RECT { left: cx, top: y + sc(2), right: cx + sc(210), bottom: y + sc(36) };
            draw_pill(dc, or, t!("Открыть виджеты", "Open widgets", ja: "ウィジェットを開く", pt: "Abrir widgets"), false, hot == Some(Hit::OpenWidgets), f_semi, None, false);
            hits.push((or, Hit::OpenWidgets));
        }

        Tab::Language => {
            use i18n::Lang::*;
            SelectObject(dc, f_title);
            text(dc, t!("Язык", "Language", ja: "言語", pt: "Idioma"), RECT { left: cx, top: y, right: cx + pw, bottom: y + sc(28) }, pal().text, DT_LEFT);
            y += sc(38);
            y = section(dc, y, t!("ЯЗЫК ВСЕХ ПРОГРАММ WINRUST", "THE LANGUAGE OF EVERY WINRUST PROGRAM", ja: "すべての WINRUST プログラムの言語", pt: "O IDIOMA DE TODOS OS PROGRAMAS WINRUST"));
            let bw = (pw - sc(8)) / 2;
            for (k, l) in [En, Ru, Ja, Pt, Zh].into_iter().enumerate() {
                let (col, row) = (k as i32 % 2, k as i32 / 2);
                let r = RECT { left: cx + col * (bw + sc(8)), top: y + row * sc(48), right: cx + col * (bw + sc(8)) + bw, bottom: y + row * sc(48) + sc(40) };
                draw_pill(dc, r, l.name(), l == i18n::lang(), hot == Some(Hit::Lang(k as u8)), f_semi, None, false);
                hits.push((r, Hit::Lang(k as u8)));
            }
            y += sc(152); // three rows of pills
            SelectObject(dc, f_small);
            text(dc, t!("Панель и это окно переключаются сразу, остальные программы — при следующем запуске.", "The bar and this window switch at once, the other programs when they next start.", ja: "バーとこのウィンドウはすぐに、他のプログラムは次回起動時に切り替わります。", pt: "A barra e esta janela mudam na hora; os outros programas, ao abrir de novo."), RECT { left: cx, top: y, right: cx + pw, bottom: y + sc(20) }, pal().dim, DT_LEFT);
        }

        Tab::Advanced => {
            SelectObject(dc, f_title);
            text(dc, t!("Дополнительно", "Advanced", ja: "詳細設定", pt: "Avançado"), RECT { left: cx, top: y, right: cx + pw, bottom: y + sc(28) }, pal().text, DT_LEFT);
            y += sc(38);

            y = section(dc, y, t!("СБРОС НАСТРОЕК", "RESET", ja: "リセット", pt: "RESTAURAR PADRÕES"));
            let card_r = RECT { left: cx, top: y, right: cx + pw, bottom: y + sc(100) };
            fill(dc, card_r, pal().line, rr(10));
            fill(dc, RECT { left: card_r.left + 1, top: card_r.top + 1, right: card_r.right - 1, bottom: card_r.bottom - 1 }, card(), rr(9));
            SelectObject(dc, f_semi);
            text(dc, t!("Сбросить все настройки WinRust", "Reset all WinRust settings", ja: "WinRust の設定をすべて初期化", pt: "Restaurar todas as configurações do WinRust"), RECT { left: card_r.left + sc(16), top: card_r.top + sc(14), right: card_r.right - sc(16), bottom: card_r.top + sc(34) }, pal().text, DT_LEFT);
            SelectObject(dc, f_small);
            text(dc, t!("Вернёт тему, цвета, шрифт, часы, значки, виджеты, панель и проводник к умолчаниям. Шаблоны и язык останутся.", "Restores theme, colors, font, clock, icons, widgets, taskbar and Explorer to defaults. Templates and language stay.", ja: "テーマ、色、フォント、時計、アイコン、ウィジェット、バー、エクスプローラーを初期化します。テンプレートと言語は残ります。", pt: "Volta tema, cores, fonte, relógio, ícones, widgets, barra e Explorer ao padrão. Modelos e idioma permanecem."), RECT { left: card_r.left + sc(16), top: card_r.top + sc(36), right: card_r.right - sc(16), bottom: card_r.top + sc(56) }, pal().dim, DT_LEFT);
            let br = RECT { left: card_r.left + sc(16), top: card_r.top + sc(60), right: card_r.left + sc(240), bottom: card_r.top + sc(90) };
            let lbl = if CONFIRM_RESET.get() {
                t!("Нажмите ещё раз для сброса", "Click again to confirm reset", ja: "もう一度クリックして確定", pt: "Clique novamente para confirmar")
            } else {
                t!("Сбросить по умолчанию", "Reset to defaults", ja: "初期設定に戻す", pt: "Restaurar tudo ao padrão")
            };
            draw_pill(dc, br, lbl, false, hot == Some(Hit::ResetAll), f_semi, None, true);
            hits.push((br, Hit::ResetAll));
            y += sc(120);

            y = section(dc, y, t!("О ПРОГРАММЕ", "ABOUT", ja: "情報", pt: "SOBRE"));
            SelectObject(dc, f_small);
            let cfg_dir = std::env::var("ProgramData").map(|p| format!(r"{p}\WinRust")).unwrap_or_else(|_| r"%ProgramData%\WinRust".into());
            text(dc, &format!("Config: {cfg_dir}"), RECT { left: cx, top: y, right: cx + pw, bottom: y + sc(20) }, pal().dim, DT_LEFT);
        }
    }

    HITS.set(hits);
    BitBlt(hdc, 0, 0, cw, ch, dc, 0, 0, SRCCOPY);
    SelectObject(dc, old);
    for f in [f_title, f_sec, f_txt, f_semi, f_small, f_glyph] {
        DeleteObject(f as HGDIOBJ);
    }
    DeleteObject(bmp as HGDIOBJ);
    DeleteDC(dc);
    EndPaint(hw, &ps);
}

unsafe fn on_click(hw: HWND, hit: Hit) {
    use i18n::style::{self as st, Side};
    if !matches!(hit, Hit::ResetAll) {
        CONFIRM_RESET.set(false);
    }
    match hit {
        Hit::Tab(i) => {
            TAB.set(TABS.get(i).copied().unwrap_or(Tab::Taskbar));
            InvalidateRect(hw, null(), 0);
        }
        Hit::Num(i, v) => {
            if let Some(&n) = st::NUMS.get(i) {
                st::set_num(n, v);
            }
            relook(true); // font size, clock width and spacing move things on the bar
        }
        Hit::Lang(k) => {
            use i18n::Lang::*;
            i18n::set_lang([En, Ru, Ja, Pt, Zh][k as usize % 5]);
            SetWindowTextW(hw, wide(settings_title()).as_ptr());
            relook(false);
        }
        Hit::TrayApp(k) => {
            if let Some(key) = TRAY_KEYS.with_borrow(|t| t.get(k).cloned()) {
                toggle_shown(&key);
            }
            relook(true);
        }
        Hit::OpenWidgets => fly::toggle(fly::Kind::Widgets),
        Hit::Theme(k) => {
            if let Some(&s) = st::ALL.get(k) {
                st::set_style(s);
                relook(false);
            }
        }
        Hit::Opacity(v) => {
            st::set_bar_opacity(v);
            relook(false);
        }
        Hit::Color(idx) => {
            let p = pal();
            let now = [p.bg, p.line, p.hover, p.active, p.accent, p.text, p.dim];
            static CUST: std::sync::Mutex<[u32; 16]> = std::sync::Mutex::new([0xFFFFFF; 16]);
            let mut cust = CUST.lock().unwrap();
            let mut cc: CHOOSECOLORW = zeroed();
            cc.lStructSize = size_of::<CHOOSECOLORW>() as u32;
            cc.hwndOwner = hw;
            cc.rgbResult = now[idx];
            cc.lpCustColors = cust.as_mut_ptr();
            cc.Flags = CC_RGBINIT | CC_FULLOPEN;
            if ChooseColorW(&mut cc) != 0 {
                st::set_custom_color(idx, Some(cc.rgbResult));
                relook(false);
            }
        }
        Hit::ResetColors => {
            st::reset_custom();
            relook(false);
        }
        Hit::SaveTemplate => {
            let mut list = st::themes();
            let name = |n: usize| tf!("Шаблон {}", "Template {}", ja: "テンプレート {}", pt: "Modelo {}", n);
            let n = (1..).find(|&n| !list.iter().any(|t| t.name == name(n))).unwrap_or(1);
            list.push(st::Theme::current(&name(n)));
            st::save_themes(&list);
            InvalidateRect(hw, null(), 0);
        }
        Hit::ApplyTemplate(k) => {
            if let Some(th) = st::themes().get(k) {
                th.apply();
                relook(false);
            }
        }
        Hit::DelTemplate(k) => {
            let mut list = st::themes();
            if k < list.len() {
                list.remove(k);
                st::save_themes(&list);
                InvalidateRect(hw, null(), 0);
            }
        }
        Hit::Side(id) => {
            st::set_side(match id { 1 => Side::Left, 2 => Side::Right, _ => Side::Bottom });
            relook(true);
        }
        Hit::BarSize(sz) => {
            st::set_bar_size(sz);
            relook(true);
        }
        Hit::AlignLeft(on) => {
            st::set_align_left(on);
            relook(true);
        }
        Hit::ToggleAutoHide => {
            st::set_autohide(!st::autohide());
            relook(true);
        }
        Hit::ToggleSearchBtn => {
            st::set_show_search_btn(!st::show_search_btn());
            relook(false);
        }
        Hit::ToggleAltTab => {
            st::set_alttab_per_monitor(!st::alttab_per_monitor());
            InvalidateRect(hw, null(), 0);
        }
        Hit::TileWindows => tile_windows(),
        Hit::Flag(i) => {
            if let Some(&f) = st::FLAGS.get(i) {
                st::set_flag(f, !st::flag(f));
            }
            relook(true); // system icons and widgets change the bar's layout
        }
        Hit::ToggleUseRustExplorer => {
            st::set_use_rust_explorer(!st::use_rust_explorer());
            InvalidateRect(hw, null(), 0);
        }
        Hit::ExplorerStart(v) => {
            st::set_explorer_start(v);
            InvalidateRect(hw, null(), 0);
        }
        Hit::ToggleHidden => {
            st::set_explorer_hidden(!st::explorer_hidden());
            InvalidateRect(hw, null(), 0);
        }
        Hit::ToggleGroupDate => {
            set_group_by_date(!group_by_date());
            InvalidateRect(hw, null(), 0);
        }
        Hit::OpenExplorer => {
            if let Some(rx) = rust_explorer_exe() {
                open(&wide(&rx.to_string_lossy()));
            } else {
                let _ = Command::new("explorer.exe").spawn();
            }
        }
        Hit::ResetAll => {
            if CONFIRM_RESET.replace(true) {
                CONFIRM_RESET.set(false);
                st::reset_all();
                // the tray apps kept on the bar go back into the chevron menu
                let _ = std::fs::remove_file(appdata_file("topbar-tray.txt"));
                SHOWN.set(None);
                relook(true);
            } else {
                InvalidateRect(hw, null(), 0);
            }
        }
    }
}

fn hit_at(x: i32, y: i32) -> Option<Hit> {
    HITS.with_borrow(|hs| hs.iter().find(|(r, _)| x >= r.left && x < r.right && y >= r.top && y < r.bottom).map(|(_, h)| *h))
}

unsafe extern "system" fn settings_proc(hw: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => paint_settings(hw),
        WM_ERASEBKGND => return 1,
        WM_MOUSEMOVE => {
            let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
            let h = hit_at(x, y);
            if HOVER.replace(h) != h {
                InvalidateRect(hw, null(), 0);
            }
        }
        WM_LBUTTONDOWN => {
            let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
            if let Some(h) = hit_at(x, y) {
                on_click(hw, h);
            }
        }
        WM_KEYDOWN if wp == VK_ESCAPE as usize => {
            DestroyWindow(hw);
        }
        WM_DESTROY => {
            WIN.set(null_mut());
            HOVER.set(None);
            CONFIRM_RESET.set(false);
        }
        _ => return DefWindowProcW(hw, msg, wp, lp),
    }
    0
}

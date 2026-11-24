//! topbar: tray (split out of main.rs; shares its items through `super::*`).
use super::*;

/// First top-level window of the class that is not our own tray host (explorer's real taskbar).
pub(crate) unsafe fn find_other(class: &str) -> HWND {
    let (c, mut h) = (wide(class), null_mut());
    loop {
        h = FindWindowExW(null_mut(), h, c.as_ptr(), null());
        if h.is_null() || h != HOST.get() {
            return h;
        }
    }
}

pub(crate) unsafe fn tray() -> [HWND; 2] {
    [find_other("Shell_TrayWnd"), find_other("Shell_SecondaryTrayWnd")]
}

/// Tells every app to re-register its tray icon with whichever Shell_TrayWnd is found first.
pub(crate) unsafe fn announce() {
    PostMessageW(HWND_BROADCAST, RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()), 0, 0);
}

pub(crate) fn hide_onedrive() -> bool {
    // topbar.cfg sits next to the exe (the launcher writes it); no file or no line = OneDrive hidden
    std::env::current_exe().ok().and_then(|e| std::fs::read_to_string(e.with_file_name("topbar.cfg")).ok()).is_none_or(|c| !c.contains("hide_onedrive=0"))
}

/// Explorer's volume, network and power icons ({7820AE73/74/75-23E3-4229-82C1-E41CB67D5B9C}): the bar draws those itself.
/// Its other icons (printer queue, Safely Remove Hardware, microphone in use) stay (#127).
fn own_icon(guid: &[u8; 16]) -> bool {
    matches!(guid[0], 0x73..=0x75) && guid[1..] == [0xAE, 0x20, 0x78, 0xE3, 0x23, 0x29, 0x42, 0x82, 0xC1, 0xE4, 0x1C, 0xB6, 0x7D, 0x5B, 0x9C]
}

/// Matches an existing tray icon entry following Shell_NotifyIcon rules:
/// when both have a non-zero GUID, match by GUID; otherwise match by (hwnd, uid) so NIM_SETVERSION / NIM_MODIFY
/// calls that omit NIF_GUID still update the registered icon.
fn match_tray_pos(t: &[TIcon], h: HWND, uid: u32, guid: [u8; 16], has_guid: bool) -> Option<usize> {
    t.iter().position(|x| {
        if has_guid && x.guid != [0; 16] {
            x.guid == guid
        } else {
            x.hwnd == h && x.uid == uid
        }
    })
}

/// WM_COPYDATA (dwData 1) from Shell_NotifyIcon: u32 magic (0x34753423), u32 NIM_*, then the platform-neutral NOTIFYICONDATA
/// (32-bit handles): hWnd@12 uID@16 uFlags@20 callback@24 hIcon@28 szTip@32 dwState@288 mask@292 version@808 guid@944.
pub(crate) const SHELL_TRAY_MAGIC: u32 = 0x3475_3423;

pub(crate) unsafe fn tray_msg(d: &[u8]) {
    if d.len() < 32 {
        return;
    }
    let u = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
    if u(0) != SHELL_TRAY_MAGIC {
        return;
    }
    let (op, flags, uid) = (u(4), u(20), u(16));
    let h = u(12) as i32 as isize as HWND;
    let has_guid = flags & 0x20 != 0 && d.len() >= 960;
    let guid: [u8; 16] = if has_guid { d[944..960].try_into().unwrap() } else { [0; 16] };
    TAPPS.with_borrow_mut(|t| {
        let pos = match_tray_pos(t, h, uid, guid, has_guid);
        match (op, pos) {
            (2, Some(i)) => {
                DestroyIcon(t.remove(i).icon);
            }
            (4, Some(i)) if d.len() >= 812 => t[i].ver = u(808),
            (0 | 1, _) => {
                let i = match pos {
                    Some(i) => i,
                    None if op == 1 => return, // modify of an icon we never saw
                    None => {
                        let exe = exe_of(h);
                        let n = exe.rsplit('\\').next().unwrap_or("");
                        if (n == "explorer.exe" && own_icon(&guid)) || n == "topbar.exe" || (n == "onedrive.exe" && hide_onedrive()) {
                            return; // the system's own icons are drawn by the bar itself
                        }
                        // Never collide protected/unreadable processes under empty key ""
                        let key = if n.is_empty() { format!("hwnd:{:x}", h as usize) } else { n.to_string() };
                        t.push(TIcon { hwnd: h, uid, guid, icon: null_mut(), cb: 0, ver: 0, hidden: false, tip: String::new(), key });
                        t.len() - 1
                    }
                };
                let x = &mut t[i];
                if has_guid && x.guid == [0; 16] {
                    x.guid = guid;
                }
                if flags & 1 != 0 {
                    x.cb = u(24);
                }
                if flags & 2 != 0 {
                    let ic = CopyIcon(u(28) as i32 as isize as HICON);
                    if !ic.is_null() {
                        if !x.icon.is_null() {
                            DestroyIcon(x.icon);
                        }
                        x.icon = ic;
                    }
                }
                if flags & 4 != 0 && d.len() > 32 {
                    let end = d.len().min(288);
                    let w: Vec<u16> = d[32..end].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
                    x.tip = String::from_utf16_lossy(&w);
                }
                if flags & 8 != 0 && d.len() >= 296 && u(292) & 1 != 0 {
                    x.hidden = u(288) & 1 != 0; // NIS_HIDDEN
                }
            }
            _ => {}
        }
    });
}

pub(crate) unsafe extern "system" fn tray_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_COPYDATA {
        if lp == 0 {
            return 0;
        }
        let c = &*(lp as *const Cds);
        if c.dw == 1 && c.cb >= 160 && !c.p.is_null() {
            tray_msg(std::slice::from_raw_parts(c.p, c.cb as usize));
            return 1;
        }
        // SHAppBarMessage (our own bar's work-area reservation, ABM_SETSTATE...) also goes to "Shell_TrayWnd": hand it to explorer's
        // with a timeout: a hung explorer must not freeze the bar
        let mut r = 0usize;
        SendMessageTimeoutW(find_other("Shell_TrayWnd"), msg, wp, lp, SMTO_ABORTIFHUNG, 2000, &mut r);
        return r as LRESULT;
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// Computes the exact mouse/semantic events to post to a tray icon owner for single/double left click or right click.
pub(crate) fn tray_click_events(ver: u32, left: bool, dbl: bool) -> &'static [u32] {
    // what Explorer sends: the raw button messages always, plus NIN_SELECT / WM_CONTEXTMENU for version 4. Many apps open
    // only on the button-up or the double click (Steam), so NIN_SELECT alone left them dead (#115).
    match (left, dbl, ver >= 4) {
        (true, true, _) => &[WM_LBUTTONDBLCLK, WM_LBUTTONUP],
        (true, false, true) => &[WM_LBUTTONDOWN, WM_LBUTTONUP, 0x400 /* NIN_SELECT */],
        (true, false, false) => &[WM_LBUTTONDOWN, WM_LBUTTONUP],
        (false, _, true) => &[WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU],
        (false, _, false) => &[WM_RBUTTONDOWN, WM_RBUTTONUP],
    }
}

/// Posts the same messages explorer would for a click (or double-click) on the icon; returns false if PostMessageW failed (e.g. UIPI).
pub(crate) unsafe fn tray_click(t: (HWND, u32, u32, u32), left: bool, dbl: bool) -> bool {
    let (hwnd, uid, cb, ver) = t;
    if cb == 0 {
        return false;
    }
    // ASFW_ANY (u32::MAX): apps with a hidden tray message-only window often open their popup menu from another thread/process.
    AllowSetForegroundWindow(u32::MAX);
    let mut pt: POINT = zeroed();
    GetCursorPos(&mut pt);
    let mut ok = true;
    for &ev in tray_click_events(ver, left, dbl) {
        let r = if ver >= 4 {
            PostMessageW(hwnd, cb, ((pt.x as u32 & 0xffff) | (pt.y as u32) << 16) as usize, (ev | (uid & 0xffff) << 16) as isize)
        } else {
            PostMessageW(hwnd, cb, uid as usize, ev as isize)
        };
        if r == 0 {
            ok = false;
        }
    }
    ok
}

/// Indices into TAPPS of the icons kept on the bar; the rest live in the chevron flyout.
pub(crate) fn bar_idx() -> Vec<usize> {
    let shown = SHOWN.with_borrow(|s| s.clone());
    TAPPS.with_borrow(|t| t.iter().enumerate().filter(|(_, x)| !x.hidden && shown.as_ref().is_some_and(|s| s.contains(&x.key))).map(|(i, _)| i).collect())
}
/// Bar slots of the tray: the chevron plus the icons kept on the bar.
pub(crate) fn tray_count() -> usize {
    bar_idx().len() + 1
}

/// Indices into TAPPS of the icons only the flyout shows (everything not kept on the bar).
pub(crate) fn over_idx() -> Vec<usize> {
    let bar = bar_idx();
    TAPPS.with_borrow(|t| (0..t.len()).filter(|i| !t[*i].hidden && !bar.contains(i)).collect())
}

pub(crate) fn toggle_shown(key: &str) {
    let mut cur = SHOWN.with_borrow(|s| s.clone()).unwrap_or_else(|| TAPPS.with_borrow(|t| bar_idx().into_iter().map(|i| t[i].key.clone()).collect()));
    match cur.iter().position(|k| k == key) {
        Some(p) => drop(cur.remove(p)),
        None => cur.push(key.to_string()),
    }
    let _ = std::fs::write(appdata_file("topbar-tray.txt"), cur.join("\n"));
    SHOWN.set(Some(cur));
}

// ---------- tray overflow flyout (the chevron): every icon, click / right click work as in the real tray ----------

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(hwnd: usize, uid: u32, guid: [u8; 16]) -> TIcon {
        TIcon { hwnd: hwnd as HWND, uid, guid, icon: null_mut(), cb: 0x400, ver: 0, hidden: false, tip: String::new(), key: "app.exe".into() }
    }

    #[test]
    fn tray_match_handles_guid_and_setversion_without_guid() {
        let g = [7u8; 16];
        let icons = vec![mk(0x100, 1, g), mk(0x200, 2, [0; 16])];
        // Match by GUID even if hwnd/uid differs
        assert_eq!(match_tray_pos(&icons, 0x999 as HWND, 9, g, true), Some(0));
        // NIM_SETVERSION / NIM_MODIFY omitting NIF_GUID still finds the icon by (hwnd, uid)
        assert_eq!(match_tray_pos(&icons, 0x100 as HWND, 1, [0; 16], false), Some(0));
        // Plain legacy icon without GUID matches by (hwnd, uid)
        assert_eq!(match_tray_pos(&icons, 0x200 as HWND, 2, [0; 16], false), Some(1));
        assert_eq!(match_tray_pos(&icons, 0x300 as HWND, 1, [0; 16], false), None);
    }

    #[test]
    fn tray_click_events_cover_double_click_and_versions() {
        assert_eq!(tray_click_events(0, true, false), &[WM_LBUTTONDOWN, WM_LBUTTONUP]);
        assert_eq!(tray_click_events(0, true, true), &[WM_LBUTTONDBLCLK, WM_LBUTTONUP]);
        assert_eq!(tray_click_events(4, true, false), &[WM_LBUTTONDOWN, WM_LBUTTONUP, 0x400]);
        assert_eq!(tray_click_events(4, true, true), &[WM_LBUTTONDBLCLK, WM_LBUTTONUP]);
        assert_eq!(tray_click_events(0, false, false), &[WM_RBUTTONDOWN, WM_RBUTTONUP]);
        assert_eq!(tray_click_events(4, false, false), &[WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU]);
    }
}

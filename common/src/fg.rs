//! Bringing our own popup (Start, the search card) to the front with the keyboard focus. Windows refuses the foreground to a
//! process that did not get the last input: the popup then shows, but typing goes to the old window (#72). Used by topbar
//! and rs-launcher; call it on the thread that owns `h`.
use windows_sys::Win32::{
    Foundation::HWND,
    System::Threading::{AttachThreadInput, GetCurrentThreadId},
    UI::{
        Input::KeyboardAndMouse::{INPUT, INPUT_KEYBOARD, KEYEVENTF_KEYUP, SendInput, SetFocus, VK_MENU},
        WindowsAndMessaging::{
            AllowSetForegroundWindow, BringWindowToTop, GetForegroundWindow, GetLastActivePopup, GetWindowThreadProcessId, IsIconic,
            IsWindowVisible, SW_RESTORE, SetForegroundWindow, ShowWindow, SwitchToThisWindow,
        },
    },
};

/// Another program's window to the front (a taskbar button, Alt+Tab), restored if minimized. ShowWindow on a minimized
/// window of a process above ours (an app run as administrator, or the UAC prompt that waits minimized behind its taskbar
/// button when it was asked for from the background) fails with access denied: the button did nothing and the UAC prompt
/// never came up. SwitchToThisWindow (what Windows' own Alt+Tab uses) is allowed to restore those.
pub fn raise(h: HWND) {
    let target = unsafe {
        if IsIconic(h) != 0 && (ShowWindow(h, SW_RESTORE) == 0 || IsIconic(h) != 0) {
            SwitchToThisWindow(h, 1);
        }
        // If `h` owns an active modal/popup dialog, `h` itself is disabled and refuses focus: raise the popup instead.
        let p = GetLastActivePopup(h);
        if !p.is_null() && p != h && IsWindowVisible(p) != 0 { p } else { h }
    };
    force(target);
}

pub fn force(h: HWND) {
    unsafe {
        AllowSetForegroundWindow(u32::MAX);
        // a click on our own window already gives us the right; attaching while its button is down would hand it back
        if SetForegroundWindow(h) != 0 && GetForegroundWindow() == h {
            BringWindowToTop(h);
            let me = GetCurrentThreadId();
            if GetWindowThreadProcessId(h, std::ptr::null_mut()) == me {
                SetFocus(h);
            }
            return;
        }
        // 1) share the input queue of both the current foreground thread and the target thread for a moment
        let me = GetCurrentThreadId();
        let ft = GetWindowThreadProcessId(GetForegroundWindow(), std::ptr::null_mut());
        let tt = GetWindowThreadProcessId(h, std::ptr::null_mut());
        let attach_ft = ft != 0 && ft != me && AttachThreadInput(me, ft, 1) != 0;
        let attach_tt = tt != 0 && tt != me && tt != ft && AttachThreadInput(me, tt, 1) != 0;
        BringWindowToTop(h);
        SetForegroundWindow(h);
        if tt == me || attach_tt || (tt == ft && attach_ft) {
            SetFocus(h);
        }
        if attach_tt {
            AttachThreadInput(me, tt, 0);
        }
        if attach_ft {
            AttachThreadInput(me, ft, 0);
        }
        // 2) nothing to attach to (looking at the desktop: no foreground window) or an elevated window in front (UIPI refuses
        // the attach): Windows lets the foreground change while Alt is down. 0xE8 (unassigned) in between, so the release is
        // not a lone Alt that opens a menu bar.
        if GetForegroundWindow() != h {
            keys(&[(VK_MENU, false), (0xE8, false), (0xE8, true)]);
            BringWindowToTop(h);
            SetForegroundWindow(h);
            keys(&[(VK_MENU, true)]);
        }
        if tt == me {
            SetFocus(h); // detaching drops this thread's keyboard focus; without it our own popup gets no typing
        }
    }
}

fn keys(k: &[(u16, bool)]) {
    for &(vk, up) in k {
        let mut i: INPUT = unsafe { std::mem::zeroed() };
        i.r#type = INPUT_KEYBOARD;
        i.Anonymous.ki.wVk = vk;
        i.Anonymous.ki.dwFlags = if up { KEYEVENTF_KEYUP } else { 0 };
        unsafe { SendInput(1, &i, size_of::<INPUT>() as i32) };
    }
}

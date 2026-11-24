//! Animation frames in step with the display. `start(hwnd, id)` makes the window get `WM_TIMER` with `wParam = id` once
//! per screen refresh (60, 120, 144 Hz...), until `stop(hwnd, id)`; a drop-in for `SetTimer(hwnd, id, 8..16, None)` /
//! `KillTimer(hwnd, id)` on animation timers.
//!
//! Why: a `SetTimer` period is rounded to the 15.6 ms system tick and drifts against the monitor's refresh, so every
//! second or so a frame is shown twice or skipped (visible stutter in slides and fades). One shared thread waits for
//! each composition with `DwmFlush` and then hands the frame to every animating window. The message is sent (not posted)
//! with a short timeout, so a slow window never piles up frames and a hung one never holds the others up.
use std::sync::{Condvar, Mutex, Once};
use windows_sys::Win32::{
    Foundation::HWND,
    Graphics::Dwm::DwmFlush,
    UI::WindowsAndMessaging::{IsWindow, SMTO_ABORTIFHUNG, SMTO_NORMAL, SendMessageTimeoutW, WM_TIMER},
};

static WANT: Mutex<Vec<(isize, usize)>> = Mutex::new(Vec::new());
static WAKE: Condvar = Condvar::new();
static PUMP: Once = Once::new();

/// Frames for (`hwnd`, `id`) from the next refresh on. Calling it again while running changes nothing.
pub fn start(hwnd: HWND, id: usize) {
    PUMP.call_once(|| {
        std::thread::spawn(pump);
    });
    let mut w = WANT.lock().unwrap();
    if !w.contains(&(hwnd as isize, id)) {
        w.push((hwnd as isize, id));
        WAKE.notify_one();
    }
}

/// No more frames for (`hwnd`, `id`). A frame already on its way may still arrive once.
pub fn stop(hwnd: HWND, id: usize) {
    WANT.lock().unwrap().retain(|&e| e != (hwnd as isize, id));
}

/// Whether (`hwnd`, `id`) is getting frames.
pub fn running(hwnd: HWND, id: usize) -> bool {
    WANT.lock().unwrap().contains(&(hwnd as isize, id))
}

fn pump() {
    loop {
        let now: Vec<(isize, usize)> = {
            let mut w = WANT.lock().unwrap();
            while w.is_empty() {
                w = WAKE.wait(w).unwrap();
            }
            w.clone()
        };
        // DwmFlush returns right after the next composition; it fails only without DWM (never on Windows 8+, but a
        // remote session can say so): fall back to a plain 60 Hz sleep so the loop cannot spin.
        if unsafe { DwmFlush() } < 0 {
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
        for (h, id) in now {
            let hw = h as HWND;
            if unsafe { IsWindow(hw) } == 0 {
                stop(hw, id);
                continue;
            }
            let mut r = 0usize;
            unsafe { SendMessageTimeoutW(hw, WM_TIMER, id, 0, SMTO_NORMAL | SMTO_ABORTIFHUNG, 50, &mut r) };
        }
    }
}

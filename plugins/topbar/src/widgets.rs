//! topbar: widgets. CPU, memory, disk and network load with a minute of history, and the media playing now.
//! Nothing runs in the background: the numbers are read once a second only while the widgets panel is open or a mini
//! widget is on the bar, and the media thread lives only while the panel is open.
use super::*;
use std::sync::atomic::{AtomicU32, AtomicU64};
use windows_sys::Win32::{
    NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2},
    Storage::FileSystem::GetDiskFreeSpaceExW,
    System::{
        SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
        Threading::GetSystemTimes,
    },
};

/// Samples kept per series: one a second, a minute.
pub(crate) const HIST: usize = 60;

#[derive(Default)]
pub(crate) struct Stats {
    at: u64,
    full_at: u64,
    prev_cpu: Option<(u64, u64)>,     // idle, total (100 ns)
    prev_io: Option<(u64, u64)>,      // bytes read + written, at (ms)
    prev_net: Option<(u64, u64, u64)>, // bytes in, out, at (ms)
    pub cpu: Vec<f32>,                // 0..1
    pub ram: Vec<f32>,                // 0..1
    pub disk: Vec<f32>,               // bytes/s
    pub net: Vec<f32>,                // bytes/s, in + out
    pub ram_gb: (f32, f32),           // used, total
    pub c_drive: (f32, f32),          // used, total (GB)
    pub net_io: (f32, f32),           // in, out (bytes/s)
}

thread_local! {
    static S: RefCell<Stats> = RefCell::new(Stats::default());
}

fn push(v: &mut Vec<f32>, x: f32) {
    v.push(x);
    if v.len() > HIST {
        v.remove(0);
    }
}

fn ft(f: FILETIME) -> u64 {
    (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, info: *mut std::ffi::c_void, len: u32, ret: *mut u32) -> i32;
}

/// Bytes read and written by the whole system so far (SystemPerformanceInformation: IoReadTransferCount at 8,
/// IoWriteTransferCount at 16). No admin rights or performance counters needed.
unsafe fn io_bytes() -> Option<u64> {
    let mut buf = [0u64; 128];
    let mut got = 0u32;
    if NtQuerySystemInformation(2, buf.as_mut_ptr().cast(), size_of_val(&buf) as u32, &mut got) < 0 || got < 24 {
        return None;
    }
    Some(buf[1].wrapping_add(buf[2]))
}

/// Bytes in and out over the hardware network adapters that are up.
unsafe fn net_bytes() -> Option<(u64, u64)> {
    let mut t: *mut MIB_IF_TABLE2 = null_mut();
    if GetIfTable2(&mut t) != 0 || t.is_null() {
        return None;
    }
    let n = (*t).NumEntries as usize;
    let rows = std::slice::from_raw_parts((*t).Table.as_ptr(), n);
    let (mut i, mut o) = (0u64, 0u64);
    for r in rows {
        // HardwareInterface is bit 0 of the flags; 24 = loopback; 1 = IfOperStatusUp
        if r.InterfaceAndOperStatusFlags._bitfield & 1 != 0 && r.Type != 24 && r.OperStatus == 1 {
            i += r.InOctets;
            o += r.OutOctets;
        }
    }
    FreeMibTable(t.cast());
    Some((i, o))
}

/// Reads the numbers if a second has passed since the last time; `full` adds disk and network (the panel shows them).
/// Returns whether anything new was read.
pub(crate) fn sample(full: bool) -> bool {
    let now = unsafe { now_ms() };
    S.with_borrow_mut(|s| {
        if now.saturating_sub(s.at) < 900 && (!full || now.saturating_sub(s.full_at) < 900) {
            return false;
        }
        unsafe {
            if now.saturating_sub(s.at) >= 900 {
                s.at = now;
                let (mut idle, mut kern, mut user): (FILETIME, FILETIME, FILETIME) = (zeroed(), zeroed(), zeroed());
                if GetSystemTimes(&mut idle, &mut kern, &mut user) != 0 {
                    let (i, t) = (ft(idle), ft(kern) + ft(user)); // kernel time includes the idle time
                    if let Some((pi, pt)) = s.prev_cpu {
                        let dt = t.saturating_sub(pt).max(1);
                        push(&mut s.cpu, 1.0 - (i.saturating_sub(pi) as f32 / dt as f32).clamp(0.0, 1.0));
                    }
                    s.prev_cpu = Some((i, t));
                }
                let mut m: MEMORYSTATUSEX = zeroed();
                m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
                if GlobalMemoryStatusEx(&mut m) != 0 && m.ullTotalPhys > 0 {
                    let used = m.ullTotalPhys - m.ullAvailPhys;
                    push(&mut s.ram, used as f32 / m.ullTotalPhys as f32);
                    s.ram_gb = (used as f32 / 1073741824.0, m.ullTotalPhys as f32 / 1073741824.0);
                }
            }
            if full {
                s.full_at = now;
                if let Some(b) = io_bytes() {
                    if let Some((pb, pt)) = s.prev_io {
                        let secs = now.saturating_sub(pt).max(1) as f32 / 1000.0;
                        push(&mut s.disk, b.saturating_sub(pb) as f32 / secs);
                    }
                    s.prev_io = Some((b, now));
                }
                if let Some((i, o)) = net_bytes() {
                    if let Some((pi, po, pt)) = s.prev_net {
                        let secs = now.saturating_sub(pt).max(1) as f32 / 1000.0;
                        let (ri, ro) = (i.saturating_sub(pi) as f32 / secs, o.saturating_sub(po) as f32 / secs);
                        s.net_io = (ri, ro);
                        push(&mut s.net, ri + ro);
                    }
                    s.prev_net = Some((i, o, now));
                }
                let (mut free, mut total) = (0u64, 0u64);
                if GetDiskFreeSpaceExW(wide("C:\\").as_ptr(), null_mut(), &mut total, &mut free) != 0 && total > 0 {
                    s.c_drive = ((total - free) as f32 / 1073741824.0, total as f32 / 1073741824.0);
                }
            }
        }
        true
    })
}

pub(crate) fn with<T>(f: impl FnOnce(&Stats) -> T) -> T {
    S.with_borrow(|s| f(s))
}

/// The latest CPU and memory load in whole percent (for the bar's mini widgets and its repaint check).
pub(crate) fn cpu_ram_pct() -> (u8, u8) {
    with(|s| ((s.cpu.last().copied().unwrap_or(0.0) * 100.0).round() as u8, (s.ram.last().copied().unwrap_or(0.0) * 100.0).round() as u8))
}

/// "1.2 MB/s", "340 KB/s".
pub(crate) fn rate(b: f32) -> String {
    if b >= 1048576.0 {
        format!("{:.1} MB/s", b / 1048576.0)
    } else if b >= 1024.0 {
        format!("{:.0} KB/s", b / 1024.0)
    } else {
        format!("{:.0} B/s", b)
    }
}

/// Whether a mini widget is on the bar (then the bar samples CPU and memory on its own ticks).
pub(crate) fn mini_on() -> bool {
    use i18n::style::{Flag, flag};
    flag(Flag::MiniCpu) || flag(Flag::MiniRam)
}

// ---------- media (GlobalSystemMediaTransportControls) ----------

#[derive(Clone, Default, PartialEq)]
pub(crate) struct Media {
    pub title: String,
    pub artist: String,
    pub app: String,
    pub playing: bool,
}

static MEDIA: Mutex<Option<Media>> = Mutex::new(None);
static MEDIA_GEN: AtomicU32 = AtomicU32::new(0);
static PANEL: AtomicIsize = AtomicIsize::new(0);
static OPT_AT: AtomicU64 = AtomicU64::new(0); // when a button was pressed: the thread keeps the shown state a moment

pub(crate) fn media() -> Option<Media> {
    MEDIA.lock().ok().and_then(|m| m.clone())
}

/// "Spotify.exe" -> "Spotify", "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic" -> "ZuneMusic".
fn app_name(aumid: &str) -> String {
    let s = aumid.rsplit('!').next().unwrap_or(aumid);
    let s = s.rsplit('\\').next().unwrap_or(s);
    let s = s.strip_suffix(".exe").or_else(|| s.strip_suffix(".EXE")).unwrap_or(s);
    s.rsplit('.').next().filter(|x| !x.is_empty()).unwrap_or(s).to_string()
}

type Mgr = windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;

fn manager() -> Option<Mgr> {
    Mgr::RequestAsync().ok()?.join().ok()
}

fn read_media(m: &Mgr) -> Option<Media> {
    let s = m.GetCurrentSession().ok()?;
    let p = s.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
    let playing = s.GetPlaybackInfo().ok()?.PlaybackStatus().ok()? == windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing;
    let title = p.Title().map(|t| t.to_string()).unwrap_or_default();
    if title.is_empty() {
        return None;
    }
    Some(Media { title, artist: p.Artist().map(|a| a.to_string()).unwrap_or_default(), app: s.SourceAppUserModelId().map(|a| app_name(&a.to_string())).unwrap_or_default(), playing })
}

fn store(m: Option<Media>) {
    let changed = MEDIA.lock().map(|mut cur| if *cur != m { *cur = m; true } else { false }).unwrap_or(false);
    let hw = PANEL.load(Relaxed);
    if changed && hw != 0 {
        unsafe { InvalidateRect(hw as HWND, null(), 0) };
    }
}

/// The panel opened: a thread reads what is playing once a second until `stop_media`.
pub(crate) fn start_media(panel: HWND) {
    PANEL.store(panel as isize, Relaxed);
    let my = MEDIA_GEN.fetch_add(1, Relaxed) + 1;
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        let mut mgr = None;
        while MEDIA_GEN.load(Relaxed) == my {
            if mgr.is_none() {
                mgr = manager();
            }
            if now_ms().saturating_sub(OPT_AT.load(Relaxed)) > 1500 {
                store(mgr.as_ref().and_then(read_media));
            }
            std::thread::sleep(std::time::Duration::from_millis(1000));
        }
    });
}

/// The panel closed: the media thread ends after its current sleep.
pub(crate) fn stop_media() {
    MEDIA_GEN.fetch_add(1, Relaxed);
    PANEL.store(0, Relaxed);
}

/// 0 previous, 1 play / pause, 2 next. Runs on its own thread (the calls wait on the player).
pub(crate) fn media_cmd(k: u8) {
    OPT_AT.store(unsafe { now_ms() }, Relaxed);
    if k == 1 {
        // show the new state at once; the thread confirms it a moment later
        let cur = media();
        store(cur.map(|m| Media { playing: !m.playing, ..m }));
    }
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        let Some(m) = manager() else { return };
        if let Ok(s) = m.GetCurrentSession() {
            let op = match k {
                0 => s.TrySkipPreviousAsync(),
                1 => s.TryTogglePlayPauseAsync(),
                _ => s.TrySkipNextAsync(),
            };
            if let Ok(op) = op {
                let _ = op.join();
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
        OPT_AT.store(0, Relaxed);
        store(read_media(&m));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_names() {
        assert_eq!(app_name("Spotify.exe"), "Spotify");
        assert_eq!(app_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), "ZuneMusic");
        assert_eq!(app_name("chrome"), "chrome");
    }

    #[test]
    fn rates() {
        assert_eq!(rate(512.0), "512 B/s");
        assert_eq!(rate(2048.0), "2 KB/s");
        assert_eq!(rate(3.0 * 1048576.0), "3.0 MB/s");
    }

    #[test]
    fn history_is_a_minute() {
        let mut v = vec![];
        (0..100).for_each(|i| push(&mut v, i as f32));
        assert_eq!(v.len(), HIST);
        assert_eq!(v[0], 40.0);
    }
}

//! topbar: status (split out of main.rs; shares its items through `super::*`).
use super::*;

// ---------- change notifications ----------
// Volume, network and battery are read when Windows says they changed, never on a timer: the bar does no work for them
// while nothing happens (a game in front included). Callbacks come on other threads and only post STATUS to the bar.

/// Posted to the bar: wp 0 = volume changed, 1 = network changed, 2 = default audio device changed,
/// 3 = a background read for the open flyout finished (Wi-Fi list, quick tiles), 4 = repaint only.
pub(crate) const STATUS: u32 = WM_APP + 17;

pub(crate) fn post_status(wp: usize) {
    let bar = BAR.load(Relaxed) as HWND;
    if !bar.is_null() {
        unsafe { PostMessageW(bar, STATUS, wp, 0) };
    }
}

#[windows::core::implement(IAudioEndpointVolumeCallback)]
struct VolCb;

impl IAudioEndpointVolumeCallback_Impl for VolCb_Impl {
    fn OnNotify(&self, _: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        post_status(0);
        Ok(())
    }
}

#[windows::core::implement(IMMNotificationClient)]
struct DevCb;

impl IMMNotificationClient_Impl for DevCb_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnDeviceAdded(&self, _: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnDeviceRemoved(&self, _: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _: &PCWSTR) -> windows::core::Result<()> {
        if flow == eRender && role == eConsole {
            post_status(2);
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &windows::Win32::Foundation::PROPERTYKEY) -> windows::core::Result<()> {
        Ok(())
    }
}

thread_local! {
    // kept alive for as long as they are registered
    static VOL_CB: RefCell<Option<IAudioEndpointVolumeCallback>> = const { RefCell::new(None) };
    static DEV: RefCell<Option<(IMMDeviceEnumerator, IMMNotificationClient)>> = const { RefCell::new(None) };
}

pub(crate) unsafe fn endpoint() -> Option<IAudioEndpointVolume> {
    EP.with_borrow_mut(|e| {
        if e.is_none() {
            *e = (|| -> windows::core::Result<IAudioEndpointVolume> {
                let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
                if DEV.with_borrow(|d| d.is_none()) {
                    let client: IMMNotificationClient = DevCb.into();
                    if en.RegisterEndpointNotificationCallback(&client).is_ok() {
                        DEV.set(Some((en.clone(), client)));
                    }
                }
                let ep: IAudioEndpointVolume = en.GetDefaultAudioEndpoint(eRender, eConsole)?.Activate(CLSCTX_ALL, None)?;
                let cb: IAudioEndpointVolumeCallback = VolCb.into();
                if ep.RegisterControlChangeNotify(&cb).is_ok() {
                    VOL_CB.set(Some(cb));
                }
                Ok(ep)
            })()
            .ok();
        }
        e.clone()
    })
}

/// The default output device changed: drop the old endpoint (and its callback) and read the new one.
pub(crate) unsafe fn audio_device_changed() {
    if let (Some(ep), Some(cb)) = (EP.take(), VOL_CB.take()) {
        let _ = ep.UnregisterControlChangeNotify(&cb);
    }
    read_volume();
}

pub(crate) unsafe fn read_battery() {
    let mut p: SYSTEM_POWER_STATUS = zeroed();
    let ok = GetSystemPowerStatus(&mut p) != 0 && p.BatteryFlag & 128 == 0 && p.BatteryLifePercent <= 100;
    BAT.set(if ok { (p.BatteryLifePercent as i32, p.ACLineStatus == 1) } else { (-1, false) });
}

unsafe extern "system" fn net_hint(_: *const core::ffi::c_void, _: windows_sys::Win32::Networking::WinSock::NL_NETWORK_CONNECTIVITY_HINT) {
    post_status(1);
}

unsafe extern "system" fn wlan_note(_: *mut windows_sys::Win32::NetworkManagement::WiFi::L2_NOTIFICATION_DATA, _: *mut core::ffi::c_void) {
    post_status(1); // connect, disconnect, signal quality change
}

/// Reads volume, network and battery once and subscribes to their changes. Call once the bar window exists.
pub(crate) unsafe fn watch_status() {
    read_battery();
    read_volume(); // registers the volume and device callbacks
    read_wifi(); // opens the WLAN handle
    let mut h: HANDLE = null_mut();
    windows_sys::Win32::NetworkManagement::IpHelper::NotifyNetworkConnectivityHintChange(Some(net_hint), null(), false, &mut h);
    let wl = WLAN.get() as HANDLE;
    if !wl.is_null() {
        use windows_sys::Win32::NetworkManagement::WiFi::{WLAN_NOTIFICATION_SOURCE_ACM, WLAN_NOTIFICATION_SOURCE_MSM, WlanRegisterNotification};
        WlanRegisterNotification(wl, WLAN_NOTIFICATION_SOURCE_ACM | WLAN_NOTIFICATION_SOURCE_MSM, 0, Some(wlan_note), null(), null(), null_mut());
    }
}

pub(crate) unsafe fn read_volume() {
    let v = endpoint().and_then(|e| Some(((e.GetMasterVolumeLevelScalar().ok()? * 100.0).round() as i32, e.GetMute().ok()?.as_bool())));
    if v.is_none() {
        EP.set(None); // device changed: look the endpoint up again next time
    }
    VOL.set(v.unwrap_or((-1, false)));
}

pub(crate) unsafe fn set_volume(v: i32) {
    if let Some(e) = endpoint() {
        let _ = e.SetMasterVolumeLevelScalar(v.clamp(0, 100) as f32 / 100.0, null());
        let _ = e.SetMute(false, null());
        read_volume();
    }
}

pub(crate) unsafe fn add_volume(delta: i32) {
    if let Some(e) = endpoint() {
        let v = (VOL.get().0.max(0) + delta).clamp(0, 100);
        let _ = e.SetMasterVolumeLevelScalar(v as f32 / 100.0, null());
        let _ = e.SetMute(false, null());
        read_volume();
    }
}

pub(crate) unsafe fn toggle_mute() {
    if let Some(e) = endpoint() {
        let _ = e.SetMute(!VOL.get().1, null());
        read_volume();
    }
}

thread_local! {
    static WLAN: Cell<usize> = const { Cell::new(0) }; // WLAN client handle, opened once (opening it talks to wlansvc every time)
}

pub(crate) unsafe fn read_wifi() {
    let mut h: HANDLE = WLAN.get() as HANDLE;
    let mut q = -1;
    if h.is_null() {
        let mut ver = 0u32;
        if WlanOpenHandle(2, null(), &mut ver, &mut h) != 0 {
            h = null_mut();
        }
        WLAN.set(h as usize);
    }
    if !h.is_null() {
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = null_mut();
        if WlanEnumInterfaces(h, null(), &mut list) != 0 {
            WlanCloseHandle(h, null()); // stale handle (service restarted): reopen next time
            WLAN.set(0);
        } else {
            let l = &*list;
            for i in 0..l.dwNumberOfItems as usize {
                let ifc = &*l.InterfaceInfo.as_ptr().add(i);
                let (mut size, mut data) = (0u32, null_mut());
                if ifc.isState == wlan_interface_state_connected && WlanQueryInterface(h, &ifc.InterfaceGuid, wlan_intf_opcode_current_connection, null(), &mut size, &mut data, null_mut()) == 0 {
                    q = (*(data as *const WLAN_CONNECTION_ATTRIBUTES)).wlanAssociationAttributes.wlanSignalQuality as i32;
                    WlanFreeMemory(data);
                }
            }
            WlanFreeMemory(list as *const _);
        }
    }
    WIFI.set(q);
    // Windows' own verdict (the same one its tray icon shows): a connection without internet is not "online"
    use windows_sys::Win32::{NetworkManagement::IpHelper::GetNetworkConnectivityHint, Networking::WinSock::{NL_NETWORK_CONNECTIVITY_HINT, NetworkConnectivityLevelHintConstrainedInternetAccess, NetworkConnectivityLevelHintInternetAccess}};
    let mut hint: NL_NETWORK_CONNECTIVITY_HINT = zeroed();
    // a failed call (old Windows) counts as online: never show "no internet" on a guess
    INET.set(GetNetworkConnectivityHint(&mut hint) != 0 || {
        let lvl = hint.ConnectivityLevel;
        lvl == NetworkConnectivityLevelHintInternetAccess || lvl == NetworkConnectivityLevelHintConstrainedInternetAccess
    });
}

/// What the network icon shows.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Link {
    Wifi(i32),      // on wifi with internet, signal 0..100
    NoInternet(i32), // on wifi, but no way out
    Cable,          // internet over ethernet
    Offline,        // no connection at all
}

pub(crate) fn net() -> Link {
    match (WIFI.get(), INET.get()) {
        (q, true) if q >= 0 => Link::Wifi(q),
        (q, false) if q >= 0 => Link::NoInternet(q),
        (_, true) => Link::Cable,
        _ => Link::Offline,
    }
}

/// Wi-Fi arcs lit for a signal of `q` percent: 1..3.
pub(crate) fn arcs_lit(q: i32) -> i32 {
    if q >= 67 { 3 } else if q >= 34 { 2 } else { 1 }
}

/// The Wi-Fi symbol drawn with anti-aliased arcs: `lit` of the 3 arcs in `on`, the rest in `off`, centered on (cx, cy),
/// `s` px tall. `mark`: 0 none, 1 a cross (not connected), 2 an exclamation mark (connected, no internet).
pub(crate) unsafe fn wifi_glyph(dc: HDC, cx: i32, cy: i32, s: i32, lit: i32, on: u32, off: u32, mark: u8) {
    let mut g: *mut GpGraphics = null_mut();
    if GdipCreateFromHDC(dc, &mut g) != 0 || g.is_null() {
        return;
    }
    GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
    let argb = |c: u32| 0xFF00_0000 | (c & 255) << 16 | (c & 0xFF00) | (c >> 16 & 255);
    let pen = |c: u32, w: f32| {
        let mut p: *mut GpPen = null_mut();
        GdipCreatePen1(argb(c), w, UnitPixel, &mut p);
        GdipSetPenStartCap(p, LineCapRound);
        GdipSetPenEndCap(p, LineCapRound);
        p
    };
    let (sf, w) = (s as f32, (s as f32 * 0.12).max(1.5));
    let (bx, by) = (cx as f32, cy as f32 + sf * 0.30); // visually centered around cy
    for i in 0..3 {
        let r = sf * (0.30 + 0.25 * i as f32);
        let p = pen(if i < lit { on } else { off }, w);
        GdipDrawArc(g, p, bx - r, by - r, 2.0 * r, 2.0 * r, 225.0, 90.0);
        GdipDeletePen(p);
    }
    let mut br: *mut GpSolidFill = null_mut();
    GdipCreateSolidFill(argb(if lit > 0 { on } else { off }), &mut br);
    GdipFillEllipse(g, br as *mut GpBrush, bx - w * 0.9, by - w * 0.9, w * 1.8, w * 1.8);
    GdipDeleteBrush(br as *mut GpBrush);
    // the mark sits at the lower right, on a cut-out of the background so it stays readable over the arcs
    let (mx, my, mr) = (bx + sf * 0.42, by - sf * 0.08, sf * 0.26);
    if mark > 0 {
        let mut bg: *mut GpSolidFill = null_mut();
        GdipCreateSolidFill(argb(pal().bg), &mut bg);
        GdipFillEllipse(g, bg as *mut GpBrush, mx - mr, my - mr, 2.0 * mr, 2.0 * mr);
        GdipDeleteBrush(bg as *mut GpBrush);
        let c = if mark == 1 { on } else { WARN };
        let p = pen(c, (w * 0.8).max(1.4));
        let d = mr * 0.5;
        if mark == 1 {
            GdipDrawLine(g, p, mx - d, my - d, mx + d, my + d);
            GdipDrawLine(g, p, mx + d, my - d, mx - d, my + d);
        } else {
            GdipDrawLine(g, p, mx, my - d * 1.2, mx, my + d * 0.2);
            GdipDrawLine(g, p, mx, my + d * 0.95, mx, my + d * 1.0);
        }
        GdipDeletePen(p);
    }
    GdipDeleteGraphics(g);
}

/// Hover key of the tray chevron's turn.
pub(crate) const TF_KEY: i32 = -13;

/// A chevron of half-width `s` around (cx, cy), drawn with round line ends; `deg` 0 points up, 180 down.
pub(crate) unsafe fn chevron(dc: HDC, cx: i32, cy: i32, s: i32, deg: f32, c: u32) {
    let mut g: *mut GpGraphics = null_mut();
    if GdipCreateFromHDC(dc, &mut g) != 0 || g.is_null() {
        return;
    }
    GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
    let mut p: *mut GpPen = null_mut();
    GdipCreatePen1(0xFF00_0000 | (c & 255) << 16 | (c & 0xFF00) | (c >> 16 & 255), (s as f32 * 0.32).max(1.4), UnitPixel, &mut p);
    GdipSetPenStartCap(p, LineCapRound);
    GdipSetPenEndCap(p, LineCapRound);
    let (sn, cs) = deg.to_radians().sin_cos();
    let pt = |x: f32, y: f32| (cx as f32 + x * cs - y * sn, cy as f32 + x * sn + y * cs);
    let (sf, h) = (s as f32, s as f32 * 0.5);
    let (a, b, d) = (pt(-sf, h), pt(0.0, -h), pt(sf, h));
    GdipDrawLine(g, p, a.0, a.1, b.0, b.1);
    GdipDrawLine(g, p, b.0, b.1, d.0, d.1);
    GdipDeletePen(p);
    GdipDeleteGraphics(g);
}

/// The network icon of the bar and the panels, for the current state.
pub(crate) unsafe fn net_icon(dc: HDC, cx: i32, cy: i32, s: i32, f_glyph: HFONT) {
    let (on, off) = (pal().text, mix(pal().dim, pal().bg, 0.45));
    match net() {
        Link::Wifi(q) => wifi_glyph(dc, cx, cy, s, arcs_lit(q), on, off, 0),
        Link::NoInternet(q) => wifi_glyph(dc, cx, cy, s, arcs_lit(q), mix(on, pal().bg, 0.3), off, 2),
        Link::Offline => wifi_glyph(dc, cx, cy, s, 0, on, mix(pal().dim, pal().bg, 0.2), 1),
        Link::Cable => {
            SelectObject(dc, f_glyph);
            text(dc, "\u{E839}", RECT { left: cx - s, top: cy - s, right: cx + s, bottom: cy + s }, on, DT_CENTER);
        }
    }
}

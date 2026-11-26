//! Windows 11 tray clock for DeepSeek peak/off-peak pricing.
//!
//! The tray dot opens a card that mirrors the GNOME Shell extension's menu:
//! status dot, live countdown, the 24-hour timeline, the next price windows and
//! the current rates, in the Windows light or dark app theme. The card is
//! drawn by [`paint`], not assembled from Win32 menu items, which is what makes
//! the two builds look like the same app.
//!
//! It is still small: one hidden window, one tray icon, and a card that only
//! exists while it is open. The pricing rules live in
//! [`deepseek_price_clock::pricing`] and are shared with the GNOME extension.
//!
//! Build it from any platform:
//!
//! ```text
//! cargo build --release --target x86_64-pc-windows-gnu
//! ```

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "The tray clock only runs on Windows. Its pricing rules are pure Rust, so \
         `cargo test --lib` works here."
    );
}

#[cfg(windows)]
fn main() {
    win::run();
}

#[cfg(windows)]
mod paint;

#[cfg(windows)]
mod win {
    use std::cell::UnsafeCell;
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use deepseek_price_clock::pricing::{self, Kind};

    use crate::paint::{self, Action, Card, Model, RateRow, WindowRow, Zone};

    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, HWND, LPARAM, LRESULT,
        POINT, RECT, SYSTEMTIME, WPARAM,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        CreateBitmap, CreateDIBSection, DeleteObject, GetDC, GetMonitorInfoW, MonitorFromPoint,
        ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC, MONITORINFO,
        MONITOR_DEFAULTTONEAREST, RGBQUAD,
    };
    use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
        HKEY_CURRENT_USER, KEY_SET_VALUE, REG_DWORD, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    };
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::System::Time::SystemTimeToTzSpecificLocalTime;
    use windows_sys::Win32::UI::HiDpi::{
        GetDpiForMonitor, GetDpiForSystem, SetProcessDpiAwarenessContext,
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TRACKMOUSEEVENT};
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO,
        NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateIconIndirect, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyWindow,
        DispatchMessageW, GetCursorPos, GetMessageW, KillTimer, LoadCursorW, PostQuitMessage,
        RegisterClassW, RegisterWindowMessageW, SetCursor, SetForegroundWindow, SetTimer,
        ShowWindow, TranslateMessage, UpdateLayeredWindow, HICON, ICONINFO, IDC_ARROW, IDC_HAND,
        MSG, SW_SHOWNA, WM_ACTIVATE, WM_APP, WM_CONTEXTMENU, WM_DESTROY, WM_ENDSESSION, WM_KEYDOWN,
        WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER, WNDCLASSW,
        WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };

    const APP_NAME: &str = "DeepSeek Price Clock";
    const WINDOW_CLASS: &str = "DeepSeekPriceClockWindow";
    const MUTEX_NAME: &str = "Local\\DeepSeekPriceClock";
    const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const RUN_VALUE: &str = "DeepSeekPriceClock";
    const SETTINGS_KEY: &str = "Software\\DeepSeekPriceClock";
    const THEME_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";
    const PRICING_URL: &str = "https://api-docs.deepseek.com/quick_start/pricing";
    const TASKBAR_CREATED_MESSAGE: &str = "TaskbarCreated";

    const TRAY_ICON_ID: u32 = 1;
    const TRAY_MESSAGE: u32 = WM_APP + 1;
    const TIMER_REFRESH: usize = 1;
    const TIMER_POPUP: usize = 2;
    /// The tooltip and the dot only need a minute's worth of accuracy; the card
    /// has its own one second timer while it is open.
    const REFRESH_MS: u32 = 20_000;
    const POPUP_MS: u32 = 1_000;

    /// `WM_MOUSELEAVE`, which windows-sys only exports from the controls module.
    const WM_MOUSELEAVE: u32 = 0x02A3;
    /// `WM_THEMECHANGED`.
    const WM_THEMECHANGED: u32 = 0x031A;
    const VK_ESCAPE: usize = 0x1B;

    /// Same dot colours as the GNOME extension: green off-peak, red peak.
    const OFF_RGB: (u8, u8, u8) = (87, 227, 137);
    const PEAK_RGB: (u8, u8, u8) = (255, 123, 114);

    /// Rates per 1M tokens (input cache hit, input cache miss, output), from the
    /// upstream extension's table and `price.js`.
    const MODELS: &[ModelRate] = &[
        ModelRate {
            name: "V4.1 Flash",
            off: ["$0.15", "$0.003", "$0.60"],
            peak: ["$0.30", "$0.006", "$1.20"],
        },
        ModelRate {
            name: "V4 Pro",
            off: ["$0.66", "$0.022", "$1.98"],
            peak: ["$1.32", "$0.044", "$3.96"],
        },
    ];

    struct ModelRate {
        name: &'static str,
        off: [&'static str; 3],
        peak: [&'static str; 3],
    }

    /// `TaskbarCreated`, registered once before the icon is added. Explorer
    /// broadcasts it when it restarts, which is how the indicator comes back
    /// after a crash instead of silently vanishing.
    static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

    /// The app lives for the whole message loop, so the window procedure needs a
    /// stable place to find it. Windows and the card run on this one thread.
    struct AppCell(UnsafeCell<Option<App>>);
    // SAFETY: only touched from the thread that created the window.
    unsafe impl Sync for AppCell {}

    static APP: AppCell = AppCell(UnsafeCell::new(None));

    /// Run `f` against the app if it has been created. The closure must not pump
    /// messages, so no second `&mut App` can exist while it runs.
    unsafe fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
        (*APP.0.get()).as_mut().map(f)
    }

    struct App {
        hwnd: HWND,
        nid: NOTIFYICONDATAW,
        off_icon: HICON,
        peak_icon: HICON,
        last_kind: Option<Kind>,
        notify: bool,
        zone: Zone,
        dark: bool,
        icon_added: bool,
        shown: bool,
        hover: Option<Action>,
        hits: Vec<(Action, RECT)>,
        /// Screen position of the card's visible top-left, kept while it is
        /// open so a repaint never makes it jump under the mouse.
        origin: (i32, i32),
        hidden_at_ms: i64,
        exe: String,
    }

    impl App {
        fn new(hwnd: HWND) -> App {
            let mut nid: NOTIFYICONDATAW = unsafe { zeroed() };
            nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = hwnd;
            nid.uID = TRAY_ICON_ID;
            nid.uCallbackMessage = TRAY_MESSAGE;
            App {
                hwnd,
                nid,
                off_icon: draw_dot(OFF_RGB),
                peak_icon: draw_dot(PEAK_RGB),
                last_kind: None,
                notify: read_dword("Notify").map(|v| v != 0).unwrap_or(true),
                zone: match read_dword("Zone") {
                    Some(1) => Zone::Utc,
                    _ => Zone::Local,
                },
                dark: system_is_dark(),
                icon_added: false,
                shown: false,
                hover: None,
                hits: Vec::new(),
                origin: (0, 0),
                hidden_at_ms: 0,
                exe: current_exe(),
            }
        }

        fn icon_for(&self, kind: Kind) -> HICON {
            if kind.is_peak() {
                self.peak_icon
            } else {
                self.off_icon
            }
        }

        /// Paint the tray dot and tooltip. Called every 20 s and after a theme
        /// switch, so a lost icon is put back within one tick.
        fn refresh_tray(&mut self) {
            let now = now_secs();
            let kind = pricing::kind_at(now);
            let next = pricing::next_switch(now);
            let left = pricing::format_countdown(next.time - now);

            self.nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            self.nid.hIcon = self.icon_for(kind);
            self.nid.szTip = fixed_text(&format!(
                "DeepSeek {} · {left} left",
                kind.label().to_lowercase()
            ));

            if self.icon_added {
                self.icon_added = unsafe { Shell_NotifyIconW(NIM_MODIFY, &self.nid) != 0 };
            }
            if !self.icon_added {
                self.icon_added = unsafe { Shell_NotifyIconW(NIM_ADD, &self.nid) != 0 };
            }

            if let Some(last) = self.last_kind {
                if last != kind && self.notify {
                    self.notify_switch(kind);
                }
            }
            self.last_kind = Some(kind);
        }

        fn notify_switch(&mut self, kind: Kind) {
            let model = &MODELS[0];
            let (title, rates) = if kind.is_peak() {
                ("DeepSeek: peak pricing started", &model.peak)
            } else {
                ("DeepSeek: off-peak started (half price)", &model.off)
            };
            let body = format!(
                "{} is now {} in, {} out per 1M tokens.",
                model.name, rates[0], rates[2]
            );

            self.nid.uFlags = NIF_INFO;
            self.nid.szInfo = fixed_text(&body);
            self.nid.szInfoTitle = fixed_text(title);
            self.nid.dwInfoFlags = NIIF_INFO;
            unsafe {
                Shell_NotifyIconW(NIM_MODIFY, &self.nid);
            }
        }

        /// Render and show the card. `keep_origin` is used for the one second
        /// tick and hover repaints so the panel does not follow the pointer.
        fn show_popup(&mut self, keep_origin: bool) -> bool {
            let model = build_model(
                now_secs(),
                self.zone,
                self.dark,
                self.hover,
                self.notify,
                autostart_enabled(),
            );
            let scale = dpi_scale_at_cursor();
            let Some(card) = paint::render(&model, scale) else {
                return false;
            };

            if !keep_origin || !self.shown {
                let (x, y) = place_card(&card);
                self.origin = (x, y);
            }

            let (x, y) = self.origin;
            unsafe {
                let screen: HDC = GetDC(null_mut());
                if screen.is_null() {
                    card.destroy();
                    return false;
                }
                let destination = POINT {
                    x: x - card.offset_x,
                    y: y - card.offset_y,
                };
                let size = windows_sys::Win32::Foundation::SIZE {
                    cx: card.width,
                    cy: card.height,
                };
                let source = POINT { x: 0, y: 0 };
                let blend = windows_sys::Win32::Graphics::Gdi::BLENDFUNCTION {
                    BlendOp: windows_sys::Win32::Graphics::Gdi::AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: windows_sys::Win32::Graphics::Gdi::AC_SRC_ALPHA as u8,
                };
                UpdateLayeredWindow(
                    self.hwnd,
                    screen,
                    &destination,
                    &size,
                    card.hdc,
                    &source,
                    0,
                    &blend,
                    windows_sys::Win32::UI::WindowsAndMessaging::ULW_ALPHA,
                );
                ReleaseDC(null_mut(), screen);
            }

            self.hits = card.hits.clone();
            card.destroy();

            if !self.shown {
                unsafe {
                    ShowWindow(self.hwnd, SW_SHOWNA);
                    SetForegroundWindow(self.hwnd);
                    SetTimer(self.hwnd, TIMER_POPUP, POPUP_MS, None);
                }
                self.shown = true;
            }
            true
        }

        fn hide_popup(&mut self) {
            if !self.shown {
                return;
            }
            unsafe {
                KillTimer(self.hwnd, TIMER_POPUP);
                ShowWindow(
                    self.hwnd,
                    windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE,
                );
            }
            self.shown = false;
            self.hover = None;
            self.hits.clear();
            self.hidden_at_ms = now_ms();
        }

        fn toggle_popup(&mut self) {
            // Clicking the tray icon while the card is open first deactivates
            // the card (which hides it) and then arrives here; without this
            // guard the same click would immediately reopen it.
            if !self.shown && now_ms() - self.hidden_at_ms < 300 {
                return;
            }
            if self.shown {
                self.hide_popup();
            } else {
                self.show_popup(false);
            }
        }

        fn action_at(&self, x: i32, y: i32) -> Option<Action> {
            self.hits.iter().rev().find_map(|(action, area)| {
                (x >= area.left && x < area.right && y >= area.top && y < area.bottom)
                    .then_some(*action)
            })
        }

        fn command(&mut self, action: Action) {
            match action {
                Action::ZoneLocal | Action::ZoneUtc => {
                    self.zone = if action == Action::ZoneLocal {
                        Zone::Local
                    } else {
                        Zone::Utc
                    };
                    write_dword("Zone", if self.zone == Zone::Utc { 1 } else { 0 });
                    self.show_popup(true);
                }
                Action::ToggleNotify => {
                    self.notify = !self.notify;
                    write_dword("Notify", self.notify as u32);
                    self.show_popup(true);
                }
                Action::ToggleAutostart => {
                    let enabled = autostart_enabled();
                    set_autostart(&self.exe, !enabled);
                    self.show_popup(true);
                }
                Action::OpenPricing => {
                    self.hide_popup();
                    open_pricing_page();
                }
                Action::Quit => self.exit(),
            }
        }

        /// Remove the dot before leaving so the shell does not keep a ghost,
        /// then end the message loop.
        fn exit(&mut self) {
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &self.nid);
                self.icon_added = false;
                KillTimer(self.hwnd, TIMER_REFRESH);
                KillTimer(self.hwnd, TIMER_POPUP);
                DestroyWindow(self.hwnd);
            }
        }
    }

    // ----------------------------------------------------------------------
    // Card content

    fn build_model(
        now: i64,
        zone: Zone,
        dark: bool,
        hover: Option<Action>,
        notify: bool,
        autostart: bool,
    ) -> Model {
        let kind = pricing::kind_at(now);
        let peak = kind.is_peak();
        let windows = pricing::window_list(now, 4);
        let end = windows[0].end;
        let current = if peak { 1 } else { 0 };
        let next = 1 - current;

        let zone_caption = match zone {
            Zone::Utc => "UTC · all windows in UTC".to_string(),
            Zone::Local => {
                let offset = local_offset_minutes(now);
                let sign = if offset < 0 { "\u{2212}" } else { "+" };
                let offset = offset.abs();
                let hours = offset / 60;
                let minutes = offset % 60;
                if minutes == 0 {
                    format!("Local time · GMT{sign}{hours}")
                } else {
                    format!("Local time · GMT{sign}{hours}:{minutes:02}")
                }
            }
        };

        let rows = windows
            .iter()
            .map(|window| {
                let range = if same_day(window.start, window.end, zone) {
                    format!("{} – {}", hhmm(window.start, zone), hhmm(window.end, zone))
                } else {
                    format!(
                        "{} – {}",
                        hhmm(window.start, zone),
                        day_hhmm(window.end, zone)
                    )
                };
                let meta = if window.current {
                    format!("{} left", pricing::format_away(window.end - now))
                } else {
                    pricing::format_duration(window.end - window.start)
                };
                WindowRow {
                    kind: window.kind,
                    range,
                    meta,
                    current: window.current,
                }
            })
            .collect();

        let rates = MODELS
            .iter()
            .map(|model| RateRow {
                name: model.name.to_string(),
                amounts: [
                    model.rates(current)[0].to_string(),
                    model.rates(current)[1].to_string(),
                    model.rates(current)[2].to_string(),
                ],
            })
            .collect();

        let next_rates = format!(
            "From {} · {}: {}",
            flip_label(end, now, zone),
            if next == 1 { "peak" } else { "off-peak" },
            MODELS
                .iter()
                .map(|model| {
                    let rates = model.rates(next);
                    format!("{} {} / {} / {}", model.name, rates[0], rates[1], rates[2])
                })
                .collect::<Vec<_>>()
                .join(" · ")
        );

        Model {
            peak,
            zone,
            dark,
            zone_caption,
            badge: if peak {
                "Peak pricing".to_string()
            } else {
                "Off-peak · half price".to_string()
            },
            countdown: format!("{} left", pricing::format_clock(end - now)),
            until: format!(
                "{} starts {}",
                if peak { "Off-peak" } else { "Peak" },
                flip_label(end, now, zone)
            ),
            segments: pricing::display_segments(
                now,
                match zone {
                    Zone::Utc => 0,
                    Zone::Local => local_offset_minutes(now),
                },
            ),
            now_minute: minute_of_day(now, zone),
            windows_caption: match zone {
                Zone::Utc => "Price windows · UTC".to_string(),
                Zone::Local => "Price windows · your time".to_string(),
            },
            windows: rows,
            rates_caption: format!("Paying now · {}", if peak { "peak" } else { "off-peak" }),
            rates_header: "per 1M tokens".to_string(),
            rates,
            next_rates,
            notify,
            autostart,
            hover,
        }
    }

    impl ModelRate {
        fn rates(&self, index: usize) -> &[&'static str; 3] {
            if index == 1 {
                &self.peak
            } else {
                &self.off
            }
        }
    }

    // ----------------------------------------------------------------------
    // Time and zone labels

    fn local_parts(secs: i64) -> (i64, i64, i64, i64, i64, i64) {
        let day = secs.div_euclid(86_400);
        let rest = secs.rem_euclid(86_400);
        let (year, month, date) = pricing::civil_from_days(day);
        let utc = SYSTEMTIME {
            wYear: year as u16,
            wMonth: month as u16,
            wDayOfWeek: 0,
            wDay: date as u16,
            wHour: (rest / 3_600) as u16,
            wMinute: (rest % 3_600 / 60) as u16,
            wSecond: (rest % 60) as u16,
            wMilliseconds: 0,
        };
        let mut local: SYSTEMTIME = unsafe { zeroed() };
        let converted = unsafe { SystemTimeToTzSpecificLocalTime(null(), &utc, &mut local) };
        if converted != 0 {
            (
                local.wYear as i64,
                local.wMonth as i64,
                local.wDay as i64,
                local.wHour as i64,
                local.wMinute as i64,
                local.wSecond as i64,
            )
        } else {
            (
                year,
                month,
                date,
                rest / 3_600,
                rest % 3_600 / 60,
                rest % 60,
            )
        }
    }

    fn parts(secs: i64, zone: Zone) -> (i64, i64, i64, i64, i64) {
        if zone == Zone::Utc {
            let day = secs.div_euclid(86_400);
            let rest = secs.rem_euclid(86_400);
            let (year, month, date) = pricing::civil_from_days(day);
            (year, month, date, rest / 3_600, rest % 3_600 / 60)
        } else {
            let (year, month, date, hour, minute, _) = local_parts(secs);
            (year, month, date, hour, minute)
        }
    }

    fn local_offset_minutes(secs: i64) -> i64 {
        let (year, month, date, hour, minute, second) = local_parts(secs);
        let wall = pricing::days_from_civil(year, month, date) * 86_400
            + hour * 3_600
            + minute * 60
            + second;
        (wall - secs) / 60
    }

    fn minute_of_day(secs: i64, zone: Zone) -> f32 {
        let (_, _, _, hour, minute) = parts(secs, zone);
        hour as f32 * 60.0 + minute as f32 + (secs.rem_euclid(60)) as f32 / 60.0
    }

    fn weekday_name((year, month, date, _, _): (i64, i64, i64, i64, i64)) -> &'static str {
        let index = (pricing::days_from_civil(year, month, date) + 3).rem_euclid(7);
        pricing::WEEKDAYS[index as usize]
    }

    fn hhmm(secs: i64, zone: Zone) -> String {
        let (_, _, _, hour, minute) = parts(secs, zone);
        format!("{hour:02}:{minute:02}")
    }

    fn day_hhmm(secs: i64, zone: Zone) -> String {
        let value = parts(secs, zone);
        format!("{} {:02}:{:02}", weekday_name(value), value.3, value.4)
    }

    fn same_day(a: i64, b: i64, zone: Zone) -> bool {
        let (ay, am, ad, _, _) = parts(a, zone);
        let (by, bm, bd, _, _) = parts(b, zone);
        (ay, am, ad) == (by, bm, bd)
    }

    /// "Sat 04:00" when the flip crosses midnight, plain time otherwise.
    fn flip_label(secs: i64, from: i64, zone: Zone) -> String {
        if same_day(secs, from, zone) {
            hhmm(secs, zone)
        } else {
            day_hhmm(secs, zone)
        }
    }

    // ----------------------------------------------------------------------
    // Window procedure

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            TRAY_MESSAGE => {
                // The low word carries the mouse message in the legacy callback
                // layout used when no icon version was requested.
                match lparam as u32 & 0xffff {
                    WM_LBUTTONUP | WM_RBUTTONUP | WM_CONTEXTMENU => {
                        with_app(|app| app.toggle_popup());
                    }
                    _ => {}
                }
                0
            }
            WM_TIMER => {
                match wparam {
                    TIMER_POPUP => {
                        with_app(|app| {
                            if app.shown {
                                app.show_popup(true);
                            }
                        });
                    }
                    _ => {
                        with_app(|app| app.refresh_tray());
                    }
                }
                0
            }
            WM_MOUSEMOVE => {
                let x = (lparam & 0xffff) as i16 as i32;
                let y = ((lparam >> 16) & 0xffff) as i16 as i32;
                with_app(|app| {
                    let hover = app.action_at(x, y);
                    if hover != app.hover {
                        app.hover = hover;
                        app.show_popup(true);
                    }
                    let cursor = if hover.is_some() { IDC_HAND } else { IDC_ARROW };
                    SetCursor(LoadCursorW(null_mut(), cursor));
                });
                let mut track = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: windows_sys::Win32::UI::Input::KeyboardAndMouse::TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut track);
                0
            }
            WM_MOUSELEAVE => {
                with_app(|app| {
                    if app.hover.is_some() {
                        app.hover = None;
                        app.show_popup(true);
                    }
                });
                0
            }
            WM_LBUTTONUP => {
                let x = (lparam & 0xffff) as i16 as i32;
                let y = ((lparam >> 16) & 0xffff) as i16 as i32;
                with_app(|app| {
                    if let Some(action) = app.action_at(x, y) {
                        app.command(action);
                    }
                });
                0
            }
            WM_KEYDOWN => {
                if wparam == VK_ESCAPE {
                    with_app(|app| app.hide_popup());
                    0
                } else {
                    DefWindowProcW(hwnd, message, wparam, lparam)
                }
            }
            WM_ACTIVATE => {
                // The card is a flyout: clicking anywhere else dismisses it.
                if wparam as u32 & 0xffff == 0 {
                    with_app(|app| app.hide_popup());
                }
                0
            }
            WM_SETTINGCHANGE | WM_THEMECHANGED => {
                with_app(|app| {
                    let dark = system_is_dark();
                    if dark != app.dark {
                        app.dark = dark;
                        if app.shown {
                            app.show_popup(true);
                        }
                    }
                });
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_ENDSESSION => {
                // Logging off or shutting down: take the dot with us.
                if wparam != 0 {
                    with_app(|app| unsafe {
                        Shell_NotifyIconW(NIM_DELETE, &app.nid);
                        app.icon_added = false;
                    });
                }
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => {
                let created = TASKBAR_CREATED.load(Ordering::Relaxed);
                if created != 0 && message == created {
                    // Explorer restarted, so every tray icon was dropped.
                    with_app(|app| {
                        app.icon_added = false;
                        app.refresh_tray();
                    });
                    return 0;
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
        }
    }

    // ----------------------------------------------------------------------
    // Startup

    pub fn run() {
        // Per-monitor DPI awareness, so the card is crisp at any scale instead
        // of being bitmap-stretched by the shell.
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }

        let args: Vec<String> = std::env::args().collect();
        if let Some(index) = args.iter().position(|arg| arg == "--preview") {
            let Some(path) = args.get(index + 1) else {
                eprintln!(
                    "usage: DeepSeekPriceClock.exe --preview <file.bmp> \
                     [unix-seconds] [scale] [local|utc]"
                );
                return;
            };
            let now = args
                .get(index + 2)
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or_else(now_secs);
            let scale = args
                .get(index + 3)
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(1.0);
            let zone = match args.get(index + 4).map(String::as_str) {
                Some("utc") => Zone::Utc,
                _ => Zone::Local,
            };
            preview(path, now, scale, zone);
            return;
        }

        unsafe {
            let mutex_name = wide(MUTEX_NAME);
            let mutex = CreateMutexW(null(), 0, mutex_name.as_ptr());
            if mutex.is_null() {
                return;
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                // One dot per session, exactly like the PowerShell script.
                CloseHandle(mutex);
                return;
            }

            let taskbar_created = RegisterWindowMessageW(wide(TASKBAR_CREATED_MESSAGE).as_ptr());
            TASKBAR_CREATED.store(taskbar_created, Ordering::Relaxed);

            let instance = GetModuleHandleW(null());
            let class_name = wide(WINDOW_CLASS);
            let window_class = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                hIcon: null_mut(),
                hCursor: null_mut(),
                hbrBackground: null_mut(),
                lpszMenuName: null(),
                lpszClassName: class_name.as_ptr(),
            };
            RegisterClassW(&window_class);

            let title = wide(APP_NAME);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            if hwnd.is_null() {
                CloseHandle(mutex);
                return;
            }

            *APP.0.get() = Some(App::new(hwnd));
            with_app(|app| app.refresh_tray());
            SetTimer(hwnd, TIMER_REFRESH, REFRESH_MS, None);

            let mut msg: MSG = zeroed();
            // GetMessageW returns 0 on WM_QUIT and -1 on error; either ends the loop.
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            if let Some(app) = (*APP.0.get()).take() {
                Shell_NotifyIconW(NIM_DELETE, &app.nid);
                if !app.off_icon.is_null() {
                    DestroyIcon(app.off_icon);
                }
                if !app.peak_icon.is_null() {
                    DestroyIcon(app.peak_icon);
                }
            }
            CloseHandle(mutex);
        }
    }

    /// Render the card to BMP files without a window, for screenshots and for
    /// checking the layout on a machine that is not running Windows.
    fn preview(path: &str, now: i64, scale: f32, zone: Zone) {
        for dark in [true, false] {
            let model = build_model(now, zone, dark, None, true, false);
            let Some(card) = paint::render(&model, scale) else {
                eprintln!("could not render the card");
                return;
            };
            let file = if dark {
                path.to_string()
            } else if let Some(stem) = path.strip_suffix(".bmp") {
                format!("{stem}-light.bmp")
            } else {
                format!("{path}-light.bmp")
            };
            if card.save_bmp(&file) {
                println!("wrote {file}");
            } else {
                eprintln!("could not write {file}");
            }
            card.destroy();
        }
    }

    // ----------------------------------------------------------------------
    // Placement and DPI

    /// Put the card above the pointer, right edges roughly aligned like a tray
    /// flyout, and keep it inside the monitor's work area.
    fn place_card(card: &Card) -> (i32, i32) {
        unsafe {
            let mut point: POINT = zeroed();
            GetCursorPos(&mut point);
            let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
            let mut info: MONITORINFO = zeroed();
            info.cbSize = size_of::<MONITORINFO>() as u32;
            let work = if GetMonitorInfoW(monitor, &mut info) != 0 {
                info.rcWork
            } else {
                RECT {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1040,
                }
            };

            let card_w = card.card_w;
            let card_h = card.card_h;

            let mut left = point.x + 10 - card_w;
            let mut top = point.y - 10 - card_h;
            if top < work.top + 4 {
                top = point.y + 18;
            }
            left = left.clamp(work.left + 4, (work.right - card_w - 4).max(work.left + 4));
            top = top.clamp(work.top + 4, (work.bottom - card_h - 4).max(work.top + 4));
            (left, top)
        }
    }

    fn dpi_scale_at_cursor() -> f32 {
        unsafe {
            let mut point: POINT = zeroed();
            GetCursorPos(&mut point);
            let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
            let mut dpi_x = 96u32;
            let mut dpi_y = 96u32;
            if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) != 0 {
                dpi_x = GetDpiForSystem();
            }
            if dpi_x == 0 {
                dpi_x = 96;
            }
            (dpi_x as f32 / 96.0).clamp(0.75, 4.0)
        }
    }

    // ----------------------------------------------------------------------
    // Settings and autostart

    fn reg_read(name: &str, flag: u32) -> Option<Vec<u8>> {
        unsafe {
            let key = wide(SETTINGS_KEY);
            let value = wide(name);
            let mut size: u32 = 4;
            let mut data = vec![0u8; size as usize];
            let status = RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                flag,
                null_mut(),
                data.as_mut_ptr() as *mut c_void,
                &mut size,
            );
            if status != ERROR_SUCCESS {
                return None;
            }
            data.truncate(size as usize);
            Some(data)
        }
    }

    fn read_dword(name: &str) -> Option<u32> {
        let data = reg_read(name, RRF_RT_REG_DWORD)?;
        (data.len() >= 4).then(|| u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
    }

    fn write_dword(name: &str, value: u32) {
        unsafe {
            let key_name = wide(SETTINGS_KEY);
            let mut key = null_mut();
            if RegCreateKeyExW(
                HKEY_CURRENT_USER,
                key_name.as_ptr(),
                0,
                null(),
                0,
                KEY_SET_VALUE,
                null_mut(),
                &mut key,
                null_mut(),
            ) != ERROR_SUCCESS
            {
                return;
            }
            let value_name = wide(name);
            let data = value.to_le_bytes();
            RegSetValueExW(
                key,
                value_name.as_ptr(),
                0,
                REG_DWORD,
                data.as_ptr(),
                data.len() as u32,
            );
            RegCloseKey(key);
        }
    }

    fn system_is_dark() -> bool {
        unsafe {
            let key = wide(THEME_KEY);
            let value = wide("AppsUseLightTheme");
            let mut data: u32 = 1;
            let mut size: u32 = 4;
            let status = RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_DWORD,
                null_mut(),
                &mut data as *mut u32 as *mut c_void,
                &mut size,
            );
            // Windows always sets this; if it is missing, prefer the dark card,
            // which is the one the GNOME extension's screenshots show.
            status != ERROR_SUCCESS || data == 0
        }
    }

    fn autostart_enabled() -> bool {
        unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                wide(RUN_KEY).as_ptr(),
                wide(RUN_VALUE).as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                null_mut(),
                null_mut(),
            ) == ERROR_SUCCESS
        }
    }

    fn set_autostart(exe: &str, enabled: bool) -> bool {
        unsafe {
            let key_name = wide(RUN_KEY);
            let mut key = null_mut();
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                key_name.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut key,
            ) != ERROR_SUCCESS
            {
                return false;
            }
            let value_name = wide(RUN_VALUE);
            let ok = if enabled {
                let data: Vec<u16> = format!("\"{exe}\"").encode_utf16().chain([0]).collect();
                RegSetValueExW(
                    key,
                    value_name.as_ptr(),
                    0,
                    REG_SZ,
                    data.as_ptr() as *const u8,
                    (data.len() * size_of::<u16>()) as u32,
                ) == ERROR_SUCCESS
            } else {
                RegDeleteValueW(key, value_name.as_ptr()) == ERROR_SUCCESS
            };
            RegCloseKey(key);
            ok
        }
    }

    fn open_pricing_page() {
        unsafe {
            let verb = wide("open");
            let url = wide(PRICING_URL);
            ShellExecuteW(
                null_mut(),
                verb.as_ptr(),
                url.as_ptr(),
                null(),
                null(),
                windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
            );
        }
    }

    fn current_exe() -> String {
        let mut buffer = [0u16; 32_768];
        let length =
            unsafe { GetModuleFileNameW(null_mut(), buffer.as_mut_ptr(), buffer.len() as u32) }
                as usize;
        String::from_utf16_lossy(&buffer[..length])
    }

    // ----------------------------------------------------------------------
    // Drawing the tray icon

    /// A 32 px dot: a soft halo with the solid dot on top, both in one colour.
    /// Drawn by supersampling in code so the binary carries no image assets and
    /// the icon needs no GDI+ or Direct2D. Called twice at startup.
    fn draw_dot(rgb: (u8, u8, u8)) -> HICON {
        const SIZE: i32 = 32;
        const SAMPLES: u32 = 4;
        const HALO_RADIUS: f64 = 14.0;
        const DOT_RADIUS: f64 = 8.0;
        const CENTRE: f64 = SIZE as f64 / 2.0;
        const HALO_ALPHA: f64 = 70.0;

        unsafe {
            let header = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: SIZE,
                biHeight: -SIZE, // top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            };
            let info = BITMAPINFO {
                bmiHeader: header,
                bmiColors: [RGBQUAD {
                    rgbBlue: 0,
                    rgbGreen: 0,
                    rgbRed: 0,
                    rgbReserved: 0,
                }],
            };

            let mut bits: *mut c_void = null_mut();
            let colour: windows_sys::Win32::Graphics::Gdi::HBITMAP =
                CreateDIBSection(null_mut(), &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if colour.is_null() || bits.is_null() {
                return null_mut();
            }

            let pixels =
                std::slice::from_raw_parts_mut(bits as *mut u8, (SIZE * SIZE * 4) as usize);
            let samples = (SAMPLES * SAMPLES) as f64;
            for y in 0..SIZE {
                for x in 0..SIZE {
                    let mut halo = 0u32;
                    let mut dot = 0u32;
                    for sy in 0..SAMPLES {
                        for sx in 0..SAMPLES {
                            let fx = x as f64 + (sx as f64 + 0.5) / SAMPLES as f64;
                            let fy = y as f64 + (sy as f64 + 0.5) / SAMPLES as f64;
                            let dx = fx - CENTRE;
                            let dy = fy - CENTRE;
                            let d2 = dx * dx + dy * dy;
                            if d2 <= HALO_RADIUS * HALO_RADIUS {
                                halo += 1;
                            }
                            if d2 <= DOT_RADIUS * DOT_RADIUS {
                                dot += 1;
                            }
                        }
                    }

                    let halo_alpha = HALO_ALPHA * halo as f64 / samples;
                    let dot_alpha = 255.0 * dot as f64 / samples;
                    let alpha = dot_alpha + halo_alpha * (1.0 - dot_alpha / 255.0);

                    let index = ((y * SIZE + x) * 4) as usize;
                    if alpha < 0.5 {
                        pixels[index..index + 4].fill(0);
                        continue;
                    }
                    pixels[index] = rgb.2; // B
                    pixels[index + 1] = rgb.1; // G
                    pixels[index + 2] = rgb.0; // R
                    pixels[index + 3] = alpha.round() as u8;
                }
            }

            // A 1 bpp mask has four bytes per 32 px row. Zero means "use the
            // colour bitmap", and passing a defined buffer avoids whatever
            // CreateBitmap(NULL) would leave behind.
            let mask_bits = [0u8; (SIZE as usize) * 4];
            let mask = CreateBitmap(SIZE, SIZE, 1, 1, mask_bits.as_ptr() as *const c_void);
            let icon_info = ICONINFO {
                fIcon: 1,
                xHotspot: 0,
                yHotspot: 0,
                hbmMask: mask,
                hbmColor: colour,
            };
            let icon = CreateIconIndirect(&icon_info);

            // The icon owns its copies, so the bitmaps can go now.
            DeleteObject(colour);
            DeleteObject(mask);
            icon
        }
    }

    // ----------------------------------------------------------------------
    // Small helpers

    fn now_secs() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs() as i64)
            .unwrap_or(0)
    }

    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as i64)
            .unwrap_or(0)
    }

    /// NUL-terminated UTF-16 for the wide Win32 calls.
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }

    /// Fixed-size, NUL-padded UTF-16 field such as `szTip` or `szInfo`.
    fn fixed_text<const N: usize>(text: &str) -> [u16; N] {
        let mut out = [0u16; N];
        for (slot, unit) in out.iter_mut().take(N - 1).zip(text.encode_utf16()) {
            *slot = unit;
        }
        out
    }
}

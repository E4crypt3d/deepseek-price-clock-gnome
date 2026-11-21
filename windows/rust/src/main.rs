//! Windows 11 tray clock for DeepSeek peak/off-peak pricing.
//!
//! The native counterpart of `windows/DeepSeekPriceClock.ps1`: the same dot,
//! countdown, menu and switch notifications, without PowerShell. The pricing
//! rules are shared with the GNOME extension through
//! [`deepseek_price_clock::pricing`].
//!
//! It is deliberately small: one hidden window, one 20 second timer, two icons
//! drawn once at startup, and a popup menu built only while it is open. Nothing
//! is allocated on the timer path except a short tooltip string, and every GDI
//! object is either cached for the life of the process or destroyed as soon as
//! it is not needed.
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
mod win {
    use std::cell::UnsafeCell;
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use deepseek_price_clock::pricing::{self, Kind};

    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, HWND, LPARAM, LRESULT, POINT,
        SYSTEMTIME, WPARAM,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
        RGBQUAD,
    };
    use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW, HKEY_CURRENT_USER, KEY_SET_VALUE,
        REG_SZ, RRF_RT_REG_SZ,
    };
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::System::Time::SystemTimeToTzSpecificLocalTime;
    use windows_sys::Win32::UI::Shell::{
        Shell_NotifyIconW, ShellExecuteW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
        NIM_MODIFY, NOTIFYICONDATAW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu,
        DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, KillTimer, PostMessageW, PostQuitMessage,
        RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, SetTimer, TrackPopupMenu, TranslateMessage,
        HICON, HMENU, ICONINFO, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MSG, TPM_LEFTALIGN,
        TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY, WM_ENDSESSION, WM_LBUTTONUP,
        WM_NULL, WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
    };

    const APP_NAME: &str = "DeepSeek Price Clock";
    const WINDOW_CLASS: &str = "DeepSeekPriceClockWindow";
    const MUTEX_NAME: &str = "Local\\DeepSeekPriceClock";
    const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const RUN_VALUE: &str = "DeepSeekPriceClock";
    const PRICING_URL: &str = "https://api-docs.deepseek.com/quick_start/pricing";
    const TASKBAR_CREATED_MESSAGE: &str = "TaskbarCreated";

    const TRAY_ICON_ID: u32 = 1;
    const TRAY_MESSAGE: u32 = WM_APP + 1;
    const TIMER_ID: usize = 1;
    /// 20 s keeps the minute countdown honest and catches a flip promptly
    /// without doing any work in between, same as the PowerShell app.
    const REFRESH_MS: u32 = 20_000;

    const MENU_PRICING: u32 = 1001;
    const MENU_AUTOSTART: u32 = 1002;
    const MENU_NOTIFY: u32 = 1003;
    const MENU_EXIT: u32 = 1004;

    // The same dot colours as the tray script and the GNOME extension.
    const OFF_RGB: (u8, u8, u8) = (87, 227, 137);
    const PEAK_RGB: (u8, u8, u8) = (255, 123, 114);

    /// Rates per 1M tokens (input cache hit, input cache miss, output), from the
    /// upstream extension's table and `price.js`.
    const MODELS: &[Model] = &[
        Model {
            name: "V4.1 Flash",
            off: ["$0.15", "$0.003", "$0.60"],
            peak: ["$0.30", "$0.006", "$1.20"],
        },
        Model {
            name: "V4 Pro",
            off: ["$0.66", "$0.022", "$1.98"],
            peak: ["$1.32", "$0.044", "$3.96"],
        },
    ];

    struct Model {
        name: &'static str,
        off: [&'static str; 3],
        peak: [&'static str; 3],
    }

    /// `TaskbarCreated`, registered once before the icon is added. Explorer
    /// broadcasts it when it restarts, which is how the indicator comes back
    /// after a crash instead of silently vanishing.
    static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

    /// The app lives for the whole message loop, so the window procedure needs a
    /// stable place to find it. Windows and the menu run on this one thread.
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
        icon_added: bool,
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
                notify: true,
                icon_added: false,
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

        /// Paint the current state, adding the dot if the shell does not have
        /// it. Called every tick, so a lost or rejected icon is put back within
        /// 20 s instead of staying gone for the session.
        fn refresh(&mut self) {
            let now = now_secs();
            let kind = pricing::kind_at(now);
            let next = pricing::next_switch(now);
            let left = pricing::format_countdown(next.time - now);

            self.nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            self.nid.hIcon = self.icon_for(kind);
            self.nid.szTip = fixed_text(&format!("DeepSeek {}, {left} left", kind.label().to_lowercase()));

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
                ("Peak pricing started", &model.peak)
            } else {
                ("Off-peak pricing started", &model.off)
            };
            let body = format!(
                "DeepSeek {} is now {} in, {} out per 1M tokens.",
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

        /// Build a fresh menu with current times. The caller owns and destroys it.
        fn build_menu(&mut self) -> HMENU {
            let now = now_secs();
            let kind = pricing::kind_at(now);
            let next = pricing::next_switch(now);
            let left = pricing::format_countdown(next.time - now);

            unsafe {
                let menu = CreatePopupMenu();

                add_info(menu, kind.label());
                add_info(
                    menu,
                    &format!(
                        "{} until {}, {} from now",
                        kind.label(),
                        local_hhmm(next.time),
                        left
                    ),
                );
                AppendMenuW(menu, MF_SEPARATOR, 0, null());

                let switches = CreatePopupMenu();
                for switch in pricing::next_switches(now, 4) {
                    add_info(
                        switches,
                        &format!(
                            "{} {}, in {}",
                            local_hhmm(switch.time),
                            switch.kind.label().to_lowercase(),
                            pricing::format_countdown(switch.time - now)
                        ),
                    );
                }
                add_submenu(menu, "Next switches", switches);

                let rates = CreatePopupMenu();
                add_info(rates, "model, input / cached input / output");
                for model in MODELS {
                    add_info(rates, model.name);
                    add_info(rates, &format!("  off   {}", model.off.join(" / ")));
                    add_info(rates, &format!("  peak  {}", model.peak.join(" / ")));
                }
                add_submenu(menu, "Rates per 1M tokens", rates);

                AppendMenuW(menu, MF_SEPARATOR, 0, null());
                add_action(menu, MENU_PRICING, "Open pricing page", false);
                add_action(menu, MENU_AUTOSTART, "Start with Windows", autostart_enabled());
                add_action(menu, MENU_NOTIFY, "Notify when prices switch", self.notify);
                AppendMenuW(menu, MF_SEPARATOR, 0, null());
                add_action(menu, MENU_EXIT, "Exit", false);

                menu
            }
        }

        fn command(&mut self, command: u32) {
            match command {
                MENU_PRICING => open_pricing_page(),
                MENU_AUTOSTART => {
                    if autostart_enabled() {
                        set_autostart(&self.exe, false);
                    } else {
                        set_autostart(&self.exe, true);
                    }
                }
                MENU_NOTIFY => self.notify = !self.notify,
                MENU_EXIT => self.exit(),
                _ => {}
            }
        }

        /// Remove the dot before leaving so the shell does not keep a ghost,
        /// then end the message loop.
        fn exit(&mut self) {
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &self.nid);
                self.icon_added = false;
                KillTimer(self.hwnd, TIMER_ID);
                DestroyWindow(self.hwnd);
            }
        }
    }

    unsafe fn add_info(menu: HMENU, text: &str) {
        let text = wide(text);
        AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, text.as_ptr());
    }

    unsafe fn add_action(menu: HMENU, id: u32, text: &str, checked: bool) {
        let text = wide(text);
        let flags = MF_STRING | if checked { MF_CHECKED } else { 0 };
        AppendMenuW(menu, flags, id as usize, text.as_ptr());
    }

    unsafe fn add_submenu(parent: HMENU, title: &str, submenu: HMENU) {
        let title = wide(title);
        AppendMenuW(parent, MF_POPUP, submenu as usize, title.as_ptr());
    }

    /// Open the menu at the cursor. The menu is built, shown and destroyed
    /// within this call so it holds no memory between clicks.
    fn open_menu() {
        let built = unsafe { with_app(|app| (app.build_menu(), app.hwnd)) };
        let Some((menu, hwnd)) = built else { return };
        if menu.is_null() {
            return;
        }

        unsafe {
            let mut point: POINT = zeroed();
            GetCursorPos(&mut point);
            // Without this the menu does not close when clicking elsewhere.
            SetForegroundWindow(hwnd);
            let command = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN,
                point.x,
                point.y,
                0,
                hwnd,
                null(),
            );
            PostMessageW(hwnd, WM_NULL, 0, 0);
            DestroyMenu(menu);

            if command != 0 {
                with_app(|app| app.command(command as u32));
            }
        }
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match message {
            TRAY_MESSAGE => {
                // The low word carries the mouse message, in the legacy callback
                // layout used when no icon version was requested.
                match lparam as u32 & 0xffff {
                    WM_LBUTTONUP | WM_RBUTTONUP | WM_CONTEXTMENU => open_menu(),
                    _ => {}
                }
                0
            }
            WM_TIMER => {
                with_app(|app| app.refresh());
                0
            }
            WM_COMMAND => {
                let command = (wparam as u32) & 0xffff;
                with_app(|app| app.command(command));
                0
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
                        app.refresh();
                    });
                    return 0;
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
        }
    }

    pub fn run() {
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
                0,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
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
            with_app(|app| app.refresh());
            SetTimer(hwnd, TIMER_ID, REFRESH_MS, None);

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

    // --- Autostart ---------------------------------------------------------
    // A per-user Run value, so no shortcut, no admin rights and nothing
    // outside HKEY_CURRENT_USER.

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
            if RegOpenKeyExW(HKEY_CURRENT_USER, key_name.as_ptr(), 0, KEY_SET_VALUE, &mut key) != ERROR_SUCCESS {
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

    fn current_exe() -> String {
        let mut buffer = [0u16; 32_768];
        let length = unsafe { GetModuleFileNameW(null_mut(), buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
        String::from_utf16_lossy(&buffer[..length])
    }

    // --- Drawing -----------------------------------------------------------

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
            let colour: HBITMAP = CreateDIBSection(null_mut(), &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if colour.is_null() || bits.is_null() {
                return null_mut();
            }

            let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, (SIZE * SIZE * 4) as usize);
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

    // --- Small helpers -----------------------------------------------------

    fn now_secs() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs() as i64)
            .unwrap_or(0)
    }

    /// HH:MM in the user's time zone for a UTC instant.
    fn local_hhmm(secs: i64) -> String {
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
            format!("{:02}:{:02}", local.wHour, local.wMinute)
        } else {
            format!("{:02}:{:02}", utc.wHour, utc.wMinute)
        }
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

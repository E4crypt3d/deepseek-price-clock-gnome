//! The GNOME-style popup card for the Windows clock.
//!
//! Windows has no supported way to put a live widget in the taskbar, so the
//! tray icon opens this: a rounded panel that mirrors the GNOME Shell menu —
//! status dot, big countdown, 24-hour timeline, upcoming windows and the rate
//! table. It is drawn by hand into a 32-bit DIB (so it can be blitted as a
//! layered window with a soft shadow and real rounded corners) instead of
//! using a Win32 menu, which is what made the old Windows build look nothing
//! like the extension.
//!
//! Shapes are rasterised here with 4x4 supersampling; text is GDI (Segoe UI)
//! because that is the one text renderer every Windows machine already has.
//! The card follows the Windows light/dark app theme, as the extension follows
//! the shell's.

use std::mem::size_of;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW, GetDC,
    GetStockObject, GetTextExtentPoint32W, ReleaseDC, SelectObject, SetBkMode, SetTextColor,
    ANTIALIASED_QUALITY, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CLIP_DEFAULT_PRECIS,
    DEFAULT_CHARSET, DEFAULT_GUI_FONT, DEFAULT_PITCH, DIB_RGB_COLORS, DT_CALCRECT, DT_CENTER,
    DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_TOP, DT_VCENTER, DT_WORDBREAK, FF_DONTCARE,
    FW_BOLD, FW_NORMAL, FW_SEMIBOLD, HBITMAP, HDC, HFONT, HGDIOBJ, OUT_TT_PRECIS, RGBQUAD,
    TRANSPARENT,
};

use deepseek_price_clock::pricing::{Kind, Segment};

/// Clicks the card understands. Anything else is not interactive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    ZoneLocal,
    ZoneUtc,
    ToggleNotify,
    ToggleAutostart,
    OpenPricing,
    Quit,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Zone {
    Local,
    Utc,
}

/// One row of the upcoming-price-window list.
pub struct WindowRow {
    pub kind: Kind,
    pub range: String,
    pub meta: String,
    pub current: bool,
}

/// One model's three current rates: input, cached input, output.
pub struct RateRow {
    pub name: String,
    pub amounts: [String; 3],
}

/// Everything the card shows, already formatted by the caller so this module
/// only has to worry about layout and paint.
pub struct Model {
    pub peak: bool,
    pub zone: Zone,
    pub dark: bool,
    pub zone_caption: String,
    pub badge: String,
    pub countdown: String,
    pub until: String,
    pub segments: Vec<Segment>,
    /// Minute of the displayed day, 0..1440, where the "now" line goes.
    pub now_minute: f32,
    pub windows_caption: String,
    pub windows: Vec<WindowRow>,
    pub rates_caption: String,
    pub rates_header: String,
    pub rates: Vec<RateRow>,
    pub next_rates: String,
    pub notify: bool,
    pub autostart: bool,
    pub hover: Option<Action>,
}

/// A rendered card: a memory DC holding a premultiplied BGRA bitmap ready for
/// `UpdateLayeredWindow`, plus the click areas in bitmap coordinates.
pub struct Card {
    pub hdc: HDC,
    pub bitmap: HBITMAP,
    pub bits: *mut u8,
    pub width: i32,
    pub height: i32,
    /// Size of the visible card, i.e. the bitmap minus its shadow margin.
    pub card_w: i32,
    pub card_h: i32,
    /// Where the visible card starts inside the bitmap (the rest is shadow).
    pub offset_x: i32,
    pub offset_y: i32,
    pub hits: Vec<(Action, RECT)>,
}

impl Card {
    /// Write the rendered bitmap as a 32-bit BMP. Used by `--preview`, which is
    /// how the README screenshots and the layout are checked without Windows.
    pub fn save_bmp(&self, path: &str) -> bool {
        use std::io::Write;
        let stride = (self.width * 4) as usize;
        let image = stride * self.height as usize;
        let mut header = Vec::with_capacity(54);
        let file_size = 54 + image;
        header.extend_from_slice(b"BM");
        header.extend_from_slice(&(file_size as u32).to_le_bytes());
        header.extend_from_slice(&[0, 0, 0, 0]);
        header.extend_from_slice(&54u32.to_le_bytes());
        header.extend_from_slice(&40u32.to_le_bytes());
        header.extend_from_slice(&self.width.to_le_bytes());
        header.extend_from_slice(&(-self.height).to_le_bytes());
        header.extend_from_slice(&1u16.to_le_bytes());
        header.extend_from_slice(&32u16.to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());
        header.extend_from_slice(&(image as u32).to_le_bytes());
        header.extend_from_slice(&[0u8; 16]);
        let Ok(mut file) = std::fs::File::create(path) else {
            return false;
        };
        let pixels = unsafe { std::slice::from_raw_parts(self.bits, image) };
        file.write_all(&header).is_ok() && file.write_all(pixels).is_ok()
    }

    pub fn destroy(self) {
        unsafe {
            // Deleting the DC releases the bitmap it holds; the bitmap then has
            // to be deleted on its own.
            DeleteDC(self.hdc);
            DeleteObject(self.bitmap as HGDIOBJ);
        }
    }
}

#[derive(Clone, Copy)]
struct Rgb(u8, u8, u8);

#[derive(Clone, Copy)]
struct Ink {
    rgb: Rgb,
    alpha: f32,
}

const fn ink(r: u8, g: u8, b: u8) -> Ink {
    Ink {
        rgb: Rgb(r, g, b),
        alpha: 1.0,
    }
}

impl Ink {
    fn alpha(self, alpha: f32) -> Ink {
        Ink { alpha, ..self }
    }
}

struct Palette {
    card: Ink,
    border: Ink,
    title: Ink,
    strong: Ink,
    muted: Ink,
    faint: Ink,
    off: Ink,
    peak: Ink,
    toggle_bg: Ink,
    toggle_fg: Ink,
    toggle_active_bg: Ink,
    toggle_active_fg: Ink,
    row_hover: Ink,
    track_bg: Ink,
    switch_off: Ink,
    switch_on: Ink,
    knob: Ink,
    shade: Ink,
    shadow_alpha: f32,
}

fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            card: ink(36, 36, 36),
            border: ink(255, 255, 255).alpha(0.10),
            title: ink(255, 255, 255),
            strong: ink(230, 230, 230),
            muted: ink(176, 176, 176),
            faint: ink(138, 138, 138),
            off: ink(87, 227, 137),
            peak: ink(255, 123, 114),
            toggle_bg: ink(255, 255, 255).alpha(0.08),
            toggle_fg: ink(208, 208, 208),
            toggle_active_bg: ink(87, 227, 137).alpha(0.28),
            toggle_active_fg: ink(255, 255, 255),
            row_hover: ink(255, 255, 255).alpha(0.06),
            track_bg: ink(255, 255, 255).alpha(0.05),
            switch_off: ink(255, 255, 255).alpha(0.16),
            switch_on: ink(53, 132, 228),
            knob: ink(246, 245, 244),
            shade: ink(0, 0, 0),
            shadow_alpha: 0.46,
        }
    } else {
        Palette {
            card: ink(250, 250, 250),
            border: ink(0, 0, 0).alpha(0.12),
            title: ink(28, 28, 28),
            strong: ink(36, 41, 47),
            muted: ink(87, 96, 106),
            faint: ink(110, 119, 129),
            off: ink(26, 127, 55),
            peak: ink(207, 34, 46),
            toggle_bg: ink(0, 0, 0).alpha(0.06),
            toggle_fg: ink(36, 41, 47),
            toggle_active_bg: ink(26, 127, 55).alpha(0.18),
            toggle_active_fg: ink(15, 46, 24),
            row_hover: ink(0, 0, 0).alpha(0.05),
            track_bg: ink(0, 0, 0).alpha(0.05),
            switch_off: ink(0, 0, 0).alpha(0.16),
            switch_on: ink(28, 113, 216),
            knob: ink(255, 255, 255),
            shade: ink(0, 0, 0),
            shadow_alpha: 0.24,
        }
    }
}

// --------------------------------------------------------------------------
// Canvas

#[derive(Clone, Copy)]
struct RectF {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl RectF {
    fn right(&self) -> f32 {
        self.x + self.w
    }

    fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// Signed distance from a point to a rounded rectangle: negative inside.
fn round_rect_distance(px: f32, py: f32, rect: RectF, radius: f32) -> f32 {
    let half_w = rect.w / 2.0;
    let half_h = rect.h / 2.0;
    let r = radius.min(half_w).min(half_h).max(0.0);
    let qx = (px - (rect.x + half_w)).abs() - (half_w - r);
    let qy = (py - (rect.y + half_h)).abs() - (half_h - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

/// Four by four sample offsets, the same pattern for every shape so edges are
/// smooth at any size and DPI.
const SAMPLES: f32 = 4.0;

fn rounded_coverage(px: f32, py: f32, rect: RectF, radius: f32) -> f32 {
    let mut covered = 0.0;
    for sy in 0..4 {
        for sx in 0..4 {
            let ox = (sx as f32 + 0.5) / SAMPLES;
            let oy = (sy as f32 + 0.5) / SAMPLES;
            if round_rect_distance(px + ox, py + oy, rect, radius) <= 0.0 {
                covered += 1.0;
            }
        }
    }
    covered / (SAMPLES * SAMPLES)
}

struct Canvas {
    w: i32,
    h: i32,
    /// Card-local drawing coordinates are shifted by the shadow margin.
    ox: i32,
    oy: i32,
    bits: *mut u8,
}

impl Canvas {
    unsafe fn new(w: i32, h: i32, ox: i32, oy: i32, bits: *mut u8) -> Canvas {
        let canvas = Canvas { w, h, ox, oy, bits };
        std::ptr::write_bytes(bits, 0, (w * h * 4) as usize);
        canvas
    }

    /// Source-over composite in straight (not premultiplied) alpha. The final
    /// pass premultiplies once for `UpdateLayeredWindow`.
    fn blend(&mut self, x: i32, y: i32, color: Ink) {
        let (x, y) = (x + self.ox, y + self.oy);
        if x < 0 || y < 0 || x >= self.w || y >= self.h || color.alpha <= 0.0 {
            return;
        }
        let i = ((y * self.w + x) * 4) as usize;
        let sa = color.alpha.clamp(0.0, 1.0);
        unsafe {
            let dst = self.bits.add(i);
            let da = *dst.add(3) as f32 / 255.0;
            let oa = sa + da * (1.0 - sa);
            if oa <= 0.0 {
                return;
            }
            let src = [color.rgb.2 as f32, color.rgb.1 as f32, color.rgb.0 as f32]; // BGR
            for c in 0..3 {
                let d = *dst.add(c) as f32;
                let v = (src[c] * sa + d * da * (1.0 - sa)) / oa;
                *dst.add(c) = v.round().clamp(0.0, 255.0) as u8;
            }
            *dst.add(3) = (oa * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }

    fn fill_rect(&mut self, rect: RectF, color: Ink) {
        let (x0, y0) = (rect.x.floor() as i32, rect.y.floor() as i32);
        let (x1, y1) = (rect.right().ceil() as i32, rect.bottom().ceil() as i32);
        for y in y0..y1 {
            for x in x0..x1 {
                self.blend(x, y, color);
            }
        }
    }

    fn fill_round_rect(&mut self, rect: RectF, radius: f32, color: Ink) {
        let (x0, y0) = (rect.x.floor() as i32, rect.y.floor() as i32);
        let (x1, y1) = (rect.right().ceil() as i32, rect.bottom().ceil() as i32);
        for y in y0..y1 {
            for x in x0..x1 {
                let coverage = rounded_coverage(x as f32, y as f32, rect, radius);
                if coverage > 0.0 {
                    self.blend(x, y, color.alpha(color.alpha * coverage));
                }
            }
        }
    }

    /// The 1 px hairline around the card, sampled from the same distance field
    /// so it follows the corners cleanly.
    fn stroke_round_rect(&mut self, rect: RectF, radius: f32, width: f32, color: Ink) {
        let (x0, y0) = (rect.x.floor() as i32, rect.y.floor() as i32);
        let (x1, y1) = (rect.right().ceil() as i32, rect.bottom().ceil() as i32);
        for y in y0..y1 {
            for x in x0..x1 {
                let mut inside: f32 = 0.0;
                let mut band: f32 = 0.0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let ox = (sx as f32 + 0.5) / SAMPLES;
                        let oy = (sy as f32 + 0.5) / SAMPLES;
                        let d = round_rect_distance(x as f32 + ox, y as f32 + oy, rect, radius);
                        if d <= 0.0 {
                            inside += 1.0;
                        }
                        if d > -width && d <= 0.0 {
                            band += 1.0;
                        }
                    }
                }
                if band > 0.0 {
                    // Clip the stroke to the pixels the shape covers, so the
                    // outside of the card keeps its shadow-only alpha.
                    let coverage = (band / 16.0).min(inside / 16.0);
                    self.blend(x, y, color.alpha(color.alpha * coverage));
                }
            }
        }
    }

    fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: Ink) {
        let rect = RectF {
            x: cx - radius,
            y: cy - radius,
            w: radius * 2.0,
            h: radius * 2.0,
        };
        self.fill_round_rect(rect, radius, color);
    }

    /// Fill a horizontal slice of a rounded bar, clipped to the bar's corners.
    /// Used for the timeline bands and its "now" marker.
    fn fill_bar_slice(&mut self, bar: RectF, radius: f32, x0: f32, x1: f32, color: Ink) {
        let x0 = x0.max(bar.x).floor() as i32;
        let x1 = x1.min(bar.right()).ceil() as i32;
        for y in bar.y.floor() as i32..bar.bottom().ceil() as i32 {
            for x in x0..x1 {
                let coverage = rounded_coverage(x as f32, y as f32, bar, radius);
                if coverage > 0.0 {
                    self.blend(x, y, color.alpha(color.alpha * coverage));
                }
            }
        }
    }

    /// Soft drop shadow: alpha falls off with the distance from a rounded
    /// rectangle, so the card floats instead of sitting in a hard-edged box.
    fn shadow(&mut self, rect: RectF, radius: f32, blur: f32, color: Ink, strength: f32) {
        let x0 = (rect.x - blur).floor() as i32;
        let y0 = (rect.y - blur).floor() as i32;
        let x1 = (rect.right() + blur).ceil() as i32;
        let y1 = (rect.bottom() + blur).ceil() as i32;
        for y in y0..y1 {
            for x in x0..x1 {
                let d = round_rect_distance(x as f32 + 0.5, y as f32 + 0.5, rect, radius);
                if d > blur {
                    continue;
                }
                let t = ((blur - d) / blur).clamp(0.0, 1.0);
                let alpha = strength * t * t;
                if alpha > 0.002 {
                    self.blend(x, y, color.alpha(alpha));
                }
            }
        }
    }

    /// Force the card's interior fully opaque (GDI text does not maintain the
    /// alpha channel) and premultiply everything for the layered window.
    fn seal(&mut self, mask: &[u8]) {
        for i in 0..(self.w * self.h) as usize {
            unsafe {
                let p = self.bits.add(i * 4);
                let a = if mask[i] == 255 { 255 } else { *p.add(3) };
                *p.add(3) = a;
                if a != 255 {
                    for c in 0..3 {
                        *p.add(c) = ((*p.add(c) as u32 * a as u32 + 127) / 255) as u8;
                    }
                }
            }
        }
    }
}

// --------------------------------------------------------------------------
// Text

struct Fonts {
    big: HFONT,
    title: HFONT,
    badge: HFONT,
    body: HFONT,
    strong: HFONT,
    caption: HFONT,
    tiny: HFONT,
    pill: HFONT,
}

fn make_font(size: f32, weight: u32) -> HFONT {
    let name: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
    unsafe {
        CreateFontW(
            -(size.round() as i32),
            0,
            0,
            0,
            weight as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_TT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            ANTIALIASED_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            name.as_ptr(),
        )
    }
}

impl Fonts {
    fn new(scale: f32) -> Fonts {
        Fonts {
            big: make_font(30.0 * scale, FW_BOLD),
            title: make_font(15.0 * scale, FW_BOLD),
            badge: make_font(13.5 * scale, FW_SEMIBOLD),
            body: make_font(13.0 * scale, FW_NORMAL),
            strong: make_font(13.0 * scale, FW_SEMIBOLD),
            caption: make_font(12.0 * scale, FW_NORMAL),
            tiny: make_font(11.0 * scale, FW_NORMAL),
            pill: make_font(11.5 * scale, FW_NORMAL),
        }
    }

    fn destroy(&self) {
        unsafe {
            for font in [
                self.big,
                self.title,
                self.badge,
                self.body,
                self.strong,
                self.caption,
                self.tiny,
                self.pill,
            ] {
                DeleteObject(font as HGDIOBJ);
            }
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn colorref(rgb: Rgb) -> u32 {
    rgb.0 as u32 | (rgb.1 as u32) << 8 | (rgb.2 as u32) << 16
}

unsafe fn text_width(dc: HDC, font: HFONT, text: &str) -> i32 {
    SelectObject(dc, font as HGDIOBJ);
    let text = wide(text);
    let mut size = SIZE { cx: 0, cy: 0 };
    GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32 - 1, &mut size);
    size.cx
}

unsafe fn draw_text(dc: HDC, font: HFONT, color: Ink, text: &str, rect: RECT, align: u32) {
    SelectObject(dc, font as HGDIOBJ);
    SetTextColor(dc, colorref(color.rgb));
    let text = wide(text);
    let mut rect = rect;
    DrawTextW(
        dc,
        text.as_ptr(),
        -1,
        &mut rect,
        align | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
    );
}

unsafe fn measure_wrapped(dc: HDC, font: HFONT, text: &str, width: i32) -> i32 {
    SelectObject(dc, font as HGDIOBJ);
    let text = wide(text);
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: 0,
    };
    DrawTextW(
        dc,
        text.as_ptr(),
        -1,
        &mut rect,
        DT_CALCRECT | DT_WORDBREAK | DT_LEFT | DT_NOPREFIX,
    );
    rect.bottom - rect.top
}

unsafe fn draw_wrapped(dc: HDC, font: HFONT, color: Ink, text: &str, rect: RECT) {
    SelectObject(dc, font as HGDIOBJ);
    SetTextColor(dc, colorref(color.rgb));
    let text = wide(text);
    let mut rect = rect;
    DrawTextW(
        dc,
        text.as_ptr(),
        -1,
        &mut rect,
        DT_WORDBREAK | DT_LEFT | DT_TOP | DT_NOPREFIX,
    );
}

// --------------------------------------------------------------------------
// Layout

fn rect(x: f32, y: f32, w: f32, h: f32) -> RECT {
    RECT {
        left: x.round() as i32,
        top: y.round() as i32,
        right: (x + w).round() as i32,
        bottom: (y + h).round() as i32,
    }
}

struct Plan {
    card_w: f32,
    card_h: f32,
    title: RECT,
    local: RECT,
    utc: RECT,
    zone: RECT,
    dot: (f32, f32),
    badge: RECT,
    count: RECT,
    until: RECT,
    bar: RectF,
    ticks: RECT,
    windows_caption: RECT,
    window_rows: Vec<(RECT, RECT, RECT)>,
    rates_caption: RECT,
    rates_header: RECT,
    rate_rows: Vec<(RECT, [RECT; 3])>,
    next_rates: RECT,
    separator: RECT,
    switch_rows: Vec<(Action, RECT, RECT)>,
    link_row: RECT,
    quit_row: RECT,
    link: RECT,
    quit: RECT,
    hits: Vec<(Action, RECT)>,
    radius: f32,
}

fn plan(model: &Model, scale: f32, dc: HDC, fonts: &Fonts) -> Plan {
    let s = |v: f32| v * scale;
    let pad = s(18.0);
    let card_w = s(400.0);
    let content_w = card_w - pad * 2.0;
    let right = card_w - pad;
    let mut y = s(16.0);
    let mut hits = Vec::new();

    // Header: title on the left, Local/UTC pills on the right.
    let header_h = s(20.0);
    let title = rect(pad, y, content_w * 0.62, header_h);

    let pill_h = s(20.0);
    let pill_pad = s(9.0);
    let local_text = unsafe { text_width(dc, fonts.pill, "Local") } as f32;
    let utc_text = unsafe { text_width(dc, fonts.pill, "UTC") } as f32;
    let local_w = local_text + pill_pad * 2.0;
    let utc_w = utc_text + pill_pad * 2.0;
    let utc = rect(right - utc_w, y, utc_w, pill_h);
    let local = rect(right - utc_w - s(2.0) - local_w, y, local_w, pill_h);
    hits.push((Action::ZoneLocal, local));
    hits.push((Action::ZoneUtc, utc));
    y += header_h + s(4.0);

    let zone = rect(pad, y, content_w, s(16.0));
    y += s(16.0) + s(9.0);

    // Hero: status dot, badge, the countdown and when the next flip is.
    let dot_y = y + s(9.0);
    let badge = rect(pad + s(22.0), y, content_w - s(22.0), s(18.0));
    y += s(22.0);
    let count = rect(pad, y, content_w, s(38.0));
    y += s(38.0) + s(2.0);
    let until = rect(pad, y, content_w, s(18.0));
    y += s(18.0) + s(10.0);

    let bar = RectF {
        x: pad,
        y,
        w: content_w,
        h: s(9.0),
    };
    y += s(9.0) + s(4.0);
    let ticks = rect(pad, y, content_w, s(14.0));
    y += s(14.0) + s(12.0);

    let windows_caption = rect(pad, y, content_w, s(16.0));
    y += s(16.0) + s(4.0);
    let mut window_rows = Vec::new();
    for _ in 0..model.windows.len() {
        let kind = rect(pad, y, s(58.0), s(21.0));
        let range = rect(pad + s(58.0), y, content_w - s(58.0) - s(72.0), s(21.0));
        let meta = rect(right - s(72.0), y, s(72.0), s(21.0));
        window_rows.push((kind, range, meta));
        y += s(21.0);
    }

    y += s(8.0);
    let rates_caption = rect(pad, y, content_w * 0.6, s(16.0));
    let rates_header = rect(pad + content_w * 0.6, y, content_w * 0.4, s(16.0));
    y += s(16.0) + s(4.0);
    let mut rate_rows = Vec::new();
    let column_w = s(62.0);
    for _ in 0..model.rates.len() {
        let name = rect(pad, y, content_w - column_w * 3.0, s(20.0));
        let mut amounts = [RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        }; 3];
        for (i, slot) in amounts.iter_mut().enumerate() {
            let end = right - column_w * (2.0 - i as f32);
            *slot = rect(end - column_w, y, column_w, s(20.0));
        }
        rate_rows.push((name, amounts));
        y += s(20.0);
    }

    y += s(5.0);
    let next_h = unsafe { measure_wrapped(dc, fonts.tiny, &model.next_rates, content_w as i32) };
    let next_rates = rect(pad, y, content_w, next_h as f32);
    y += next_h as f32 + s(12.0);

    let separator = rect(0.0, y, card_w, s(1.0));
    y += s(1.0) + s(3.0);

    let row_h = s(40.0);
    let switch_w = s(40.0);
    let switch_h = s(22.0);
    let mut switch_rows = Vec::new();
    for action in [Action::ToggleNotify, Action::ToggleAutostart] {
        let row = rect(s(8.0), y, card_w - s(16.0), row_h);
        let track = rect(
            right - switch_w,
            y + (row_h - switch_h) / 2.0,
            switch_w,
            switch_h,
        );
        hits.push((action, row));
        switch_rows.push((action, row, track));
        y += row_h;
    }

    let footer_h = s(34.0);
    let link_row = rect(s(8.0), y, card_w - s(16.0), footer_h);
    let link = rect(pad, y, content_w, footer_h);
    y += footer_h;
    let quit_row = rect(s(8.0), y, card_w - s(16.0), footer_h);
    let quit = rect(pad, y, content_w, footer_h);
    y += footer_h;
    hits.push((Action::OpenPricing, link_row));
    hits.push((Action::Quit, quit_row));

    let card_h = y + s(10.0);

    Plan {
        card_w,
        card_h,
        title,
        local,
        utc,
        zone,
        dot: (pad + s(7.5), dot_y),
        badge,
        count,
        until,
        bar,
        ticks,
        windows_caption,
        window_rows,
        rates_caption,
        rates_header,
        rate_rows,
        next_rates,
        separator,
        switch_rows,
        link_row,
        quit_row,
        link,
        quit,
        hits,
        radius: s(14.0),
    }
}

// --------------------------------------------------------------------------
// Render

/// Render the card. The caller owns the result and must call `destroy`.
pub fn render(model: &Model, scale: f32) -> Option<Card> {
    let scale = scale.clamp(0.75, 4.0);
    unsafe {
        let screen = GetDC(null_mut() as HWND);
        if screen.is_null() {
            return None;
        }
        let fonts = Fonts::new(scale);
        // Measurements need a DC before the bitmap exists; a scratch DC is
        // deleted before the fonts so they can be deleted cleanly.
        let measure = CreateCompatibleDC(screen);
        if measure.is_null() {
            fonts.destroy();
            ReleaseDC(null_mut() as HWND, screen);
            return None;
        }

        let plan = plan(model, scale, measure, &fonts);

        // Room for the shadow around the card.
        let margin_x = (22.0 * scale).round() as i32;
        let margin_top = (16.0 * scale).round() as i32;
        let margin_bottom = (28.0 * scale).round() as i32;
        let width = plan.card_w.round() as i32 + margin_x * 2;
        let height = plan.card_h.round() as i32 + margin_top + margin_bottom;

        SelectObject(measure, GetStockObject(DEFAULT_GUI_FONT) as HGDIOBJ);
        DeleteDC(measure);

        let header = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height, // top-down
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
        let mut bits: *mut core::ffi::c_void = null_mut();
        let bitmap = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if bitmap.is_null() || bits.is_null() {
            fonts.destroy();
            ReleaseDC(null_mut() as HWND, screen);
            return None;
        }
        let hdc = CreateCompatibleDC(screen);
        if hdc.is_null() {
            DeleteObject(bitmap as HGDIOBJ);
            fonts.destroy();
            ReleaseDC(null_mut() as HWND, screen);
            return None;
        }
        SelectObject(hdc, bitmap as HGDIOBJ);
        SetBkMode(hdc, TRANSPARENT as i32);

        let bits = bits as *mut u8;
        let mut canvas = Canvas::new(width, height, margin_x, margin_top, bits);
        let colors = palette(model.dark);
        let card = RectF {
            x: 0.0,
            y: 0.0,
            w: plan.card_w,
            h: plan.card_h,
        };

        // Shadow, then the panel itself. The mask keeps the card interior fully
        // opaque after GDI text has been painted over it.
        canvas.shadow(
            RectF {
                x: -2.0 * scale,
                y: 3.0 * scale,
                w: card.w + 4.0 * scale,
                h: card.h,
            },
            plan.radius,
            18.0 * scale,
            colors.shade,
            colors.shadow_alpha,
        );
        let mut mask = vec![0u8; (width * height) as usize];
        {
            let (w, h) = (plan.card_w.ceil() as i32, plan.card_h.ceil() as i32);
            for y in 0..h {
                for x in 0..w {
                    let coverage = rounded_coverage(x as f32, y as f32, card, plan.radius);
                    if coverage > 0.0 {
                        let i = ((y + margin_top) * width + x + margin_x) as usize;
                        mask[i] = (coverage * 255.0).round() as u8;
                        canvas.blend(x, y, colors.card.alpha(coverage));
                    }
                }
            }
        }
        canvas.stroke_round_rect(card, plan.radius, 1.0 * scale, colors.border);

        // Header pills.
        let draw_pill = |canvas: &mut Canvas, area: RECT, active: bool| {
            let pill = RectF {
                x: area.left as f32,
                y: area.top as f32,
                w: (area.right - area.left) as f32,
                h: (area.bottom - area.top) as f32,
            };
            let (bg, fg) = if active {
                (colors.toggle_active_bg, colors.toggle_active_fg)
            } else {
                (colors.toggle_bg, colors.toggle_fg)
            };
            canvas.fill_round_rect(pill, 6.0 * scale, bg);
            fg
        };
        let local_fg = draw_pill(&mut canvas, plan.local, model.zone == Zone::Local);
        let utc_fg = draw_pill(&mut canvas, plan.utc, model.zone == Zone::Utc);

        // Hero.
        let accent = if model.peak { colors.peak } else { colors.off };
        canvas.fill_circle(plan.dot.0, plan.dot.1, 7.5 * scale, accent.alpha(0.22));
        canvas.fill_circle(plan.dot.0, plan.dot.1, 4.5 * scale, accent);

        // Timeline: row background, bands, then the now marker.
        canvas.fill_round_rect(plan.bar, 4.5 * scale, colors.track_bg);
        for segment in &model.segments {
            let x0 = plan.bar.x + segment.start as f32 / 1440.0 * plan.bar.w;
            let x1 = plan.bar.x + segment.end as f32 / 1440.0 * plan.bar.w;
            let color = if segment.kind == Kind::Peak {
                colors.peak
            } else {
                colors.off
            };
            canvas.fill_bar_slice(plan.bar, 4.5 * scale, x0, x1, color.alpha(0.95));
        }
        let now_x = plan.bar.x + (model.now_minute.clamp(0.0, 1440.0) / 1440.0) * plan.bar.w;
        canvas.fill_bar_slice(
            plan.bar,
            4.5 * scale,
            now_x - 2.0 * scale,
            now_x + 2.0 * scale,
            colors.card,
        );
        canvas.fill_bar_slice(
            plan.bar,
            4.5 * scale,
            now_x - 1.0 * scale,
            now_x + 1.0 * scale,
            colors.title,
        );

        // Row highlights, behind the rows' content.
        if let Some(hover) = model.hover {
            let area = match hover {
                Action::ZoneLocal => Some(plan.local),
                Action::ZoneUtc => Some(plan.utc),
                Action::ToggleNotify | Action::ToggleAutostart => plan
                    .switch_rows
                    .iter()
                    .find(|(action, _, _)| *action == hover)
                    .map(|(_, row, _)| *row),
                Action::OpenPricing => Some(plan.link_row),
                Action::Quit => Some(plan.quit_row),
            };
            if let Some(area) = area {
                // Pills get a wash on their own shape; menu rows get a slightly
                // inset one so neighbouring rows do not touch.
                let pill = matches!(hover, Action::ZoneLocal | Action::ZoneUtc);
                let inset = if pill { 0.0 } else { 2.0 * scale };
                let radius = if pill { 6.0 * scale } else { 8.0 * scale };
                canvas.fill_round_rect(
                    RectF {
                        x: area.left as f32,
                        y: area.top as f32 + inset,
                        w: (area.right - area.left) as f32,
                        h: (area.bottom - area.top) as f32 - inset * 2.0,
                    },
                    radius,
                    colors.row_hover,
                );
            }
        }

        // Switches.
        for (action, _, track) in &plan.switch_rows {
            let on = match action {
                Action::ToggleNotify => model.notify,
                _ => model.autostart,
            };
            let track = RectF {
                x: track.left as f32,
                y: track.top as f32,
                w: (track.right - track.left) as f32,
                h: (track.bottom - track.top) as f32,
            };
            let fill = if on {
                colors.switch_on
            } else {
                colors.switch_off
            };
            canvas.fill_round_rect(track, track.h / 2.0, fill);
            let knob_r = track.h / 2.0 - 2.0 * scale;
            let knob_x = if on {
                track.right() - knob_r - 2.0 * scale
            } else {
                track.x + knob_r + 2.0 * scale
            };
            canvas.fill_circle(knob_x, track.y + track.h / 2.0, knob_r, colors.knob);
        }

        canvas.fill_rect(
            RectF {
                x: card.x,
                y: plan.separator.top as f32,
                w: plan.card_w,
                h: 1.0 * scale,
            },
            colors.border,
        );

        // Text last, so GDI composites glyphs against the finished background.
        // GDI takes bitmap coordinates, so re-apply the shadow margin here.
        let at = |area: RECT| RECT {
            left: area.left + margin_x,
            top: area.top + margin_top,
            right: area.right + margin_x,
            bottom: area.bottom + margin_top,
        };
        draw_text(
            hdc,
            fonts.title,
            colors.title,
            "DeepSeek Price Clock",
            at(plan.title),
            DT_LEFT,
        );
        draw_text(
            hdc,
            fonts.pill,
            local_fg,
            "Local",
            at(plan.local),
            DT_CENTER,
        );
        draw_text(hdc, fonts.pill, utc_fg, "UTC", at(plan.utc), DT_CENTER);
        draw_text(
            hdc,
            fonts.caption,
            colors.muted,
            &model.zone_caption,
            at(plan.zone),
            DT_LEFT,
        );
        draw_text(
            hdc,
            fonts.badge,
            colors.strong,
            &model.badge,
            at(plan.badge),
            DT_LEFT,
        );
        draw_text(
            hdc,
            fonts.big,
            colors.title,
            &model.countdown,
            at(plan.count),
            DT_LEFT,
        );
        draw_text(
            hdc,
            fonts.body,
            colors.muted,
            &model.until,
            at(plan.until),
            DT_LEFT,
        );

        for (i, label) in ["00", "06", "12", "18", "24"].iter().enumerate() {
            let fraction = i as f32 / 4.0;
            let center =
                plan.ticks.left as f32 + fraction * (plan.ticks.right - plan.ticks.left) as f32;
            // The end labels sit on the bar's ends, not centred on them, so
            // they are not drawn outside the card.
            let (x, align) = match i {
                0 => (plan.ticks.left as f32, DT_LEFT),
                4 => (plan.ticks.right as f32 - 60.0 * scale, DT_RIGHT),
                _ => (center - 30.0 * scale, DT_CENTER),
            };
            let area = rect(x, plan.ticks.top as f32, 60.0 * scale, 14.0 * scale);
            draw_text(hdc, fonts.tiny, colors.faint, label, at(area), align);
        }

        draw_text(
            hdc,
            fonts.caption,
            colors.muted,
            &model.windows_caption,
            at(plan.windows_caption),
            DT_LEFT,
        );
        for (i, window) in model.windows.iter().enumerate() {
            if i >= plan.window_rows.len() {
                break;
            }
            let (kind, range, meta) = plan.window_rows[i];
            let kind_color = if window.kind == Kind::Peak {
                colors.peak
            } else {
                colors.off
            };
            let font = if window.current {
                fonts.strong
            } else {
                fonts.body
            };
            let meta_font = if window.current {
                fonts.strong
            } else {
                fonts.caption
            };
            let label = if window.kind == Kind::Peak {
                "Peak"
            } else {
                "Off-peak"
            };
            draw_text(hdc, font, kind_color, label, at(kind), DT_LEFT);
            draw_text(hdc, font, colors.strong, &window.range, at(range), DT_LEFT);
            draw_text(
                hdc,
                meta_font,
                colors.faint,
                &window.meta,
                at(meta),
                DT_RIGHT,
            );
        }

        draw_text(
            hdc,
            fonts.caption,
            colors.muted,
            &model.rates_caption,
            at(plan.rates_caption),
            DT_LEFT,
        );
        draw_text(
            hdc,
            fonts.caption,
            colors.muted,
            &model.rates_header,
            at(plan.rates_header),
            DT_RIGHT,
        );
        for (i, rate) in model.rates.iter().enumerate() {
            if i >= plan.rate_rows.len() {
                break;
            }
            let (name, amounts) = &plan.rate_rows[i];
            draw_text(
                hdc,
                fonts.body,
                colors.strong,
                &rate.name,
                at(*name),
                DT_LEFT,
            );
            for (slot, amount) in amounts.iter().zip(rate.amounts.iter()) {
                draw_text(hdc, fonts.body, colors.strong, amount, at(*slot), DT_RIGHT);
            }
        }
        draw_wrapped(
            hdc,
            fonts.tiny,
            colors.faint,
            &model.next_rates,
            at(plan.next_rates),
        );

        for (action, row, _) in &plan.switch_rows {
            let label = match action {
                Action::ToggleNotify => "Notify me when prices switch",
                _ => "Start with Windows",
            };
            let area = rect(
                plan.link.left as f32,
                row.top as f32,
                (plan.link.right - plan.link.left) as f32 - 52.0 * scale,
                (row.bottom - row.top) as f32,
            );
            draw_text(hdc, fonts.body, colors.strong, label, at(area), DT_LEFT);
        }
        draw_text(
            hdc,
            fonts.body,
            colors.strong,
            "Open pricing page \u{2192}",
            at(plan.link),
            DT_LEFT,
        );
        draw_text(
            hdc,
            fonts.body,
            colors.strong,
            "Quit",
            at(plan.quit),
            DT_LEFT,
        );

        canvas.seal(&mask);

        // Deselect the fonts (the bitmap stays selected) before deleting them.
        SelectObject(hdc, bitmap as HGDIOBJ);
        fonts.destroy();
        ReleaseDC(null_mut() as HWND, screen);

        let offset_hits = plan
            .hits
            .iter()
            .map(|(action, area)| {
                (
                    *action,
                    RECT {
                        left: area.left + margin_x,
                        top: area.top + margin_top,
                        right: area.right + margin_x,
                        bottom: area.bottom + margin_top,
                    },
                )
            })
            .collect();

        Some(Card {
            hdc,
            bitmap,
            bits,
            width,
            height,
            card_w: plan.card_w.round() as i32,
            card_h: plan.card_h.round() as i32,
            offset_x: margin_x,
            offset_y: margin_top,
            hits: offset_hits,
        })
    }
}

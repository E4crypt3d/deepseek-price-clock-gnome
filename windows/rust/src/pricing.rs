//! DeepSeek peak/off-peak pricing rules.
//!
//! Ported from deepakness's extension
//! <https://github.com/deepakness/deepseek-price-clock> (price.js, MIT (c) 2026
//! deepakness). The rules are his work; this is a straight port.
//!
//! Everything here is plain integer math on Unix seconds, so it does not depend
//! on the machine's time zone or on daylight saving.

pub const SWITCH_MINUTES: [i64; 4] = [60, 240, 360, 600];

/// Chinese public holidays, as Beijing dates. DeepSeek bills them off-peak all
/// day. Add the next year's dates when the State Council publishes them.
pub const HOLIDAYS: &[(&str, &str)] = &[
    ("2026-09-25", "2026-09-27"), // Mid-Autumn
    ("2026-10-01", "2026-10-07"), // National Day
    ("2027-01-01", "2027-01-03"), // New Year
    ("2027-02-05", "2027-02-13"), // Spring Festival
    ("2027-04-03", "2027-04-05"), // Qingming
    ("2027-05-01", "2027-05-05"), // Labour Day
    ("2027-06-09", "2027-06-09"), // Dragon Boat
    ("2027-09-15", "2027-09-15"), // Mid-Autumn
    ("2027-10-01", "2027-10-07"), // National Day
];

/// Days since 1970-01-01 for a civil date, Howard Hinnant's algorithm.
pub const fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// The split only applies Monday to Friday Beijing time from this instant on:
/// 2026-08-22 16:00 UTC, which is 2026-08-23 00:00 Beijing.
pub const WEEKEND_RULE_START: i64 = days_from_civil(2026, 8, 22) * 86400 + 16 * 3600;

pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Off,
    Peak,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Off => "Off-peak",
            Kind::Peak => "Peak",
        }
    }

    pub fn is_peak(self) -> bool {
        self == Kind::Peak
    }
}

/// Beijing calendar date for an instant, as YYYY-MM-DD.
pub fn beijing_date(secs: i64) -> String {
    let (y, m, d) = civil_from_days((secs + 8 * 3600).div_euclid(86400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// 0 = Sunday, 6 = Saturday, in Beijing time.
pub fn beijing_weekday(secs: i64) -> i64 {
    // Day 0 of the Unix epoch was a Thursday, so shift by 4 before taking the
    // week index; otherwise Thursdays read as Sundays and weekends read wrong.
    ((secs + 8 * 3600).div_euclid(86400) + 4).rem_euclid(7)
}

pub fn is_weekend(secs: i64) -> bool {
    let day = beijing_weekday(secs);
    day == 0 || day == 6
}

pub fn is_holiday(secs: i64) -> bool {
    let key = beijing_date(secs);
    HOLIDAYS
        .iter()
        .any(|(from, to)| key.as_str() >= *from && key.as_str() <= *to)
}

/// Saturdays, Sundays and public holidays are off-peak all day under the new
/// rules.
pub fn off_peak_all_day(secs: i64) -> bool {
    secs >= WEEKEND_RULE_START && (is_weekend(secs) || is_holiday(secs))
}

pub fn minute_of_day(secs: i64) -> i64 {
    secs.rem_euclid(86400) / 60
}

pub fn kind_at(secs: i64) -> Kind {
    if off_peak_all_day(secs) {
        return Kind::Off;
    }
    let minute = minute_of_day(secs);
    let peak = (60..240).contains(&minute) || (360..600).contains(&minute);
    if peak {
        Kind::Peak
    } else {
        Kind::Off
    }
}

/// A switch minute is a real price change only outside gated weekends and
/// holidays.
pub fn is_switch_instant(secs: i64) -> bool {
    SWITCH_MINUTES.contains(&minute_of_day(secs)) && !off_peak_all_day(secs)
}

#[derive(Clone, Copy, Debug)]
pub struct Switch {
    pub time: i64,
    pub kind: Kind,
}

/// The next `count` price changes.
pub fn next_switches(now: i64, count: usize) -> Vec<Switch> {
    let day_start = now.div_euclid(86400) * 86400;
    let mut out = Vec::with_capacity(count);
    let mut day = 0i64;
    let mut index = 0usize;
    while out.len() < count {
        let instant = day_start + day * 86400 + SWITCH_MINUTES[index] * 60;
        if instant > now && is_switch_instant(instant) {
            out.push(Switch {
                time: instant,
                kind: kind_at(instant),
            });
        }
        index += 1;
        if index == SWITCH_MINUTES.len() {
            index = 0;
            day += 1;
        }
    }
    out
}

pub fn next_switch(now: i64) -> Switch {
    next_switches(now, 1)[0]
}

/// The instant the current window began, looking back far enough to cross a
/// weekend plus holiday stretch.
pub fn current_window_start(now: i64) -> i64 {
    let day_start = now.div_euclid(86400) * 86400;
    let mut best: Option<i64> = None;
    for day in -21..=0i64 {
        for minute in SWITCH_MINUTES {
            let instant = day_start + day * 86400 + minute * 60;
            if instant <= now && is_switch_instant(instant) && best.is_none_or(|b| instant > b) {
                best = Some(instant);
            }
        }
    }
    best.unwrap_or(now - 86400)
}

/// Compact countdown for the tray tooltip and notifications: 1h 23m, 4m, 42s.
pub fn format_countdown(secs: i64) -> String {
    let secs = secs.max(0);
    if secs >= 3600 {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// h:mm:ss, matching the GNOME extension's countdown (`clock()` in
/// extension.js). Hours are not padded, so the number stays steady as it
/// counts down.
pub fn format_clock(secs: i64) -> String {
    let secs = secs.max(0);
    format!("{}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
}

/// Time left, in the wording the GNOME menu uses: 2h 25m, 45m.
pub fn format_away(secs: i64) -> String {
    let minutes = secs.max(0) / 60;
    if minutes >= 60 {
        format!("{}h {:02}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

/// Length of a price window: 15h, 2h 30m, 45m.
pub fn format_duration(secs: i64) -> String {
    let minutes = (secs.max(0) + 30) / 60;
    let (hours, rest) = (minutes / 60, minutes % 60);
    if hours == 0 {
        format!("{rest}m")
    } else if rest == 0 {
        format!("{hours}h")
    } else {
        format!("{hours}h {rest:02}m")
    }
}

/// One painted slice of the 24-hour timeline, in minutes of the day being
/// shown (0..1440).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub start: i64,
    pub end: i64,
    pub kind: Kind,
}

/// Peak/off-peak bands across the displayed day, mirroring `displaySegments`
/// in the GNOME extension's price.js. `offset_minutes` is the zone the bar is
/// drawn in (0 for UTC), so the same code serves both the Local and UTC views.
pub fn display_segments(now: i64, offset_minutes: i64) -> Vec<Segment> {
    let shifted = now + offset_minutes * 60;
    let day_start = shifted.div_euclid(86400) * 86400 - offset_minutes * 60;

    let mut boundaries = vec![0i64, 1440];
    for minute in SWITCH_MINUTES {
        boundaries.push((minute + offset_minutes).rem_euclid(1440));
    }
    // 2026-08-22 16:00 UTC, where the weekend rule begins, as a clock time.
    boundaries.push((960 + offset_minutes).rem_euclid(1440));
    boundaries.sort_unstable();
    boundaries.dedup();

    let mut out: Vec<Segment> = Vec::new();
    for pair in boundaries.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if end <= start {
            continue;
        }
        // Sample the middle of the slice so a boundary minute never decides
        // the colour of the slice on its own.
        let kind = kind_at(day_start + start * 60 + 30);
        match out.last_mut() {
            Some(last) if last.kind == kind => last.end = end,
            _ => out.push(Segment { start, end, kind }),
        }
    }
    out
}

/// A price window as the card lists it: when it runs, and whether it is the
/// one in effect now.
#[derive(Clone, Copy, Debug)]
pub struct PriceWindow {
    pub start: i64,
    pub end: i64,
    pub kind: Kind,
    pub current: bool,
}

/// The current window followed by the next `count - 1`, as start/end pairs.
pub fn window_list(now: i64, count: usize) -> Vec<PriceWindow> {
    let upcoming = next_switches(now, count);
    let mut start = current_window_start(now);
    let mut kind = kind_at(now);
    let mut out = Vec::with_capacity(count);
    for (i, switch) in upcoming.iter().enumerate() {
        out.push(PriceWindow {
            start,
            end: switch.time,
            kind,
            current: i == 0,
        });
        start = switch.time;
        kind = switch.kind;
    }
    out
}

/// Mon=0 .. Sun=6 for a UTC instant, used for the short day names in the
/// window rows.
pub fn weekday(secs: i64) -> usize {
    (secs.div_euclid(86400) + 3).rem_euclid(7) as usize
}

pub const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

#[cfg(test)]
mod tests {
    use super::*;

    fn at(iso: &str) -> i64 {
        // yyyy-mm-ddThh:mm:ssZ only, enough for the tests.
        let year: i64 = iso[0..4].parse().unwrap();
        let month: i64 = iso[5..7].parse().unwrap();
        let day: i64 = iso[8..10].parse().unwrap();
        let hour: i64 = iso[11..13].parse().unwrap();
        let minute: i64 = iso[14..16].parse().unwrap();
        days_from_civil(year, month, day) * 86400 + hour * 3600 + minute * 60
    }

    #[test]
    fn peak_windows() {
        assert_eq!(kind_at(at("2026-11-24T02:00:00Z")), Kind::Peak);
        assert_eq!(kind_at(at("2026-11-24T03:30:00Z")), Kind::Peak);
        assert_eq!(kind_at(at("2026-11-24T00:59:00Z")), Kind::Off);
        assert_eq!(kind_at(at("2026-11-24T06:00:00Z")), Kind::Peak);
        assert_eq!(kind_at(at("2026-11-24T09:59:00Z")), Kind::Peak);
        assert_eq!(kind_at(at("2026-11-24T10:00:00Z")), Kind::Off);
    }

    #[test]
    fn weekend_rule() {
        assert_eq!(kind_at(at("2026-08-16T02:00:00Z")), Kind::Peak);
        assert_eq!(kind_at(at("2026-08-22T15:59:00Z")), Kind::Off);
        assert_eq!(kind_at(at("2026-08-22T16:00:00Z")), Kind::Off);
        assert_eq!(kind_at(at("2026-08-23T02:00:00Z")), Kind::Off);
        assert_eq!(kind_at(at("2026-11-21T05:00:00Z")), Kind::Off);
    }

    #[test]
    fn holidays() {
        assert_eq!(kind_at(at("2026-10-02T02:00:00Z")), Kind::Off);
        assert_eq!(kind_at(at("2026-10-07T23:59:00Z")), Kind::Off);
        assert_eq!(kind_at(at("2026-10-08T01:30:00Z")), Kind::Peak);
        assert_eq!(kind_at(at("2027-02-08T06:00:00Z")), Kind::Off);
        assert_eq!(kind_at(at("2027-06-09T02:00:00Z")), Kind::Off);
    }

    #[test]
    fn switch_sequence() {
        let next = next_switch(at("2026-11-24T02:00:00Z"));
        assert_eq!(next.time, at("2026-11-24T04:00:00Z"));
        assert_eq!(next.kind, Kind::Off);

        let next = next_switch(at("2026-11-26T09:59:00Z"));
        assert_eq!(next.time, at("2026-11-26T10:00:00Z"));

        // Weekend switches are skipped entirely.
        let next = next_switch(at("2026-11-27T10:30:00Z"));
        assert_eq!(next.time, at("2026-11-30T01:00:00Z"));
        assert_eq!(next.kind, Kind::Peak);

        // A holiday stretch is skipped the same way.
        let next = next_switch(at("2026-09-30T11:00:00Z"));
        assert_eq!(next.time, at("2026-10-08T01:00:00Z"));
    }

    #[test]
    fn window_start() {
        assert_eq!(
            current_window_start(at("2026-11-24T03:30:00Z")),
            at("2026-11-24T01:00:00Z")
        );
        assert_eq!(
            current_window_start(at("2026-11-24T00:30:00Z")),
            at("2026-11-23T10:00:00Z")
        );
        assert_eq!(
            current_window_start(at("2026-11-21T05:00:00Z")),
            at("2026-11-20T10:00:00Z")
        );
        assert_eq!(
            current_window_start(at("2026-11-29T05:00:00Z")),
            at("2026-11-27T10:00:00Z")
        );
    }

    #[test]
    fn countdowns() {
        assert_eq!(format_countdown(83 * 60), "1h 23m");
        assert_eq!(format_countdown(270), "4m");
        assert_eq!(format_countdown(42), "42s");
        assert_eq!(format_countdown(-5), "0s");
    }

    #[test]
    fn countdown_formats() {
        assert_eq!(format_clock(2 * 3600 + 25 * 60 + 17), "2:25:17");
        assert_eq!(format_clock(59), "0:00:59");
        assert_eq!(format_clock(-4), "0:00:00");
        assert_eq!(format_away(2 * 3600 + 25 * 60), "2h 25m");
        assert_eq!(format_away(45 * 60), "45m");
        assert_eq!(format_duration(15 * 3600), "15h");
        assert_eq!(format_duration(2 * 3600 + 30 * 60), "2h 30m");
        assert_eq!(format_duration(45 * 60), "45m");
        assert_eq!(WEEKDAYS[weekday(at("2026-11-26T12:00:00Z"))], "Thu");
    }

    #[test]
    fn timeline_covers_the_day() {
        let now = at("2026-11-24T02:25:00Z");
        for offset in [0, 60, 330, -300, 345, -720, 840] {
            let segments = display_segments(now, offset);
            assert_eq!(segments.first().unwrap().start, 0);
            assert_eq!(segments.last().unwrap().end, 1440);
            for pair in segments.windows(2) {
                assert_eq!(pair[0].end, pair[1].start, "no gaps at offset {offset}");
                assert_ne!(pair[0].kind, pair[1].kind, "bands must be merged");
            }

            // Every minute of the bar must agree with the rules. The day is the
            // local day at that offset, which starts `offset` minutes off UTC.
            let shifted = now + offset * 60;
            let day_start = shifted.div_euclid(86400) * 86400 - offset * 60;
            for segment in &segments {
                let mid = day_start + (segment.start + segment.end) / 2 * 60;
                assert_eq!(kind_at(mid), segment.kind);
            }
        }
    }

    #[test]
    fn window_list_is_contiguous() {
        let now = at("2026-11-24T02:25:00Z");
        let windows = window_list(now, 4);
        assert_eq!(windows.len(), 4);
        assert!(windows[0].current);
        assert_eq!(windows[0].start, current_window_start(now));
        assert_eq!(windows[0].end, next_switch(now).time);
        assert_eq!(windows[0].kind, kind_at(now));
        for pair in windows.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
            assert_ne!(pair[0].kind, pair[1].kind);
            assert!(!pair[1].current);
        }
        assert_eq!(windows[3].end, next_switches(now, 4)[3].time);
    }

    #[test]
    fn every_hour_is_sane() {
        let mut now = at("2026-01-01T00:00:00Z");
        let end = at("2028-01-01T00:00:00Z");
        let mut samples = 0;
        while now < end {
            let kind = kind_at(now);
            let upcoming = next_switches(now, 4);
            let first = upcoming[0];
            assert!(first.time > now, "next switch must be in the future");
            assert_ne!(first.kind, kind, "a switch must flip the pricing");
            assert!(!off_peak_all_day(first.time), "no switch on a gated day");
            assert!(SWITCH_MINUTES.contains(&minute_of_day(first.time)));
            for pair in upcoming.windows(2) {
                assert!(pair[0].time < pair[1].time);
                assert_ne!(pair[0].kind, pair[1].kind);
            }
            samples += 1;
            now += 3600;
        }
        assert_eq!(samples, 17520);
    }
}

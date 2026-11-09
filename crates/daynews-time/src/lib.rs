// SPDX-License-Identifier: Apache-2.0
//! The wall clock and the reader's UTC offset, asked of the host.
//!
//! Fleet Reader needs exactly two facts about time-of-day, and neither is in the standard library
//! everywhere this app runs:
//!
//! - **Now.** `SystemTime::now()` traps on `wasm32-unknown-unknown` rather than failing, so the
//!   web build asks the page for `Date.now()`.
//! - **The local UTC offset at an instant, DST included.** "Today" is a claim about the reader's
//!   calendar, so the timeline's cut-off is local midnight rather than UTC midnight, and turning
//!   an instant into a local one needs the reader's zone rules.
//!
//! Those rules come from the host, never from a database shipped in the binary: POSIX
//! `localtime_r` reads the platform's own zoneinfo, Win32 converts through the zone the machine
//! is set to, and on web the browser's `Intl` implementation answers. Every one of them is
//! already installed and already kept current, where a bundled IANA database is ~200 KB that goes
//! stale between releases.
//!
//! An unanswerable offset is `None` rather than a guess; callers read that as UTC, which is the
//! same thing a machine with no zone configured would say.

/// Milliseconds since the Unix epoch.
pub fn now_epoch_ms() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        day_toolkit_dom::now_epoch_ms()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// The wall clock as seconds since the Unix epoch.
pub fn now_unix() -> i64 {
    (now_epoch_ms() / 1000) as i64
}

/// The reader's offset from UTC at `at_unix`, in seconds east of UTC (New York in January is
/// `-18_000`), with the zone's daylight-saving rules applied at that instant: the offset in
/// force then, not the one in force now.
///
/// `None` when the host cannot answer: a machine with no zone configured, or a web page served by
/// a shim older than the offset key. Treat it as UTC.
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub fn local_offset_seconds(at_unix: i64) -> Option<i32> {
    // `localtime_r` resolves the instant against the platform's time-zone database (the same
    // one `date` and every other program on the machine reads) and reports the offset it used in
    // `tm_gmtoff`. macOS, Linux, iOS, Android and OpenHarmony all carry that field.
    // Infer the ABI type from localtime_r; libc deprecates the time_t alias on OHOS.
    let t = at_unix as _;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `t` is a valid `time_t` and `tm` is a valid, writable `struct tm` for the duration
    // of the call. `localtime_r` fills it and answers null on failure, which is checked before
    // any field is read.
    let filled = unsafe { libc::localtime_r(&t, &mut tm) };
    if filled.is_null() {
        return None;
    }
    i32::try_from(tm.tm_gmtoff).ok()
}

/// The Windows shape of the same question.
///
/// Win32 has no `tm_gmtoff`, so the offset is measured rather than read: hand the instant to the
/// OS, let it convert with the zone's own rules (`SystemTimeToTzSpecificLocalTime` with a null
/// zone means "the one this machine is set to", daylight saving included), and take the
/// difference between what went in and what came out.
///
/// Three declarations rather than a `windows-sys` dependency: they are stable kernel32 entry
/// points that std already links, and this is the whole of what the app needs from Win32.
#[cfg(all(windows, not(target_arch = "wasm32")))]
pub fn local_offset_seconds(at_unix: i64) -> Option<i32> {
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    unsafe extern "system" {
        fn FileTimeToSystemTime(ft: *const FileTime, st: *mut SystemTime) -> i32;
        fn SystemTimeToTzSpecificLocalTime(
            zone: *const core::ffi::c_void,
            utc: *const SystemTime,
            local: *mut SystemTime,
        ) -> i32;
        fn SystemTimeToFileTime(st: *const SystemTime, ft: *mut FileTime) -> i32;
    }

    // FILETIME counts 100-nanosecond ticks from 1601-01-01; 11 644 473 600 seconds separate that
    // epoch from 1970's. A pre-1601 instant has no FILETIME, which is the `None` below.
    const TICKS_PER_SEC: i128 = 10_000_000;
    const EPOCH_DELTA_SECS: i128 = 11_644_473_600;
    let ticks = u64::try_from((i128::from(at_unix) + EPOCH_DELTA_SECS) * TICKS_PER_SEC).ok()?;
    let utc_ft = FileTime {
        low: ticks as u32,
        high: (ticks >> 32) as u32,
    };
    let mut utc = SystemTime::default();
    let mut local = SystemTime::default();
    let mut local_ft = FileTime::default();
    // SAFETY: every pointer is to a live, correctly shaped local, and each call's result is
    // checked before the value it wrote is used.
    unsafe {
        if FileTimeToSystemTime(&utc_ft, &mut utc) == 0
            || SystemTimeToTzSpecificLocalTime(core::ptr::null(), &utc, &mut local) == 0
            || SystemTimeToFileTime(&local, &mut local_ft) == 0
        {
            return None;
        }
    }
    let local_ticks = u64::from(local_ft.low) | (u64::from(local_ft.high) << 32);
    let delta = (local_ticks as i128 - ticks as i128) / TICKS_PER_SEC;
    i32::try_from(delta).ok()
}

/// The web shape of the same question.
///
/// The browser is the tz database on this target. `host_env` is day-toolkit-dom's channel for page facts
/// (the reserved `tz` key beside it answers the IANA zone id), and the offset key takes the
/// instant because daylight saving makes the answer a function of it. An older shim does not know
/// the key and answers nothing, which reads as UTC rather than as a wrong hour.
#[cfg(target_arch = "wasm32")]
pub fn local_offset_seconds(at_unix: i64) -> Option<i32> {
    let ms = at_unix.checked_mul(1000)?;
    let minutes: i32 = day_toolkit_dom::host_env(&format!("tzoffset:{ms}"))?
        .parse()
        .ok()?;
    minutes.checked_mul(60)
}

/// The instant local midnight last happened, as unix seconds.
///
/// The offset is taken twice: once for now, to land on the right calendar day, and
/// once for the midnight that day arithmetic produced. On the two days a year a zone changes its
/// offset, those differ, and the boundary the reader means is the one that was in force at
/// midnight, not the one in force at breakfast.
pub fn start_of_day(now_unix: i64) -> i64 {
    let off = local_offset_seconds(now_unix).map_or(0, i64::from);
    let midnight = |off: i64| (now_unix + off).div_euclid(86_400) * 86_400 - off;
    let first = midnight(off);
    match local_offset_seconds(first).map(i64::from) {
        Some(then) if then != off => midnight(then),
        _ => first,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_a_plausible_wall_clock() {
        // Later than 2020-01-01 and earlier than 2100: enough to catch a zero clock, a
        // milliseconds/seconds mix-up, and an epoch that is not the Unix one.
        let now = now_unix();
        assert!(
            (1_577_836_800..4_102_444_800).contains(&now),
            "now_unix = {now}"
        );
        assert_eq!(now_epoch_ms() / 1000, now as u64, "the two clocks disagree");
    }

    #[test]
    fn offset_is_a_whole_number_of_minutes_within_a_day() {
        // Every zone the IANA database has ever carried is within ±26 hours of UTC and lands on a
        // whole minute; this is the shape check that catches a units bug (minutes read as
        // seconds, or a FILETIME delta left in ticks).
        let off = local_offset_seconds(now_unix()).expect("the host knows its own zone");
        assert!(off.abs() <= 26 * 3600, "offset {off}s is not a real zone");
        assert_eq!(off % 60, 0, "offset {off}s is not a whole minute");
    }

    #[test]
    fn start_of_day_is_local_midnight_at_or_before_now() {
        let now = now_unix();
        let start = start_of_day(now);
        assert!(start <= now, "start {start} is after now {now}");
        assert!(
            now - start < 25 * 3600,
            "start {start} is more than a day before now {now}"
        );
        // Local midnight is an exact multiple of a day once the offset in force there is added
        // back, the property the DST re-derivation exists to keep true.
        let off = i64::from(local_offset_seconds(start).unwrap_or(0));
        assert_eq!(
            (start + off) % 86_400,
            0,
            "start {start} is not on a local day boundary"
        );
    }

    #[test]
    fn start_of_day_holds_across_a_dst_transition() {
        // 2026-03-08 07:00Z is one hour after the US spring-forward. Whatever zone the machine
        // running this is in, the answer has to be a day boundary in that zone and no more than
        // 25 hours back, the case that broke when the offset was read only once.
        let after_spring_forward = 1_772_953_200;
        let start = start_of_day(after_spring_forward);
        assert!(start <= after_spring_forward);
        assert!(after_spring_forward - start < 25 * 3600);
        let off = i64::from(local_offset_seconds(start).unwrap_or(0));
        assert_eq!((start + off) % 86_400, 0);
    }
}

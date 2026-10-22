//! Wall-clock time for the library: reading it, and turning a stored stamp into
//! something the dashboard can print.
//!
//! The library stores seconds since the Unix epoch and nothing else (see
//! `ondin_core::meta::DocumentMeta::created`); everything that turns one back
//! into text lives here.
//!
//! The civil date is local time on Windows, Linux and macOS ([`local_offset`]),
//! which needs the zone's history, not today's offset; each platform's own call
//! answers for a given instant without a dependency (§15 D959). Relative labels
//! ("2h ago", "Yesterday") are differences between instants and have no timezone.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch, or `0` if the system clock is before it.
///
/// Saturating rather than erroring: a clock before 1970 is a broken machine, not
/// a reason to refuse to file a document.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Seconds between `then` and now, or `0` if `then` is in the future.
///
/// The future is not an error: a file synced from a machine whose clock runs
/// fast is honestly "just now".
pub fn since(then: u64) -> u64 {
    now().saturating_sub(then)
}

/// Split a Unix timestamp into a UTC calendar date, `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days`. The shift by 719468 moves the epoch to a
/// calendar starting on 1 March 0000, where the leap day is last in the year.
pub fn civil_from_unix(secs: u64) -> (i64, u32, u32) {
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The inverse of [`civil_from_unix`]: a calendar date back to a Unix second.
///
/// Hinnant's `days_from_civil`, the same calendar read the other way. Windows
/// answers a zone question in calendar fields, so [`local_offset`] needs it to
/// get back to seconds. Midnight of that date; the caller adds the time of day.
///
/// Windows and tests only: the Unix arm never needs it, and CI's Linux and macOS
/// clippy legs deny the dead code (§15 D959).
#[cfg(any(windows, test))]
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The local zone's offset from UTC, in seconds east, for that instant (§15 D384).
///
/// "For that instant" is the point: offsets differ between July and January, and
/// DST rules themselves change by year, so adding today's offset is an hour out
/// on half the library.
///
/// Windows: `GetDynamicTimeZoneInformation` returns the zone with its registry
/// key name, and `SystemTimeToTzSpecificLocalTimeEx` converts through that key,
/// where the year-by-year rules are kept. Linux and macOS (§15 D959):
/// `localtime_r` converts through the system tz database and `tm_gmtoff` is the
/// offset in force at that instant; the zone is `TZ`, else `/etc/localtime`.
/// Zero (UTC) on any other host.
pub fn local_offset(secs: u64) -> i64 {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::Time::{
            DYNAMIC_TIME_ZONE_INFORMATION, GetDynamicTimeZoneInformation,
            SystemTimeToTzSpecificLocalTimeEx,
        };

        let (y, m, d) = civil_from_unix(secs);
        // `SYSTEMTIME` holds the year in a `u16`; a stamp past 65535 is a broken
        // clock and gets UTC rather than a wrapped year.
        let Ok(year) = u16::try_from(y) else {
            return 0;
        };
        let rest = secs % 86_400;
        let utc = SYSTEMTIME {
            wYear: year,
            wMonth: m as u16,
            // Ignored on input by the conversion, which reads the date fields.
            wDayOfWeek: 0,
            wDay: d as u16,
            wHour: (rest / 3_600) as u16,
            wMinute: (rest % 3_600 / 60) as u16,
            wSecond: (rest % 60) as u16,
            wMilliseconds: 0,
        };
        // SAFETY: both calls write into stack-allocated, fully-initialized
        // structures and read the one we hand them; neither retains a pointer.
        // Failure of either leaves `local` untouched, hence the `BOOL` gate.
        unsafe {
            let mut zone: DYNAMIC_TIME_ZONE_INFORMATION = std::mem::zeroed();
            // Only `TIME_ZONE_ID_INVALID` (`u32::MAX`) is a failure; the rest,
            // including `TIME_ZONE_ID_UNKNOWN` (no daylight saving), succeed.
            if GetDynamicTimeZoneInformation(&mut zone) == u32::MAX {
                return 0;
            }
            let mut local: SYSTEMTIME = std::mem::zeroed();
            if SystemTimeToTzSpecificLocalTimeEx(&zone, &utc, &mut local) == 0 {
                return 0;
            }
            let days = days_from_civil(local.wYear as i64, local.wMonth as u32, local.wDay as u32);
            let local_secs = days * 86_400
                + local.wHour as i64 * 3_600
                + local.wMinute as i64 * 60
                + local.wSecond as i64;
            local_secs - secs as i64
        }
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        // A stamp past `time_t`'s range is the same broken clock; same answer.
        let Ok(t) = libc::time_t::try_from(secs) else {
            return 0;
        };
        // SAFETY: `localtime_r` reads the one `time_t` we hand it and writes
        // the one `tm` we own, both on the stack; it keeps neither pointer.
        // Null is its failure. It initialises the zone from `TZ` itself on
        // glibc, musl and Apple's libc, so no `tzset` is called first.
        unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&t, &mut tm).is_null() {
                return 0;
            }
            tm.tm_gmtoff as i64
        }
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = secs;
        0
    }
}

/// The three-letter month abbreviations the dashboard's date column uses.
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A stored stamp as `"12 Mar 2026"` — the *Created* column's format, in local
/// time ([`local_offset`]).
pub fn date_label(secs: u64) -> String {
    // Clamped at the epoch: a shifted stamp west of Greenwich would go negative,
    // out of `civil_from_unix`'s range. The epoch is a broken clock anyway, and
    // the clamp keeps that label timezone-independent for the test.
    let local = (secs as i64 + local_offset(secs)).max(0) as u64;
    let (y, m, d) = civil_from_unix(local);
    // `m` is in 1..=12; clamped anyway, as a wrong index panics in a list row.
    let month = MONTHS[(m.clamp(1, 12) - 1) as usize];
    format!("{d} {month} {y}")
}

/// How long ago, in the dashboard's voice: `"2h ago"`, `"Yesterday"`,
/// `"Last week"`.
///
/// Coarser than the save dot's tooltip (`app::save_label`) on purpose: that
/// answers "did my last edit make it to disk", where a minute matters; this
/// answers "which file was I working on", where forty rows of "47m ago" are noise.
pub fn relative_label(secs_ago: u64) -> String {
    match secs_ago {
        0..=59 => "Just now".into(),
        60..=3_599 => format!("{}m ago", secs_ago / 60),
        // Up to 18 hours reads as hours; past that "Yesterday" is what a person says.
        3_600..=64_799 => format!("{}h ago", secs_ago / 3_600),
        64_800..=172_799 => "Yesterday".into(),
        172_800..=604_799 => format!("{}d ago", secs_ago / 86_400),
        604_800..=1_209_599 => "Last week".into(),
        1_209_600..=2_591_999 => format!("{}w ago", secs_ago / 604_800),
        2_592_000..=5_183_999 => "Last month".into(),
        5_184_000..=31_535_999 => format!("{}mo ago", secs_ago / 2_592_000),
        _ => format!("{}y ago", secs_ago / 31_536_000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Known instants, including a leap day and 1 January.
    #[test]
    fn the_civil_date_matches_known_instants() {
        assert_eq!(civil_from_unix(0), (1970, 1, 1));
        // 2026-03-26T00:00:00Z.
        assert_eq!(civil_from_unix(1_774_483_200), (2026, 3, 26));
        // 2024-02-29, a leap day.
        assert_eq!(civil_from_unix(1_709_164_800), (2024, 2, 29));
        // 2000-03-01: the 400-year rule, which a naive /4 //100 gets wrong.
        assert_eq!(civil_from_unix(951_868_800), (2000, 3, 1));
    }

    /// The seconds within a day must not shift the date.
    ///
    /// flip: `(secs + 86_399) / 86_400` in `civil_from_unix` → leaves the
    /// midnight cases above green and fails here.
    #[test]
    fn any_time_within_a_day_is_the_same_date() {
        let midnight = 1_774_483_200;
        assert_eq!(civil_from_unix(midnight), (2026, 3, 26));
        assert_eq!(civil_from_unix(midnight + 1), (2026, 3, 26));
        assert_eq!(civil_from_unix(midnight + 86_399), (2026, 3, 26));
        assert_eq!(civil_from_unix(midnight + 86_400), (2026, 3, 27));
    }

    /// Both stamps are ones the local offset cannot move: noon is safe for every
    /// offset in `(-12h, +12h)`, and the epoch is clamped.
    #[test]
    fn the_date_label_reads_the_way_the_column_shows_it() {
        // 2026-03-26T12:00:00Z.
        assert_eq!(date_label(1_774_483_200 + 43_200), "26 Mar 2026");
        assert_eq!(date_label(0), "1 Jan 1970");
    }

    /// The *Created* column names the local day. The probes sit either side of
    /// midnight so one fires in any non-UTC zone; the test reads the offset and
    /// asserts what it implies, and checks nothing at UTC (CI's runners), which
    /// is why the Unix arm has a test that picks its zone.
    ///
    /// flip: `date_label` ignoring the offset, on a UTC+1 machine → fails at
    /// "23:30 UTC is already tomorrow"; the other arm stays green.
    #[test]
    fn the_date_label_crosses_midnight_with_the_local_zone() {
        // 2026-03-26T23:30:00Z and 2026-03-26T00:30:00Z.
        let late = 1_774_483_200 + 84_600;
        let early = 1_774_483_200 + 1_800;
        let off = local_offset(late);
        assert!(
            (-12 * 3_600..=14 * 3_600).contains(&off),
            "no zone on earth is at {off} seconds"
        );
        assert_eq!(off % 60, 0, "and none of them is a fraction of a minute");

        if off >= 1_800 {
            assert_eq!(
                date_label(late),
                "27 Mar 2026",
                "23:30 UTC is already tomorrow"
            );
        } else if local_offset(early) <= -1_800 {
            assert_eq!(
                date_label(early),
                "25 Mar 2026",
                "00:30 UTC is still yesterday"
            );
        } else {
            assert_eq!(off, 0, "a zone within half an hour of UTC that is not UTC");
        }
    }

    /// The offset is a function of the instant, not of today: a current-bias
    /// implementation would return one number for January, July and July 2006.
    ///
    /// Vacuous in a zone with no summer time, as a zone this process is not in
    /// cannot be asked for; so it asserts the set of legal DST steps (none, half
    /// an hour for Lord Howe, an hour).
    #[test]
    fn the_offset_answers_for_the_instant_rather_than_for_today() {
        // 2026-01-15 and 2026-07-15, both at midnight UTC.
        let (january, july) = (1_768_435_200u64, 1_784_073_600u64);
        let step = local_offset(july) - local_offset(january);
        assert!(
            [0, 1_800, 3_600].contains(&step.abs()),
            "summer time moves a zone by an hour, half an hour, or not at all — \
             not by {step} seconds"
        );
        // The same month twenty years earlier, which the dynamic (key-name)
        // conversion is for.
        let old = local_offset(1_152_921_600);
        assert_eq!(
            old % 900,
            0,
            "every zone offset ever used is a quarter hour"
        );
    }

    /// On Linux and macOS the offset is the zone's for the instant, in a zone the
    /// test picks. CI runs at UTC, where the tests above pass on an arm returning
    /// 0, so this re-runs itself in a child with `TZ` set to a POSIX Central
    /// European rule (no tz database needed). A child, not `std::env::set_var`,
    /// because other tests read the environment on other threads.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn the_unix_offset_is_the_zones_for_the_instant() {
        const CHILD: &str = "ONDIN_CLOCK_TZ_CHILD";
        const NAME: &str = "library::clock::tests::the_unix_offset_is_the_zones_for_the_instant";
        if std::env::var_os(CHILD).is_some() {
            // 2026-01-15 and 2026-07-15, midnight UTC; then July 2006.
            assert_eq!(local_offset(1_768_435_200), 3_600, "January, CET");
            assert_eq!(local_offset(1_784_073_600), 7_200, "July, CEST");
            assert_eq!(local_offset(1_152_921_600), 7_200, "July 2006, CEST");
            return;
        }
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([NAME, "--exact", "--test-threads=1"])
            .env(CHILD, "1")
            .env("TZ", "CET-1CEST,M3.5.0,M10.5.0/3")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "the child failed:\n{stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        // It ran the test rather than matching nothing.
        assert!(
            stdout.contains("1 passed"),
            "the child ran nothing:\n{stdout}"
        );
    }

    /// `days_from_civil` inverts `civil_from_unix`, checked on every day of sixty
    /// years rather than a sample, since the seams (leap day, century years,
    /// month lengths) are what a sample misses.
    #[test]
    fn the_calendar_round_trips_in_both_directions() {
        for day in 0..(60 * 365 + 15) {
            let secs = day as u64 * 86_400;
            let (y, m, d) = civil_from_unix(secs);
            assert_eq!(
                days_from_civil(y, m, d),
                day,
                "{y}-{m:02}-{d:02} came back as a different day"
            );
        }
        // Before the epoch: the `- 399` and `- 146_096` arms no library date reaches.
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(days_from_civil(1900, 1, 1), -25_567);
    }

    /// Every boundary, from both sides, to catch an off-by-one range.
    #[test]
    fn the_relative_label_changes_voice_at_each_boundary() {
        assert_eq!(relative_label(0), "Just now");
        assert_eq!(relative_label(59), "Just now");
        assert_eq!(relative_label(60), "1m ago");
        assert_eq!(relative_label(3_599), "59m ago");
        assert_eq!(relative_label(3_600), "1h ago");
        assert_eq!(relative_label(64_799), "17h ago");
        assert_eq!(relative_label(64_800), "Yesterday");
        assert_eq!(relative_label(172_799), "Yesterday");
        assert_eq!(relative_label(172_800), "2d ago");
        assert_eq!(relative_label(604_799), "6d ago");
        assert_eq!(relative_label(604_800), "Last week");
        assert_eq!(relative_label(1_209_600), "2w ago");
        assert_eq!(relative_label(2_592_000), "Last month");
        assert_eq!(relative_label(31_536_000), "1y ago");
    }

    /// A stamp from the future reads as "just now", not a wrapped number.
    #[test]
    fn a_timestamp_from_the_future_is_just_now() {
        assert_eq!(since(now() + 10_000), 0);
        assert_eq!(relative_label(since(now() + 10_000)), "Just now");
    }
}

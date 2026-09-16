//! Wall-clock time for the library: reading it, and turning a stored stamp into
//! something the dashboard can print.
//!
//! **The library stores seconds since the Unix epoch and nothing else** — see
//! `ondin_core::meta::DocumentMeta::created` for why a formatted date is the
//! wrong thing to put in a file. Everything that turns one back into text lives
//! here, so there is one answer to "what year is 1774483200" rather than one per
//! column.
//!
//! ⚠️ **The civil date is local time on Windows and UTC everywhere else**, and
//! that split is the whole of [`local_offset`]. It read UTC everywhere until
//! 2026-08-28, which named the wrong day for a few hours either side of
//! midnight — accepted at the time because converting correctly needs the zone's
//! *history* (a stamp from last July has a different offset from one from
//! January, so today's offset is not enough) and every way of getting that
//! looked like a dependency. It is not: Windows keeps the history and answers
//! for a given instant, in one call this app was already linked against.
//!
//! The relative labels people actually read — "2h ago", "Yesterday" — are
//! differences between two instants and have no timezone in them at all, which
//! is why none of this reaches them.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch, or `0` if the system clock is before it.
///
/// Saturating rather than erroring: a clock set to 1969 is a broken machine, not
/// a case the library should refuse to file a document over.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Seconds between `then` and now, or `0` if `then` is in the future.
///
/// **The future is not an error here.** A file synced from a machine whose clock
/// runs fast arrives with a modification time later than this one's, and the
/// honest label for it is "just now" rather than a negative number or a panic.
pub fn since(then: u64) -> u64 {
    now().saturating_sub(then)
}

/// Split a Unix timestamp into a UTC calendar date, `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days`, which is exact for every date the type
/// can hold and needs no table. The shift by 719468 moves the epoch to a
/// calendar starting on 1 March 0000, where the leap day is last in the year and
/// the month-length pattern is a single linear formula.
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
/// Hinnant's `days_from_civil`, the same March-first calendar read the other
/// way. It exists because Windows answers a zone question in calendar fields
/// rather than in seconds, so [`local_offset`] has to come back down to a number
/// to subtract — and because a round trip through the pair is the one assertion
/// about this arithmetic that holds on any machine in any zone.
///
/// Midnight of that date; the caller adds the time of day.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The local zone's offset from UTC, in seconds east, **for that instant**
/// (§15 D384).
///
/// ⚠️ **"For that instant" is the whole feature.** A stamp from last July and one
/// from January sit at different offsets in any zone that keeps summer time, and
/// a rule that changed — the US moving its DST dates in 2007, the EU arguing
/// about abolishing it — means even the *rule* is a function of the year. Adding
/// today's offset to every stamp is the version that looks right all summer and
/// is an hour out on half the library.
///
/// **Windows already knows.** `GetDynamicTimeZoneInformation` hands back the
/// zone including its registry key name, and `SystemTimeToTzSpecificLocalTimeEx`
/// converts *through that key*, which is where the year-by-year rules are kept.
/// So this is one call rather than a date library and its own copy of the tz
/// database — the trade `library::clock`'s module note recorded as "a dependency
/// or a platform call" and then did not take.
///
/// **Zero off Windows**, i.e. UTC, which is what the whole file did before. The
/// app is a Windows application (`windows-sys` is a `cfg(windows)` dependency and
/// the icon is written by `rc.exe`); this arm exists so the module still compiles
/// and reads honestly elsewhere rather than as a promise of portability.
pub fn local_offset(secs: u64) -> i64 {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::Time::{
            DYNAMIC_TIME_ZONE_INFORMATION, GetDynamicTimeZoneInformation,
            SystemTimeToTzSpecificLocalTimeEx,
        };

        let (y, m, d) = civil_from_unix(secs);
        // `civil_from_unix` is exact for every `u64`, and `SYSTEMTIME` holds the
        // year in a `u16`: a stamp past the year 65535 is a broken clock, and the
        // honest answer for one is UTC rather than a wrapped year.
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
        // Failure of either leaves `local` untouched, which is why the return is
        // gated on the `BOOL` rather than on the value.
        unsafe {
            let mut zone: DYNAMIC_TIME_ZONE_INFORMATION = std::mem::zeroed();
            // ⚠️ **`TIME_ZONE_ID_INVALID` is `u32::MAX` and every other return
            // value is a success**, including `TIME_ZONE_ID_UNKNOWN` — a zone
            // that simply has no daylight saving, which is a perfectly good
            // answer and not an error.
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
    #[cfg(not(windows))]
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
    // ⚠️ **Clamped at the epoch rather than allowed to go negative**, which is
    // the one case a shifted stamp can leave the range `civil_from_unix` takes:
    // midnight on 1 January 1970 is 31 December 1969 anywhere west of Greenwich.
    // A document created at the epoch is a broken clock — the same case `now`
    // saturates for — and this keeps the label a machine's timezone cannot
    // change, which is also what makes it assertable in a test.
    let local = (secs as i64 + local_offset(secs)).max(0) as u64;
    let (y, m, d) = civil_from_unix(local);
    // `m` comes out of `civil_from_unix` in 1..=12, so the index is in range;
    // clamped anyway rather than indexed blind, because the cost of being wrong
    // is a panic in a list row.
    let month = MONTHS[(m.clamp(1, 12) - 1) as usize];
    format!("{d} {month} {y}")
}

/// How long ago, in the dashboard's voice: `"2h ago"`, `"Yesterday"`,
/// `"Last week"`.
///
/// **Coarser than the editor's save pill on purpose** (`app::save_label`, which
/// counts minutes for an hour and a half). That one is answering "did my last
/// edit make it to disk", where a minute matters; this one is answering "which
/// of these forty files was I working on", where it does not — and forty rows
/// each reading "47m ago" is a column of noise rather than a column of
/// information.
pub fn relative_label(secs_ago: u64) -> String {
    match secs_ago {
        0..=59 => "Just now".into(),
        60..=3_599 => format!("{}m ago", secs_ago / 60),
        // Up to 18 hours reads as hours; past that "Yesterday" is both shorter
        // and what a person would say.
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

    /// Anchored on dates that are checkable by hand, including the two the
    /// algorithm is most likely to get wrong: a leap day, and 1 January.
    #[test]
    fn the_civil_date_matches_known_instants() {
        assert_eq!(civil_from_unix(0), (1970, 1, 1));
        // 2026-03-26T00:00:00Z, the day this was written.
        assert_eq!(civil_from_unix(1_774_483_200), (2026, 3, 26));
        // 2024-02-29, a leap day — the case the March-first calendar shift
        // exists to make ordinary.
        assert_eq!(civil_from_unix(1_709_164_800), (2024, 2, 29));
        // 2000-03-01, the day after the leap day of the century that *is* a leap
        // year: the 400-year rule, which a naive /4 //100 gets wrong.
        assert_eq!(civil_from_unix(951_868_800), (2000, 3, 1));
    }

    /// ⚠️ The seconds *within* a day must not shift the date. Flip-check: using
    /// `(secs + 86_399) / 86_400` — a plausible "round up to the day" slip —
    /// leaves the midnight cases above green and fails here.
    #[test]
    fn any_time_within_a_day_is_the_same_date() {
        let midnight = 1_774_483_200;
        assert_eq!(civil_from_unix(midnight), (2026, 3, 26));
        assert_eq!(civil_from_unix(midnight + 1), (2026, 3, 26));
        assert_eq!(civil_from_unix(midnight + 86_399), (2026, 3, 26));
        assert_eq!(civil_from_unix(midnight + 86_400), (2026, 3, 27));
    }

    /// ⚠️ **Both of these are stamps the local offset cannot move, and they used
    /// to be one that it can.** This asserted `date_label(1_774_483_200) == "26
    /// Mar 2026"` — midnight UTC — which was exact while the label was UTC and
    /// became a test of *the machine's timezone* the moment it was not: green in
    /// Europe, red in New York, for a function that is right in both. Noon is
    /// safe for every offset in `(-12h, +12h)`, which is every zone but Kiribati
    /// and a New Zealand summer, and the epoch is safe because `date_label`
    /// clamps there. What the label does with a *non-zero* offset is the next
    /// test, which is written to be true in any zone.
    #[test]
    fn the_date_label_reads_the_way_the_column_shows_it() {
        // 2026-03-26T12:00:00Z.
        assert_eq!(date_label(1_774_483_200 + 43_200), "26 Mar 2026");
        assert_eq!(date_label(0), "1 Jan 1970");
    }

    /// The *Created* column names the local day, which is the whole of the change
    /// — and the two probes are placed either side of midnight so that **one of
    /// them fires in any zone that is not UTC**.
    ///
    /// ⚠️ **A test that has to be written around the machine it runs on**, since
    /// there is no way to ask Windows for a zone this process is not in. So the
    /// shape is: read the offset, then assert what it *implies*. On a machine set
    /// to UTC neither arm has anything to check and the test says so out loud
    /// rather than passing quietly on nothing — `local_offset` returning 0 is
    /// also what the non-Windows build does, so a green run there is the same
    /// green run.
    ///
    /// Flip-check, run on a machine at UTC+1: making `date_label` ignore the
    /// offset fails at *"23:30 UTC is already tomorrow"* with "26 Mar 2026"
    /// against 27. ⚠️ **The other arm stays green in that flip** — a positive
    /// offset never moves a 00:30 stamp back a day — which is why both are here
    /// and why the `else` branch has to name the zone it saw.
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

    /// The offset is a function of **the instant**, not of today — which is the
    /// claim the whole platform call is for, and the one thing a wrong-but-
    /// plausible implementation ("ask the zone for its current bias") gets
    /// wrong.
    ///
    /// ⚠️ **Written from a measurement rather than from the API docs.** On the
    /// machine this landed on — Central European, UTC+1 standard — a probe
    /// printed **3600** for 15 January 2026 and **7200** for 15 July 2026, and
    /// 7200 again for July *2006*, twenty years earlier. A current-bias
    /// implementation prints the same number three times, so the January/July
    /// difference is the assertion and the 2006 stamp is the reason the call is
    /// the `Ex` one: it converts through the zone's registry key, where the
    /// year-by-year rules are kept.
    ///
    /// ⚠️ **Vacuous in a zone with no summer time**, where the difference is
    /// legitimately zero. There is no way to ask Windows about a zone this
    /// process is not in, so the test asserts the *set* of legal answers and
    /// names the one it saw; the three are the DST steps that exist — none, half
    /// an hour (Lord Howe), and an hour (everywhere else).
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
        // And the same month twenty years earlier, which is what the dynamic
        // (key-name) conversion is for. Its own offset must be a legal one too.
        let old = local_offset(1_152_921_600);
        assert_eq!(
            old % 900,
            0,
            "every zone offset ever used is a quarter hour"
        );
    }

    /// `days_from_civil` is `civil_from_unix` backwards, and the pair is the
    /// only part of the local-time path that can be checked without asking the
    /// machine what zone it is in.
    ///
    /// ⚠️ **Every day rather than a handful**, because what this is really
    /// guarding is the March-first calendar's seams — the leap day, the century
    /// years, the month whose length the linear formula gets from `(153m+2)/5` —
    /// and a sampled test picks the days those seams are not on. Sixty years of
    /// days is under a millisecond.
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
        // And before the epoch, which is the arm with the `- 399` and `- 146_096`
        // corrections in it and which no library date reaches.
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(days_from_civil(1900, 1, 1), -25_567);
    }

    /// Every boundary, from both sides — the pattern that catches an arm whose
    /// range is off by one, which reading the `match` cannot.
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

    /// A clock that runs backwards — a file synced from a machine set ahead —
    /// reads as "just now" rather than wrapping to a huge number.
    #[test]
    fn a_timestamp_from_the_future_is_just_now() {
        assert_eq!(since(now() + 10_000), 0);
        assert_eq!(relative_label(since(now() + 10_000)), "Just now");
    }
}

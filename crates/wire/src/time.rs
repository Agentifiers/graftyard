//! Timestamps as GitHub renders them: UTC ISO 8601 with a `Z`, to the second (`research/wire-census.md` #25).

use graftyard_types::Timestamp;

pub fn iso(t: Timestamp) -> String {
    let secs = (t.0 / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// A UTC ISO 8601 timestamp, or one with a numeric offset, to at least the second. Fractional seconds finer than a
/// millisecond are truncated, as `Timestamp` is millisecond precision.
pub fn parse_iso(s: &str) -> Option<Timestamp> {
    let b = s.as_bytes();
    if b.len() < 20
        || b.get(4) != Some(&b'-')
        || b.get(7) != Some(&b'-')
        || b.get(10) != Some(&b'T')
        || b.get(13) != Some(&b':')
        || b.get(16) != Some(&b':')
    {
        return None;
    }
    let number = |from: usize, to: usize| -> Option<i64> {
        let digits = b.get(from..to)?;
        digits.iter().all(u8::is_ascii_digit).then(|| std::str::from_utf8(digits).ok()?.parse().ok()).flatten()
    };
    let (y, m, d) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let (h, min, sec) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=12).contains(&m)
        || d < 1
        || d > i64::from(days[usize::try_from(m).ok()? - 1])
        || h > 23
        || min > 59
        || sec > 59
    {
        return None;
    }
    let mut at = 19;
    let mut millis = 0_u64;
    if b.get(at) == Some(&b'.') {
        at += 1;
        let start = at;
        while b.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        if at == start {
            return None;
        }
        for i in 0..3 {
            millis = millis * 10 + b.get(start + i).filter(|b| b.is_ascii_digit()).map_or(0, |b| u64::from(b - b'0'));
        }
    }
    let offset = match b.get(at) {
        Some(b'Z') if at + 1 == b.len() => 0,
        Some(sign @ (b'+' | b'-')) if at + 6 == b.len() && b.get(at + 3) == Some(&b':') => {
            let (oh, om) = (number(at + 1, at + 3)?, number(at + 4, at + 6)?);
            if oh > 23 || om > 59 {
                return None;
            }
            let seconds = (oh * 60 + om) * 60;
            if *sign == b'+' { seconds } else { -seconds }
        }
        _ => return None,
    };
    let seconds = days_from_civil(y, m, d).checked_mul(86_400)?.checked_add(h * 3600 + min * 60 + sec - offset)?;
    let milliseconds = seconds.checked_mul(1000)?;
    (milliseconds >= 0).then(|| Timestamp(milliseconds as u64 + millis))
}

/// Howard Hinnant's days_from_civil, the inverse of `civil`.
fn days_from_civil(mut y: i64, m: i64, d: i64) -> i64 {
    y -= i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// RFC 7231's IMF-fixdate, for `Last-Modified` and `Date`.
pub fn http_date(t: Timestamp) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let secs = (t.0 / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil(days);
    format!(
        "{}, {d:02} {} {y:04} {:02}:{:02}:{:02} GMT",
        DAYS[days.rem_euclid(7) as usize],
        MONTHS[m as usize - 1],
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Howard Hinnant's civil_from_days.
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_render_as_github_does() {
        assert_eq!(iso(Timestamp(0)), "1970-01-01T00:00:00Z");
        assert_eq!(iso(Timestamp(1_793_000_000_000)), "2026-10-26T07:33:20Z");
        assert_eq!(iso(Timestamp(951_782_400_000)), "2000-02-29T00:00:00Z");
        assert_eq!(http_date(Timestamp(784_111_777_000)), "Sun, 06 Nov 1994 08:49:37 GMT");
    }

    #[test]
    fn timestamps_parse_iso_with_fractions_and_offsets() {
        for t in [0, 1_793_000_000_000, 951_782_400_000, 784_111_777_000] {
            assert_eq!(parse_iso(&iso(Timestamp(t))), Some(Timestamp(t)));
        }
        assert_eq!(parse_iso("2024-02-29T23:59:58Z"), Some(Timestamp(1_709_251_198_000)));
        assert_eq!(parse_iso("2024-02-29T23:59:58.123Z"), Some(Timestamp(1_709_251_198_123)));
        assert_eq!(parse_iso("2026-11-12T03:00:00+02:00"), parse_iso("2026-11-12T01:00:00Z"));
        assert_eq!(parse_iso("2026-11-12T00:00:00-01:30"), parse_iso("2026-11-12T01:30:00Z"));
        for bad in ["2026-13-01T00:00:00Z", "2026-02-30T00:00:00Z", "garbage", "2024-02-29T00:00:00.Z"] {
            assert_eq!(parse_iso(bad), None, "{bad}");
        }
    }
}

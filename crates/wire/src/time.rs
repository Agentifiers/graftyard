//! Timestamps as GitHub renders them: UTC ISO 8601 with a `Z`, to the second (`research/wire-census.md` #25).

use graftyard_types::Timestamp;

pub fn iso(t: Timestamp) -> String {
    let secs = (t.0 / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
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
}

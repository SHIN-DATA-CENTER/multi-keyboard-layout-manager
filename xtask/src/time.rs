//! UTC dates without a date crate: the HTTP `Date` of GitHub (the trusted time of design m5b
//! B.3 step 7), GitHub's `publishedAt` (RFC 3339 in UTC) and the dates xtask prints.

pub const DAY: u64 = 86_400;

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The date of a day number of [`days_from_civil`].
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn month_days(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

/// Unix seconds of a UTC date and time; `None` for an impossible one or before 1970.
fn unix(year: i64, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> Option<u64> {
    if !(1..=12).contains(&month)
        || day == 0
        || day > month_days(year, month)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let seconds = days * DAY as i64
        + i64::from(hour) * 3600
        + i64::from(minute) * 60
        + i64::from(second.min(59));
    u64::try_from(seconds).ok()
}

fn number(text: &str, digits: usize) -> Option<u32> {
    if text.len() != digits || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// `HH:MM:SS`.
fn clock(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.split(':');
    let hour = number(parts.next()?, 2)?;
    let minute = number(parts.next()?, 2)?;
    let second = number(parts.next()?, 2)?;
    parts.next().is_none().then_some((hour, minute, second))
}

/// An IMF-fixdate (RFC 9110 5.6.7): `Tue, 29 Sep 2026 12:34:56 GMT`.
pub fn parse_http_date(text: &str) -> Option<u64> {
    let fields: Vec<&str> = text.split_whitespace().collect();
    let [weekday, day, month, year, time, "GMT"] = fields.as_slice() else {
        return None;
    };
    const WEEKDAYS: [&str; 7] = ["Mon,", "Tue,", "Wed,", "Thu,", "Fri,", "Sat,", "Sun,"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let weekday = WEEKDAYS.iter().position(|w| w == weekday)?;
    let month = MONTHS.iter().position(|m| m == month)? as u32 + 1;
    let day = number(day, 2)?;
    let year = i64::from(number(year, 4)?);
    let (hour, minute, second) = clock(time)?;
    let seconds = unix(year, month, day, hour, minute, second)?;
    // 1970-01-01 was a Thursday (index 3).
    let actual = ((seconds / DAY) + 3) % 7;
    (actual == weekday as u64).then_some(seconds)
}

/// `2026-09-29T12:34:56Z` (GitHub's `publishedAt`), fractions of a second allowed.
pub fn parse_rfc3339_utc(text: &str) -> Option<u64> {
    let (date, time) = text.split_once('T')?;
    let time = time.strip_suffix('Z')?;
    let time = time.split_once('.').map_or(time, |(whole, fraction)| {
        if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
            ""
        } else {
            whole
        }
    });
    let mut parts = date.split('-');
    let year = i64::from(number(parts.next()?, 4)?);
    let month = number(parts.next()?, 2)?;
    let day = number(parts.next()?, 2)?;
    if parts.next().is_some() {
        return None;
    }
    let (hour, minute, second) = clock(time)?;
    unix(year, month, day, hour, minute, second)
}

/// `2026-09-29`.
pub fn format_date(unix: u64) -> String {
    let (year, month, day) = civil_from_days((unix / DAY) as i64);
    format!("{year:04}-{month:02}-{day:02}")
}

/// `2026-09-29 12:34:56 UTC`.
pub fn format_utc(unix: u64) -> String {
    let seconds = unix % DAY;
    format!(
        "{} {:02}:{:02}:{:02} UTC",
        format_date(unix),
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates() {
        // Design m5b A.2: 1792022400 is 2026-10-15, and 180 days later 2027-04-13.
        assert_eq!(format_date(1_792_022_400), "2026-10-15");
        assert_eq!(format_date(1_807_574_400), "2027-04-13");
        assert_eq!(format_utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_utc(1_792_022_400 + 3661), "2026-10-15 01:01:01 UTC");
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        for days in [-1_000, 0, 59, 60, 10_000, 20_742, 30_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(civil_from_days(days_from_civil(2028, 2, 29)), (2028, 2, 29));
    }

    #[test]
    fn http_dates() {
        assert_eq!(
            parse_http_date("Thu, 15 Oct 2026 00:00:00 GMT"),
            Some(1_792_022_400)
        );
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        for bad in [
            "Wed, 15 Oct 2026 00:00:00 GMT",
            "Thu, 15 Oct 2026 00:00:00 UTC",
            "Thu, 15 Oct 2026 00:00 GMT",
            "Thu, 32 Oct 2026 00:00:00 GMT",
            "Thu, 15 Okt 2026 00:00:00 GMT",
            "Thursday, 15-Oct-26 00:00:00 GMT",
            "Thu, 5 Oct 2026 00:00:00 GMT",
            "",
        ] {
            assert_eq!(parse_http_date(bad), None, "{bad}");
        }
    }

    #[test]
    fn rfc3339_dates() {
        assert_eq!(
            parse_rfc3339_utc("2026-10-15T00:00:00Z"),
            Some(1_792_022_400)
        );
        assert_eq!(
            parse_rfc3339_utc("2026-10-15T00:00:01.5Z"),
            Some(1_792_022_401)
        );
        for bad in [
            "2026-10-15T00:00:00",
            "2026-10-15T00:00:00+09:00",
            "2026-10-15 00:00:00Z",
            "2026-02-30T00:00:00Z",
            "2026-10-15T24:00:00Z",
            "2026-10-15T00:00:00.Z",
            "26-10-15T00:00:00Z",
        ] {
            assert_eq!(parse_rfc3339_utc(bad), None, "{bad}");
        }
    }
}

const NO_LEAP: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const ALL_LEAP: [i64; 12] = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const DAY_360: [i64; 12] = [30; 12];

#[derive(Clone, Copy)]
enum Calendar {
    /// Julian before 1582-10-15, Gregorian from then on.
    Standard,
    Gregorian,
    Julian,
    /// Every year has these month lengths.
    Fixed(&'static [i64; 12]),
}

/// Formats a CF time value ("<unit> since <date>") as a calendar date, or `None` when the
/// units or calendar are not recognised and the stored value should be shown instead.
pub fn format(value: f64, units: &str, calendar: Option<&str>) -> Option<String> {
    let calendar = match calendar
        .unwrap_or("standard")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "standard" | "gregorian" => Calendar::Standard,
        "proleptic_gregorian" => Calendar::Gregorian,
        "julian" => Calendar::Julian,
        "noleap" | "365_day" => Calendar::Fixed(&NO_LEAP),
        "all_leap" | "366_day" => Calendar::Fixed(&ALL_LEAP),
        "360_day" => Calendar::Fixed(&DAY_360),
        _ => return None,
    };
    let (unit, reference) = units.trim().split_once(" since ")?;
    let seconds_per_unit = match unit.trim().to_ascii_lowercase().as_str() {
        "seconds" | "second" | "secs" | "sec" | "s" => 1.0,
        "minutes" | "minute" | "mins" | "min" => 60.0,
        "hours" | "hour" | "hrs" | "hr" | "h" => 3600.0,
        "days" | "day" | "d" => 86_400.0,
        // CF months and years have no fixed length, so they cannot be decoded exactly.
        _ => return None,
    };
    let reference = reference.trim();
    let (date, clock) = reference.split_once(['T', ' ']).unwrap_or((reference, ""));
    let (sign, date) = match date.strip_prefix('-') {
        Some(date) => (-1, date),
        None => (1, date),
    };
    let mut parts = date.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (sign * parts.next()??, parts.next()??, parts.next()??);
    if parts.next().is_some() {
        return None;
    }
    let mut seconds_of_day = 0.0;
    for (index, token) in clock.split_whitespace().enumerate() {
        let token = token.strip_suffix('Z').unwrap_or(token);
        let zero_offset = index > 0
            && token
                .trim_start_matches(['+', '-'])
                .chars()
                .all(|c| c == '0' || c == ':');
        if zero_offset || matches!(token.to_ascii_uppercase().as_str(), "" | "UTC" | "GMT") {
            continue;
        }
        if index > 0 {
            return None; // A non-zero time-zone offset is not applied.
        }
        let mut fields = token.split(':').map(|field| field.parse::<f64>().ok());
        let hours = fields.next()??;
        let minutes = fields.next().unwrap_or(Some(0.0))?;
        let seconds = fields.next().unwrap_or(Some(0.0))?;
        if fields.next().is_some() {
            return None;
        }
        seconds_of_day = hours * 3600.0 + minutes * 60.0 + seconds;
    }
    let start = calendar.days(year, month, day)?;
    // Rounding to whole seconds turns floating-point noise such as 23:59:59.9999 into midnight.
    let total = (value * seconds_per_unit + seconds_of_day).round();
    if !total.is_finite() || total.abs() > 1e15 {
        return None;
    }
    let total = total as i64;
    let (year, month, day) = calendar.date(start + total.div_euclid(86_400));
    let seconds = total.rem_euclid(86_400);
    let mut text = format!("{year:04}-{month:02}-{day:02}");
    if seconds != 0 {
        text.push_str(&format!(" {:02}:{:02}", seconds / 3600, seconds / 60 % 60));
        if seconds % 60 != 0 {
            text.push_str(&format!(":{:02}", seconds % 60));
        }
    }
    Some(text)
}

impl Calendar {
    /// Day number of a date, or `None` if the date does not exist in this calendar.
    fn days(self, year: i64, month: i64, day: i64) -> Option<i64> {
        if !(1..=12).contains(&month) {
            return None;
        }
        let days = match self {
            Calendar::Fixed(lengths) => {
                year * lengths.iter().sum::<i64>()
                    + lengths[..month as usize - 1].iter().sum::<i64>()
                    + day
                    - 1
            }
            Calendar::Standard => {
                julian_day(year, month, day, (year, month, day) >= (1582, 10, 15))
            }
            Calendar::Gregorian => julian_day(year, month, day, true),
            Calendar::Julian => julian_day(year, month, day, false),
        };
        // The round trip rejects dates such as 30 February in the Gregorian calendar.
        (self.date(days) == (year, month, day)).then_some(days)
    }

    fn date(self, days: i64) -> (i64, i64, i64) {
        match self {
            Calendar::Fixed(lengths) => {
                let year_length: i64 = lengths.iter().sum();
                let mut rest = days.rem_euclid(year_length);
                let mut month = 0;
                while rest >= lengths[month] {
                    rest -= lengths[month];
                    month += 1;
                }
                (days.div_euclid(year_length), month as i64 + 1, rest + 1)
            }
            Calendar::Standard => civil(days, days >= 2_299_161),
            Calendar::Gregorian => civil(days, true),
            Calendar::Julian => civil(days, false),
        }
    }
}

/// Julian day number of a Gregorian or Julian calendar date (Fliegel and Van Flandern).
fn julian_day(year: i64, month: i64, day: i64, gregorian: bool) -> i64 {
    let a = (14 - month) / 12;
    let y = year + 4800 - a;
    let m = month + 12 * a - 3;
    let days = day + (153 * m + 2) / 5 + 365 * y + y.div_euclid(4);
    if gregorian {
        days - y.div_euclid(100) + y.div_euclid(400) - 32_045
    } else {
        days - 32_083
    }
}

/// Calendar date of a Julian day number, the inverse of `julian_day` (Richards' algorithm).
fn civil(julian_day: i64, gregorian: bool) -> (i64, i64, i64) {
    let mut f = julian_day + 1401;
    if gregorian {
        f += (4 * julian_day + 274_277).div_euclid(146_097) * 3 / 4 - 38;
    }
    let e = 4 * f + 3;
    let h = 5 * (e.rem_euclid(1461) / 4) + 2;
    let month = (h / 153 + 2) % 12 + 1;
    (
        e.div_euclid(1461) - 4716 + (14 - month) / 12,
        month,
        h % 153 / 5 + 1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expected dates cross-checked with cftime 1.6.5 `num2date`.
    #[test]
    fn decodes_dates_in_each_cf_calendar() {
        for (value, units, calendar, expected) in [
            // NCEP climatology: crosses back over the 1582 Julian/Gregorian switch.
            (
                -15765408.0,
                "hours since 1800-01-01 00:00:0.0",
                None,
                "0001-07-01",
            ),
            (30.0, "days since 2000-01-01", Some("noleap"), "2000-01-31"),
            (59.0, "days since 2000-01-01", Some("noleap"), "2000-03-01"),
            (59.0, "days since 2000-01-01", Some("365_day"), "2000-03-01"),
            (59.0, "days since 2000-01-01", Some("360_day"), "2000-02-30"),
            (
                59.0,
                "days since 2001-01-01",
                Some("all_leap"),
                "2001-02-29",
            ),
            (1.0, "days since 1582-10-04", Some("standard"), "1582-10-15"),
            (
                -1.0,
                "days since 1582-10-15",
                Some("standard"),
                "1582-10-04",
            ),
            (
                1.0,
                "days since 1582-10-04",
                Some("proleptic_gregorian"),
                "1582-10-05",
            ),
            (1.0, "days since 1900-02-28", Some("julian"), "1900-02-29"),
            (0.0, "days since 1-1-1", Some("360_day"), "0001-01-01"),
        ] {
            assert_eq!(
                format(value, units, calendar).as_deref(),
                Some(expected),
                "{value} {units} {calendar:?}"
            );
        }
    }

    #[test]
    fn shows_the_time_of_day_only_when_it_is_not_midnight() {
        for (value, units, calendar, expected) in [
            (
                1_700_000_000.0,
                "seconds since 1970-01-01T00:00:00Z",
                None,
                "2023-11-14 22:13:20",
            ),
            (
                3_155_803_200.0,
                "seconds since 1900-01-01 00:00:00",
                Some("noleap"),
                "2000-01-26 12:00",
            ),
            (
                -7.0,
                "hours since 2000-01-01 06:00",
                Some("gregorian"),
                "1999-12-31 23:00",
            ),
            (
                90.0,
                "minutes since 2000-01-01 00:00:00 UTC",
                None,
                "2000-01-01 01:30",
            ),
            // Floating-point noise just below midnight rounds to the next day.
            (1.99999999999, "days since 2000-01-01", None, "2000-01-03"),
        ] {
            assert_eq!(
                format(value, units, calendar).as_deref(),
                Some(expected),
                "{value} {units} {calendar:?}"
            );
        }
    }

    #[test]
    fn leaves_values_it_cannot_decode_exactly() {
        for (value, units, calendar) in [
            (1.0, "months since 2000-01-01", None), // months and years have no fixed length
            (1.0, "Year mid-point", None),
            (5.0, "m", None),
            (1.0, "days since 2000-13-01", None),
            (1.0, "days since 2000-02-30", None), // not a date in the standard calendar
            (1.0, "days since 2000-01-01 00:00:00 +05:00", None), // offsets are not applied
            (1.0, "days since 2000-01-01", Some("none")),
            (1.0, "days since 2000-01-01", Some("lunar")),
            (f64::NAN, "days since 2000-01-01", None),
        ] {
            assert_eq!(
                format(value, units, calendar),
                None,
                "{value} {units} {calendar:?}"
            );
        }
    }
}

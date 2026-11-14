//! Calendar dates from a count of days, for the places that read today's date from the
//! system clock.

/// The proleptic Gregorian date `days` after 1970-01-01 (day 0; earlier days are negative), as
/// the year, the month (1 to 12) and the day of the month (1 to 31), by Howard Hinnant's
/// `civil_from_days`.
#[must_use]
pub fn civil_from_days(days: i64) -> (i64, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (
        year,
        u8::try_from(month).unwrap_or(1),
        u8::try_from(day).unwrap_or(1),
    )
}

#[cfg(test)]
mod tests {
    use super::civil_from_days as date;

    #[test]
    fn days_become_dates() {
        assert_eq!(date(0), (1970, 1, 1));
        assert_eq!(date(364), (1970, 12, 31));
        assert_eq!(date(365), (1971, 1, 1));
        assert_eq!(date(20_385), (2025, 10, 24));
        assert_eq!(date(20_761), (2026, 11, 4));
        assert_eq!(date(20_088), (2024, 12, 31));
        assert_eq!(date(20_089), (2025, 1, 1));
    }

    #[test]
    fn days_before_1970_are_negative() {
        assert_eq!(date(-1), (1969, 12, 31));
        assert_eq!(date(-365), (1969, 1, 1));
        assert_eq!(date(-366), (1968, 12, 31));
        assert_eq!(date(-672), (1968, 2, 29));
        assert_eq!(date(-719_162), (1, 1, 1));
        assert_eq!(date(-719_163), (0, 12, 31));
    }

    #[test]
    fn leap_days_fall_where_the_gregorian_calendar_puts_them() {
        assert_eq!(date(788), (1972, 2, 28));
        assert_eq!(date(789), (1972, 2, 29));
        assert_eq!(date(790), (1972, 3, 1));
        assert_eq!(date(1095), (1972, 12, 31));
        assert_eq!(date(1096), (1973, 1, 1));
        // 1600 and 2000 are leap years; 1900 and 2100 are not.
        assert_eq!(date(-135_081), (1600, 2, 29));
        assert_eq!(date(11_016), (2000, 2, 29));
        assert_eq!(date(11_017), (2000, 3, 1));
        assert_eq!(date(-25_509), (1900, 2, 28));
        assert_eq!(date(-25_508), (1900, 3, 1));
        assert_eq!(date(47_540), (2100, 2, 28));
        assert_eq!(date(47_541), (2100, 3, 1));
        assert_eq!(date(2_932_896), (9999, 12, 31));
    }
}

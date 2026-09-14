/// When a file was last written, as `YYYY-MM-DD HH:MM`, in UTC.
///
/// The stamp is whole seconds since the epoch, which is what the index stores.
/// Nothing here reads a date out of the picture's own metadata: most of the
/// formats this reads do not carry one.
pub(super) fn file_date(mtime_seconds: i64) -> String {
    let (days, rest) = (
        mtime_seconds.div_euclid(86_400),
        mtime_seconds.rem_euclid(86_400),
    );
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        rest / 3600,
        (rest % 3600) / 60
    )
}

/// Days since 1970-01-01 to a calendar date, by Howard Hinnant's method: the year
/// is shifted to start in March so a leap day falls at the end of it and every
/// four hundred years is one cycle of a fixed length.
pub(super) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

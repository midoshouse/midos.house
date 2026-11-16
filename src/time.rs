use {
    sqlx::postgres::types::PgInterval,
    crate::prelude::*,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum PgIntervalDecodeError {
    #[error(transparent)] TryFromInt(#[from] std::num::TryFromIntError),
    #[error("found PgInterval with nonzero months in database")]
    Months,
    #[error("PgInterval too long")]
    Range,
}

pub(crate) fn decode_pginterval(PgInterval { months, days, microseconds }: PgInterval) -> Result<Duration, PgIntervalDecodeError> {
    if months == 0 {
        Duration::from_hours(u64::try_from(days)? * 24)
            .checked_add(Duration::from_micros(microseconds.try_into()?))
            .ok_or(PgIntervalDecodeError::Range)
    } else {
        Err(PgIntervalDecodeError::Months)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum DurationUnit {
    Hours,
    Minutes,
    Seconds,
}

impl DurationUnit {
    fn with_magnitude(&self, magnitude: u64) -> Duration {
        match self {
            Self::Hours => Duration::from_hours(magnitude),
            Self::Minutes => Duration::from_mins(magnitude),
            Self::Seconds => Duration::from_secs(magnitude),
        }
    }
}

pub(crate) fn parse_duration(mut s: &str, default_unit: Option<DurationUnit>) -> Option<Duration> {
    Some(if let Some((_, hours, minutes, seconds)) = regex_captures!("^ *([0-9]+):([0-9]+):([0-9]+) *$", s).filter(|_| default_unit.is_none()) {
        Duration::from_hours(hours.parse::<u64>().ok()?) + Duration::from_mins(minutes.parse::<u64>().ok()?) + Duration::from_secs(seconds.parse::<u64>().ok()?)
    } else if let Some((_, minutes, seconds)) = regex_captures!("^ *([0-9]+):([0-9]+) *$", s).filter(|_| default_unit.is_none()) {
        Duration::from_mins(minutes.parse::<u64>().ok()?) + Duration::from_secs(seconds.parse::<u64>().ok()?)
    } else {
        let mut duration = Duration::default();
        let mut default_unit = Some(default_unit.unwrap_or(DurationUnit::Hours));
        let mut last_magnitude = None;
        loop {
            match s.chars().next() {
                None => break,
                Some(' ') => s = &s[1..],
                Some('0'..='9') => {
                    let (_, magnitude, rest) = regex_captures!("^([0-9]+)(.*)$", s)?; //TODO allow fractional magnitudes? (e.g. 2.5h = 2h30m)
                    if last_magnitude.replace(magnitude.parse().ok()?).is_some() {
                        return None // multiple whitespace-separated numbers
                    }
                    s = rest;
                }
                Some(':') => {
                    let magnitude = last_magnitude.take()?;
                    duration += default_unit?.with_magnitude(magnitude);
                    default_unit = match default_unit? {
                        DurationUnit::Hours => Some(DurationUnit::Minutes),
                        DurationUnit::Minutes => Some(DurationUnit::Seconds),
                        DurationUnit::Seconds => None,
                    };
                    s = &s[1..];
                }
                Some('H' | 'h') => {
                    let magnitude = last_magnitude.take()?;
                    duration += Duration::from_hours(magnitude);
                    default_unit = Some(DurationUnit::Minutes);
                    (_, s) = regex_captures!("^h(?:(?:ou)?r)?s?(.*)$"i, s)?;
                }
                Some('M' | 'm') => {
                    let magnitude = last_magnitude.take()?;
                    duration += Duration::from_mins(magnitude);
                    default_unit = Some(DurationUnit::Seconds);
                    (_, s) = regex_captures!("^m(?:n|in(?:ute)?)?s?(.*)$"i, s)?;
                }
                Some('S' | 's') => {
                    let magnitude = last_magnitude.take()?;
                    duration += Duration::from_secs(magnitude);
                    default_unit = None;
                    (_, s) = regex_captures!("^s(?:ec(?:ond)?)?s?(.*)$"i, s)?;
                }
                _ => return None,
            }
        }
        if let Some(magnitude) = last_magnitude {
            duration += default_unit?.with_magnitude(magnitude);
        }
        duration
    })
}

pub(crate) fn unparse_duration(duration: Duration) -> String {
    let mut buf = String::default();
    let secs = duration.as_secs();
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let secs = secs % 60;
    if hours > 0 {
        buf.push_str(&format!("{hours}h"));
        if mins > 0 || secs > 0 {
            buf.push_str(&format!("{mins:02}m"));
        }
        if secs > 0 {
            buf.push_str(&format!("{secs:02}s"));
        }
    } else if mins > 0 {
        buf.push_str(&format!("{mins}m"));
        if secs > 0 {
            buf.push_str(&format!("{secs:02}s"));
        }
    } else {
        buf.push_str(&format!("{secs}s"));
    }
    buf
}

pub(crate) struct DateTimeFormat {
    pub(crate) long: bool,
    pub(crate) running_text: bool,
}

pub(crate) fn format_datetime(datetime: impl Into<Timestamp>, format: DateTimeFormat) -> RawHtml<String> {
    let datetime = datetime.into();
    let utc = datetime.to_zoned_datetime(TimeZone::UTC);
    let paris = datetime.in_tz("Europe/Paris").unwrap();
    let new_york = datetime.in_tz("America/New_York").unwrap();
    let paris_same_date = paris.date() == utc.date();
    let new_york_same_date = new_york.date() == utc.date();
    let paris = paris.strftime(if paris_same_date { "%H:%M %Z" } else { "%a %H:%M %Z" }).to_string();
    let new_york = new_york.strftime(match (new_york_same_date, new_york.minute() == 0) {
        (false, false) => "%a %-I:%M %p %Z",
        (false, true) => "%a %-I%p %Z",
        (true, false) => "%-I:%M %p %Z",
        (true, true) => "%-I%p %Z",
    }).to_string();
    html! {
        //TODO format as a link, e.g. https://hammertime.cyou/?t=1723402800.000
        span(class = "datetime", data_timestamp = datetime.as_millisecond(), data_long = format.long.to_string()) {
            : utc.strftime("%A, %B %-d, %Y, %H:%M UTC").to_string();
            @if format.running_text {
                : " (";
            } else {
                : " • ";
            }
            @if new_york_same_date && !paris_same_date {
                : new_york;
            } else {
                : paris;
            }
            @if format.running_text {
                : ", ";
            } else {
                : " • ";
            }
            @if new_york_same_date && !paris_same_date {
                : paris;
            } else {
                : new_york;
            }
            @if format.running_text {
                : ")";
            }
        }
    }
}

pub(crate) fn format_date_range(start: impl Into<Timestamp>, end: impl Into<Timestamp>) -> RawHtml<String> {
    let start = start.into();
    let end = end.into();
    html! {
        span(class = "daterange", data_start = start.as_millisecond(), data_end = end.as_millisecond()) {
            @let start = start.to_zoned_datetime(TimeZone::UTC);
            @let end = end.to_zoned_datetime(TimeZone::UTC);
            @if start.year() != end.year() {
                : start.strftime("%B %-d, %Y").to_string();
                : "–";
                : end.strftime("%B %-d, %Y").to_string();
            } else if start.month() != end.month() {
                : start.strftime("%B %-d").to_string();
                : "–";
                : end.strftime("%B %-d, %Y").to_string();
            } else if start.day() != end.day() {
                : start.strftime("%B %-d").to_string();
                : "–";
                : end.strftime("%-d, %Y").to_string();
            } else {
                : start.strftime("%B %-d, %Y").to_string();
            }
        }
    }
}

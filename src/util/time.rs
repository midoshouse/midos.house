use {
    std::{
        fmt,
        time::Duration,
    },
    chrono::prelude::*,
    chrono_tz::{
        America,
        Europe,
    },
    lazy_regex::regex_captures,
    rocket::response::content::RawHtml,
    rocket_util::html,
    sqlx::postgres::types::PgInterval,
    super::natjoin_str,
};

macro_rules! utc {
    ($year:expr, $month:expr, $day:expr, $hour:expr, $minute:expr, $second:expr) => {
        Utc.with_ymd_and_hms($year, $month, $day, $hour, $minute, $second).single().expect("wrong hardcoded datetime")
    };
    ($year:expr, $month:expr, $day:expr, $hour:expr, $minute:expr, $second:expr, $milli:expr) => {
        utc!($year, $month, $day, $hour, $minute, $second).with_nanosecond($milli * 1_000_000).expect("wrong hardcoded datetime")
    };
}

pub(crate) use utc;

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
        Duration::from_secs(u64::try_from(days)? * 60 * 60 * 24)
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
        Duration::from_secs(match self {
            Self::Hours => 60 * 60 * magnitude,
            Self::Minutes => 60 * magnitude,
            Self::Seconds => magnitude,
        })
    }
}

pub(crate) fn parse_duration(mut s: &str, default_unit: DurationUnit) -> Option<Duration> {
    let mut duration = Duration::default();
    let mut default_unit = Some(default_unit);
    let mut last_magnitude = None;
    loop {
        match s.chars().next() {
            None => break,
            Some(' ') => s = &s[1..],
            Some('0'..='9') => {
                let (_, magnitude, rest) = regex_captures!("^([0-9]+)(.*)$", s)?;
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
                duration += Duration::from_secs(60 * 60 * magnitude);
                default_unit = Some(DurationUnit::Minutes);
                (_, s) = regex_captures!("^h(?:(?:ou)?r)?s?(.*)$"i, s)?;
            }
            Some('M' | 'm') => {
                let magnitude = last_magnitude.take()?;
                duration += Duration::from_secs(60 * magnitude);
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
    Some(duration)
}

pub(crate) fn format_duration(duration: Duration, running_text: bool) -> String {
    let secs = duration.as_secs();
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let secs = secs % 60;
    if running_text {
        let parts = (hours > 0).then(|| format!("{hours} hour{}", if hours == 1 { "" } else { "s" })).into_iter()
            .chain((mins > 0).then(|| format!("{mins} minute{}", if mins == 1 { "" } else { "s" })))
            .chain((secs > 0).then(|| format!("{secs} second{}", if secs == 1 { "" } else { "s" })));
        natjoin_str(parts).unwrap_or_else(|| format!("0 seconds"))
    } else {
        format!("{hours}:{mins:02}:{secs:02}")
    }
}

pub(crate) struct DateTimeFormat {
    pub(crate) long: bool,
    pub(crate) running_text: bool,
}

pub(crate) fn format_datetime<Tz: TimeZone>(datetime: DateTime<Tz>, format: DateTimeFormat) -> RawHtml<String> {
    let utc = datetime.with_timezone(&Utc);
    let berlin = datetime.with_timezone(&Europe::Berlin);
    let new_york = datetime.with_timezone(&America::New_York);
    html! {
        span(class = "datetime", data_timestamp = datetime.timestamp_millis(), data_long = format.long.to_string()) {
            : utc.format("%A, %B %-d, %Y, %H:%M UTC").to_string();
            @if format.running_text {
                : " (";
            } else {
                : " • ";
            }
            : berlin.format(if berlin.date_naive() == utc.date_naive() { "%H:%M %Z" } else { "%A %H:%M %Z" }).to_string();
            @if format.running_text {
                : ", ";
            } else {
                : " • ";
            }
            : new_york.format(match (new_york.date_naive() == utc.date_naive(), new_york.minute() == 0) {
                (false, false) => "%A %-I:%M %p %Z",
                (false, true) => "%A %-I%p %Z",
                (true, false) => "%-I:%M %p %Z",
                (true, true) => "%-I%p %Z",
            }).to_string();
            @if format.running_text {
                : ")";
            }
        }
    }
}

pub(crate) fn format_date_range<Tz: TimeZone>(start: DateTime<Tz>, end: DateTime<Tz>) -> RawHtml<String>
where Tz::Offset: fmt::Display {
    html! {
        span(class = "daterange", data_start = start.timestamp_millis(), data_end = end.timestamp_millis()) {
            @if start.year() != end.year() {
                : start.format("%B %-d, %Y").to_string();
                : "–";
                : end.format("%B %-d, %Y").to_string();
            } else if start.month() != end.month() {
                : start.format("%B %-d").to_string();
                : "–";
                : end.format("%B %-d, %Y").to_string();
            } else if start.day() != end.day() {
                : start.format("%B %-d").to_string();
                : "–";
                : end.format("%-d, %Y").to_string();
            } else {
                : start.format("%B %-d, %Y").to_string();
            }
        }
    }
}

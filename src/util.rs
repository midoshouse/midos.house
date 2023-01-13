use {
    std::{
        fmt,
        io,
        iter,
        mem,
        str::FromStr,
        time::Duration,
    },
    async_trait::async_trait,
    chrono::prelude::*,
    chrono_tz::{
        America,
        Europe,
    },
    derive_more::From,
    itertools::Itertools as _,
    lazy_regex::regex_captures,
    rand::prelude::*,
    rocket::{
        FromForm,
        Responder,
        UriDisplayPath,
        UriDisplayQuery,
        form::{
            self,
            FromFormField,
        },
        http::Status,
        request::FromParam,
        response::{
            Redirect,
            content::RawHtml,
        },
    },
    rocket_csrf::CsrfToken,
    rocket_util::{
        ToHtml,
        html,
    },
    serde::{
        Deserialize,
        Serialize,
    },
    serenity::{
        model::prelude::*,
        utils::MessageBuilder,
    },
    sqlx::{
        Database,
        Decode,
        Encode,
        Postgres,
        Transaction,
        postgres::types::PgInterval,
    },
    url::Url,
    crate::{
        http::{
            PageError,
            static_url,
        },
        team::Team,
        user::User,
    },
};

macro_rules! as_variant {
    ($value:expr, $variant:path) => {
        if let $variant(field) = $value {
            Some(field)
        } else {
            None
        }
    };
    ($variant:path) => {
        |value| as_variant!(value, $variant)
    };
}

macro_rules! utc {
    ($year:expr, $month:expr, $day:expr, $hour:expr, $minute:expr, $second:expr) => {
        Utc.with_ymd_and_hms($year, $month, $day, $hour, $minute, $second).single().expect("wrong hardcoded datetime")
    };
    ($year:expr, $month:expr, $day:expr, $hour:expr, $minute:expr, $second:expr, $milli:expr) => {
        utc!($year, $month, $day, $hour, $minute, $second).with_nanosecond($milli * 1_000_000).expect("wrong hardcoded datetime")
    };
}

pub(crate) use as_variant;
pub(crate) use utc;

#[async_trait]
pub(crate) trait MessageBuilderExt {
    fn mention_command(&mut self, command_id: CommandId, name: &str) -> &mut Self;
    async fn mention_team(&mut self, transaction: &mut Transaction<'_, Postgres>, guild: GuildId, team: &Team) -> sqlx::Result<&mut Self>;
    fn mention_user(&mut self, user: &User) -> &mut Self;
}

#[async_trait]
impl MessageBuilderExt for MessageBuilder {
    fn mention_command(&mut self, command_id: CommandId, name: &str) -> &mut Self {
        self.push("</").push(name).push(':').push(command_id).push('>')
    }

    async fn mention_team(&mut self, transaction: &mut Transaction<'_, Postgres>, guild: GuildId, team: &Team) -> sqlx::Result<&mut Self> {
        let team_role = if let Some(ref racetime_slug) = team.racetime_slug {
            sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM discord_roles WHERE guild = $1 AND racetime_team = $2"#, i64::from(guild), racetime_slug).fetch_optional(transaction).await?
        } else {
            None
        };
        if let Some(Id(team_role)) = team_role {
            self.role(team_role);
        } else if let Some(ref team_name) = team.name {
            //TODO pothole if racetime slug exists?
            self.push_italic_safe(team_name);
        } else {
            //TODO pothole if racetime slug exists?
            self.push("an unnamed team");
        }
        Ok(self)
    }

    fn mention_user(&mut self, user: &User) -> &mut Self {
        if let Some(discord_id) = user.discord_id {
            self.mention(&discord_id)
        } else {
            self.push_safe(user.display_name())
        }
    }
}

/// A form that only holds a CSRF token
#[derive(FromForm)]
pub(crate) struct EmptyForm {
    csrf: String,
}

impl EmptyForm {
    pub(crate) fn verify(&self, token: &Option<CsrfToken>) -> Result<(), rocket_csrf::VerificationFailure> {
        if let Some(token) = token {
            token.verify(&self.csrf)
        } else {
            Err(rocket_csrf::VerificationFailure)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize, UriDisplayPath, UriDisplayQuery)]
#[serde(transparent)]
pub(crate) struct Id(pub(crate) u64);

pub(crate) enum IdTable {
    Notifications,
    Races,
    Teams,
    Users,
}

impl Id {
    pub(crate) async fn new(transaction: &mut Transaction<'_, Postgres>, table: IdTable) -> sqlx::Result<Self> {
        Ok(loop {
            let id = Self(thread_rng().gen());
            let query = match table {
                IdTable::Notifications => sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM notifications WHERE id = $1) AS "exists!""#, i64::from(id)),
                IdTable::Races => sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM races WHERE id = $1) AS "exists!""#, i64::from(id)),
                IdTable::Teams => sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams WHERE id = $1) AS "exists!""#, i64::from(id)),
                IdTable::Users => sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE id = $1) AS "exists!""#, i64::from(id)),
            };
            if !query.fetch_one(&mut *transaction).await? { break id }
        })
    }
}

impl From<u64> for Id {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl From<i64> for Id {
    fn from(id: i64) -> Self {
        Self(id as u64)
    }
}

impl From<Id> for i64 {
    fn from(Id(id): Id) -> Self {
        id as Self
    }
}

impl FromStr for Id {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u64>()
            .map(Self)
            .or_else(|_| s.parse::<i64>().map(Self::from))
    }
}

impl<'r, DB: Database> Decode<'r, DB> for Id
where i64: Decode<'r, DB> {
    fn decode(value: <DB as sqlx::database::HasValueRef<'r>>::ValueRef) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        i64::decode(value).map(|id| Self(id as u64))
    }
}

impl<'q, DB: Database> Encode<'q, DB> for Id
where i64: Encode<'q, DB> {
    fn encode_by_ref(&self, buf: &mut <DB as sqlx::database::HasArguments<'q>>::ArgumentBuffer) -> sqlx::encode::IsNull {
        (self.0 as i64).encode(buf)
    }

    fn encode(self, buf: &mut <DB as sqlx::database::HasArguments<'q>>::ArgumentBuffer) -> sqlx::encode::IsNull {
        (self.0 as i64).encode(buf)
    }

    fn produces(&self) -> Option<<DB as Database>::TypeInfo> {
        (self.0 as i64).produces()
    }

    fn size_hint(&self) -> usize {
        Encode::size_hint(&(self.0 as i64))
    }
}

impl<DB: Database> sqlx::Type<DB> for Id
where i64: sqlx::Type<DB> {
    fn type_info() -> <DB as Database>::TypeInfo {
        i64::type_info()
    }

    fn compatible(ty: &<DB as Database>::TypeInfo) -> bool {
        i64::compatible(ty)
    }
}

impl<'a> FromParam<'a> for Id {
    type Error = &'a str;

    fn from_param(param: &'a str) -> Result<Self, Self::Error> {
        u64::from_param(param)
            .map(Self)
            .or_else(|_| i64::from_param(param).map(Self::from))
    }
}

impl<'v> FromFormField<'v> for Id
where i64: FromFormField<'v>, u64: FromFormField<'v> {
    fn from_value(field: form::ValueField<'v>) -> form::Result<'v, Self> {
        u64::from_value(field.clone())
            .map(Self)
            .or_else(|_| i64::from_value(field).map(Self::from))
    }

    fn default() -> Option<Self> { None }
}

pub(crate) fn natjoin_html<T: ToHtml>(elts: impl IntoIterator<Item = T>) -> Option<RawHtml<String>> {
    let mut elts = elts.into_iter().fuse();
    match (elts.next(), elts.next(), elts.next()) {
        (None, _, _) => None,
        (Some(elt), None, _) => Some(html! {
            : elt;
        }),
        (Some(elt1), Some(elt2), None) => Some(html! {
            : elt1;
            : " and ";
            : elt2;
        }),
        (Some(elt1), Some(elt2), Some(elt3)) => {
            let mut rest = iter::once(elt3).chain(elts).collect_vec();
            let last = rest.pop().expect("rest contains at least elt3");
            Some(html! {
                : elt1;
                : ", ";
                : elt2;
                @for elt in rest {
                    : ", ";
                    : elt;
                }
                : ", and ";
                : last;
            })
        }
    }
}

pub(crate) fn natjoin_str<T: fmt::Display>(elts: impl IntoIterator<Item = T>) -> Option<String> {
    let mut elts = elts.into_iter().fuse();
    match (elts.next(), elts.next(), elts.next()) {
        (None, _, _) => None,
        (Some(elt), None, _) => Some(elt.to_string()),
        (Some(elt1), Some(elt2), None) => Some(format!("{elt1} and {elt2}")),
        (Some(elt1), Some(elt2), Some(elt3)) => {
            let mut rest = [elt2, elt3].into_iter().chain(elts).collect_vec();
            let last = rest.pop().expect("rest contains at least elt2 and elt3");
            Some(format!("{elt1}, {}, and {last}", rest.into_iter().format(", ")))
        }
    }
}

#[derive(Responder)]
pub(crate) enum RedirectOrContent {
    Redirect(Redirect),
    Content(RawHtml<String>),
}

pub(crate) fn render_form_error(error: &form::Error<'_>) -> RawHtml<String> {
    html! {
        p(class = "error") : error.to_string();
    }
}

pub(crate) fn form_field(name: &str, errors: &mut Vec<&form::Error<'_>>, content: impl ToHtml) -> RawHtml<String> {
    let field_errors;
    (field_errors, *errors) = mem::take(errors).into_iter().partition(|error| error.is_for(name));
    html! {
        fieldset(class? = (!field_errors.is_empty()).then(|| "error")) {
            @for error in field_errors {
                : render_form_error(error);
            }
            : content;
        }
    }
}

#[derive(Responder, From)]
pub(crate) enum StatusOrError<E> {
    Status(Status),
    #[from]
    Err(E),
}

impl From<sqlx::Error> for StatusOrError<PageError> {
    fn from(e: sqlx::Error) -> Self {
        Self::Err(PageError::Sql(e))
    }
}

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

pub(crate) fn favicon(url: &Url) -> RawHtml<String> {
    match url.host_str() {
        Some("challonge.com" | "www.challonge.com") => html! {
            img(class = "favicon", alt = "external link (challonge.com)", srcset = "https://assets.challonge.com/favicon-16x16.png 16w, https://assets.challonge.com/favicon-32x32.png 32w");
        },
        Some("docs.google.com") if url.path_segments().into_iter().flatten().next() == Some("document") => html! {
            img(class = "favicon", alt = "external link (docs.google.com/document)", src = "https://ssl.gstatic.com/docs/documents/images/kix-favicon7.ico");
        },
        Some("docs.google.com") if url.path_segments().into_iter().flatten().next() == Some("forms") => html! {
            img(class = "favicon", alt = "external link (docs.google.com/forms)", srcset = "https://ssl.gstatic.com/docs/spreadsheets/forms/favicon_qp2.png 16w, https://ssl.gstatic.com/docs/forms/device_home/android_192.png 192w");
        },
        Some("docs.google.com") if url.path_segments().into_iter().flatten().next() == Some("spreadsheets") => html! {
            img(class = "favicon", alt = "external link (docs.google.com/spreadsheets)", src = "https://ssl.gstatic.com/docs/spreadsheets/favicon3.ico");
        },
        Some("ootrandomizer.com") => html! {
            img(class = "favicon", alt = "external link (ootrandomizer.com)", src = "https://ootrandomizer.com/img/favicon.ico");
        },
        Some("youtube.com" | "www.youtube.com") => html! {
            img(class = "favicon", alt = "external link (youtube.com)", srcset = "https://www.youtube.com/s/desktop/435d54f2/img/favicon.ico 16w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_32x32.png 32w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_48x48.png 48w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_96x96.png 96w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_144x144.png 144w");
        },
        Some("zeldaspeedruns.com" | "www.zeldaspeedruns.com") => html! {
            img(class = "favicon", alt = "external link (zeldaspeedruns.com)", srcset = "https://www.zeldaspeedruns.com/favicon-16x16.png 16w, https://www.zeldaspeedruns.com/favicon-32x32.png 32w, https://www.zeldaspeedruns.com/favicon-96x96.png 96w, https://www.zeldaspeedruns.com/android-chrome-192x192.png 192w, https://www.zeldaspeedruns.com/favicon-194x194.png 194w");
        },
        Some("racetime.gg") => html! {
            img(class = "favicon", alt = "external link (racetime.gg)", src = static_url("racetimeGG-favicon.svg"));
        },
        Some("start.gg" | "www.start.gg") => html! {
            img(class = "favicon", alt = "external link (start.gg)", src = "https://www.start.gg/__static/images/favicon/favicon.ico");
        },
        Some("twitch.tv" | "www.twitch.tv") => html! {
            img(class = "favicon", alt = "external link (twitch.tv)", srcset = "https://static.twitchcdn.net/assets/favicon-16-52e571ffea063af7a7f4.png 16w, https://static.twitchcdn.net/assets/favicon-32-e29e246c157142c94346.png 32w");
        },
        _ => html! {
            : "🌐";
        },
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

pub(crate) fn io_error_from_reqwest(e: reqwest::Error) -> io::Error {
    io::Error::new(if e.is_timeout() {
        io::ErrorKind::TimedOut
    } else {
        io::ErrorKind::Other
    }, e)
}

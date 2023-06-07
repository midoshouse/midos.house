use {
    std::{
        fmt,
        io,
        iter,
    },
    async_trait::async_trait,
    chrono::prelude::*,
    itertools::Itertools as _,
    rocket::{
        Responder,
        http::Status,
        response::{
            Redirect,
            content::RawHtml,
        },
    },
    rocket_util::{
        ToHtml,
        html,
    },
    serenity::{
        all::MessageBuilder,
        model::prelude::*,
    },
    sqlx::{
        Postgres,
        Transaction,
    },
    url::Url,
    crate::{
        cal::Entrant,
        discord_bot,
        http::static_url,
        team::Team,
        user::User,
    },
};
pub(crate) use self::{
    form::{
        EmptyForm,
        form_field,
        full_form,
    },
    id::{
        Id,
        IdTable,
    },
    time::{
        DateTimeFormat,
        DurationUnit,
        PgIntervalDecodeError,
        decode_pginterval,
        format_date_range,
        format_datetime,
        format_duration,
        parse_duration,
    },
};

mod form;
mod id;
pub(crate) mod sync;
mod time;

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

pub(crate) use as_variant;

#[async_trait]
pub(crate) trait MessageBuilderExt {
    fn mention_command(&mut self, command_id: CommandId, name: &str) -> &mut Self;
    async fn mention_entrant(&mut self, transaction: &mut Transaction<'_, Postgres>, guild: Option<GuildId>, entrant: &Entrant) -> sqlx::Result<&mut Self>;
    async fn mention_team(&mut self, transaction: &mut Transaction<'_, Postgres>, guild: Option<GuildId>, team: &Team) -> sqlx::Result<&mut Self>;
    fn mention_user(&mut self, user: &User) -> &mut Self;
    fn push_timestamp<Z: TimeZone>(&mut self, timestamp: DateTime<Z>, format: discord_bot::TimestampStyle) -> &mut Self;
}

#[async_trait]
impl MessageBuilderExt for MessageBuilder {
    fn mention_command(&mut self, command_id: CommandId, name: &str) -> &mut Self {
        self.push("</").push(name).push(':').push(command_id.to_string()).push('>')
    }

    async fn mention_entrant(&mut self, transaction: &mut Transaction<'_, Postgres>, guild: Option<GuildId>, entrant: &Entrant) -> sqlx::Result<&mut Self> {
        match entrant {
            Entrant::MidosHouseTeam(team) => { self.mention_team(transaction, guild, team).await?; }
            Entrant::Named(name) => { self.push_safe(name); }
        }
        Ok(self)
    }

    async fn mention_team(&mut self, transaction: &mut Transaction<'_, Postgres>, guild: Option<GuildId>, team: &Team) -> sqlx::Result<&mut Self> {
        if let Ok(member) = team.members(&mut *transaction).await?.into_iter().exactly_one() {
            self.mention_user(&member);
        } else {
            let team_role = if let (Some(guild), Some(racetime_slug)) = (guild, &team.racetime_slug) {
                sqlx::query_scalar!(r#"SELECT id AS "id: Id" FROM discord_roles WHERE guild = $1 AND racetime_team = $2"#, i64::from(guild), racetime_slug).fetch_optional(&mut *transaction).await?
            } else {
                None
            };
            if let Some(Id(team_role)) = team_role {
                self.role(team_role);
            } else if let Some(team_name) = team.name(transaction).await? {
                //TODO pothole if racetime slug exists?
                self.push_italic_safe(team_name);
            } else {
                //TODO pothole if racetime slug exists?
                self.push("an unnamed team");
            }
        }
        Ok(self)
    }

    fn mention_user(&mut self, user: &User) -> &mut Self {
        if let Some(ref discord) = user.discord {
            self.mention(&discord.id)
        } else {
            self.push_safe(user.display_name())
        }
    }

    fn push_timestamp<Z: TimeZone>(&mut self, timestamp: DateTime<Z>, format: discord_bot::TimestampStyle) -> &mut Self {
        self.push("<t:")
            .push(timestamp.timestamp().to_string())
            .push(":")
            .push(char::from(format).to_string())
            .push(">")
    }
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

#[derive(Responder)]
pub(crate) enum StatusOrError<E> {
    Status(Status),
    Err(E),
}

pub(crate) fn favicon(url: &Url) -> RawHtml<String> {
    match url.host_str() {
        Some("multistre.am") => html! {
            img(class = "favicon", alt = "external link (multistre.am)", src = static_url!("multistream-favicon.jpg"));
        },
        Some("youtu.be") => html! {
            img(class = "favicon", alt = "external link (youtu.be)", srcset = "https://www.youtube.com/s/desktop/435d54f2/img/favicon.ico 16w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_32x32.png 32w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_48x48.png 48w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_96x96.png 96w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_144x144.png 144w");
        },
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
        Some("ootrandomizer.com" | "league.ootrandomizer.com") => html! {
            img(class = "favicon", alt = "external link (ootrandomizer.com)", src = "https://ootrandomizer.com/img/favicon.ico");
        },
        Some("youtube.com" | "www.youtube.com") => html! {
            img(class = "favicon", alt = "external link (youtube.com)", srcset = "https://www.youtube.com/s/desktop/435d54f2/img/favicon.ico 16w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_32x32.png 32w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_48x48.png 48w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_96x96.png 96w, https://www.youtube.com/s/desktop/435d54f2/img/favicon_144x144.png 144w");
        },
        Some("zeldaspeedruns.com" | "www.zeldaspeedruns.com") => html! {
            img(class = "favicon", alt = "external link (zeldaspeedruns.com)", srcset = "https://www.zeldaspeedruns.com/favicon-16x16.png 16w, https://www.zeldaspeedruns.com/favicon-32x32.png 32w, https://www.zeldaspeedruns.com/favicon-96x96.png 96w, https://www.zeldaspeedruns.com/android-chrome-192x192.png 192w, https://www.zeldaspeedruns.com/favicon-194x194.png 194w");
        },
        Some("racetime.gg") => html! {
            img(class = "favicon", alt = "external link (racetime.gg)", src = static_url!("racetimeGG-favicon.svg"));
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

pub(crate) fn io_error_from_reqwest(e: reqwest::Error) -> io::Error {
    io::Error::new(if e.is_timeout() {
        io::ErrorKind::TimedOut
    } else {
        io::ErrorKind::Other
    }, e)
}

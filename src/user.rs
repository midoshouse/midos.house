use {
    rocket::{
        State,
        http::Status,
        response::content::RawHtml,
        uri,
    },
    rocket_util::{
        ToHtml,
        html,
    },
    sqlx::PgPool,
    crate::{
        PageError,
        PageKind,
        PageStyle,
        page,
        util::{
            Id,
            Origin,
            StatusOrError,
        },
    },
};

/// User preference that determines which external account a user's display name is be based on.
#[derive(sqlx::Type)]
#[sqlx(type_name = "user_display_source", rename_all = "lowercase")]
enum DisplaySource {
    RaceTime,
    Discord,
}

pub(crate) struct User {
    pub(crate) id: Id,
    display_source: DisplaySource, //TODO allow users with both accounts connected to set this in their preferences
    pub(crate) racetime_id: Option<String>,
    pub(crate) racetime_display_name: Option<String>,
    pub(crate) discord_id: Option<Id>,
    pub(crate) discord_display_name: Option<String>,
}

impl User {
    pub(crate) async fn from_id(pool: &PgPool, id: Id) -> sqlx::Result<Option<Self>> {
        sqlx::query_as!(Self, r#"SELECT
            id AS "id: Id",
            display_source AS "display_source: DisplaySource",
            racetime_id,
            racetime_display_name,
            discord_id AS "discord_id: Id",
            discord_display_name
        FROM users WHERE id = $1"#, i64::from(id)).fetch_optional(pool).await
    }

    pub(crate) async fn from_racetime(pool: &PgPool, racetime_id: &str) -> sqlx::Result<Option<Self>> {
        sqlx::query_as!(Self, r#"SELECT
            id AS "id: Id",
            display_source AS "display_source: DisplaySource",
            racetime_id,
            racetime_display_name,
            discord_id AS "discord_id: Id",
            discord_display_name
        FROM users WHERE racetime_id = $1"#, racetime_id).fetch_optional(pool).await
    }

    pub(crate) async fn from_discord(pool: &PgPool, discord_id: u64) -> sqlx::Result<Option<Self>> {
        sqlx::query_as!(Self, r#"SELECT
            id AS "id: Id",
            display_source AS "display_source: DisplaySource",
            racetime_id,
            racetime_display_name,
            discord_id AS "discord_id: Id",
            discord_display_name
        FROM users WHERE discord_id = $1"#, discord_id as i64).fetch_optional(pool).await
    }

    pub(crate) fn display_name(&self) -> &str {
        match self.display_source {
            DisplaySource::RaceTime => self.racetime_display_name.as_ref().expect("user with racetime.gg display preference but no racetime.gg display name"),
            DisplaySource::Discord => self.discord_display_name.as_ref().expect("user with Discord display preference but no Discord display name"),
        }
    }
}

impl ToHtml for User {
    fn to_html(&self) -> RawHtml<String> {
        html! {
            a(href = uri!(profile(self.id)).to_string()) : self.display_name();
        }
    }
}

impl PartialEq for User {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for User {}

#[rocket::get("/user/<id>")]
pub(crate) async fn profile(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, id: Id) -> Result<RawHtml<String>, StatusOrError<PageError>> {
    let user = if let Some(user) = User::from_id(pool, id).await? {
        user
    } else {
        return Err(StatusOrError::Status(Status::NotFound))
    };
    page(pool, &me, &uri, PageStyle { kind: if me.as_ref().map_or(false, |me| *me == user) { PageKind::MyProfile } else { PageKind::Other }, ..PageStyle::default() }, &format!("{} — Mido's House", user.display_name()), html! {
        h1 : user.display_name();
        p {
            : "Mido's House user ID: ";
            code : user.id.0;
        }
        @if let Some(ref racetime_id) = user.racetime_id {
            p {
                : "racetime.gg: ";
                a(href = format!("https://racetime.gg/user/{racetime_id}")) : user.racetime_display_name; //TODO racetime.gg display name with discriminator
            }
        } else if me.as_ref().map_or(false, |me| me.id == user.id) {
            p {
                a(href = uri!(crate::auth::racetime_login(Some(uri!(profile(id))))).to_string()) : "Connect a racetime.gg account";
            }
        }
        @if let Some(Id(discord_id)) = user.discord_id {
            p {
                : "Discord: ";
                a(href = format!("https://discord.com/users/{discord_id}")) : user.discord_display_name; //TODO Discord display name with discriminator
            }
        } else if me.as_ref().map_or(false, |me| me.id == user.id) {
            p {
                a(href = uri!(crate::auth::discord_login(Some(uri!(profile(id))))).to_string()) : "Connect a Discord account";
            }
        }
    }).await.map_err(StatusOrError::Err)
}

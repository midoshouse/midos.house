use {
    rocket::{
        State,
        http::{
            CookieJar,
            Status,
        },
        response::content::RawHtml,
        uri,
    },
    rocket_util::{
        Origin,
        ToHtml,
        html,
    },
    serde::Deserialize,
    serenity::model::prelude::*,
    sqlx::{
        PgExecutor,
        PgPool,
    },
    crate::{
        auth::{
            DiscordUser,
            RaceTimeUser,
        },
        http::{
            PageError,
            PageKind,
            PageStyle,
            page,
        },
        util::{
            Id,
            StatusOrError,
        },
    },
};

/// User preference that determines which external account a user's display name is be based on.
#[derive(Debug, sqlx::Type)]
#[sqlx(type_name = "user_display_source", rename_all = "lowercase")]
enum DisplaySource {
    RaceTime,
    Discord,
}

#[derive(Debug, sqlx::Type, Deserialize)]
#[sqlx(type_name = "racetime_pronouns", rename_all = "snake_case")]
pub(crate) enum RaceTimePronouns {
    #[serde(rename = "she/her")]
    She,
    #[serde(rename = "he/him")]
    He,
    #[serde(rename = "they/them")]
    They,
    #[serde(rename = "she/they")]
    SheThey,
    #[serde(rename = "he/they")]
    HeThey,
    #[serde(rename = "other/ask!")]
    Other,
}

pub(crate) struct User {
    pub(crate) id: Id,
    display_source: DisplaySource, //TODO allow users with both accounts connected to set this in their preferences
    pub(crate) racetime_id: Option<String>,
    pub(crate) racetime_display_name: Option<String>,
    racetime_pronouns: Option<RaceTimePronouns>,
    pub(crate) discord_id: Option<UserId>,
    pub(crate) discord_display_name: Option<String>,
}

impl User {
    pub(crate) async fn from_id(pool: impl PgExecutor<'_>, id: Id) -> sqlx::Result<Option<Self>> {
        Ok(
            sqlx::query!(r#"SELECT
                display_source AS "display_source: DisplaySource",
                racetime_id,
                racetime_display_name,
                racetime_pronouns AS "racetime_pronouns: RaceTimePronouns",
                discord_id AS "discord_id: Id",
                discord_display_name
            FROM users WHERE id = $1"#, i64::from(id)).fetch_optional(pool).await?
                .map(|row| Self {
                    display_source: row.display_source,
                    racetime_id: row.racetime_id,
                    racetime_display_name: row.racetime_display_name,
                    racetime_pronouns: row.racetime_pronouns,
                    discord_id: row.discord_id.map(|Id(id)| id.into()),
                    discord_display_name: row.discord_display_name,
                    id,
                })
        )
    }

    pub(crate) async fn from_racetime(pool: impl PgExecutor<'_>, racetime_id: &str) -> sqlx::Result<Option<Self>> {
        Ok(
            sqlx::query!(r#"SELECT
                id AS "id: Id",
                display_source AS "display_source: DisplaySource",
                racetime_display_name,
                racetime_pronouns AS "racetime_pronouns: RaceTimePronouns",
                discord_id AS "discord_id: Id",
                discord_display_name
            FROM users WHERE racetime_id = $1"#, racetime_id).fetch_optional(pool).await?
                .map(|row| Self {
                    id: row.id,
                    display_source: row.display_source,
                    racetime_id: Some(racetime_id.to_owned()),
                    racetime_display_name: row.racetime_display_name,
                    racetime_pronouns: row.racetime_pronouns,
                    discord_id: row.discord_id.map(|Id(id)| id.into()),
                    discord_display_name: row.discord_display_name,
                })
        )
    }

    pub(crate) async fn from_discord(pool: impl PgExecutor<'_>, discord_id: UserId) -> sqlx::Result<Option<Self>> {
        Ok(
            sqlx::query!(r#"SELECT
                id AS "id: Id",
                display_source AS "display_source: DisplaySource",
                racetime_id,
                racetime_display_name,
                racetime_pronouns AS "racetime_pronouns: RaceTimePronouns",
                discord_display_name
            FROM users WHERE discord_id = $1"#, i64::from(discord_id)).fetch_optional(pool).await?
                .map(|row| Self {
                    id: row.id,
                    display_source: row.display_source,
                    racetime_id: row.racetime_id,
                    racetime_display_name: row.racetime_display_name,
                    racetime_pronouns: row.racetime_pronouns,
                    discord_id: Some(discord_id),
                    discord_display_name: row.discord_display_name,
                })
        )
    }

    pub(crate) fn display_name(&self) -> &str {
        match self.display_source {
            DisplaySource::RaceTime => self.racetime_display_name.as_ref().expect("user with racetime.gg display preference but no racetime.gg display name"),
            DisplaySource::Discord => self.discord_display_name.as_ref().expect("user with Discord display preference but no Discord display name"),
        }
    }

    pub(crate) fn possessive_pronoun(&self) -> &'static str {
        match self.racetime_pronouns {
            Some(RaceTimePronouns::He | RaceTimePronouns::HeThey) => "his",
            Some(RaceTimePronouns::She | RaceTimePronouns::SheThey) => "her",
            Some(RaceTimePronouns::They | RaceTimePronouns::Other) | None => "their",
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
pub(crate) async fn profile(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, client: &State<reqwest::Client>, cookies: &CookieJar<'_>, id: Id) -> Result<RawHtml<String>, StatusOrError<PageError>> {
    let user = if let Some(user) = User::from_id(&**pool, id).await? {
        user
    } else {
        return Err(StatusOrError::Status(Status::NotFound))
    };
    let racetime = if let Some(ref racetime_id) = user.racetime_id {
        html! {
            p {
                : "racetime.gg: ";
                a(href = format!("https://racetime.gg/user/{racetime_id}")) : user.racetime_display_name; //TODO racetime.gg display name with discriminator
            }
        }
    } else if me.as_ref().map_or(false, |me| me.id == user.id) {
        let mut racetime_user = None;
        if let Some(token) = cookies.get_private("racetime_token") {
            if let Ok(response) = client.get("https://racetime.gg/o/userinfo")
                .bearer_auth(token.value())
                .send().await
                .and_then(|response| response.error_for_status())
            {
                if let Ok(user_data) = response.json::<RaceTimeUser>().await {
                    if let Ok(user) = User::from_racetime(&**pool, &user_data.id).await {
                        racetime_user = user;
                    }
                }
            }
        }
        if let Some(racetime_user) = racetime_user {
            let fenhl = User::from_id(&**pool, Id(14571800683221815449)).await?.ok_or(PageError::FenhlUserData)?;
            html! {
                p {
                    : "You are also signed in via racetime.gg as ";
                    a(href = format!("https://racetime.gg/user/{}", racetime_user.racetime_id.expect("racetime.gg user without racetime.gg ID"))) : racetime_user.racetime_display_name; //TODO racetime.gg display name with discriminator
                    : " which belongs to a different Mido's House account. ";
                    @if racetime_user.discord_id.is_some() {
                        : "That Mido's House account is also connected to a Discord account. If you would like to merge your accounts, please contact ";
                        : fenhl;
                        : ".";
                    } else {
                        a(class = "button", href = uri!(crate::auth::merge_accounts).to_string()) : "Merge Accounts";
                    }
                }
            }
        } else {
            html! {
                p {
                    a(href = uri!(crate::auth::racetime_login(Some(uri!(profile(id))))).to_string()) : "Connect a racetime.gg account";
                }
            }
        }
    } else {
        html! {}
    };
    let discord = if let Some(ref discord_id) = user.discord_id {
        html! {
            p {
                : "Discord: ";
                a(href = format!("https://discord.com/users/{discord_id}")) : user.discord_display_name; //TODO Discord display name with discriminator
            }
        }
    } else if me.as_ref().map_or(false, |me| me.id == user.id) {
        let mut discord_user = None;
        if let Some(token) = cookies.get_private("discord_token") {
            if let Ok(response) = client.get("https://discord.com/api/v9/users/@me")
                .bearer_auth(token.value())
                .send().await
                .and_then(|response| response.error_for_status())
            {
                if let Ok(user_data) = response.json::<DiscordUser>().await {
                    if let Ok(user) = User::from_discord(&**pool, user_data.id).await {
                        discord_user = user;
                    }
                }
            }
        }
        if let Some(discord_user) = discord_user {
            let fenhl = User::from_id(&**pool, Id(14571800683221815449)).await?.ok_or(PageError::FenhlUserData)?;
            html! {
                p {
                    : "You are also signed in via Discord as ";
                    a(href = format!("https://discord.com/users/{}", discord_user.discord_id.expect("Discord user without Discord ID"))) : discord_user.discord_display_name; //TODO Discord display name with discriminator
                    : " which belongs to a different Mido's House account. ";
                    @if discord_user.racetime_id.is_some() {
                        : "That Mido's House account is also connected to a raceitme.gg account. If you would like to merge your accounts, please contact ";
                        : fenhl;
                        : ".";
                    } else {
                        a(class = "button", href = uri!(crate::auth::merge_accounts).to_string()) : "Merge Accounts";
                    }
                }
            }
        } else {
            html! {
                p {
                    a(href = uri!(crate::auth::discord_login(Some(uri!(profile(id))))).to_string()) : "Connect a Discord account";
                }
            }
        }
    } else {
        html! {}
    };
    page(pool, &me, &uri, PageStyle { kind: if me.as_ref().map_or(false, |me| *me == user) { PageKind::MyProfile } else { PageKind::Other }, ..PageStyle::default() }, &format!("{} — Mido's House", user.display_name()), html! {
        h1 : user.display_name();
        p {
            : "Mido's House user ID: ";
            code : user.id.0;
        }
        : racetime;
        : discord;
    }).await.map_err(StatusOrError::Err)
}

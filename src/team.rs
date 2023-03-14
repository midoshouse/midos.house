use {
    std::fmt,
    rocket::response::content::RawHtml,
    rocket_util::html,
    serenity::model::prelude::*,
    sqlx::{
        Postgres,
        Transaction,
    },
    crate::{
        event::Series,
        user::User,
        util::Id,
    },
};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Team {
    pub(crate) id: Id,
    pub(crate) name: Option<String>,
    pub(crate) racetime_slug: Option<String>,
    pub(crate) plural_name: Option<bool>,
}

impl Team {
    pub(crate) async fn from_id(transaction: &mut Transaction<'_, Postgres>, id: Id) -> sqlx::Result<Option<Self>> {
        sqlx::query_as!(Self, r#"SELECT id AS "id: Id", name, racetime_slug, plural_name FROM teams WHERE id = $1"#, i64::from(id)).fetch_optional(transaction).await
    }

    pub(crate) async fn from_discord(transaction: &mut Transaction<'_, Postgres>, discord_role: RoleId) -> sqlx::Result<Option<Self>> {
        sqlx::query_as!(Self, r#"SELECT teams.id AS "id: Id", name, racetime_slug, plural_name FROM teams, discord_roles WHERE discord_roles.id = $1 AND racetime_slug = racetime_team"#, i64::from(discord_role)).fetch_optional(transaction).await
    }

    pub(crate) async fn from_racetime(transaction: &mut Transaction<'_, Postgres>, series: Series, event: &str, racetime_slug: &str) -> sqlx::Result<Option<Self>> {
        sqlx::query_as!(Self, r#"SELECT id AS "id: Id", name, racetime_slug, plural_name FROM teams WHERE series = $1 AND event = $2 AND racetime_slug = $3"#, series as _, event, racetime_slug).fetch_optional(transaction).await
    }

    pub(crate) async fn from_startgg(transaction: &mut Transaction<'_, Postgres>, startgg_id: &str) -> sqlx::Result<Option<Self>> {
        sqlx::query_as!(Self, r#"SELECT id AS "id: Id", name, racetime_slug, plural_name FROM teams WHERE startgg_id = $1"#, startgg_id).fetch_optional(transaction).await
    }

    pub(crate) async fn for_event(transaction: &mut Transaction<'_, Postgres>, series: Series, event: &str) -> sqlx::Result<Vec<Self>> {
        sqlx::query_as!(Self, r#"SELECT id AS "id: Id", name, racetime_slug, plural_name FROM teams WHERE series = $1 AND event = $2"#, series as _, event).fetch_all(transaction).await
    }

    pub(crate) fn name_is_plural(&self) -> bool {
        self.plural_name.unwrap_or(false)
    }

    pub(crate) fn to_html(&self, running_text: bool) -> RawHtml<String> {
        let inner = html! {
            @if let Some(ref name) = self.name {
                @if running_text {
                    i : name;
                } else {
                    : name;
                }
            } else {
                @if running_text {
                    : "an unnamed team";
                } else {
                    i : "(unnamed)";
                }
            }
        };
        html! {
            @if let Some(ref racetime_slug) = self.racetime_slug {
                a(href = format!("https://racetime.gg/team/{racetime_slug}")) : inner;
            } else {
                : inner;
            }
        }
    }

    async fn member_ids(&self, transaction: &mut Transaction<'_, Postgres>) -> sqlx::Result<Vec<Id>> {
        sqlx::query_scalar!(r#"SELECT member AS "member: Id" FROM team_members WHERE team = $1"#, i64::from(self.id)).fetch_all(&mut *transaction).await
    }

    pub(crate) async fn members(&self, transaction: &mut Transaction<'_, Postgres>) -> sqlx::Result<Vec<User>> {
        let user_ids = self.member_ids(&mut *transaction).await?;
        let mut members = Vec::with_capacity(user_ids.len());
        for user_id in user_ids {
            members.push(User::from_id(&mut *transaction, user_id).await?.expect("database constraint violated: nonexistent team member"));
        }
        Ok(members)
    }
}

impl fmt::Display for Team {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref name) = self.name {
            name.fmt(f)
        } else {
            write!(f, "(unnamed)")
        }
    }
}

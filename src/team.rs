use {
    std::borrow::Cow,
    itertools::Itertools as _,
    rocket::response::content::RawHtml,
    rocket_util::{
        ToHtml as _,
        html,
    },
    serenity::model::prelude::*,
    sqlx::{
        Postgres,
        Transaction,
    },
    crate::{
        Environment,
        event::{
            Role,
            Series,
        },
        user::User,
        util::Id,
    },
};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Team {
    pub(crate) name: Option<String>,
    pub(crate) id: Id,
    pub(crate) racetime_slug: Option<String>,
    pub(crate) plural_name: Option<bool>,
    pub(crate) restream_consent: bool,
}

impl Team {
    pub(crate) async fn from_id(transaction: &mut Transaction<'_, Postgres>, id: Id) -> sqlx::Result<Option<Self>> {
        sqlx::query_as!(Self, r#"SELECT id AS "id: Id", name, racetime_slug, plural_name, restream_consent FROM teams WHERE id = $1"#, id as _).fetch_optional(&mut **transaction).await
    }

    pub(crate) async fn from_discord(transaction: &mut Transaction<'_, Postgres>, discord_role: RoleId) -> sqlx::Result<Option<Self>> {
        sqlx::query_as!(Self, r#"SELECT teams.id AS "id: Id", name, racetime_slug, plural_name, restream_consent FROM teams, discord_roles WHERE discord_roles.id = $1 AND racetime_slug = racetime_team"#, i64::from(discord_role)).fetch_optional(&mut **transaction).await
    }

    pub(crate) async fn from_racetime(transaction: &mut Transaction<'_, Postgres>, series: Series, event: &str, racetime_slug: &str) -> sqlx::Result<Option<Self>> {
        sqlx::query_as!(Self, r#"SELECT id AS "id: Id", name, racetime_slug, plural_name, restream_consent FROM teams WHERE series = $1 AND event = $2 AND racetime_slug = $3"#, series as _, event, racetime_slug).fetch_optional(&mut **transaction).await
    }

    pub(crate) async fn from_startgg(transaction: &mut Transaction<'_, Postgres>, startgg_id: &str) -> sqlx::Result<Option<Self>> {
        sqlx::query_as!(Self, r#"SELECT id AS "id: Id", name, racetime_slug, plural_name, restream_consent FROM teams WHERE startgg_id = $1"#, startgg_id).fetch_optional(&mut **transaction).await
    }

    pub(crate) async fn for_event(transaction: &mut Transaction<'_, Postgres>, series: Series, event: &str) -> sqlx::Result<Vec<Self>> {
        sqlx::query_as!(Self, r#"SELECT id AS "id: Id", name, racetime_slug, plural_name, restream_consent FROM teams WHERE series = $1 AND event = $2 AND NOT resigned"#, series as _, event).fetch_all(&mut **transaction).await
    }

    pub(crate) async fn name(&self, transaction: &mut Transaction<'_, Postgres>) -> sqlx::Result<Option<Cow<'_, str>>> {
        Ok(if let Some(ref name) = self.name {
            Some(Cow::Borrowed(name))
        } else if let Ok(member) = self.members(transaction).await?.into_iter().exactly_one() {
            Some(Cow::Owned(member.display_name().to_owned()))
        } else {
            None
        })
    }

    pub(crate) fn name_is_plural(&self) -> bool {
        self.plural_name.unwrap_or(false)
    }

    pub(crate) async fn possessive_determiner(&self, transaction: &mut Transaction<'_, Postgres>) -> sqlx::Result<&'static str> {
        Ok(if let Ok(member) = self.members(transaction).await?.into_iter().exactly_one() {
            member.possessive_determiner()
        } else {
            "their"
        })
    }

    pub(crate) async fn to_html(&self, transaction: &mut Transaction<'_, Postgres>, env: Environment, running_text: bool) -> sqlx::Result<RawHtml<String>> {
        Ok(if let Ok(member) = self.members(transaction).await?.into_iter().exactly_one() {
            member.to_html()
        } else {
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
                    a(href = format!("https://{}/team/{racetime_slug}", env.racetime_host())) : inner;
                } else {
                    : inner;
                }
            }
        })
    }

    async fn member_ids(&self, transaction: &mut Transaction<'_, Postgres>) -> sqlx::Result<Vec<Id>> {
        sqlx::query_scalar!(r#"SELECT member AS "member: Id" FROM team_members WHERE team = $1"#, self.id as _).fetch_all(&mut **transaction).await
    }

    async fn member_ids_roles(&self, transaction: &mut Transaction<'_, Postgres>) -> sqlx::Result<Vec<(Id, Role)>> {
        Ok(
            sqlx::query!(r#"SELECT member AS "member: Id", role AS "role: Role" FROM team_members WHERE team = $1"#, self.id as _).fetch_all(&mut **transaction).await?
                .into_iter()
                .map(|row| (row.member, row.role))
                .collect()
        )
    }

    pub(crate) async fn members(&self, transaction: &mut Transaction<'_, Postgres>) -> sqlx::Result<Vec<User>> {
        let user_ids = self.member_ids(&mut *transaction).await?;
        let mut members = Vec::with_capacity(user_ids.len());
        for user_id in user_ids {
            members.push(User::from_id(&mut **transaction, user_id).await?.expect("database constraint violated: nonexistent team member"));
        }
        Ok(members)
    }

    pub(crate) async fn members_roles(&self, transaction: &mut Transaction<'_, Postgres>) -> sqlx::Result<Vec<(User, Role)>> {
        let rows = self.member_ids_roles(&mut *transaction).await?;
        let mut members = Vec::with_capacity(rows.len());
        for (user_id, role) in rows {
            members.push((User::from_id(&mut **transaction, user_id).await?.expect("database constraint violated: nonexistent team member"), role));
        }
        Ok(members)
    }
}

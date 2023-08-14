use {
    chrono::prelude::*,
    rocket::response::content::RawHtml,
    rocket_util::html,
    serde::{
        Deserialize,
        Deserializer,
        de::Error as _,
    },
    serenity::model::prelude::*,
    sqlx::{
        Postgres,
        Transaction,
    },
    crate::{
        cal::Entrant,
        event::{
            Data,
            InfoError,
        },
        lang::Language::English,
    },
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(Some(html! {
        article {
            p {
                : "This is OoTR League season ";
                : data.event;
                : ", organized by shaun1e, ";
                : English.join_html(data.organizers(transaction).await?);
                : ". See ";
                a(href = "https://league.ootrandomizer.com/") : "the official website";
                : " for details.";
            }
        }
    }))
}

#[derive(Deserialize)]
#[serde(transparent)]
struct JsonScheduleVersion(u8);

#[derive(Debug, thiserror::Error)]
#[error("expected League schedule format version 1, got version {0}")]
struct ScheduleVersionMismatch(u8);

impl TryFrom<JsonScheduleVersion> for ScheduleVersion {
    type Error = ScheduleVersionMismatch;

    fn try_from(JsonScheduleVersion(version): JsonScheduleVersion) -> Result<Self, ScheduleVersionMismatch> {
        if version == 1 {
            Ok(Self)
        } else {
            Err(ScheduleVersionMismatch(version))
        }
    }
}

#[derive(Deserialize)]
#[serde(try_from = "JsonScheduleVersion")]
struct ScheduleVersion;

#[derive(Deserialize)]
pub(crate) struct Schedule {
    #[allow(unused)] // version check
    version: ScheduleVersion,
    pub(crate) matches: Vec<Match>,
}

fn deserialize_datetime<'de, D: Deserializer<'de>>(deserializer: D) -> Result<DateTime<Utc>, D::Error> {
    // workaround for https://github.com/chronotope/chrono/issues/330
    Ok(Utc.datetime_from_str(&format!("{}:00", <&str>::deserialize(deserializer)?), "%Y-%m-%d %H:%M:%S").map_err(D::Error::custom)?)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Match {
    #[serde(rename = "timeUTC", deserialize_with = "deserialize_datetime")]
    pub(crate) time_utc: DateTime<Utc>,
    pub(crate) player_a: User,
    pub(crate) player_b: User,
    pub(crate) division: String,
    pub(crate) status: MatchStatus,
    pub(crate) restreamers: Vec<User>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct User {
    pub(crate) discord_id: Option<UserId>,
    pub(crate) username: String,
    pub(crate) twitch_username: Option<String>,
}

impl User {
    pub(crate) fn into_entrant(self) -> Entrant {
        match (self.discord_id, self.twitch_username) {
            (None, None) => Entrant::Named(self.username),
            (None, Some(twitch_username)) => Entrant::NamedWithTwitch(self.username, twitch_username),
            (Some(discord_id), None) => Entrant::Discord(discord_id),
            (Some(discord_id), Some(twitch_username)) => Entrant::DiscordTwitch(discord_id, twitch_username),
        }
    }
}

#[derive(Deserialize)]
pub(crate) enum MatchStatus {
    Canceled,
    Confirmed,
}

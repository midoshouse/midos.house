use {
    chrono::prelude::*,
    rocket::response::content::RawHtml,
    rocket_util::html,
    serde::{
        Deserialize,
        Deserializer,
        de::Error as _,
    },
    sqlx::{
        Postgres,
        Transaction,
    },
    crate::{
        event::{
            Data,
            InfoError,
        },
        util::natjoin_html,
    },
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "4" => Some(html! {
            article {
                p {
                    : "This is OoTR League season 4, organized by shaun1e, ";
                    : natjoin_html(data.organizers(transaction).await?);
                    : ". See ";
                    a(href = "https://league.ootrandomizer.com/") : "the official website";
                    : " for details.";
                }
            }
        }),
        _ => None,
    })
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
    Ok(DateTime::parse_from_str(<_>::deserialize(deserializer)?, "%Y-%m-%d %H:%M").map_err(D::Error::custom)?.with_timezone(&Utc))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Match {
    #[serde(rename = "timeUTC", deserialize_with = "deserialize_datetime")]
    pub(crate) time_utc: DateTime<Utc>,
    pub(crate) player_a: User,
    pub(crate) player_b: User,
    pub(crate) division: String,
    #[allow(unused)] // only one variant
    status: MatchStatus,
    pub(crate) restreamers: Vec<User>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct User {
    pub(crate) username: String,
    pub(crate) twitch_username: String,
}

#[derive(Deserialize)]
enum MatchStatus {
    Confirmed,
}

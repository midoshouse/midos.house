//! Utilities for working with Google Sheets.

// This module is only used for events which publish their race schedules as Google sheets.
// Allow it to remain unused between those events rather than deleting and restoring it each time.
#![allow(unused)]

use {
    yup_oauth2::{
        ServiceAccountAuthenticator,
        read_service_account_key,
    },
    crate::{
        cal::Source,
        prelude::*,
    },
};

/// from <https://developers.google.com/sheets/api/limits#quota>:
///
/// > Read requests […] Per minute per user per project […] 60
const RATE_LIMIT: Duration = Duration::from_secs(1);

static CACHE: LazyLock<Mutex<(Instant, HashMap<(String, String), (Instant, Vec<Vec<String>>)>)>> = LazyLock::new(|| Mutex::new((Instant::now() + RATE_LIMIT, HashMap::default())));

#[derive(Debug, thiserror::Error)]
enum UncachedError {
    #[error(transparent)] OAuth(#[from] yup_oauth2::Error),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[error("empty token is not valid")]
    EmptyToken,
    #[error("OAuth token is expired")]
    TokenExpired,
}

impl IsNetworkError for UncachedError {
    fn is_network_error(&self) -> bool {
        match self {
            Self::OAuth(_) => false,
            Self::Reqwest(e) => e.is_network_error(),
            Self::Wheel(e) => e.is_network_error(),
            Self::EmptyToken => false,
            Self::TokenExpired => false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{source}")]
pub(crate) struct Error {
    source: UncachedError,
    cache: CacheMissReason,
}

impl IsNetworkError for Error {
    fn is_network_error(&self) -> bool {
        self.source.is_network_error()
    }
}

#[derive(Debug)]
enum CacheMissReason {
    Elapsed,
    Vacant,
}

pub(crate) async fn values(http_client: reqwest::Client, sheet_id: &str, range: &str) -> Result<Vec<Vec<String>>, Error> {
    #[derive(Deserialize)]
    struct ValueRange {
        values: Vec<Vec<String>>,
    }

    async fn values_uncached(http_client: &reqwest::Client, sheet_id: &str, range: &str, next_request: &mut Instant) -> Result<Vec<Vec<String>>, UncachedError> {
        sleep_until(*next_request).await;
        let gsuite_secret = read_service_account_key("assets/google-client-secret.json").await.at("assets/google-client-secret.json")?;
        let auth = ServiceAccountAuthenticator::builder(gsuite_secret)
            .build().await.at_unknown()?;
        let token = auth.token(&["https://www.googleapis.com/auth/spreadsheets"]).await?;
        if token.is_expired() { return Err(UncachedError::TokenExpired) }
        let Some(token) = token.token() else { return Err(UncachedError::EmptyToken) };
        if token.is_empty() { return Err(UncachedError::EmptyToken) }
        let ValueRange { values } = http_client.get(&format!("https://sheets.googleapis.com/v4/spreadsheets/{sheet_id}/values/{range}"))
            .bearer_auth(token)
            .query(&[
                ("valueRenderOption", "FORMATTED_VALUE"),
                ("dateTimeRenderOption", "FORMATTED_STRING"),
                ("majorDimension", "ROWS"),
            ])
            .send().await?
            .detailed_error_for_status().await?
            .json_with_text_in_error::<ValueRange>().await?;
        *next_request = Instant::now() + RATE_LIMIT;
        Ok(values)
    }

    let key = (sheet_id.to_owned(), range.to_owned());
    lock!(cache = CACHE; {
        let (ref mut next_request, ref mut cache) = *cache;
        Ok(match cache.entry(key) {
            hash_map::Entry::Occupied(mut entry) => {
                let (retrieved, values) = entry.get();
                if retrieved.elapsed() < Duration::from_secs(5 * 60) {
                    values.clone()
                } else {
                    match values_uncached(&http_client, sheet_id, range, next_request).await {
                        Ok(values) => {
                            entry.insert((Instant::now(), values.clone()));
                            values
                        }
                        Err(e) if e.is_network_error() && retrieved.elapsed() < Duration::from_secs(60 * 60) => values.clone(),
                        Err(source) => return Err(Error { cache: CacheMissReason::Elapsed, source }),
                    }
                }
            }
            hash_map::Entry::Vacant(entry) => {
                let values = values_uncached(&http_client, sheet_id, range, next_request).await.map_err(|source| Error { cache: CacheMissReason::Vacant, source })?;
                entry.insert((Instant::now(), values.clone()));
                values
            }
        })
    })
}

async fn update_race(transaction: &mut Transaction<'_, Postgres>, found_race: &mut Race, race: Race) -> sqlx::Result<()> {
    if !found_race.schedule.start_matches(&race.schedule) {
        match race.schedule {
            RaceSchedule::Unscheduled => found_race.schedule = RaceSchedule::Unscheduled,
            RaceSchedule::Live { start, .. } => match found_race.schedule {
                RaceSchedule::Unscheduled => found_race.schedule = race.schedule,
                RaceSchedule::Live { start: ref mut old_start, .. } => *old_start = start,
                RaceSchedule::Async { .. } => unimplemented!("race listed as async in database was rescheduled as live"), //TODO
            },
            RaceSchedule::Async { start1, start2, start3, .. } => match found_race.schedule {
                RaceSchedule::Unscheduled => found_race.schedule = race.schedule,
                RaceSchedule::Live { .. } => unimplemented!("race listed as live in database was rescheduled as async"), //TODO
                RaceSchedule::Async { start1: ref mut old_start1, start2: ref mut old_start2, start3: ref mut old_start3, .. } => {
                    *old_start1 = start1;
                    *old_start2 = start2;
                    *old_start3 = start3;
                }
            },
        }
    }
    if race.video_urls.iter().any(|(language, new_url)| found_race.video_urls.get(language).is_none_or(|old_url| old_url != new_url)) {
        if found_race.video_urls.iter().all(|(language, old_url)| race.video_urls.get(language).is_none_or(|new_url| old_url == new_url)) { //TODO make sure manually entered restreams aren't changed automatically, then remove this condition
            for language in all() {
                if let Some(url) = race.video_urls.get(&language) {
                    found_race.video_urls.insert(language, url.clone());
                }
            }
        }
    }
    if race.restreamers.iter().any(|(language, new_restreamer)| found_race.restreamers.get(language).is_none_or(|old_restreamer| old_restreamer != new_restreamer)) {
        if found_race.restreamers.iter().all(|(language, old_restreamer)| race.restreamers.get(language).is_none_or(|new_restreamer| old_restreamer == new_restreamer)) { //TODO make sure manually entered restreams aren't changed automatically, then remove this condition
            for language in all() {
                if let Some(restreamer) = race.restreamers.get(&language) {
                    found_race.restreamers.insert(language, restreamer.clone());
                }
            }
        }
    }
    Ok(())
}

pub(crate) async fn add_or_update_race(transaction: &mut Transaction<'_, Postgres>, races: &mut Vec<Race>, race: Race) -> sqlx::Result<()> {
    if let Some(found_race) = races.iter_mut().find(|iter_race| match &race.source {
        Source::Challonge { id } => if let Source::Challonge { id: iter_id } = &iter_race.source { iter_id == id } else { false },
        Source::League { id } => if let Source::League { id: iter_id } = &iter_race.source { iter_id == id } else { false },
        Source::StartGG { event, set } => if let Source::StartGG { event: iter_event, set: iter_set } = &iter_race.source { iter_event == event && iter_set == set } else { false },
        Source::SpeedGamingOnline { id } => if let Source::SpeedGamingOnline { id: iter_id } = &iter_race.source { iter_id == id } else { false },
        Source::SpeedGamingInPerson { id } => if let Source::SpeedGamingInPerson { id: iter_id } = &iter_race.source { iter_id == id } else { false },
        Source::Sheet { .. } | Source::Manual =>
            matches!(iter_race.source, Source::Sheet { .. } | Source::Manual)
            && iter_race.series == race.series
            && iter_race.event == race.event
            && iter_race.phase == race.phase
            && iter_race.round == race.round
            && iter_race.game == race.game
            && iter_race.entrants == race.entrants
            && !iter_race.schedule_locked,
    }) {
        update_race(transaction, found_race, race).await?;
    } else {
        // add race to database
        race.save(transaction).await?;
        races.push(race);
    }
    Ok(())
}

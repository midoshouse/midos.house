//! Utilities for working with Google Sheets.

// This module is only used for events which publish their race schedules as Google sheets.
// Allow it to remain unused between those events rather than deleting and restoring it each time.
#![allow(unused)]

use {
    yup_oauth2::{
        ServiceAccountAuthenticator,
        read_service_account_key,
    },
    crate::prelude::*,
};

static CACHE: LazyLock<Mutex<(Instant, HashMap<(String, String), (Instant, Vec<Vec<String>>)>)>> = LazyLock::new(|| Mutex::new((Instant::now(), HashMap::default())));

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
        // from https://developers.google.com/sheets/api/limits#quota
        // “Read requests […] Per minute per user per project […] 60”
        *next_request = Instant::now() + Duration::from_secs(1);
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

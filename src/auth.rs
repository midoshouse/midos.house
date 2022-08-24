use {
    std::{
        collections::HashMap,
        time::Duration,
    },
    rocket::{
        State,
        http::{
            Cookie,
            CookieJar,
            SameSite,
            Status,
            ext::IntoOwned as _,
        },
        outcome::Outcome,
        request::{
            self,
            FromRequest,
            Request,
        },
        response::Redirect,
        uri,
    },
    rocket_oauth2::{
        OAuth2,
        TokenResponse,
    },
    rocket_util::{
        Error,
        Origin,
        html,
    },
    serde::Deserialize,
    serenity::model::prelude::*,
    sqlx::PgPool,
    crate::{
        http::{
            PageError,
            PageKind,
            PageResult,
            PageStyle,
            page,
        },
        user::{
            RaceTimePronouns,
            User,
        },
        util::{
            Id,
            IdTable,
            RedirectOrContent,
        },
    },
};

macro_rules! guard_try {
    ($res:expr) => {
        match $res {
            Ok(x) => x,
            Err(e) => return Outcome::Failure((Status::InternalServerError, e.into())),
        }
    };
}

pub(crate) struct ViewAs(pub(crate) HashMap<Id, Id>);

pub(crate) enum RaceTime {}
pub(crate) enum Discord {}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum UserFromRequestError {
    #[error(transparent)] OAuth(#[from] rocket_oauth2::Error),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Time(#[from] rocket::time::error::ConversionRange),
    #[error(transparent)] TryFromInt(#[from] std::num::TryFromIntError),
    #[error("neither racetime_token cookie nor discord_token cookie present")]
    Cookie,
    #[error("missing database connection")]
    Database,
    #[error("missing HTTP client")]
    HttpClient,
    #[error("missing view-as map")]
    ViewAs,
    #[error("user to view as does not exist")]
    ViewAsNoSuchUser,
}

async fn handle_racetime_token_response(client: &reqwest::Client, cookies: &CookieJar<'_>, token: &TokenResponse<RaceTime>) -> Result<RaceTimeUser, UserFromRequestError> {
    let mut cookie = Cookie::build("racetime_token", token.access_token().to_owned())
        .same_site(SameSite::Lax);
    if let Some(expires_in) = token.expires_in() {
        cookie = cookie.max_age(Duration::from_secs(u64::try_from(expires_in)?.saturating_sub(60)).try_into()?);
    }
    cookies.add_private(cookie.finish());
    if let Some(refresh_token) = token.refresh_token() {
        cookies.add_private(Cookie::build("racetime_refresh_token", refresh_token.to_owned())
            .same_site(SameSite::Lax)
            .finish());
    }
    Ok(client.get("https://racetime.gg/o/userinfo")
        .bearer_auth(token.access_token())
        .send().await?
        .error_for_status()?
        .json().await?)
}

async fn handle_discord_token_response(client: &reqwest::Client, cookies: &CookieJar<'_>, token: &TokenResponse<Discord>) -> Result<DiscordUser, UserFromRequestError> {
    let mut cookie = Cookie::build("discord_token", token.access_token().to_owned())
        .same_site(SameSite::Lax);
    if let Some(expires_in) = token.expires_in() {
        cookie = cookie.max_age(Duration::from_secs(u64::try_from(expires_in)?.saturating_sub(60)).try_into()?);
    }
    cookies.add_private(cookie.finish());
    if let Some(refresh_token) = token.refresh_token() {
        cookies.add_private(Cookie::build("discord_refresh_token", refresh_token.to_owned())
            .same_site(SameSite::Lax)
            .finish());
    }
    Ok(client.get("https://discord.com/api/v9/users/@me")
        .bearer_auth(token.access_token())
        .send().await?
        .error_for_status()?
        .json().await?)
}

#[derive(Deserialize)]
pub(crate) struct RaceTimeUser {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) discriminator: Option<String>,
    pronouns: Option<RaceTimePronouns>,
}

#[derive(Deserialize)]
pub(crate) struct DiscordUser {
    pub(crate) id: UserId,
    pub(crate) username: String,
    pub(crate) discriminator: String,
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for RaceTimeUser {
    type Error = UserFromRequestError;

    async fn from_request(req: &'r Request<'_>) -> request::Outcome<Self, UserFromRequestError> {
        match req.guard::<&CookieJar<'_>>().await {
            Outcome::Success(cookies) => match req.guard::<&State<reqwest::Client>>().await {
                Outcome::Success(client) => if let Some(token) = cookies.get_private("racetime_token") {
                    match client.get("https://racetime.gg/o/userinfo")
                        .bearer_auth(token.value())
                        .send().await
                        .and_then(|response| response.error_for_status())
                    {
                        Ok(response) => Outcome::Success(guard_try!(response.json().await)),
                        Err(e) => Outcome::Failure((Status::BadGateway, e.into())),
                    }
                } else if let Some(token) = cookies.get_private("racetime_refresh_token") {
                    match req.guard::<OAuth2<RaceTime>>().await {
                        Outcome::Success(oauth) => Outcome::Success(guard_try!(handle_racetime_token_response(client, cookies, &guard_try!(oauth.refresh(token.value()).await)).await)),
                        Outcome::Failure((status, ())) => Outcome::Failure((status, UserFromRequestError::Cookie)),
                        Outcome::Forward(()) => Outcome::Forward(()),
                    }
                } else {
                    Outcome::Failure((Status::Unauthorized, UserFromRequestError::Cookie))
                },
                Outcome::Failure((status, ())) => Outcome::Failure((status, UserFromRequestError::HttpClient)),
                Outcome::Forward(()) => Outcome::Forward(()),
            },
            Outcome::Failure((_, never)) => match never {},
            Outcome::Forward(()) => Outcome::Forward(()),
        }
    }
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for DiscordUser {
    type Error = UserFromRequestError;

    async fn from_request(req: &'r Request<'_>) -> request::Outcome<Self, UserFromRequestError> {
        match req.guard::<&CookieJar<'_>>().await {
            Outcome::Success(cookies) => match req.guard::<&State<reqwest::Client>>().await {
                Outcome::Success(client) => if let Some(token) = cookies.get_private("discord_token") {
                    match client.get("https://discord.com/api/v9/users/@me")
                        .bearer_auth(token.value())
                        .send().await
                        .and_then(|response| response.error_for_status())
                    {
                        Ok(response) => Outcome::Success(guard_try!(response.json().await)),
                        Err(e) => Outcome::Failure((Status::BadGateway, e.into())),
                    }
                } else if let Some(token) = cookies.get_private("discord_refresh_token") {
                    match req.guard::<OAuth2<Discord>>().await {
                        Outcome::Success(oauth) => Outcome::Success(guard_try!(handle_discord_token_response(client, cookies, &guard_try!(oauth.refresh(token.value()).await)).await)),
                        Outcome::Failure((status, ())) => Outcome::Failure((status, UserFromRequestError::Cookie)),
                        Outcome::Forward(()) => Outcome::Forward(()),
                    }
                } else {
                    Outcome::Failure((Status::Unauthorized, UserFromRequestError::Cookie))
                },
                Outcome::Failure((status, ())) => Outcome::Failure((status, UserFromRequestError::HttpClient)),
                Outcome::Forward(()) => Outcome::Forward(()),
            },
            Outcome::Failure((_, never)) => match never {},
            Outcome::Forward(()) => Outcome::Forward(()),
        }
    }
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for User {
    type Error = UserFromRequestError;

    async fn from_request(req: &'r Request<'_>) -> request::Outcome<Self, UserFromRequestError> {
        match req.guard::<&State<PgPool>>().await {
            Outcome::Success(pool) => match req.guard::<&State<ViewAs>>().await {
                Outcome::Success(view_as) => {
                    let mut found_user = Err((Status::Unauthorized, UserFromRequestError::Cookie));
                    match req.guard::<RaceTimeUser>().await {
                        Outcome::Success(racetime_user) => if let Some(user) = guard_try!(User::from_racetime(&**pool, &racetime_user.id).await) {
                            guard_try!(sqlx::query!("UPDATE users SET racetime_display_name = $1, racetime_pronouns = $2 WHERE id = $3", racetime_user.name, racetime_user.pronouns as _, i64::from(user.id)).execute(&**pool).await);
                            found_user = found_user.or(Ok(user));
                        },
                        Outcome::Forward(()) => {}
                        Outcome::Failure(e) => found_user = found_user.or(Err(e)),
                    }
                    match req.guard::<DiscordUser>().await {
                        Outcome::Success(discord_user) => if let Some(user) = guard_try!(User::from_discord(&**pool, discord_user.id).await) {
                            guard_try!(sqlx::query!("UPDATE users SET discord_display_name = $1 WHERE id = $2", discord_user.username, i64::from(user.id)).execute(&**pool).await);
                            found_user = found_user.or(Ok(user));
                        },
                        Outcome::Forward(()) => {},
                        Outcome::Failure(e) => found_user = found_user.or(Err(e)),
                    };
                    match found_user {
                        Ok(user) => if let Some(&user_id) = view_as.inner().0.get(&user.id) {
                            if let Some(user) = guard_try!(User::from_id(&**pool, user_id).await) {
                                Outcome::Success(user)
                            } else {
                                Outcome::Failure((Status::InternalServerError, UserFromRequestError::ViewAsNoSuchUser))
                            }
                        } else {
                            Outcome::Success(user)
                        },
                        Err(e) => Outcome::Failure(e),
                    }
                },
                Outcome::Failure((status, ())) => Outcome::Failure((status, UserFromRequestError::ViewAs)),
                Outcome::Forward(()) => Outcome::Forward(()),
            },
            Outcome::Failure((status, ())) => Outcome::Failure((status, UserFromRequestError::Database)),
            Outcome::Forward(()) => Outcome::Forward(()),
        }
    }
}

#[rocket::get("/login?<redirect_to>")]
pub(crate) async fn login(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, redirect_to: Option<Origin<'_>>) -> PageResult {
    page(pool, &me, &uri, PageStyle { kind: PageKind::Login, ..PageStyle::default() }, "Login — Mido's House", if let Some(ref me) = me {
        html! {
            p {
                : "You are already signed in as ";
                : me;
                : ".";
            }
            ul {
                @if me.racetime_id.is_none() {
                    li {
                        a(href = uri!(racetime_login(redirect_to.clone())).to_string()) : "Connect a racetime.gg account";
                    }
                }
                @if me.discord_id.is_none() {
                    li {
                        a(href = uri!(discord_login(redirect_to.clone())).to_string()) : "Connect a Discord account";
                    }
                }
                li {
                    a(href = uri!(logout(redirect_to)).to_string()) : "Sign out";
                }
            }
        }
    } else {
        html! {
            p : "To sign in or create a new account, please sign in via one of the following services:";
            ul {
                li {
                    a(href = uri!(racetime_login(redirect_to.clone())).to_string()) : "Sign in with racetime.gg";
                }
                li {
                    a(href = uri!(discord_login(redirect_to)).to_string()) : "Sign in with Discord";
                }
            }
        }
    }).await
}

#[rocket::get("/login/racetime?<redirect_to>")]
pub(crate) fn racetime_login(oauth: OAuth2<RaceTime>, cookies: &CookieJar<'_>, redirect_to: Option<Origin<'_>>) -> Result<Redirect, Error<rocket_oauth2::Error>> {
    if let Some(redirect_to) = redirect_to {
        cookies.add(Cookie::build("redirect_to", redirect_to).same_site(SameSite::Lax).finish());
    }
    oauth.get_redirect(cookies, &["read"]).map_err(Error)
}

#[rocket::get("/login/discord?<redirect_to>")]
pub(crate) fn discord_login(oauth: OAuth2<Discord>, cookies: &CookieJar<'_>, redirect_to: Option<Origin<'_>>) -> Result<Redirect, Error<rocket_oauth2::Error>> {
    if let Some(redirect_to) = redirect_to {
        cookies.add(Cookie::build("redirect_to", redirect_to).same_site(SameSite::Lax).finish());
    }
    oauth.get_redirect(cookies, &["identify"]).map_err(Error)
}

#[derive(Debug, thiserror::Error, Error)]
pub(crate) enum RaceTimeCallbackError {
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] UserFromRequest(#[from] UserFromRequestError),
}

#[rocket::get("/auth/racetime")]
pub(crate) async fn racetime_callback(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, client: &State<reqwest::Client>, token: TokenResponse<RaceTime>, cookies: &CookieJar<'_>) -> Result<RedirectOrContent, RaceTimeCallbackError> {
    let racetime_user = handle_racetime_token_response(client, cookies, &token).await?;
    let redirect_uri = cookies.get("redirect_to").and_then(|cookie| rocket::http::uri::Origin::try_from(cookie.value()).ok()).map_or_else(|| uri!(crate::http::index), |uri| uri.into_owned());
    Ok(if User::from_racetime(&**pool, &racetime_user.id).await?.is_some() {
        RedirectOrContent::Redirect(Redirect::to(redirect_uri))
    } else if let Some(me) = me {
        RedirectOrContent::Content(page(pool, &None, &uri, PageStyle { kind: PageKind::Login, ..PageStyle::default() }, "Connect Account — Mido's House", html! {
            p {
                : "This racetime.gg account is not associated with a Mido's House account, but you are signed in as ";
                : me;
                : ".";
            }
            ul {
                li {
                    a(href = uri!(register_racetime).to_string()) : "Connect this racetime.gg account to your Mido's House account";
                }
                li {
                    a(href = uri!(logout(Some(redirect_uri))).to_string()) : "Cancel";
                }
            }
        }).await?)
    } else {
        RedirectOrContent::Content(page(pool, &None, &uri, PageStyle { kind: PageKind::Login, ..PageStyle::default() }, "Create Account — Mido's House", html! {
            p : "This racetime.gg account is not associated with a Mido's House account.";
            ul {
                li {
                    a(href = uri!(register_racetime).to_string()) : "Create a new Mido's House account from this racetime.gg account";
                }
                li {
                    a(href = uri!(discord_login(_)).to_string()) : "Sign in with Discord";
                    : " to associate this racetime.gg account with an existing Mido's House account";
                }
                li {
                    a(href = uri!(logout(Some(redirect_uri))).to_string()) : "Cancel";
                }
            }
        }).await?)
    })
}

#[derive(Debug, thiserror::Error, Error)]
pub(crate) enum DiscordCallbackError {
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] ParseInt(#[from] std::num::ParseIntError),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] UserFromRequest(#[from] UserFromRequestError),
}

#[rocket::get("/auth/discord")]
pub(crate) async fn discord_callback(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, client: &State<reqwest::Client>, token: TokenResponse<Discord>, cookies: &CookieJar<'_>) -> Result<RedirectOrContent, DiscordCallbackError> {
    let discord_user = handle_discord_token_response(client, cookies, &token).await?;
    let redirect_uri = cookies.get("redirect_to").and_then(|cookie| rocket::http::uri::Origin::try_from(cookie.value()).ok()).map_or_else(|| uri!(crate::http::index), |uri| uri.into_owned());
    Ok(if User::from_discord(&**pool, discord_user.id).await?.is_some() {
        RedirectOrContent::Redirect(Redirect::to(redirect_uri))
    } else if let Some(me) = me {
        RedirectOrContent::Content(page(pool, &None, &uri, PageStyle { kind: PageKind::Login, ..PageStyle::default() }, "Connect Account — Mido's House", html! {
            p {
                : "This Discord account is not associated with a Mido's House account, but you are signed in as ";
                : me;
                : ".";
            }
            ul {
                li {
                    a(href = uri!(register_discord).to_string()) : "Connect this Discord account to your Mido's House account";
                }
                li {
                    a(href = uri!(logout(Some(redirect_uri))).to_string()) : "Cancel";
                }
            }
        }).await?)
    } else {
        RedirectOrContent::Content(page(pool, &None, &uri, PageStyle { kind: PageKind::Login, ..PageStyle::default() }, "Create Account — Mido's House", html! {
            p : "This Discord account is not associated with a Mido's House account.";
            ul {
                li {
                    a(href = uri!(register_discord).to_string()) : "Create a new Mido's House account from this Discord account";
                }
                li {
                    a(href = uri!(racetime_login(_)).to_string()) : "Sign in with racetime.gg";
                    : " to associate this Discord account with an existing Mido's House account";
                }
                li {
                    a(href = uri!(logout(Some(redirect_uri))).to_string()) : "Cancel";
                }
            }
        }).await?)
    })
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum RegisterError {
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("there is already an account associated with this Discord account")]
    ExistsDiscord,
    #[error("there is already an account associated with this racetime.gg account")]
    ExistsRaceTime,
}

#[rocket::get("/register/racetime")]
pub(crate) async fn register_racetime(pool: &State<PgPool>, me: Option<User>, racetime_user: Option<RaceTimeUser>) -> Result<Redirect, RegisterError> {
    Ok(if let Some(racetime_user) = racetime_user {
        let mut transaction = pool.begin().await?;
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE racetime_id = $1) AS "exists!""#, racetime_user.id).fetch_one(&mut transaction).await? {
            return Err(RegisterError::ExistsRaceTime) //TODO user-facing error message
        } else if let Some(me) = me {
            sqlx::query!("UPDATE users SET racetime_id = $1, racetime_display_name = $2, racetime_pronouns = $3 WHERE id = $4", racetime_user.id, racetime_user.name, racetime_user.pronouns as _, i64::from(me.id)).execute(&mut transaction).await?;
            transaction.commit().await?;
            Redirect::to(uri!(crate::user::profile(me.id)))
        } else {
            let id = Id::new(&mut transaction, IdTable::Users).await?;
            sqlx::query!("INSERT INTO users (id, display_source, racetime_id, racetime_display_name, racetime_pronouns) VALUES ($1, 'racetime', $2, $3, $4)", id as _, racetime_user.id, racetime_user.name, racetime_user.pronouns as _).execute(&mut transaction).await?;
            transaction.commit().await?;
            Redirect::to(uri!(crate::user::profile(id)))
        }
    } else {
        Redirect::to(uri!(racetime_login(_)))
    })
}

#[rocket::get("/register/discord")]
pub(crate) async fn register_discord(pool: &State<PgPool>, me: Option<User>, discord_user: Option<DiscordUser>) -> Result<Redirect, RegisterError> {
    Ok(if let Some(discord_user) = discord_user {
        let mut transaction = pool.begin().await?;
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE discord_id = $1) AS "exists!""#, i64::from(discord_user.id)).fetch_one(&mut transaction).await? {
            return Err(RegisterError::ExistsDiscord) //TODO user-facing error message
        } else if let Some(me) = me {
            sqlx::query!("UPDATE users SET discord_id = $1, discord_display_name = $2 WHERE id = $3", i64::from(discord_user.id), discord_user.username, i64::from(me.id)).execute(&mut transaction).await?;
            transaction.commit().await?;
            Redirect::to(uri!(crate::user::profile(me.id)))
        } else {
            let id = Id::new(&mut transaction, IdTable::Users).await?;
            sqlx::query!("INSERT INTO users (id, display_source, discord_id, discord_display_name) VALUES ($1, 'discord', $2, $3)", id as _, i64::from(discord_user.id), discord_user.username).execute(&mut transaction).await?;
            transaction.commit().await?;
            Redirect::to(uri!(crate::user::profile(id)))
        }
    } else {
        Redirect::to(uri!(discord_login(_)))
    })
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum MergeAccountsError {
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error("accounts already merged")]
    AlreadyMerged,
    #[error("failed to merge accounts")]
    Other,
}

#[rocket::get("/merge-accounts")]
pub(crate) async fn merge_accounts(pool: &State<PgPool>, me: User, racetime_user: Option<RaceTimeUser>, discord_user: Option<DiscordUser>) -> Result<Redirect, MergeAccountsError> {
    let mut transaction = pool.begin().await?;
    match (me.racetime_id, me.discord_id) {
        (Some(_), Some(_)) => return Err(MergeAccountsError::AlreadyMerged),
        (Some(_), None) => if let Some(discord_user) = discord_user {
            if let Ok(Some(discord_user)) = User::from_discord(&**pool, discord_user.id).await { //TODO use the transaction
                if discord_user.racetime_id.is_none() {
                    sqlx::query!("UPDATE users SET discord_id = $1, discord_display_name = $2 WHERE id = $3", i64::from(discord_user.discord_id.expect("Discord user without Discord ID")), discord_user.discord_display_name, i64::from(me.id)).execute(&mut transaction).await?;
                    sqlx::query!("DELETE FROM users WHERE id = $1", i64::from(discord_user.id)).execute(&mut transaction).await?;
                    transaction.commit().await?;
                    return Ok(Redirect::to(uri!(crate::user::profile(me.id))))
                }
            }
        },
        (None, Some(_)) => if let Some(racetime_user) = racetime_user {
            if let Ok(Some(racetime_user)) = User::from_racetime(&**pool, &racetime_user.id).await { //TODO use the transaction
                if racetime_user.discord_id.is_none() {
                    sqlx::query!("UPDATE users SET racetime_id = $1, racetime_display_name = $2, racetime_pronouns = $3 WHERE id = $4", racetime_user.racetime_id, racetime_user.racetime_display_name, racetime_user.racetime_pronouns as _, i64::from(me.id)).execute(&mut transaction).await?;
                    sqlx::query!("DELETE FROM users WHERE id = $1", i64::from(racetime_user.id)).execute(&mut transaction).await?;
                    transaction.commit().await?;
                    return Ok(Redirect::to(uri!(crate::user::profile(me.id))))
                }
            }
        },
        (None, None) => unreachable!("signed in but nether account connected"),
    }
    transaction.rollback().await?;
    Err(MergeAccountsError::Other)
}

#[rocket::get("/logout?<redirect_to>")]
pub(crate) fn logout(cookies: &CookieJar<'_>, redirect_to: Option<Origin<'_>>) -> Redirect {
    cookies.remove_private(Cookie::named("racetime_token"));
    cookies.remove_private(Cookie::named("discord_token"));
    Redirect::to(redirect_to.map_or_else(|| uri!(crate::http::index), |uri| uri.0.into_owned()))
}

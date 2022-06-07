use {
    std::{
        collections::HashMap,
        time::Duration,
    },
    anyhow::{
        Error,
        Result,
        anyhow,
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
        response::{
            Debug,
            Redirect,
        },
        uri,
    },
    rocket_oauth2::{
        OAuth2,
        TokenResponse,
    },
    rocket_util::{
        Origin,
        html,
    },
    serde::Deserialize,
    sqlx::PgPool,
    crate::{
        PageError,
        PageKind,
        PageResult,
        PageStyle,
        page,
        user::User,
        util::{
            Id,
            IdTable,
            RedirectOrContent,
        },
    },
};

pub(crate) struct ViewAs(pub(crate) HashMap<Id, Id>);

#[rocket::async_trait]
impl<'r> FromRequest<'r> for User {
    type Error = Error;

    async fn from_request(req: &'r Request<'_>) -> request::Outcome<Self, Error> {
        macro_rules! guard_try {
            ($res:expr) => {
                match $res {
                    Ok(x) => x,
                    Err(e) => return Outcome::Failure((Status::InternalServerError, anyhow!(e))),
                }
            };
        }

        match req.guard::<&State<PgPool>>().await {
            Outcome::Success(pool) => match req.guard::<&CookieJar<'_>>().await {
                Outcome::Success(cookies) => match req.guard::<&State<ViewAs>>().await {
                    Outcome::Success(view_as) => if let Some(token) = cookies.get_private("racetime_token") {
                        match req.guard::<&State<reqwest::Client>>().await {
                            Outcome::Success(client) => match client.get("https://racetime.gg/o/userinfo")
                                .bearer_auth(token.value())
                                .send().await
                                .and_then(|response| response.error_for_status())
                            {
                                Ok(response) => {
                                    let user_data = guard_try!(response.json::<RaceTimeUser>().await);
                                    if let Some(user) = guard_try!(User::from_racetime(&**pool, &user_data.id).await) {
                                        guard_try!(sqlx::query!("UPDATE users SET racetime_display_name = $1 WHERE id = $2", user_data.name, i64::from(user.id)).execute(&**pool).await);
                                        if let Some(&user_id) = view_as.inner().0.get(&user.id) {
                                            if let Some(user) = guard_try!(User::from_id(&**pool, user_id).await) {
                                                Outcome::Success(user)
                                            } else {
                                                Outcome::Failure((Status::InternalServerError, anyhow!("user to view as does not exist")))
                                            }
                                        } else {
                                            Outcome::Success(user)
                                        }
                                    } else {
                                        Outcome::Failure((Status::Unauthorized, anyhow!("this racetime.gg account is not associated with a Mido's House account")))
                                    }
                                }
                                Err(e) => Outcome::Failure((Status::BadGateway, anyhow!(e))),
                            },
                            Outcome::Failure((status, ())) => Outcome::Failure((status, anyhow!("missing HTTP client"))),
                            Outcome::Forward(()) => Outcome::Forward(()),
                        }
                    } else if let Some(token) = cookies.get_private("discord_token") {
                        match req.guard::<&State<reqwest::Client>>().await {
                            Outcome::Success(client) => match client.get("https://discord.com/api/v9/users/@me")
                                .bearer_auth(token.value())
                                .send().await
                                .and_then(|response| response.error_for_status())
                            {
                                Ok(response) => {
                                    let user_data = guard_try!(response.json::<DiscordUser>().await);
                                    if let Some(user) = guard_try!(User::from_discord(&**pool, guard_try!(user_data.id.parse())).await) {
                                        guard_try!(sqlx::query!("UPDATE users SET discord_display_name = $1 WHERE id = $2", user_data.username, i64::from(user.id)).execute(&**pool).await);
                                        if let Some(&user_id) = view_as.inner().0.get(&user.id) {
                                            if let Some(user) = guard_try!(User::from_id(&**pool, user_id).await) {
                                                Outcome::Success(user)
                                            } else {
                                                Outcome::Failure((Status::InternalServerError, anyhow!("user to view as does not exist")))
                                            }
                                        } else {
                                            Outcome::Success(user)
                                        }
                                    } else {
                                        Outcome::Failure((Status::Unauthorized, anyhow!("this Discord account is not associated with a Mido's House account")))
                                    }
                                },
                                Err(e) => Outcome::Failure((Status::BadGateway, anyhow!(e))),
                            },
                            Outcome::Failure((status, ())) => Outcome::Failure((status, anyhow!("missing HTTP client"))),
                            Outcome::Forward(()) => Outcome::Forward(()),
                        }
                    } else {
                        Outcome::Failure((Status::Unauthorized, anyhow!("neither racetime_token cookie nor discord_token cookie present")))
                    },
                    Outcome::Failure((status, ())) => Outcome::Failure((status, anyhow!("missing view-as map"))),
                    Outcome::Forward(()) => Outcome::Forward(()),
                },
                Outcome::Failure((_, never)) => match never {},
                Outcome::Forward(()) => Outcome::Forward(()),
            },
            Outcome::Failure((status, ())) => Outcome::Failure((status, anyhow!("missing database connection"))),
            Outcome::Forward(()) => Outcome::Forward(()),
        }
    }
}

pub(crate) enum RaceTime {}
pub(crate) enum Discord {}

#[derive(Deserialize)]
pub(crate) struct RaceTimeUser {
    id: String,
    name: String,
}

#[derive(Deserialize)]
pub(crate) struct DiscordUser {
    id: String,
    username: String,
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
pub(crate) fn racetime_login(oauth2: OAuth2<RaceTime>, cookies: &CookieJar<'_>, redirect_to: Option<Origin<'_>>) -> Result<Redirect, Debug<rocket_oauth2::Error>> {
    if let Some(redirect_to) = redirect_to {
        cookies.add(Cookie::build("redirect_to", redirect_to).same_site(SameSite::Lax).finish());
    }
    oauth2.get_redirect(cookies, &["read"]).map_err(Debug)
}

#[rocket::get("/login/discord?<redirect_to>")]
pub(crate) fn discord_login(oauth2: OAuth2<Discord>, cookies: &CookieJar<'_>, redirect_to: Option<Origin<'_>>) -> Result<Redirect, Debug<rocket_oauth2::Error>> {
    if let Some(redirect_to) = redirect_to {
        cookies.add(Cookie::build("redirect_to", redirect_to).same_site(SameSite::Lax).finish());
    }
    oauth2.get_redirect(cookies, &["identify"]).map_err(Debug)
}

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum RaceTimeCallbackError {
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Time(#[from] rocket::time::error::ConversionRange),
    #[error(transparent)] TryFromInt(#[from] std::num::TryFromIntError),
}

#[rocket::get("/auth/racetime")]
pub(crate) async fn racetime_callback(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, client: &State<reqwest::Client>, token: TokenResponse<RaceTime>, cookies: &CookieJar<'_>) -> Result<RedirectOrContent, RaceTimeCallbackError> {
    let mut cookie = Cookie::build("racetime_token", token.access_token().to_owned())
        .same_site(SameSite::Lax);
    if let Some(expires_in) = token.expires_in() {
        cookie = cookie.max_age(Duration::from_secs(expires_in.try_into()?).try_into()?);
    }
    cookies.add_private(cookie.finish());
    let racetime_user = client.get("https://racetime.gg/o/userinfo")
        .bearer_auth(token.access_token())
        .send().await?
        .error_for_status()?
        .json::<RaceTimeUser>().await?;
    let redirect_uri = cookies.get("redirect_to").and_then(|cookie| rocket::http::uri::Origin::try_from(cookie.value()).ok()).map_or_else(|| uri!(crate::index), |uri| uri.into_owned());
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

#[derive(Debug, thiserror::Error, rocket_util::Error)]
pub(crate) enum DiscordCallbackError {
    #[error(transparent)] Page(#[from] PageError),
    #[error(transparent)] ParseInt(#[from] std::num::ParseIntError),
    #[error(transparent)] Reqwest(#[from] reqwest::Error),
    #[error(transparent)] Sql(#[from] sqlx::Error),
    #[error(transparent)] Time(#[from] rocket::time::error::ConversionRange),
    #[error(transparent)] TryFromInt(#[from] std::num::TryFromIntError),
}

#[rocket::get("/auth/discord")]
pub(crate) async fn discord_callback(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, client: &State<reqwest::Client>, token: TokenResponse<Discord>, cookies: &CookieJar<'_>) -> Result<RedirectOrContent, DiscordCallbackError> {
    let mut cookie = Cookie::build("discord_token", token.access_token().to_owned())
        .same_site(SameSite::Lax);
    if let Some(expires_in) = token.expires_in() {
        cookie = cookie.max_age(Duration::from_secs(expires_in.try_into()?).try_into()?);
    }
    cookies.add_private(cookie.finish());
    let discord_user = client.get("https://discord.com/api/v9/users/@me")
        .bearer_auth(token.access_token())
        .send().await?
        .error_for_status()?
        .json::<DiscordUser>().await?;
    let redirect_uri = cookies.get("redirect_to").and_then(|cookie| rocket::http::uri::Origin::try_from(cookie.value()).ok()).map_or_else(|| uri!(crate::index), |uri| uri.into_owned());
    Ok(if User::from_discord(&**pool, discord_user.id.parse()?).await?.is_some() {
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

#[rocket::get("/register/racetime")]
pub(crate) async fn register_racetime(pool: &State<PgPool>, me: Option<User>, client: &State<reqwest::Client>, cookies: &CookieJar<'_>) -> Result<Redirect, Debug<Error>> {
    Ok(if let Some(cookie) = cookies.get_private("racetime_token") {
        let racetime_user = client.get("https://racetime.gg/o/userinfo")
            .bearer_auth(cookie.value())
            .send().await.map_err(Error::from)?
            .error_for_status().map_err(Error::from)?
            .json::<RaceTimeUser>().await.map_err(Error::from)?;
        let mut transaction = pool.begin().await.map_err(Error::from)?;
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE racetime_id = $1) AS "exists!""#, racetime_user.id).fetch_one(&mut transaction).await.map_err(Error::from)? {
            return Err(Debug(anyhow!("there is already an account associated with this racetime.gg account"))) //TODO user-facing error message
        } else if let Some(me) = me {
            sqlx::query!("UPDATE users SET racetime_id = $1, racetime_display_name = $2 WHERE id = $3", racetime_user.id, racetime_user.name, i64::from(me.id)).execute(&mut transaction).await.map_err(Error::from)?;
            transaction.commit().await.map_err(Error::from)?;
            Redirect::to(uri!(crate::user::profile(me.id)))
        } else {
            let id = Id::new(&mut transaction, IdTable::Users).await.map_err(Error::from)?;
            sqlx::query!("INSERT INTO users (id, display_source, racetime_id, racetime_display_name) VALUES ($1, 'racetime', $2, $3)", id as _, racetime_user.id, racetime_user.name).execute(&mut transaction).await.map_err(Error::from)?;
            transaction.commit().await.map_err(Error::from)?;
            Redirect::to(uri!(crate::user::profile(id)))
        }
    } else {
        Redirect::to(uri!(racetime_login(_)))
    })
}

#[rocket::get("/register/discord")]
pub(crate) async fn register_discord(pool: &State<PgPool>, me: Option<User>, client: &State<reqwest::Client>, cookies: &CookieJar<'_>) -> Result<Redirect, Debug<Error>> {
    Ok(if let Some(cookie) = cookies.get_private("discord_token") {
        let discord_user = client.get("https://discord.com/api/v9/users/@me")
            .bearer_auth(cookie.value())
            .send().await.map_err(Error::from)?
            .error_for_status().map_err(Error::from)?
            .json::<DiscordUser>().await.map_err(Error::from)?;
        let snowflake = discord_user.id.parse::<u64>().map_err(Error::from)?;
        let mut transaction = pool.begin().await.map_err(Error::from)?;
        if sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE discord_id = $1) AS "exists!""#, snowflake as i64).fetch_one(&mut transaction).await.map_err(Error::from)? {
            return Err(Debug(anyhow!("there is already an account associated with this Discord account"))) //TODO user-facing error message
        } else if let Some(me) = me {
            sqlx::query!("UPDATE users SET discord_id = $1, discord_display_name = $2 WHERE id = $3", snowflake as i64, discord_user.username, i64::from(me.id)).execute(&mut transaction).await.map_err(Error::from)?;
            transaction.commit().await.map_err(Error::from)?;
            Redirect::to(uri!(crate::user::profile(me.id)))
        } else {
            let id = Id::new(&mut transaction, IdTable::Users).await.map_err(Error::from)?;
            sqlx::query!("INSERT INTO users (id, display_source, discord_id, discord_display_name) VALUES ($1, 'discord', $2, $3)", id as _, snowflake as i64, discord_user.username).execute(&mut transaction).await.map_err(Error::from)?;
            transaction.commit().await.map_err(Error::from)?;
            Redirect::to(uri!(crate::user::profile(id)))
        }
    } else {
        Redirect::to(uri!(discord_login(_)))
    })
}

#[rocket::get("/logout?<redirect_to>")]
pub(crate) fn logout(cookies: &CookieJar<'_>, redirect_to: Option<Origin<'_>>) -> Redirect {
    cookies.remove_private(Cookie::named("racetime_token"));
    cookies.remove_private(Cookie::named("discord_token"));
    Redirect::to(redirect_to.map_or_else(|| uri!(crate::index), |uri| uri.0.into_owned()))
}

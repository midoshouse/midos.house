use {
    std::time::Duration,
    anyhow::{
        Error,
        Result,
        anyhow,
    },
    horrorshow::{
        RenderBox,
        box_html,
        html,
        rocket::Result as HtmlResult,
    },
    rand::prelude::*,
    rocket::{
        Responder,
        State,
        http::{
            Cookie,
            CookieJar,
            SameSite,
            Status,
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
            content::Html,
        },
        uri,
    },
    rocket_oauth2::{
        OAuth2,
        TokenResponse,
    },
    serde::Deserialize,
    sqlx::PgPool,
    crate::{
        HeaderPage,
        HeaderStyle,
        page,
    },
};

pub(crate) struct User {
    //id: i64,
    pub(crate) display_name: String,
    //racetime_id: Option<String>,
}

impl User {
    async fn from_racetime(pool: &PgPool, racetime_id: &str) -> sqlx::Result<Option<Self>> {
        //TODO update display name from racetime user data?
        sqlx::query_as!(Self, "SELECT display_name FROM users WHERE racetime_id = $1", racetime_id).fetch_optional(pool).await
    }
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for User {
    type Error = Error;

    async fn from_request(req: &'r Request<'_>) -> request::Outcome<Self, Error> {
        match req.guard::<&State<PgPool>>().await {
            Outcome::Success(pool) => match req.guard::<&CookieJar<'_>>().await {
                Outcome::Success(cookies) => if let Some(token) = cookies.get_private("racetime_token") {
                    match req.guard::<&State<reqwest::Client>>().await {
                        Outcome::Success(client) => match client.get("https://racetime.gg/o/userinfo")
                            .bearer_auth(token.value())
                            .send().await
                            .and_then(|response| response.error_for_status())
                        {
                            Ok(response) => match response.json::<RaceTimeUser>().await {
                                Ok(user_data) => match User::from_racetime(pool, &user_data.id).await {
                                    Ok(Some(user)) => Outcome::Success(user),
                                    Ok(None) => Outcome::Failure((Status::Unauthorized, anyhow!("this racetime.gg account is not associated with a Mido's House account"))),
                                    Err(e) => Outcome::Failure((Status::InternalServerError, anyhow!(e))),
                                },
                                Err(e) => Outcome::Failure((Status::InternalServerError, anyhow!(e))),
                            },
                            Err(e) => Outcome::Failure((Status::BadGateway, anyhow!(e))),
                        },
                        Outcome::Failure((status, ())) => Outcome::Failure((status, anyhow!("missing HTTP client"))),
                        Outcome::Forward(()) => Outcome::Forward(()),
                    }
                } else {
                    Outcome::Failure((Status::Unauthorized, anyhow!("racetime_token cookie not present")))
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

#[derive(Deserialize)]
pub(crate) struct RaceTimeUser {
    id: String,
    name: String,
}

#[rocket::get("/login")]
pub(crate) fn login(user: Option<User>) -> HtmlResult {
    page(&user, HeaderStyle { page: HeaderPage::Login, ..HeaderStyle::default() }, "Login — Mido's House", if user.is_some() {
        (box_html! {
            p : "You are already signed in.";
            ul {
                //TODO link to profile
                li {
                    a(href = uri!(logout).to_string()) : "Sign out";
                }
                //TODO offer to connect another account?
            }
        }) as Box<dyn RenderBox>
    } else {
        box_html! {
            p : "To login or create a new account, please sign in with your racetime.gg account.";
            ul {
                li {
                    a(href = uri!(racetime_login).to_string()) : "Sign in with racetime.gg";
                }
            }
        }
    })
}

#[rocket::get("/login/racetime")]
pub(crate) fn racetime_login(oauth2: OAuth2<RaceTime>, cookies: &CookieJar<'_>) -> Result<Redirect, Debug<rocket_oauth2::Error>> {
    oauth2.get_redirect(cookies, &["read"]).map_err(Debug)
}

#[derive(Responder)]
pub(crate) enum RaceTimeCallbackResponse {
    Redirect(Redirect),
    Content(Html<String>),
}

#[rocket::get("/auth/racetime")]
pub(crate) async fn racetime_callback(pool: &State<PgPool>, client: &State<reqwest::Client>, token: TokenResponse<RaceTime>, cookies: &CookieJar<'_>) -> Result<RaceTimeCallbackResponse, Debug<Error>> {
    let mut cookie = Cookie::build("racetime_token", token.access_token().to_owned())
        .same_site(SameSite::Lax);
    if let Some(expires_in) = token.expires_in() {
        cookie = cookie.max_age(Duration::from_secs(expires_in.try_into().map_err(Error::from)?).try_into().map_err(Error::from)?);
    }
    cookies.add_private(cookie.finish());
    //TODO if a Discord session token is already present, offer to connect this account with it instead (only if there aren't any conflicting associations)
    let racetime_user = client.get("https://racetime.gg/o/userinfo")
        .bearer_auth(token.access_token())
        .send().await.map_err(Error::from)?
        .error_for_status().map_err(Error::from)?
        .json::<RaceTimeUser>().await.map_err(Error::from)?;
    Ok(if User::from_racetime(pool, &racetime_user.id).await.map_err(Error::from)?.is_some() {
        RaceTimeCallbackResponse::Redirect(Redirect::to(uri!(crate::index))) //TODO redirect to original page
    } else {
        RaceTimeCallbackResponse::Content(page(&None, HeaderStyle { page: HeaderPage::Login, ..HeaderStyle::default() }, "Create account — Mido's House", html! {
            p : "This racetime.gg account is not associated with a Mido's House account.";
            ul {
                li {
                    a(href = uri!(register_racetime).to_string()) : "Create a new Mido's House account from this racetime.gg account";
                }
                li {
                    a(href = uri!(logout).to_string()) : "Cancel";
                }
            }
        }).map_err(Error::from)?)
        //TODO also offer to associate with an existing account with a Discord login
    })
}

#[rocket::get("/register/racetime")]
pub(crate) async fn register_racetime(pool: &State<PgPool>, client: &State<reqwest::Client>, cookies: &CookieJar<'_>) -> Result<Redirect, Debug<Error>> {
    Ok(if let Some(cookie) = cookies.get_private("racetime_token") {
        let racetime_user = client.get("https://racetime.gg/o/userinfo")
            .bearer_auth(cookie.value())
            .send().await.map_err(Error::from)?
            .error_for_status().map_err(Error::from)?
            .json::<RaceTimeUser>().await.map_err(Error::from)?;
        if sqlx::query!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE racetime_id = $1) AS "exists!""#, racetime_user.id).fetch_one(&**pool).await.map_err(Error::from)?.exists {
            return Err(Debug(anyhow!("there is already an account associated with this racetime.gg account"))) //TODO user-facing error message
        }
        let id = loop {
            let id = thread_rng().gen::<i64>();
            if !sqlx::query!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE id = $1) AS "exists!""#, id).fetch_one(&**pool).await.map_err(Error::from)?.exists {
                break id
            }
        };
        sqlx::query!("INSERT INTO users (id, display_name, racetime_id) VALUES ($1, $2, $3)", id, racetime_user.name, racetime_user.id).execute(&**pool).await.map_err(Error::from)?;
        Redirect::to(uri!(crate::index)) //TODO redirect to an appropriate page
    } else {
        Redirect::to(uri!(racetime_login))
    })
}

#[rocket::get("/logout")]
pub(crate) fn logout(cookies: &CookieJar<'_>) -> Redirect {
    cookies.remove_private(Cookie::named("racetime_token"));
    Redirect::to(uri!(crate::index)) //TODO redirect to original page
}

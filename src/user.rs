use {
    horrorshow::{
        RenderBox,
        box_html,
        html,
    },
    rocket::{
        Responder,
        State,
        http::Status,
        response::content::Html,
        uri,
    },
    sqlx::PgPool,
    crate::{
        PageError,
        PageKind,
        PageStyle,
        page,
        util::Id,
    },
};

pub(crate) struct User {
    pub(crate) id: Id,
    pub(crate) display_name: String,
    pub(crate) racetime_id: Option<String>,
}

impl User {
    pub(crate) async fn from_id(pool: &PgPool, id: Id) -> sqlx::Result<Option<Self>> {
        Ok(sqlx::query!("SELECT * FROM users WHERE id = $1", i64::from(id)).fetch_optional(pool).await?.map(|row| Self {
            id: row.id.into(),
            display_name: row.display_name,
            racetime_id: row.racetime_id,
        }))
    }

    pub(crate) async fn from_racetime(pool: &PgPool, racetime_id: &str) -> sqlx::Result<Option<Self>> {
        Ok(sqlx::query!("SELECT * FROM users WHERE racetime_id = $1", racetime_id).fetch_optional(pool).await?.map(|row| Self {
            id: row.id.into(),
            display_name: row.display_name,
            racetime_id: row.racetime_id,
        }))
    }

    pub(crate) fn to_html<'a>(&'a self) -> Box<dyn RenderBox + 'a> {
        box_html! {
            a(href = uri!(profile(self.id)).to_string()) : &self.display_name;
        }
    }

    pub(crate) fn into_html(self) -> Box<dyn RenderBox + Send> {
        box_html! {
            a(href = uri!(profile(self.id)).to_string()) : self.display_name;
        }
    }
}

impl PartialEq for User {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for User {}

#[derive(Responder)]
pub(crate) enum ProfileError {
    NotFound(Status),
    Page(PageError),
}

#[rocket::get("/user/<id>")]
pub(crate) async fn profile(pool: &State<PgPool>, me: Option<User>, id: Id) -> Result<Html<String>, ProfileError> {
    let user = if let Some(user) = User::from_id(&pool, id).await.map_err(|e| ProfileError::Page(PageError::Sql(e)))? {
        user
    } else {
        return Err(ProfileError::NotFound(Status::NotFound))
    };
    page(&pool, &me, PageStyle { kind: if me.as_ref().map_or(false, |me| *me == user) { PageKind::MyProfile } else { PageKind::Other }, ..PageStyle::default() }, &format!("{} — Mido's House", user.display_name), html! {
        h1 : &user.display_name;
        p {
            : "Mido's House user ID: ";
            code : user.id.0;
        }
        @if let Some(ref racetime_id) = user.racetime_id {
            p {
                : "racetime.gg: ";
                a(href = format!("https://racetime.gg/user/{racetime_id}")) : user.display_name; //TODO racetime.gg display name with discriminator
            }
        }
    }).await.map_err(ProfileError::Page)
}

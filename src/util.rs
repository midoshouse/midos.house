use {
    std::{
        borrow::Cow,
        convert::Infallible as Never,
        fmt,
        iter,
        mem,
        str::FromStr,
    },
    derive_more::From,
    horrorshow::{
        RenderBox,
        RenderOnce,
        TemplateBuffer,
        box_html,
        html,
    },
    itertools::Itertools as _,
    rand::prelude::*,
    rocket::{
        FromForm,
        Responder,
        UriDisplayPath,
        UriDisplayQuery,
        form::{
            self,
            Contextual,
            FromFormField,
        },
        http::{
            Status,
            impl_from_uri_param_identity,
            uri::fmt::{
                FromUriParam,
                Query,
                UriDisplay,
            },
        },
        request::{
            self,
            FromParam,
            FromRequest,
            Request,
        },
        response::{
            Redirect,
            content::Html,
        },
    },
    rocket_csrf::CsrfToken,
    sqlx::{
        Database,
        Decode,
        Encode,
        Postgres,
        Transaction,
    },
    crate::PageError,
};

pub(crate) trait CsrfForm {
    fn csrf(&self) -> &String;
}

pub(crate) trait ContextualExt {
    fn verify(&mut self, token: &Option<CsrfToken>);
}

impl<F: CsrfForm> ContextualExt for Contextual<'_, F> {
    fn verify(&mut self, token: &Option<CsrfToken>) {
        if let Some(ref value) = self.value {
            match token.as_ref().map(|token| token.verify(value.csrf())) {
                Some(Ok(())) => {}
                Some(Err(rocket_csrf::VerificationFailure)) | None => self.context.push_error(form::Error::validation("Please submit the form again to confirm your identity.").with_name("csrf")),
            }
        }
    }
}

pub(crate) trait CsrfTokenExt {
    fn to_html(&self) -> Box<dyn RenderBox + '_>;
}

impl CsrfTokenExt for CsrfToken {
    fn to_html(&self) -> Box<dyn RenderBox + '_> {
        box_html! {
            input(type = "hidden", name = "csrf", value = self.authenticity_token());
        }
    }
}

impl CsrfTokenExt for Option<CsrfToken> {
    fn to_html(&self) -> Box<dyn RenderBox + '_> {
        box_html! {
            @if let Some(csrf) = self {
                : csrf.to_html();
            }
        }
    }
}

/// A form that only holds a CSRF token
#[derive(FromForm)]
pub(crate) struct EmptyForm {
    csrf: String,
}

impl EmptyForm {
    pub(crate) fn verify(&self, token: &Option<CsrfToken>) -> Result<(), rocket_csrf::VerificationFailure> {
        if let Some(token) = token {
            token.verify(&self.csrf)
        } else {
            Err(rocket_csrf::VerificationFailure)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, UriDisplayPath, UriDisplayQuery)]
pub(crate) struct Id(pub(crate) u64);

pub(crate) enum IdTable {
    Notifications,
    Teams,
    Users,
}

impl Id {
    pub(crate) async fn new(transaction: &mut Transaction<'_, Postgres>, table: IdTable) -> sqlx::Result<Self> {
        Ok(loop {
            let id = Self(thread_rng().gen());
            let query = match table {
                IdTable::Notifications => sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM notifications WHERE id = $1) AS "exists!""#, i64::from(id)),
                IdTable::Teams => sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM teams WHERE id = $1) AS "exists!""#, i64::from(id)),
                IdTable::Users => sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE id = $1) AS "exists!""#, i64::from(id)),
            };
            if !query.fetch_one(&mut *transaction).await? { break id }
        })
    }
}

impl From<u64> for Id {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl From<i64> for Id {
    fn from(id: i64) -> Self {
        Self(id as u64)
    }
}

impl From<Id> for i64 {
    fn from(Id(id): Id) -> Self {
        id as Self
    }
}

impl FromStr for Id {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u64>()
            .map(Self)
            .or_else(|_| s.parse::<i64>().map(Self::from))
    }
}

impl<'r, DB: Database> Decode<'r, DB> for Id
where i64: Decode<'r, DB> {
    fn decode(value: <DB as sqlx::database::HasValueRef<'r>>::ValueRef) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        i64::decode(value).map(|id| Self(id as u64))
    }
}

impl<'q, DB: Database> Encode<'q, DB> for Id
where i64: Encode<'q, DB> {
    fn encode_by_ref(&self, buf: &mut <DB as sqlx::database::HasArguments<'q>>::ArgumentBuffer) -> sqlx::encode::IsNull {
        (self.0 as i64).encode(buf)
    }

    fn encode(self, buf: &mut <DB as sqlx::database::HasArguments<'q>>::ArgumentBuffer) -> sqlx::encode::IsNull {
        (self.0 as i64).encode(buf)
    }

    fn produces(&self) -> Option<<DB as Database>::TypeInfo> {
        (self.0 as i64).produces()
    }

    fn size_hint(&self) -> usize {
        Encode::size_hint(&(self.0 as i64))
    }
}

impl<DB: Database> sqlx::Type<DB> for Id
where i64: sqlx::Type<DB> {
    fn type_info() -> <DB as Database>::TypeInfo {
        i64::type_info()
    }

    fn compatible(ty: &<DB as Database>::TypeInfo) -> bool {
        i64::compatible(ty)
    }
}

impl<'a> FromParam<'a> for Id {
    type Error = &'a str;

    fn from_param(param: &'a str) -> Result<Self, Self::Error> {
        u64::from_param(param)
            .map(Self)
            .or_else(|_| i64::from_param(param).map(Self::from))
    }
}

impl<'v> FromFormField<'v> for Id
where i64: FromFormField<'v>, u64: FromFormField<'v> {
    fn from_value(field: form::ValueField<'v>) -> form::Result<'v, Self> {
        u64::from_value(field.clone())
            .map(Self)
            .or_else(|_| i64::from_value(field).map(Self::from))
    }

    fn default() -> Option<Self> { None }
}

/// A URL without a hostname but with an absolute path and optional query.
///
/// Wrapper type used here to allow decoding from URI query
#[derive(Clone)]
pub(crate) struct Origin<'a>(pub(crate) rocket::http::uri::Origin<'a>);

#[rocket::async_trait]
impl<'a> FromRequest<'a> for Origin<'a> {
    type Error = Never;

    async fn from_request(req: &'a Request<'_>) -> request::Outcome<Self, Never> {
        <&rocket::http::uri::Origin<'_>>::from_request(req).await.map(|origin| Self(origin.clone()))
    }
}

impl<'a> FromFormField<'a> for Origin<'a> {
    fn from_value(field: form::ValueField<'a>) -> form::Result<'a, Self> {
        Ok(Self(rocket::http::uri::Origin::try_from(field.value).map_err(|e| form::Error::validation(e.to_string()))?))
    }
}

impl<'a> UriDisplay<Query> for Origin<'a> {
    fn fmt(&self, f: &mut rocket::http::uri::fmt::Formatter<'_, Query>) -> fmt::Result {
        UriDisplay::fmt(&self.0.to_string(), f)
    }
}

impl<'a> FromUriParam<Query, rocket::http::uri::Origin<'a>> for Origin<'a> {
    type Target = Self;

    fn from_uri_param(param: rocket::http::uri::Origin<'a>) -> Self {
        Self(param)
    }
}

impl_from_uri_param_identity!([Query] ('a) Origin<'a>);

impl From<Origin<'_>> for Cow<'_, str> {
    fn from(Origin(origin): Origin<'_>) -> Self {
        Self::Owned(origin.to_string())
    }
}

pub(crate) fn natjoin<'a, T: RenderOnce + Send + 'a>(elts: impl IntoIterator<Item = T>) -> Option<Box<dyn RenderBox + Send + 'a>> {
    let mut elts = elts.into_iter().fuse();
    match (elts.next(), elts.next(), elts.next()) {
        (None, _, _) => None,
        (Some(elt), None, _) => Some(box_html! {
            : elt;
        }),
        (Some(elt1), Some(elt2), None) => Some(box_html! {
            : elt1;
            : " and ";
            : elt2;
        }),
        (Some(elt1), Some(elt2), Some(elt3)) => {
            let mut rest = iter::once(elt3).chain(elts).collect_vec();
            let last = rest.pop().expect("rest contains at least elt3");
            Some(box_html! {
                : elt1;
                : ", ";
                : elt2;
                @for elt in rest {
                    : ", ";
                    : elt;
                }
                : ", and ";
                : last;
            })
        }
    }
}

#[derive(Responder)]
pub(crate) enum RedirectOrContent {
    Redirect(Redirect),
    Content(Html<String>),
}

pub(crate) fn render_form_error(tmpl: &mut TemplateBuffer<'_>, error: &form::Error<'_>) {
    tmpl << html! {
        p(class = "error") : error.to_string();
    };
}

pub(crate) fn field_errors(tmpl: &mut TemplateBuffer<'_>, errors: &mut Vec<&form::Error<'_>>, name: &str) {
    let field_errors;
    (field_errors, *errors) = mem::take(errors).into_iter().partition(|error| error.is_for(name));
    tmpl << html! {
        @for error in field_errors {
            |tmpl| render_form_error(tmpl, error);
        }
    };
}

#[derive(Responder, From)]
pub(crate) enum StatusOrError<E> {
    Status(Status),
    #[from]
    Err(E),
}

impl From<sqlx::Error> for StatusOrError<PageError> {
    fn from(e: sqlx::Error) -> Self {
        Self::Err(PageError::Sql(e))
    }
}

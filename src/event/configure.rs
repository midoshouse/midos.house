use crate::{
    event::{
        Data,
        Tab,
    },
    prelude::*,
};

async fn configure_form(mut transaction: Transaction<'_, Postgres>, env: Environment, me: Option<User>, uri: Origin<'_>, csrf: Option<&CsrfToken>, event: Data<'_>, ctx: Context<'_>) -> Result<RawHtml<String>, event::Error> {
    let header = event.header(&mut transaction, env, me.as_ref(), Tab::Configure, true).await?;
    let form = if event.is_ended() {
        html! {
            article {
                p : "This event has ended and can no longer be configured.";
            }
        }
    } else if let Some(ref me) = me {
        if event.organizers(&mut transaction).await?.contains(me) {
            let mut errors = ctx.errors().collect_vec();
            full_form(uri!(post(event.series, &*event.event)), csrf, html! {
                @if let MatchSource::StartGG(_) = event.match_source() {
                    : form_field("auto_import", &mut errors, html! {
                        input(type = "checkbox", id = "auto_import", name = "auto_import", checked? = ctx.field_value("restream_consent").map_or(event.auto_import, |value| value == "on"));
                        label(for = "auto_import") : "Automatically import new races from start.gg";
                        label(class = "help") : "(If this option is turned off, you can import races by clicking the Import button on the Races tab.)";
                    });
                }
            }, errors, "Save")
        } else {
            html! {
                article {
                    p : "This page is for organizers of this event only.";
                }
            }
        }
    } else {
        html! {
            article {
                p {
                    a(href = uri!(auth::login(Some(uri!(get(event.series, &*event.event))))).to_string()) : "Sign in or create a Mido's House account";
                    : " to configure this event.";
                }
            }
        }
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: event.chests().await, ..PageStyle::default() }, &format!("Configure — {}", event.display_name), html! {
        : header;
        : form;
    }).await?)
}

#[rocket::get("/event/<series>/<event>/configure")]
pub(crate) async fn get(pool: &State<PgPool>, env: &State<Environment>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: String) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    Ok(RedirectOrContent::Content(configure_form(transaction, **env, me, uri, csrf.as_ref(), data, Context::default()).await?))
}

#[derive(FromForm, CsrfForm)]
pub(crate) struct ConfigureForm {
    #[field(default = String::new())]
    csrf: String,
    auto_import: bool,
}

#[rocket::post("/event/<series>/<event>/configure", data = "<form>")]
pub(crate) async fn post(env: &State<Environment>, pool: &State<PgPool>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, form: Form<Contextual<'_, ConfigureForm>>) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
    let mut transaction = pool.begin().await?;
    let data = event::Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    Ok(if let Some(ref value) = form.value {
        if data.is_ended() {
            form.context.push_error(form::Error::validation("This event has ended and can no longer be configured"));
        }
        if !data.organizers(&mut transaction).await?.contains(&me) {
            form.context.push_error(form::Error::validation("You must be an organizer to configure this event."));
        }
        if form.context.errors().next().is_some() {
            RedirectOrContent::Content(configure_form(transaction, **env, Some(me), uri, csrf.as_ref(), data, form.context).await?)
        } else {
            sqlx::query!("UPDATE events SET auto_import = $1 WHERE series = $2 AND event = $3", value.auto_import, data.series as _, &data.event).execute(&mut *transaction).await?;
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(super::info(series, event))))
        }
    } else {
        RedirectOrContent::Content(configure_form(transaction, **env, Some(me), uri, csrf.as_ref(), data, form.context).await?)
    })
}

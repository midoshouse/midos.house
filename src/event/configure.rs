use crate::{
    event::{
        Data,
        Tab,
    },
    prelude::*,
};

async fn configure_form(mut transaction: Transaction<'_, Postgres>, me: Option<User>, uri: Origin<'_>, csrf: Option<&CsrfToken>, event: Data<'_>, ctx: Context<'_>) -> Result<RawHtml<String>, event::Error> {
    let header = event.header(&mut transaction, me.as_ref(), Tab::Configure, true).await?;
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
                        input(type = "checkbox", id = "auto_import", name = "auto_import", checked? = ctx.field_value("auto_import").map_or(event.auto_import, |value| value == "on"));
                        label(for = "auto_import") : "Automatically import new races from start.gg";
                        label(class = "help") : "(If this option is turned off, you can import races by clicking the Import button on the Races tab.)";
                    });
                }
                : form_field("min_schedule_notice", &mut errors, html! {
                    label(for = "min_schedule_notice") : "Minimum scheduling notice:";
                    input(type = "text", name = "min_schedule_notice", value = ctx.field_value("min_schedule_notice").map(Cow::Borrowed).unwrap_or_else(|| Cow::Owned(unparse_duration(event.min_schedule_notice)))); //TODO h:m:s fields?
                    label(class = "help") : "(Races must be scheduled at least this far in advance. Can be configured to be as low as 0 seconds, but note that if a race is scheduled less than 30 minutes in advance, the room is opened immediately, and if a race is scheduled less than 15 minutes in advance, the seed is posted immediately.)";
                });
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
                    a(href = uri!(auth::login(Some(uri!(get(event.series, &*event.event)))))) : "Sign in or create a Mido's House account";
                    : " to configure this event.";
                }
            }
        }
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: event.chests().await?, ..PageStyle::default() }, &format!("Configure — {}", event.display_name), html! {
        : header;
        : form;
    }).await?)
}

#[rocket::get("/event/<series>/<event>/configure")]
pub(crate) async fn get(pool: &State<PgPool>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: String) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    Ok(RedirectOrContent::Content(configure_form(transaction, me, uri, csrf.as_ref(), data, Context::default()).await?))
}

#[derive(FromForm, CsrfForm)]
pub(crate) struct ConfigureForm {
    #[field(default = String::new())]
    csrf: String,
    auto_import: Option<bool>,
    #[field(default = String::new())]
    min_schedule_notice: String,
}

#[rocket::post("/event/<series>/<event>/configure", data = "<form>")]
pub(crate) async fn post(pool: &State<PgPool>, me: User, uri: Origin<'_>, csrf: Option<CsrfToken>, series: Series, event: &str, form: Form<Contextual<'_, ConfigureForm>>) -> Result<RedirectOrContent, StatusOrError<event::Error>> {
    let mut transaction = pool.begin().await?;
    let data = Data::new(&mut transaction, series, event).await?.ok_or(StatusOrError::Status(Status::NotFound))?;
    let mut form = form.into_inner();
    form.verify(&csrf);
    Ok(if let Some(ref value) = form.value {
        if data.is_ended() {
            form.context.push_error(form::Error::validation("This event has ended and can no longer be configured"));
        }
        if !data.organizers(&mut transaction).await?.contains(&me) {
            form.context.push_error(form::Error::validation("You must be an organizer to configure this event."));
        }
        let min_schedule_notice = if let Some(time) = parse_duration(&value.min_schedule_notice, DurationUnit::Hours) {
            Some(time)
        } else {
            form.context.push_error(form::Error::validation("Duration must be formatted like “1:23:45” or “1h 23m 45s”.").with_name("min_schedule_notice"));
            None
        };
        if form.context.errors().next().is_some() {
            RedirectOrContent::Content(configure_form(transaction, Some(me), uri, csrf.as_ref(), data, form.context).await?)
        } else {
            if let Some(auto_import) = value.auto_import {
                sqlx::query!("UPDATE events SET auto_import = $1 WHERE series = $2 AND event = $3", auto_import, data.series as _, &data.event).execute(&mut *transaction).await?;
            }
            if let Some(min_schedule_notice) = min_schedule_notice {
                sqlx::query!("UPDATE events SET min_schedule_notice = $1 WHERE series = $2 AND event = $3", min_schedule_notice as _, data.series as _, &data.event).execute(&mut *transaction).await?;
            }
            transaction.commit().await?;
            RedirectOrContent::Redirect(Redirect::to(uri!(super::info(series, event))))
        }
    } else {
        RedirectOrContent::Content(configure_form(transaction, Some(me), uri, csrf.as_ref(), data, form.context).await?)
    })
}

use {
    rocket::response::content::RawHtml,
    rocket_util::{
        Origin,
        html,
    },
    sqlx::{
        Postgres,
        Transaction,
    },
    crate::{
        event::{
            Data,
            Error,
            Tab,
        },
        http::{
            PageStyle,
            page,
        },
        user::User,
    },
};

pub(super) fn info(event: &str) -> RawHtml<String> {
    match event {
        "5" => html! {
            article {
                p {
                    : "This is the 5th season of the Random Settings League tournament. See ";
                    a(href = "https://docs.google.com/document/d/1Js03yFcMw_mWx4UO_3UJB39CNCKa0bsxlBEYrHPq5Os/edit") : "the official document";
                    : " for details.";
                }
            }
        },
        _ => unimplemented!(),
    }
}

pub(super) async fn enter_form(mut transaction: Transaction<'_, Postgres>, me: Option<User>, uri: Origin<'_>, data: Data<'_>, event: &str) -> Result<RawHtml<String>, Error> {
    let header = data.header(&mut transaction, me.as_ref(), Tab::Enter).await?;
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Enter — {}", data.display_name), html! {
        : header;
        article {
            @match event {
                "5" => {
                    p {
                        a(href = "https://docs.google.com/forms/d/e/1FAIpQLSei3qjXA7DOHskgIOBSBObQXH3Y-qXynrsxY8rXbobFOkjdYA/viewform") : "Opt in using the official form";
                        : ".";
                    }
                    p {
                        : "Note: This page is not official. See ";
                        a(href = "https://docs.google.com/document/d/1Js03yFcMw_mWx4UO_3UJB39CNCKa0bsxlBEYrHPq5Os/edit") : "the official document";
                        : " for details.";
                    }
                }
                _ => @unimplemented
            }
        }
    }).await?)
}

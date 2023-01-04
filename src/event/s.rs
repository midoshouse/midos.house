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
        "6" => html! {
            article {
                p {
                    : "This is the 6th season of the main Ocarina of Time randomizer tournament. See ";
                    a(href = "https://docs.google.com/document/d/1Hkrh2A_szTUTgPqkzrqjSF0YWTtU34diLaypX9pyzUI/edit") : "the official document";
                    : " for details.";
                }
                h2 : "See also";
                ul {
                    li {
                        a(href = "https://challonge.com/ChallengeCupSeason6") : "Challenge Cup groups and bracket";
                    }
                }
            }
        },
        _ => unimplemented!(),
    }
}

pub(super) async fn enter_form(mut transaction: Transaction<'_, Postgres>, me: Option<User>, uri: Origin<'_>, data: Data<'_>) -> Result<RawHtml<String>, Error> {
    let header = data.header(&mut transaction, me.as_ref(), Tab::Enter).await?;
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Enter — {}", data.display_name), html! {
        : header;
        article {
            p : "Play in the qualifiers to enter this tournament.";
            p {
                : "Note: This page is not official. See ";
                a(href = "https://docs.google.com/document/d/1Hkrh2A_szTUTgPqkzrqjSF0YWTtU34diLaypX9pyzUI/edit") : "the official document";
                : " for details.";
            }
        }
    }).await?)
}

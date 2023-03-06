use {
    chrono::prelude::*,
    chrono_tz::America,
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
            InfoError,
            Tab,
        },
        http::{
            PageStyle,
            page,
        },
        user::User,
        util::{
            DateTimeFormat,
            format_datetime,
            natjoin_html,
        },
    },
};

pub(super) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<RawHtml<String>, InfoError> {
    Ok(match &*data.event {
        "2" => html! {
            article {
                p {
                    : "This is the 2nd season of the Triforce Blitz tournament, organized by Dastar, Elagatua, and ";
                    : natjoin_html(data.organizers(transaction).await?);
                    : ". See ";
                    a(href = "https://docs.google.com/document/d/1p8HAwWsjsLW7tjfDl2SK-yQ35pVqbAS9GB72bkOIDFI/edit") : "the official document";
                    : " for details.";
                }
            }
        },
        _ => unimplemented!(),
    })
}

pub(super) async fn enter_form(mut transaction: Transaction<'_, Postgres>, me: Option<User>, uri: Origin<'_>, data: Data<'_>) -> Result<RawHtml<String>, Error> {
    let header = data.header(&mut transaction, me.as_ref(), Tab::Enter, false).await?;
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Enter — {}", data.display_name), html! {
        : header;
        article {
            @match &*data.event {
                "2" => {
                    p {
                        : "To enter this tournament, join ";
                        a(href = "https://discord.gg/nRWrZDesP8") : "the OoT Randomizer Discord server";
                        : " and participate in the qualifier race, either live on ";
                        : format_datetime(America::Panama.with_ymd_and_hms(2023, 4, 8, 15, 0, 0).single().expect("wrong hardcoded datetime"), DateTimeFormat { long: true, running_text: true });
                        : " or async starting on April 2.";
                    }
                    p {
                        : "Note: This page is not official. See ";
                        a(href = "https://docs.google.com/document/d/1p8HAwWsjsLW7tjfDl2SK-yQ35pVqbAS9GB72bkOIDFI/edit") : "the official document";
                        : " for details.";
                    }
                }
                _ => @unimplemented
            }
        }
    }).await?)
}

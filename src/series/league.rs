use {
    rocket::response::content::RawHtml,
    rocket_util::html,
    sqlx::{
        Postgres,
        Transaction,
    },
    crate::{
        event::{
            Data,
            InfoError,
        },
        lang::Language::English,
    },
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "4" => Some(html! {
            article {
                p {
                    : "This is OoTR League season 4, organized by shaun1e, ";
                    : English.join_html(data.organizers(transaction).await?);
                    : ". See ";
                    a(href = "https://league.ootrandomizer.com/") : "the official website";
                    : " for details.";
                }
            }
        }),
        _ => None,
    })
}

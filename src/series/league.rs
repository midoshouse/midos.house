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
        util::natjoin_html,
    },
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<RawHtml<String>, InfoError> {
    Ok(match &*data.event {
        "4" => html! {
            article {
                p {
                    : "This is OoTR League season 4, organized by shaun1e, ";
                    : natjoin_html(data.organizers(transaction).await?);
                    : ". See ";
                    a(href = "https://league.ootrandomizer.com/") : "the official website";
                    : " for details.";
                }
            }
        },
        _ => unimplemented!(),
    })
}

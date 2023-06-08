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
        lang::Language::French,
    },
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "3" => Some(html! {
            article {
                p {
                    : "Ceci est la 3e saison du tournoi francophone, organisée par ";
                    : French.join_html(data.organizers(transaction).await?);
                    : ". Rejoignez ";
                    a(href = "https://discord.gg/wyhPVmquZC") : "le serveur Discord";
                    : " pour plus de détails.";
                }
            }
        }),
        _ => None,
    })
}

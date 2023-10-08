use crate::{
    event::{
        Data,
        InfoError,
    },
    prelude::*,
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "1" => Some(html! {
            article {
                p {
                    : "Voici la 1ère saison du tournoi organisé par ";
                    : French.join_html(data.organizers(transaction).await?);
                    : ". Rejoignez ";
                    a(href = "https://discord.gg/YKvbQSBT5") : "le serveur Discord";
                    : " pour plus de détails.";
                }
                p : "On utilisera le Standard Rulesets pour ce tournoi. Le KZ skip sera autorisé.";
            }
        }),
        _ => None,
    })
}

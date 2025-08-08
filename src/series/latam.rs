use crate::{
    event::{
        Data,
        InfoError,
    },
    prelude::*,
};

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "2025" => {
            let organizers = data.organizers(&mut *transaction).await?;
            Some(html! {
                article {
                    p(lang = "pt") {
                        : "Bem-vindos à primeira temporada da Copa Latinoamerica 2025! O torneio está sendo organizado por ";
                        : Portuguese.join_html_opt(&organizers);
                        : ". ";
                        a(href = "https://discord.gg/hRKZacDcTR") : "Junte-se ao nosso servidor do Discord";
                        : " para mais detalhes!";
                    }
                    p(lang = "es") {
                        : "Bienvenido a la primera temporada de la Copa Latinoamérica 2025. El torneo fue creado por ";
                        : Spanish.join_html_opt(&organizers);
                        : ". ";
                        a(href = "https://discord.gg/hRKZacDcTR") : "Únete a nuestro servidor de Discord";
                        : " para más informaciónes!";
                    }
                    p(lang = "en") {
                        : "Welcome to the first season of Copa Latinoamerica 2025! The tournament is organized by ";
                        : English.join_html_opt(organizers);
                        : ". Unfortunately only players from South America, Central America and Mexico can join the tournament. ";
                        a(href = "https://discord.gg/hRKZacDcTR") : "Join our Discord server";
                        : " for more details!";
                    }
                }
            })
        }
        _ => None,
    })
}

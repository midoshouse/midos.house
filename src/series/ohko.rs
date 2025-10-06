use {
    chrono::Days,
    crate::{
        event::{
            Data,
            InfoError,
        },
        prelude::*,
    },
};

pub(crate) fn next_s2_race_after(min_time: DateTime<impl TimeZone>) -> DateTime<Utc> {
    let mut time = Utc.with_ymd_and_hms(2025, 10, 18, 20, 0, 0).single().expect("wrong hardcoded datetime");
    while time <= min_time {
        let date = time.date_naive().checked_add_days(Days::new(14)).unwrap();
        time = date.and_hms_opt(20, 0, 0).unwrap().and_utc();
    }
    time
}

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "1" => Some(html! {
            article {
                p {
                    : "This is the first tournament season of Battle Royale, a game mode played on 1-hit KO where players complete challenges in the seed to score points without dying. This season is organised by ";
                    : English.join_html_opt(data.organizers(transaction).await?);
                    : ". See ";
                    a(href = "https://docs.google.com/document/d/1JB_CfbUFQwoTuV8RHniG1nfiXWki4n4NMFlKXDCp5P8/edit") : "the official document";
                    : " for details.";
                }
            }
        }),
        _ => None,
    })
}

pub(crate) fn s1_enter_form() -> RawHtml<String> {
    html! {
        article {
            p {
                : "To enter this tournament, either join the live qualifier on ";
                : format_datetime(Utc.with_ymd_and_hms(2024, 3, 9, 19, 0, 0).single().expect("wrong hardcoded datetime"), DateTimeFormat { long: true, running_text: true });
                : " or play the qualifier async (see ";
                a(href = "https://discord.com/channels/274180765816848384/1208046928504553483/1213524850627317830") : "this Discord message";
                : " for details).";
            }
            p {
                : "Note: This page is not official. See ";
                a(href = "https://docs.google.com/document/d/1JB_CfbUFQwoTuV8RHniG1nfiXWki4n4NMFlKXDCp5P8/edit") : "the official document";
                : " for details.";
            }
        }
    }
}

pub(crate) fn s2_enter_form() -> RawHtml<String> {
    html! {
        article {
            p {
                : "To enter this tournament, request the ";
                strong : "@battle royale";
                : " role on ";
                a(href = "https://discord.gg/ootrandomizer") : "the OoT Randomizer Discord server";
                : ". See ";
                a(href = "https://discord.com/channels/274180765816848384/1208046928504553483/1416697838007488572") : "this Discord message";
                : " for details.";
            }
        }
    }
}

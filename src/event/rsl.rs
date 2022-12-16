use {
    std::str::FromStr,
    enum_iterator::Sequence,
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
        util::natjoin_html,
    },
};

#[derive(Clone, Copy, Sequence, sqlx::Type)]
#[sqlx(type_name = "rsl_preset", rename_all = "lowercase")]
pub(crate) enum Preset {
    League,
    Beginner,
    Intermediate,
    Ddr,
    CoOp,
    Multiworld,
}

impl Preset {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::League => "league",
            Self::Beginner => "beginner",
            Self::Intermediate => "intermediate",
            Self::Ddr => "ddr",
            Self::CoOp => "coop",
            Self::Multiworld => "multiworld",
        }
    }

    pub(crate) fn race_info(&self) -> &'static str {
        match self {
            Self::League => "Random Settings League",
            Self::Beginner => "Random Settings for Beginners",
            Self::Intermediate => "Intermediate Random Settings",
            Self::Ddr => "Random Settings DDR",
            Self::CoOp => "Random Settings Co-Op",
            Self::Multiworld => "Random Settings Multiworld",
        }
    }
}

impl FromStr for Preset {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        Ok(match &*s.to_ascii_lowercase() {
            "league" | "rsl" | "solo" | "advanced" => Self::League,
            "beginner" => Self::Beginner,
            "intermediate" => Self::Intermediate,
            "ddr" => Self::Ddr,
            "coop" | "co-op" => Self::CoOp,
            "multiworld" | "mw" => Self::Multiworld,
            _ => return Err(()),
        })
    }
}

pub(super) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<RawHtml<String>, InfoError> {
    Ok(match &*data.event {
        "2" => html! {
            article {
                p : "This is an archive of the 2nd season of the Random Settings League tournament.";
                h2 : "See also";
                ul {
                    li {
                        a(href = "https://rsl-leaderboard.web.app/season2") : "Leaderboard (qualifiers)";
                    }
                }
            }
        },
        "3" => html! {
            article {
                p : "This is an archive of the 3rd season of the Random Settings League tournament.";
                h2 : "See also";
                ul {
                    li {
                        a(href = "https://rsl-leaderboard.web.app/season3") : "Leaderboard (qualifiers)";
                    }
                }
            }
        },
        "4" => html! {
            article {
                p : "This is an archive of the 4th season of the Random Settings League tournament.";
                h2 : "See also";
                ul {
                    li {
                        a(href = "https://rsl-leaderboard.web.app/season4") : "Leaderboard (qualifiers)";
                    }
                    li {
                        a(href = "https://docs.google.com/spreadsheets/d/1IyXCCq0iowzCoUH7mB8oSduiQU6QqLY6LE1nJEKUOMs/edit") : "Swiss pairings";
                    }
                }
            }
        },
        "5" => html! {
            article {
                p {
                    : "This is the 5th season of the Random Settings League tournament, organized by Cubsrule21, ";
                    : natjoin_html(data.organizers(transaction).await?);
                    : ". See ";
                    a(href = "https://docs.google.com/document/d/1Js03yFcMw_mWx4UO_3UJB39CNCKa0bsxlBEYrHPq5Os/edit") : "the official document";
                    : " for details.";
                }
            }
        },
        _ => unimplemented!(),
    })
}

pub(super) async fn enter_form(mut transaction: Transaction<'_, Postgres>, me: Option<User>, uri: Origin<'_>, data: Data<'_>) -> Result<RawHtml<String>, Error> {
    let header = data.header(&mut transaction, me.as_ref(), Tab::Enter).await?;
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Enter — {}", data.display_name), html! {
        : header;
        article {
            @match &*data.event {
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

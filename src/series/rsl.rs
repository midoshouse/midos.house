use crate::{
    event::{
        Data,
        InfoError,
    },
    prelude::*,
};

#[derive(Debug, Default, Clone, Copy, Sequence, sqlx::Type)]
#[sqlx(type_name = "rsl_preset", rename_all = "lowercase")]
#[cfg_attr(unix, derive(Protocol))]
pub(crate) enum Preset {
    #[default]
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
            Self::Beginner => "RSL-Lite",
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
            "beginner" | "lite" => Self::Beginner,
            "intermediate" => Self::Intermediate,
            "ddr" => Self::Ddr,
            "coop" | "co-op" => Self::CoOp,
            "multiworld" | "mw" => Self::Multiworld,
            _ => return Err(()),
        })
    }
}

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "1" => Some(html! {
            article {
                p {
                    : "This is an archive of the 1st season of the Random Settings League tournament, organized by ";
                    : English.join_html(data.organizers(transaction).await?);
                    : ". See ";
                    a(href = "https://docs.google.com/document/d/1wmoZHdwYijJwXLYgZbadjRYOGBNXio2hhKEIkFNgDgU/edit") : "the official document";
                    : " for details.";
                }
            }
        }),
        "2" => Some(html! {
            article {
                p {
                    : "This is an archive of the 2nd season of the Random Settings League tournament, organized by ";
                    : English.join_html(data.organizers(transaction).await?);
                    : ".";
                }
                h2 : "See also";
                ul {
                    li {
                        a(href = "https://rsl-leaderboard.web.app/season2") : "Leaderboard (qualifiers)";
                    }
                }
            }
        }),
        "3" => Some(html! {
            article {
                p {
                    : "This is an archive of the 3rd season of the Random Settings League tournament, organized by ";
                    : English.join_html(data.organizers(transaction).await?);
                    : ".";
                }
                h2 : "See also";
                ul {
                    li {
                        a(href = "https://rsl-leaderboard.web.app/season3") : "Leaderboard (qualifiers)";
                    }
                }
            }
        }),
        "4" => Some(html! {
            article {
                p {
                    : "This is an archive of the 4th season of the Random Settings League tournament, organized by ";
                    : English.join_html(data.organizers(transaction).await?);
                    : ".";
                }
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
        }),
        "5" => Some(html! {
            article {
                p {
                    : "This is the 5th season of the Random Settings League tournament, organized by ";
                    : English.join_html(data.organizers(transaction).await?);
                    : ". See ";
                    a(href = "https://docs.google.com/document/d/1Js03yFcMw_mWx4UO_3UJB39CNCKa0bsxlBEYrHPq5Os/edit") : "the official document";
                    : " for details.";
                }
            }
        }),
        "6" => Some(html! {
            article {
                p {
                    : "This is the 6th season of the Random Settings League tournament, organized by ";
                    : English.join_html(data.organizers(transaction).await?);
                    : ". See ";
                    a(href = "https://docs.google.com/document/d/1xpZIVh6znG7mgyEUQk8J2B-_5PfbcERen-P4tDX6VqE/edit") : "the official document";
                    : " for details.";
                }
            }
        }),
        _ => None,
    })
}

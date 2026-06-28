use {
    kuchiki::traits::TendrilSink as _,
    crate::{
        auth::random_password,
        event::{
            Data,
            InfoError,
        },
        prelude::*,
        racetime_bot::PrerollMode,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, FromFormField, UriDisplayQuery, Sequence)]
pub(crate) enum Format {
    League,
    Sgl,
    Saws,
    Bingo,
    Ice,
    Mixed,
    Franco,
    Triforce,
}

impl Format {
    pub(crate) fn slug(&self) -> &'static str {
        match self {
            Self::League => "league",
            Self::Sgl => "sgl",
            Self::Saws => "saws",
            Self::Bingo => "bingo",
            Self::Ice => "ice",
            Self::Mixed => "mixed",
            Self::Franco => "franco",
            Self::Triforce => "triforce",
        }
    }

    pub(crate) fn display_name(&self) -> &'static str {
        match self {
            Self::League => "League S9",
            Self::Sgl => "SGSS 2026",
            Self::Saws => "SAWS Beginner",
            Self::Bingo => "Bingo",
            Self::Ice => "Ice%",
            Self::Mixed => "Mixed Pools 2026",
            Self::Franco => "Franco 2026",
            Self::Triforce => "Triforce Hunt",
        }
    }

    pub(crate) fn article(&self) -> &'static str {
        match self {
            Self::League | Self::Saws | Self::Bingo | Self::Mixed | Self::Franco | Self::Triforce => "a",
            Self::Sgl | Self::Ice => "an",
        }
    }

    pub(crate) fn for_race(race: &Race) -> Option<Self> {
        if let Series::SlugOpen = race.series {
            race.draft.as_ref().and_then(|draft| draft.settings.get("sco_format")).map(|s| s.parse().expect("unexpected SlugCentral Open format"))
        } else {
            None
        }
    }

    pub(crate) fn draft_kind(&self) -> Option<draft::Kind> {
        match self {
            Self::Franco => Some(draft::Kind::TournoiFrancoS6),
            Self::League | Self::Sgl | Self::Saws | Self::Bingo | Self::Ice | Self::Mixed | Self::Triforce => None,
        }
    }

    pub(crate) fn default_race_duration(&self) -> TimeDelta {
        match self {
            Self::Ice => TimeDelta::minutes(30),
            Self::Sgl | Self::Bingo /*TODO verify */ | Self::Mixed => TimeDelta::hours(3),
            Self::League | Self::Saws | Self::Franco | Self::Triforce /*TODO verify */ => TimeDelta::hours(3) + TimeDelta::minutes(30),
        }
    }

    pub(crate) fn preroll_seeds(&self) -> PrerollMode {
        PrerollMode::Medium
    }

    pub(crate) async fn single_settings(&self, global: &GlobalState, bingo_room_name: Option<&str>) -> Result<Option<(VersionedBranch, seed::Settings, Option<String>)>, SingleSettingsError> {
        let preset = match self {
            Self::League => "League S9",
            Self::Sgl => "SGL 2026 Tournament",
            Self::Saws => "Standard Anti-Weekly Settings (Beginner)",
            Self::Bingo => "SDG Bingo Tournament 3",
            Self::Ice => "Ice%",
            Self::Mixed => "5th Mixed Pools Tournament",
            Self::Franco => return Ok(None), // settings draft
            Self::Triforce => "SlugCentral Open Triforce Hunt",
        };
        ootr_utils::Branch::DevFenhl.clone_repo(ALLOW_RIIR, true).await?;
        let mut presets = fs::read_json::<HashMap<String, seed::Settings>>(ootr_utils::Branch::DevFenhl.dir(ALLOW_RIIR)?.join("data").join("presets_default.json")).await?;
        let mut settings = presets.remove(preset).ok_or(SingleSettingsError::MissingPreset(*self, preset))?;
        settings.insert(format!("password_lock"), json!(true));
        let bingo_passphrase = if let Self::Bingo = self {
            if let Some(room_name) = bingo_room_name {
                #[derive(Serialize)]
                struct BingoForm<'a> {
                    csrfmiddlewaretoken: String,
                    room_name: &'a str,
                    passphrase: String,
                    nickname: &'static str,
                    game_type: u8,
                    variant_type: u8,
                    lockout_mode: u8,
                    is_spectator: bool,
                    hide_card: bool,
                }

                let index = global.http_client.get("https://bingosync.com/")
                    .send().await?
                    .detailed_error_for_status().await?
                    .text().await?;
                let csrfmiddlewaretoken = kuchiki::parse_html().one(index)
                    .select_first("input[name=csrfmiddlewaretoken]").map_err(|()| SingleSettingsError::BingoIndex)?
                    .attributes
                    .borrow_mut()
                    .remove("value")
                    .ok_or(SingleSettingsError::BingoIndex)?
                    .value;
                let passphrase = random_password(&mut rng(), 8);
                let response = global.http_client.post("https://bingosync.com/")
                    .form(&BingoForm {
                        passphrase: passphrase.clone(),
                        nickname: "Mido",
                        game_type: 1, // OoT
                        variant_type: 90, // Item Randomizer Blackout
                        lockout_mode: 1, // Non-Lockout
                        is_spectator: true,
                        hide_card: true,
                        csrfmiddlewaretoken, room_name,
                    })
                    .send().await?
                    .detailed_error_for_status().await?;
                settings.insert(format!("bingosync_url"), json!(response.url()));
                Some(passphrase)
            } else {
                Some(String::default())
            }
        } else {
            None
        };
        Ok(Some((VersionedBranch::Latest { branch: ootr_utils::Branch::DevFenhl }, settings, bingo_passphrase)))
    }
}

impl FromStr for Format {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        all::<Self>().find(|format| format.slug() == s).ok_or_else(|| s.to_owned())
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SingleSettingsError {
    #[error(transparent)] Clone(#[from] ootr_utils::CloneError),
    #[error(transparent)] Dir(#[from] ootr_utils::DirError),
    #[error(transparent)] Http(#[from] reqwest::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[error("failed to parse Bingosync index page")]
    BingoIndex,
    #[error("the settings preset {1:?} for SlugCentral Open format {0:?} is not available on the dev-fenhl branch of the randomizer")]
    MissingPreset(Format, &'static str),
}

impl IsNetworkError for SingleSettingsError {
    fn is_network_error(&self) -> bool {
        match self {
            Self::Clone(_) => false, //TODO implement IsNetworkError for ootr_utils::CloneError
            Self::Dir(_) => false,
            Self::Http(e) => e.is_network_error(),
            Self::Wheel(e) => e.is_network_error(),
            Self::BingoIndex => false,
            Self::MissingPreset(_, _) => false,
        }
    }
}

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "2026" => Some(html! {
            div(class = "toc") {
                article {
                    h2 : "Welcome to the Slug Open Tournament";
                    p {
                        : "Organized by ";
                        : English.join_html_opt(data.organizers(transaction).await?);
                    }
                    h2(id = "tournament-format") : "Tournament Format";
                    ul {
                        li : "1v1 format with teams of 3.";
                        li : "Each round is 3 matches. Each member of the team plays in one of the three matches per round.";
                        li : "Every match win will be awarded with 1 point. This means you can earn up to 3 points per round played.";
                        li : "The tournament will have a swiss phase and then a single elimination bracket phase.";
                        li : "The number of swiss rounds and teams in the bracket phase will be decided after sign-ups are complete. It is expected that there will be 3-5 swiss rounds and the top 8 gets put into brackets.";
                        li {
                            : "Each round is a different format based of a list of 8 pre-determined settings:";
                            ul {
                                li : "League S9 | SGSS 2026 | SAWS Beginner | Bingo";
                                li : "Ice% | Mixed Pools 2026 | Triforce Hunt | Franco 2026 (Standard Draft)";
                            }
                        }
                        li {
                            : "Clarifications for the formats are here: ";
                            a(href = "https://docs.google.com/document/d/1usEOfOk37gSWK8WUxMh2Ae8IuX7iD9gLcMNa6_AE6o0/edit") : "Slug Open Complete settings List";
                        }
                        li : "“The Wheel” bans one format and picks one setting per round for everyone. After this each team bans and picks 1 format from the remaining 6 formats.";
                        li {
                            : "Bans and Picks from “The Wheel” won't be consecutive.";
                            br;
                            : "MidoBot will choose at random which team goes first in the draft.";
                        }
                        li {
                            : "Draft order:";
                            br;
                            : "“The Wheel” - Ban";
                            br;
                            : "“The Wheel” - Pick";
                            br;
                            : "Team 1 - Ban";
                            br;
                            : "Team 2 - Ban";
                            br;
                            : "Team 1 - Pick";
                            br;
                            : "Team 2 - Pick";
                        }
                        li {
                            : "There will be a Mido command where you can set your players that will remain hidden until the other team picks their players.";
                            br;
                            : "Once both teams have their order set, you can schedule your match.";
                        }
                        li : "The teams will be allowed to be in a voice call with their playing teammate and help them with general things they might not be familiar with in regards to the settings. Also allowed will be reminders about the seed and suggestions for places to go. This is dependent on both teams being available. If one team can't be in VC for whatever reason, the other team is allowed to be in VC, but no helping is allowed then. The teammates have to be able to be heard on stream.";
                        li {
                            : "All settings will be able to be rolled on Fenhl's Dev Branch: ";
                            a(href = "https://ootrandomizer.com/generatorDev?version=devFenhl_") : "https://ootrandomizer.com/generatorDev?version=devFenhl_";
                        }
                    }
                    h2(id = "ruleset") : "Ruleset";
                    p {
                        : "The Slug Open Tournament will be operating under the current Standard ruleset. You can find the ruleset here: ";
                        a(href = "https://wiki.ootrandomizer.com/index.php?title=Standard") : "OoTR Standard Racing Ruleset";
                    }
                    p : "There is one exception to this: We are allowing you to clip out of bounds in Zora's Domain to get into Zora's fountain in all formats. The KZ skip skip if you will.";
                    h2(id = "brackets") : "Brackets";
                    p : "The tournament will feature a Swiss format into a Top 8 bracket. To get into Top 8 you need an amount of points which will be announced once all teams have signed up.";
                    p : "Timeline for The Slug Open Tournament:";
                    ul {
                        li : "Deadline for registering your team: July 31st 23:59 CEST";
                        li : "Start of Swiss format: August 5th (After “The Wheel”)";
                    }
                    h2(id = "scheduling") : "Scheduling";
                    p {
                        : "Each round of matches will take 2 weeks from the time “The Wheel” has picked and banned the settings. This draw will take place after the start of the tournament each second Wednesday at 6PM EST on ";
                        a(href = "https://www.twitch.tv/slugcentral") : "www.twitch.tv/slugcentral";
                        : ".";
                    }
                    h2(id = "asynchronous-matches-asyncs") : "Asynchronous Matches (asyncs)";
                    p : "We are allowing asyncs, if no live match is possible.";
                    p : "Here are the guidelines:";
                    ul {
                        li : "No breaks";
                        li : "15 minute FPA time allowed";
                        li : "After scheduling your async, MidoBot will post a message in your scheduling thread taking you to the race page on midos.house where you will get access to the seed. You must start the race within 10 minutes of obtaining the seed and submit your time within 5 minutes of finishing.";
                        li : "If you obtain a seed but do not submit a finish time, it will count as a forfeit.";
                    }
                    p {
                        : "Here are ";
                        em : "additional";
                        : " guidelines for the ";
                        strong : "first person playing";
                        : ":";
                    }
                    ul {
                        p : "No streaming allowed";
                        p : "Unlisted upload on YouTube. Please note that the result can already be submitted before YouTube has fully processed the upload.";
                        p : "If your teammate is in VC with you their voice needs to be recorded as well.";
                    }
                    p {
                        : "If you are the ";
                        strong : "second person playing";
                        : ", you must stream your async live on Twitch.";
                    }
                    h2(id = "fair-play-agreement-fpa") : "Fair Play Agreement (FPA)";
                    p {
                        : "The ";
                        a(href = "https://wiki.ootrandomizer.com/index.php?title=Fair_Play_Agreement") : "Fair Play Agreement";
                        : " is mandatory for all runners.";
                    }
                    h2(id = "coverage") : "Coverage";
                    p {
                        : "Feel free to restream any matches you want, provided you get permission from the players. There might be some restreams happening on ";
                        a(href = "https://www.twitch.tv/slugcentral") : "www.twitch.tv/slugcentral";
                    }
                    h2(id = "important-links") : "Important Links";
                    ul {
                        li {
                            a(href = "https://discord.gg/5KttgWp8AZ") : "The Side Quest Discord";
                        }
                        li {
                            a(href = "https://docs.google.com/document/d/1usEOfOk37gSWK8WUxMh2Ae8IuX7iD9gLcMNa6_AE6o0/edit") : "Slug Open Complete Format List";
                        }
                        li {
                            a(href = "https://docs.google.com/document/d/1sUHqnkAW3VCX0IHawehaVzgxh8U_0V4Mm0JP9W1EcNY/edit?usp=sharing") : "Bingo Goal Clarifications";
                        }
                    }
                }
                div {
                    nav {
                        strong : "Contents";
                        ul {
                            li {
                                a(href = "#tournament-format") : "Tournament Format";
                            }
                            li {
                                a(href = "#ruleset") : "Ruleset";
                            }
                            li {
                                a(href = "#brackets") : "Brackets";
                                ul {
                                    li {
                                        a(href = "#scheduling") : "Scheduling";
                                    }
                                    li {
                                        a(href = "#asynchronous-matches-asyncs") : "Asynchronous Matches (asyncs)";
                                    }
                                }
                            }
                            li {
                                a(href = "#fair-play-agreement-fpa") : "Fair Play Agreement (FPA)";
                            }
                            li {
                                a(href = "#coverage") : "Coverage";
                            }
                            li {
                                a(href = "#important-links") : "Important Links";
                            }
                        }
                    }
                }
            }
        }),
        _ => None,
    })
}

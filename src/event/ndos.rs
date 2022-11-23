use {
    futures::stream::{
        self,
        StreamExt as _,
        TryStreamExt as _,
    },
    itertools::Itertools as _,
    rocket::{
        form::Context,
        response::content::RawHtml,
        uri,
    },
    rocket_csrf::CsrfToken,
    rocket_util::{
        Origin,
        html,
    },
    sqlx::{
        PgPool,
        Postgres,
        Transaction,
    },
    crate::{
        auth,
        event::{
            Data,
            FindTeamError,
            InfoError,
            Series,
            Tab,
            TeamConfig,
        },
        http::{
            PageStyle,
            page,
        },
        user::User,
        util::{
            Id,
            natjoin_html,
            render_form_error,
        },
    },
};

pub(super) async fn info(pool: &PgPool, data: &Data<'_>) -> Result<RawHtml<String>, InfoError> {
    let organizers = stream::iter([
        5246396495391975113, // Kofca
    ])
        .map(Id)
        .then(|id| async move { User::from_id(pool, id).await?.ok_or(InfoError::OrganizerUserData) })
        .try_collect::<Vec<_>>().await?;
    Ok(html! {
        article {
            p {
                : "Day ";
                : data.event;
                : " of the ";
                a(href = "https://docs.google.com/document/d/1xELThZtIctwN-vYtYhUqtd88JigNzabk8OZHANa0gqY/edit") : "9 Days of SAWS";
                : " event, organized by ";
                : natjoin_html(organizers);
                : ", will be a ";
                @match &*data.event {
                    ("1" | "9") => : "SAWS (S6)";
                    ("2" | "6" | "7") => : "SAWS (Beginner)";
                    "3" => : "SAWS (Advanced)";
                    "4" => : "SAWS (S5) + one bonk KO";
                    "5" => : "SAWS (Beginner) + mixed pools";
                    "8" => : "SAWS (S6) + dungeon ER";
                    _ => @unimplemented
                }
                : " race";
                @match &*data.event {
                    ("1" | "3" | "4" | "5" | "7" | "9") => {}
                    ("2" | "8") => : " with 2-player co-op teams";
                    "6" => : " with 3-player multiworld teams";
                    _ => @unimplemented
                }
                : ".";
            }
            h2 : "Rules";
            p {
                : "Follow the ";
                a(href = "https://wiki.ootrandomizer.com/index.php?title=Rules#Universal_Rules") : "Universal Rules";
                : " and the ";
                a(href = "https://wiki.ootrandomizer.com/index.php?title=Standard") : "Standard";
                @if data.event == "6" {
                    : " ruleset, with a few exceptions listed below:";
                } else {
                    : " ruleset, with the exception that streaming is not required.";
                }
            }
            @if data.event == "6" {
                ul {
                    li : "Fire Arrow Entry is allowed";
                    li : "Streaming is not required";
                }
            }
            @if let TeamConfig::CoOp | TeamConfig::Multiworld = data.team_config() {
                p {
                    : "Each team will be ranked by the average of the finish times of its members. Timing for an individual player ends on the first frame of the cutscene that plays upon killing Ganon. Players are allowed to kill Ganon to stop their timer and then reset their game, allowing them to continue ";
                    @match data.team_config() {
                        TeamConfig::CoOp => : "exploring the seed";
                        TeamConfig::Multiworld => : "collecting items for their team";
                        _ => @unimplemented
                    }
                    : " if necessary.";
                }
            }
            h2 : "Settings";
            p {
                : "The seed will be rolled on ";
                a(href = "https://ootrandomizer.com/generatorDev?version=devFenhl_6.9.14") : "version 6.9.14 Fenhl-2";
                : " of the randomizer using the ";
                @match &*data.event {
                    ("1" | "4" | "8" | "9") => : "Standard Anti-Weekly Settings (S6)";
                    ("2" | "5" | "6" | "7") => : "Standard Anti-Weekly Settings (Beginner)";
                    "3" => : "Standard Anti-Weekly Settings (Advanced)";
                    _ => @unimplemented
                }
                @match &*data.event {
                    ("1" | "2" | "3" | "6" | "7" | "9") => : " preset.";
                    "4" => {
                        : " preset, with the following changes:";
                        ul {
                            li : "No dungeon boss shortcuts";
                            li : "Spawn shuffled for both ages";
                            li : "“Fix broken drops” off";
                            li : "Minimal item pool";
                            li : "Blue Fire Arrows off";
                            li : "No ice traps";
                            li : "One Bonk KO";
                            li : "Standard S5 Tournament hint distribution";
                        }
                    }
                    "5" => {
                        : " preset, with the following changes:";
                        ul {
                            li : "All interior entrances shuffled";
                            li : "Grotto entrances shuffled";
                            li : "Dungeon entrances shuffled (including Ganon's Castle)";
                            li : "Overworld entrances shuffled";
                            li : "Mixed entrance pools (interiors, grottos, dungeons, and overworld)";
                            li : "Gerudo Valley exit to Lake Hylia shuffled (full)";
                            li : "Owl drops shuffled (full)";
                            li : "Warp song destinations shuffled (full)";
                            li : "Blue warps lead to the shuffled entrances of the dungeons they're in";
                        }
                    }
                    "8" => {
                        : " preset, with the following changes:";
                        ul {
                            li : "Dungeon entrances shuffled (except Ganon's Castle)";
                            li : "Blue warps lead to the shuffled entrances of the dungeons they're in";
                        }
                    }
                    _ => @unimplemented
                }
            }
        }
    })
}

#[allow(unused_qualifications)] // rocket endpoint and uri macros don't work with relative module paths
pub(super) async fn coop_find_team_form(mut transaction: Transaction<'_, Postgres>, me: Option<User>, uri: Origin<'_>, csrf: Option<CsrfToken>, data: Data<'_>, context: Context<'_>) -> Result<RawHtml<String>, FindTeamError> {
    let header = data.header(&mut transaction, me.as_ref(), Tab::FindTeam).await?;
    let mut me_listed = false;
    let mut looking_for_team = Vec::default();
    for row in sqlx::query!(r#"SELECT user_id AS "user!: Id" FROM looking_for_team WHERE series = $1 AND event = $2"#, data.series as _, &data.event).fetch_all(&mut *transaction).await? {
        let user = User::from_id(&mut transaction, row.user).await?.ok_or(FindTeamError::UnknownUser)?;
        if me.as_ref().map_or(false, |me| user.id == me.id) { me_listed = true }
        looking_for_team.push(user);
    }
    let form = if me.is_some() {
        let errors = context.errors().collect_vec();
        if me_listed {
            None
        } else {
            let form_content = html! {
                : csrf;
                legend {
                    : "Click this button to add yourself to the list below.";
                }
                fieldset {
                    input(type = "submit", value = "Looking for Team");
                }
            };
            Some(html! {
                form(action = uri!(super::find_team_post(data.series, &*data.event)).to_string(), method = "post") {
                    @for error in errors {
                        : render_form_error(error);
                    }
                    : form_content;
                }
            })
        }
    } else {
        Some(html! {
            article {
                p {
                    a(href = uri!(auth::login(Some(uri!(super::find_team(data.series, &*data.event))))).to_string()) : "Sign in or create a Mido's House account";
                    : " to add yourself to this list.";
                }
            }
        })
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests(), ..PageStyle::default() }, &format!("Find Teammates — {}", data.display_name), html! {
        : header;
        : form;
        table {
            thead {
                tr {
                    th : "User";
                }
            }
            tbody {
                @if looking_for_team.is_empty() {
                    tr {
                        td {
                            i : "(no one currently looking for teammates)";
                        }
                    }
                } else {
                    @for user in looking_for_team {
                        tr {
                            td : user;
                        }
                    }
                }
            }
        }
    }).await?)
}

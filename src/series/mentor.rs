use crate::{
    event::{
        Data,
        Error,
        Tab,
        enter,
    },
    prelude::*,
    series::pic::EnterFormDefaults,
};

#[derive(Clone, Copy, PartialEq, Eq, FromFormField, UriDisplayQuery)]
pub(crate) enum Role {
    #[field(value = "sheikah")]
    Sheikah,
    #[field(value = "gerudo")]
    Gerudo,
}

impl ToHtml for Role {
    fn to_html(&self) -> RawHtml<String> {
        match self {
            Self::Sheikah => html! {
                span(class = "sheikah") : "mentor";
            },
            Self::Gerudo => html! {
                span(class = "gerudo") : "mentee";
            },
        }
    }
}

impl TryFrom<event::Role> for Role {
    type Error = ();

    fn try_from(role: event::Role) -> Result<Self, ()> {
        match role {
            event::Role::Sheikah => Ok(Self::Sheikah),
            event::Role::Gerudo => Ok(Self::Gerudo),
            _ => Err(()),
        }
    }
}

impl From<Role> for event::Role {
    fn from(role: Role) -> Self {
        match role {
            Role::Sheikah => Self::Sheikah,
            Role::Gerudo => Self::Gerudo,
        }
    }
}

pub(crate) async fn enter_form(mut transaction: Transaction<'_, Postgres>, global: &GlobalState, me: Option<User>, uri: Origin<'_>, csrf: Option<&CsrfToken>, data: Data<'_>, defaults: EnterFormDefaults<'_>) -> Result<RawHtml<String>, Error> {
    let header = data.header(&mut transaction, global, me.as_ref(), csrf, Tab::Enter, false).await?;
    Ok(page(transaction, global, &me, &uri, PageStyle::new(data.chests().await?), &format!("Enter — {}", data.display_name), if me.is_some() {
        let mut errors = defaults.errors();
        html! {
            : header;
            : full_form(uri!(enter::post(data.series, &*data.event)), csrf, html! {
                legend {
                    : "Fill out this form to enter the event as a team. Your teammate will receive an invitation they have to accept to confirm the signup. If you don't have a team yet, you can ";
                    @if let Some(ref find_team_url) = data.find_team_url {
                        a(href = find_team_url) : "look for a teammate";
                    } else {
                        a(href = uri!(event::find_team(data.series, &*data.event))) : "look for a teammate";
                    }
                    : " instead.";
                }
                : form_field("team_name", &mut errors, html! {
                    label(for = "team_name") : "Team Name:";
                    input(type = "text", name = "team_name", value? = defaults.team_name());
                    label(class = "help") : "(Optional unless you want to be on restream. Can be changed later. Organizers may remove inappropriate team names.)";
                });
                : form_field("my_role", &mut errors, html! {
                    label(for = "my_role") : "My Role:";
                    input(id = "my_role-sheikah", class = "sheikah", type = "radio", name = "my_role", value = "sheikah", checked? = defaults.my_role() == Some(pic::Role::Sheikah));
                    label(class = "sheikah", for = "my_role-sheikah") : "Mentor";
                    input(id = "my_role-gerudo", class = "gerudo", type = "radio", name = "my_role", value = "gerudo", checked? = defaults.my_role() == Some(pic::Role::Gerudo));
                    label(class = "gerudo", for = "my_role-gerudo") : "Mentee";
                });
                : form_field("teammate", &mut errors, html! {
                    label(for = "teammate") : "Teammate:";
                    input(type = "text", name = "teammate", value? = defaults.teammate_text().as_deref());
                    label(class = "help") : "(Enter your teammate's Mido's House user ID. It can be found on their profile page.)"; //TODO add JS-based user search?
                });
            }, errors, "Enter");
        }
    } else {
        html! {
            : header;
            article {
                p {
                    a(href = uri!(auth::login(Some(uri!(enter::get(data.series, &*data.event, defaults.my_role(), defaults.teammate())))))) : "Sign in or create a Mido's House account";
                    : " to enter this event.";
                }
            }
        }
    }).await?)
}

pub(crate) fn settings_2026() -> seed::Settings {
    collect![
        format!("user_message") => json!("Mentor Tournament 2026"),
        format!("password_lock") => json!(true),
        format!("trials") => json!(0),
        format!("shuffle_ganon_bosskey") => json!("remove"),
        format!("open_forest") => json!("closed_deku"),
        format!("open_kakariko") => json!("open"),
        format!("open_door_of_time") => json!("open"),
        format!("gerudo_fortress") => json!("fast"),
        format!("starting_age") => json!("random"),
        format!("spawn_positions") => json!([
            "child",
            "adult",
        ]),
        format!("free_bombchu_drops") => json!(false),
        format!("adult_trade_start") => json!([
            "Claim Check",
            "Eyedrops",
            "Eyeball Frog",
            "Prescription",
        ]),
        format!("shuffle_map") => json!("startwith"),
        format!("shuffle_compass") => json!("startwith"),
        format!("enhance_map_compass") => json!([
            "compass_reward",
        ]),
        format!("disabled_locations") => json!([
            "Song from Ocarina of Time",
            "Deku Theater Mask of Truth",
            "Kak 40 Gold Skulltula Reward",
            "Kak 50 Gold Skulltula Reward",
        ]),
        format!("allowed_tricks") => json!([
            "logic_grottos_without_agony",
            "logic_fewer_tunic_requirements",
            "logic_rusted_switches",
            "logic_man_on_roof",
            "logic_windmill_poh",
            "logic_crater_bean_poh_with_hovers",
            "logic_deku_b1_webs_with_bow",
            "logic_dc_scarecrow_gs",
            "logic_dc_jump",
            "logic_lens_botw",
            "logic_child_deadhand",
            "logic_forest_vines",
            "logic_lens_shadow",
            "logic_lens_shadow_platform",
            "logic_lens_bongo",
            "logic_lens_spirit",
            "logic_lens_gtg",
            "logic_lens_castle",
        ]),
        format!("starting_items") => json!({
            "Deku Shield": 1,
            "Prelude of Light": 1,
            "Ocarina": 1,
            "Zeldas Letter": 1,
        }),
        format!("add_random_starting_items") => json!(true),
        format!("random_starting_items_exclude") => json!([
            "songs",
            "bombchus",
            "shields",
            "deku_upgrades",
            "health_upgrades",
            "junk",
        ]),
        format!("random_starting_items_count") => json!(1),
        format!("start_with_consumables") => json!(true),
        format!("skip_reward_from_rauru") => json!("free"),
        format!("no_escape_sequence") => json!(true),
        format!("no_guard_stealth") => json!(true),
        format!("no_epona_race") => json!(true),
        format!("skip_some_minigame_phases") => json!(true),
        format!("scarecrow_behavior") => json!("fast"),
        format!("fast_bunny_hood") => json!(true),
        format!("ruto_already_f1_jabu") => json!(true),
        format!("fast_shadow_boat") => json!(true),
        format!("chicken_count") => json!(3),
        format!("big_poe_count") => json!(1),
        format!("hint_dist_user") => json!({
            "name": "Mentor2026",
            "gui_name": "Mentor Tournament",
            "description": "Hint Distribution for the 2026 Mentor Tournament: 4 Always, 5 Goal, 2 Barren, 3 Important Check, 2 Dual, 2 Sometimes; House of Skulltula (20/30).",
            "add_locations": [
                {
                    "location": "Sheik in Kakariko",
                    "types": [
                        "always",
                    ],
                },
                {
                    "location": "Deku Theater Skull Mask",
                    "types": [
                        "always",
                    ],
                },
            ],
            "remove_locations": [
                {
                    "location": "Sheik in Crater",
                    "types": [
                        "sometimes",
                    ],
                },
                {
                    "location": "GC Pot Freestanding PoH",
                    "types": [
                        "sometimes",
                    ],
                },
                {
                    "location": "GF HBA 1500 Points",
                    "types": [
                        "sometimes",
                    ],
                },
                {
                    "location": "Shadow Temple Freestanding Key",
                    "types": [
                        "sometimes",
                    ],
                },
                {
                    "location": "Ganons Castle Shadow Trial Golden Gauntlets Chest",
                    "types": [
                        "sometimes",
                    ],
                },
                {
                    "location": "Graveyard Dampe Race Rewards",
                    "types": [
                        "dual",
                    ],
                },
                {
                    "location": "Ganons Castle Spirit Trial Chests",
                    "types": [
                        "dual",
                    ],
                },
                {
                    "location": "GV Pieces of Heart Ledges",
                    "types": [
                        "dual",
                    ],
                },
                {
                    "location": "LH Adult Bean Destination Checks",
                    "types": [
                        "dual",
                    ],
                },
                {
                    "location": "ZD Child Checks",
                    "types": [
                        "dual",
                    ],
                },
                {
                    "location": "ZR Frogs Rewards",
                    "types": [
                        "dual",
                    ],
                },
                {
                    "location": "Kak 20 Gold Skulltula Reward",
                    "types": [
                        "sometimes",
                    ],
                },
                {
                    "location": "Dodongos Cavern Upper Business Scrubs",
                    "types": [
                        "dual",
                    ],
                },
            ],
            "add_items": [],
            "remove_items": [
                {
                    "item": "Zeldas Lullaby",
                    "types": [
                        "goal",
                    ],
                }
            ],
            "dungeons_barren_limit": 1,
            "one_hint_per_goal": true,
            "named_items_required": true,
            "vague_named_items": false,
            "use_default_goals": true,
            "distribution": {
                "trial": {
                    "order": 1,
                    "weight": 0,
                    "fixed": 0,
                    "copies": 0,
                },
                "entrance_always": {
                    "order": 2,
                    "weight": 0,
                    "fixed": 0,
                    "copies": 2,
                },
                "always": {
                    "order": 3,
                    "weight": 0,
                    "fixed": 4,
                    "copies": 2,
                    "remove_stones": [
                        "LH (Southeast Corner)",
                        "LH (Southwest Corner)",
                        "HC (Storms Grotto)",
                        "HF (Cow Grotto)",
                    ],
                },
                "goal": {
                    "order": 4,
                    "weight": 0,
                    "fixed": 5,
                    "copies": 2,
                    "remove_stones": [
                        "LH (Southeast Corner)",
                        "LH (Southwest Corner)",
                        "HC (Storms Grotto)",
                        "HF (Cow Grotto)",
                    ],
                },
                "barren": {
                    "order": 5,
                    "weight": 0,
                    "fixed": 2,
                    "copies": 2,
                    "remove_stones": [
                        "LH (Southeast Corner)",
                        "LH (Southwest Corner)",
                        "HC (Storms Grotto)",
                        "HF (Cow Grotto)",
                    ],
                },
                "entrance": {
                    "order": 6,
                    "weight": 0,
                    "fixed": 0,
                    "copies": 2,
                },
                "dual": {
                    "order": 7,
                    "weight": 0,
                    "fixed": 2,
                    "copies": 2,
                    "remove_stones": [
                        "LH (Southeast Corner)",
                        "LH (Southwest Corner)",
                        "HC (Storms Grotto)",
                        "HF (Cow Grotto)",
                    ],
                },
                "sometimes": {
                    "order": 8,
                    "weight": 1,
                    "fixed": 2,
                    "copies": 2,
                    "remove_stones": [
                        "LH (Southeast Corner)",
                        "LH (Southwest Corner)",
                        "HC (Storms Grotto)",
                        "HF (Cow Grotto)",
                    ],
                },
                "important_check": {
                    "order": 9,
                    "weight": 0,
                    "fixed": 3,
                    "copies": 2,
                    "remove_stones": [
                        "LH (Southeast Corner)",
                        "LH (Southwest Corner)",
                        "HC (Storms Grotto)",
                        "HF (Cow Grotto)",
                    ],
                },
                "junk": {
                    "order": 10,
                    "weight": 0,
                    "fixed": 4,
                    "copies": 1,
                    "priority_stones": [
                        "LH (Southeast Corner)",
                        "LH (Southwest Corner)",
                        "HC (Storms Grotto)",
                        "HF (Cow Grotto)",
                    ],
                },
            },
        }),
        format!("misc_hints") => json!([
            "altar",
            "ganondorf",
            "warp_songs_and_owls",
            "20_skulltulas",
            "30_skulltulas",
        ]),
        format!("correct_chest_appearances") => json!("both"),
        format!("correct_potcrate_appearances") => json!("off"),
        format!("key_appearance_match_dungeon") => json!(true),
        format!("blue_fire_arrows") => json!(true),
        format!("tcg_requires_lens") => json!(true),
        format!("junk_ice_traps") => json!("off"),
        format!("ice_trap_appearance") => json!("anything"),
    ]
}

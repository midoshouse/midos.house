use {
    serde_json::Value as Json,
    crate::{
        event::{
            AsyncKind,
            Data,
            Error,
            FindTeamError,
            InfoError,
            StatusContext,
            Tab,
            enter,
        },
        http,
        prelude::*,
    },
};

#[derive(Debug, Clone, Copy, sqlx::Type)]
#[sqlx(type_name = "mw_impl", rename_all = "snake_case")]
pub(crate) enum Impl {
    #[sqlx(rename = "bizhawk_co_op")]
    BizHawkCoOp,
    MidosHouse,
}

pub(crate) struct S3Setting {
    pub(crate) name: &'static str,
    pub(crate) display: &'static str,
    pub(crate) default: &'static str,
    pub(crate) default_display: &'static str,
    pub(crate) other: &'static [(&'static str, &'static str)],
    pub(crate) description: &'static str,
}

pub(crate) const S3_SETTINGS: [S3Setting; 8] = [
    S3Setting { name: "wincon", display: "win conditions", default: "meds", default_display: "default wincons", other: &[("scrubs", "Scrubs wincons"), ("th", "Triforce Hunt")], description: "wincon: meds (default: 6 Medallion Bridge + Keysy BK), scrubs (3 Stone Bridge + LACS BK), or th (Triforce Hunt 25/30)" },
    S3Setting { name: "dungeons", display: "dungeons", default: "tournament", default_display: "tournament dungeons", other: &[("skulls", "dungeon tokens"), ("keyrings", "keyrings")], description: "dungeons: tournament (default: keys shuffled in own dungeon), skulls (vanilla keys, dungeon tokens), or keyrings (small keyrings anywhere, vanilla boss keys)" },
    S3Setting { name: "er", display: "entrance rando", default: "off", default_display: "no ER", other: &[("dungeon", "dungeon ER")], description: "er: off (default) or dungeon" },
    S3Setting { name: "trials", display: "trials", default: "0", default_display: "0 trials", other: &[("2", "2 trials")], description: "trials: 0 (default) or 2" },
    S3Setting { name: "shops", display: "shops", default: "4", default_display: "shops 4", other: &[("off", "no shops")], description: "shops: 4 (default) or off" },
    S3Setting { name: "scrubs", display: "scrubs", default: "affordable", default_display: "affordable scrubs", other: &[("off", "no scrubs")], description: "scrubs: affordable (default) or off" },
    S3Setting { name: "fountain", display: "fountain", default: "closed", default_display: "closed fountain", other: &[("open", "open fountain")], description: "fountain: closed (default) or open" },
    S3Setting { name: "spawn", display: "spawns", default: "tot", default_display: "ToT spawns", other: &[("random", "random spawns & starting age")], description: "spawn: tot (default: adult start, vanilla spawns) or random (random spawns and starting age)" },
];

pub(crate) fn display_s3_draft_picks(picks: &draft::Picks) -> String {
    English.join_str(
        S3_SETTINGS.into_iter()
            .filter_map(|S3Setting { name, other, .. }| picks.get(name).and_then(|pick| other.iter().find(|(other, _)| pick == other)).map(|(_, display)| display)),
    ).unwrap_or_else(|| format!("base settings"))
}

pub(crate) fn resolve_s3_draft_settings(picks: &draft::Picks) -> serde_json::Map<String, Json> {
    let wincon = picks.get("wincon").map(|wincon| &**wincon).unwrap_or("meds");
    let dungeons = picks.get("dungeons").map(|dungeons| &**dungeons).unwrap_or("tournament");
    let er = picks.get("er").map(|er| &**er).unwrap_or("off");
    let trials = picks.get("trials").map(|trials| &**trials).unwrap_or("0");
    let shops = picks.get("shops").map(|shops| &**shops).unwrap_or("4");
    let scrubs = picks.get("scrubs").map(|scrubs| &**scrubs).unwrap_or("affordable");
    let fountain = picks.get("fountain").map(|fountain| &**fountain).unwrap_or("closed");
    let spawn = picks.get("spawn").map(|spawn| &**spawn).unwrap_or("tot");
    collect![
        format!("user_message") => json!("3rd Multiworld Tournament"),
        format!("world_count") => json!(3),
        format!("open_forest") => json!("open"),
        format!("open_kakariko") => json!("open"),
        format!("open_door_of_time") => json!(true),
        format!("zora_fountain") => json!(fountain),
        format!("gerudo_fortress") => json!("fast"),
        format!("bridge") => match wincon {
            "meds" => json!("medallions"),
            "scrubs" => json!("stones"),
            "th" => json!("dungeons"),
            _ => unreachable!(),
        },
        format!("bridge_medallions") => json!(6),
        format!("bridge_stones") => json!(3),
        format!("bridge_rewards") => json!(4),
        format!("triforce_hunt") => json!(wincon == "th"),
        format!("triforce_count_per_world") => json!(30),
        format!("triforce_goal_per_world") => json!(25),
        format!("trials") => match trials {
            "0" => json!(0),
            "2" => json!(2),
            _ => unreachable!(),
        },
        format!("shuffle_child_trade") => json!("skip_child_zelda"),
        format!("no_escape_sequence") => json!(true),
        format!("no_guard_stealth") => json!(true),
        format!("no_epona_race") => json!(true),
        format!("skip_some_minigame_phases") => json!(true),
        format!("free_scarecrow") => json!(true),
        format!("fast_bunny_hood") => json!(true),
        format!("start_with_rupees") => json!(true),
        format!("start_with_consumables") => json!(true),
        format!("big_poe_count") => json!(1),
        format!("shuffle_dungeon_entrances") => match er {
            "off" => json!("off"),
            "dungeon" => json!("simple"),
            _ => unreachable!(),
        },
        format!("spawn_positions") => json!(spawn == "random"),
        format!("shuffle_scrubs") => match scrubs {
            "affordable" => json!("low"),
            "off" => json!("off"),
            _ => unreachable!(),
        },
        format!("shopsanity") => json!(shops),
        format!("tokensanity") => match dungeons {
            "skulls" => json!("dungeons"),
            "tournament" | "keyrings" => json!("off"),
            _ => unreachable!(),
        },
        format!("shuffle_mapcompass") => json!("startwith"),
        format!("shuffle_smallkeys") => match dungeons {
            "tournament" => json!("dungeon"),
            "skulls" => json!("vanilla"),
            "keyrings" => json!("keysanity"),
            _ => unreachable!(),
        },
        format!("key_rings_choice") => match dungeons {
            "keyrings" => json!("all"),
            "tournament" | "skulls" => json!("off"),
            _ => unreachable!(),
        },
        format!("shuffle_bosskeys") => match dungeons {
            "tournament" => json!("dungeon"),
            "skulls" | "keyrings" => json!("vanilla"),
            _ => unreachable!(),
        },
        format!("shuffle_ganon_bosskey") => match wincon {
            "meds" => json!("remove"),
            "scrubs" => json!("on_lacs"),
            "th" => json!("triforce"),
            _ => unreachable!(),
        },
        format!("enhance_map_compass") => json!(true),
        format!("disabled_locations") => json!([
            "Deku Theater Mask of Truth",
            "Kak 40 Gold Skulltula Reward",
            "Kak 50 Gold Skulltula Reward",
        ]),
        format!("allowed_tricks") => json!([
            "logic_fewer_tunic_requirements",
            "logic_grottos_without_agony",
            "logic_child_deadhand",
            "logic_man_on_roof",
            "logic_dc_jump",
            "logic_rusted_switches",
            "logic_windmill_poh",
            "logic_crater_bean_poh_with_hovers",
            "logic_forest_vines",
            "logic_lens_botw",
            "logic_lens_castle",
            "logic_lens_gtg",
            "logic_lens_shadow",
            "logic_lens_shadow_platform",
            "logic_lens_bongo",
            "logic_lens_spirit",
            "logic_dc_scarecrow_gs",
        ]),
        format!("adult_trade_start") => json!(["Claim Check"]),
        format!("starting_items") => json!([
            "ocarina",
            "farores_wind",
            "lens",
        ]),
        format!("correct_chest_appearances") => json!("both"),
        format!("hint_dist") => json!("mw3"),
        format!("ice_trap_appearance") => json!("junk_only"),
        format!("junk_ice_traps") => json!("off"),
        format!("starting_age") => match spawn {
            "tot" => json!("adult"),
            "random" => json!("random"),
            _ => unreachable!(),
        },
    ]
}

pub(crate) fn s3_chests(picks: &draft::Picks) -> ChestAppearances {
    static WEIGHTS: Lazy<HashMap<String, Vec<(ChestAppearances, usize)>>> = Lazy::new(|| serde_json::from_str(include_str!("../../assets/event/mw/chests-3-6.2.181.json")).expect("failed to parse chest weights")); //TODO update to 6.2.205

    if let Some(settings_weights) = WEIGHTS.get(&display_s3_draft_picks(picks)) {
        settings_weights.choose_weighted(&mut thread_rng(), |(_, weight)| *weight).expect("failed to choose random chest textures").0
    } else {
        ChestAppearances::INVISIBLE
    }
}

pub(crate) fn s4_test_base_settings() -> serde_json::Map<String, Json> {
    collect![
        format!("user_message") => json!("4th Multiworld Tournament Test Settings"),
        format!("world_count") => json!(3),
        format!("open_forest") => json!("open"),
        format!("open_kakariko") => json!("open"),
        format!("open_door_of_time") => json!(true),
        format!("zora_fountain") => json!("closed"),
        format!("gerudo_fortress") => json!("fast"),
        format!("bridge") => json!("medallions"),
        format!("bridge_medallions") => json!(6),
        format!("bridge_stones") => json!(3),
        format!("bridge_rewards") => json!(4),
        format!("triforce_hunt") => json!(false),
        format!("triforce_count_per_world") => json!(30),
        format!("triforce_goal_per_world") => json!(25),
        format!("trials") => json!(0),
        format!("shuffle_child_trade") => json!([]),
        format!("no_escape_sequence") => json!(true),
        format!("no_guard_stealth") => json!(true),
        format!("no_epona_race") => json!(true),
        format!("skip_some_minigame_phases") => json!(true),
        format!("free_scarecrow") => json!(true),
        format!("fast_bunny_hood") => json!(true),
        format!("start_with_rupees") => json!(true),
        format!("start_with_consumables") => json!(true),
        format!("chicken_count") => json!(3),
        format!("big_poe_count") => json!(1),
        format!("ruto_already_f1_jabu") => json!(true),
        format!("shuffle_dungeon_entrances") => json!("off"),
        format!("spawn_positions") => json!([]),
        format!("free_bombchu_drops") => json!(false),
        format!("shuffle_scrubs") => json!("low"),
        format!("shopsanity") => json!("4"),
        format!("tokensanity") => json!("off"),
        format!("shuffle_mapcompass") => json!("startwith"),
        format!("shuffle_smallkeys") => json!("dungeon"),
        format!("key_rings_choice") => json!("off"),
        format!("shuffle_bosskeys") => json!("dungeon"),
        format!("shuffle_ganon_bosskey") => json!("medallions"),
        format!("enhance_map_compass") => json!(true),
        format!("disabled_locations") => json!([
            "Deku Theater Mask of Truth",
            "Kak 40 Gold Skulltula Reward",
            "Kak 50 Gold Skulltula Reward",
        ]),
        format!("allowed_tricks") => json!([
            "logic_fewer_tunic_requirements",
            "logic_grottos_without_agony",
            "logic_child_deadhand",
            "logic_man_on_roof",
            "logic_dc_jump",
            "logic_rusted_switches",
            "logic_windmill_poh",
            "logic_crater_bean_poh_with_hovers",
            "logic_forest_vines",
            "logic_lens_botw",
            "logic_lens_castle",
            "logic_lens_gtg",
            "logic_lens_shadow",
            "logic_lens_shadow_platform",
            "logic_lens_bongo",
            "logic_lens_spirit",
            "logic_dc_scarecrow_gs",
        ]),
        format!("adult_trade_start") => json!(["Claim Check"]),
        format!("starting_inventory") => json!([
            "ocarina",
            "farores_wind",
            "lens",
            "zeldas_letter",
        ]),
        format!("correct_chest_appearances") => json!("textures"),
        format!("key_appearance_match_dungeon") => json!(true),
        format!("hint_dist") => json!("mw3"),
        format!("misc_hints") => json!([
            "altar",
            "ganondorf",
            "warp_songs_and_owls",
            "20_skulltulas",
            "30_skulltulas",
            "unique_merchants",
            "frogs2",
        ]),
        format!("blue_fire_arrows") => json!(true),
        format!("ice_trap_appearance") => json!("junk_only"),
        format!("junk_ice_traps") => json!("off"),
        format!("starting_age") => json!("adult"),
    ]
}

pub(crate) async fn info(transaction: &mut Transaction<'_, Postgres>, data: &Data<'_>) -> Result<Option<RawHtml<String>>, InfoError> {
    Ok(match &*data.event {
        "1" => Some(html! {
            article {
                p {
                    : "This is an archive of the first Ocarina of Time randomizer multiworld tournament, organized by ";
                    : English.join_html(data.organizers(transaction).await?);
                    : ".";
                }
            }
        }),
        "2" => Some(html! {
            article {
                p {
                    : "This is an archive of the second Ocarina of Time randomizer multiworld tournament, organized by ";
                    : English.join_html(data.organizers(transaction).await?);
                    : ". Click the “teams” link above to see the results of the qualifier async.";
                }
                h2 : "See also";
                ul {
                    li {
                        a(href = "https://docs.google.com/document/d/e/2PACX-1vS6vGCH8ZTA5bDCv3Z8meiUK4hMEfWN3vLttjNIOXbIAbRFNuGi-NzwJ68o31gVJgUigblLmW2tkZRu/pub") : "Tournament format, rules, and settings";
                    }
                    li {
                        a(href = "https://challonge.com/OoTRMWSeason2Swiss") : "Swiss results";
                    }
                    li {
                        a(href = "https://docs.google.com/spreadsheets/d/101zNpL1uvmIONb59kXYVyoa7YaHy8Y_OJv3M3vOKdBA/edit#gid=104642672") : "Tiebreaker scoresheet";
                    }
                    li {
                        a(href = "https://challonge.com/OoTRMWSeason2Finals") : "Top 8 results";
                    }
                }
            }
        }),
        "3" => Some(html! {
            article {
                p {
                    : "Hello and welcome to the official rules document for the Ocarina of Time Randomizer Multiworld Tournament Season 3, organized by ";
                    : English.join_html(data.organizers(transaction).await?);
                    : ".";
                }
                h2 : "Tournament Format";
                p : "All teams are required to play a single asynchronous seed with the default race settings to participate. The results of this seed will be used to seed the settings draft.";
                p {
                    : "The tournament itself will begin with a randomly seeded series of ";
                    a(href = "https://en.wikipedia.org/wiki/Swiss-system_tournament") : "Swiss";
                    : " rounds. These will be played as best of 1. There will be 6 rounds, with each round lasting two weeks. In the event that teams do not schedule in time, tournament organizers will use their discretion to determine the correct outcome based on the failures to schedule. In unusual circumstances that affect the entire tournament base (such as a GDQ), a round can be extended at the discretion of tournament organizers.";
                }
                p : "After all Swiss rounds are done, plus an additional tiebreaker async, the top 8 teams will advance to a single elimination bracket to crown the champions. The bracket stage of the tournament will be played as best of 3.";
                h2 : "Match Format";
                p : "Each match will consist of a 3v3 Multiworld where both sides compete to see which team will have all three of its members beat the game with the lowest average time of finish:";
                ul {
                    li : "In a Triforce Hunt seed, timing for an individual player ends on the first completely black frame after that player has obtained the last required piece. (Due to how Triforce Hunt works in multiworld, all players on a team will normally have the same finish time, but if a player savescums a Triforce piece they found, they can have a lower Triforce count than their teammates.)";
                    li : "In all other seeds, timing for an individual player ends on the first frame of the cutscene that plays upon killing Ganon. Players are allowed to kill Ganon to stop their timer and then reset their game, allowing them to continue collecting items for their team if necessary.";
                }
                h2 : "Fair Play Agreement";
                p {
                    : "By joining this tournament, teams must accept the terms of the ";
                    a(href = "https://docs.google.com/document/d/e/2PACX-1vQd3S28r8SOBy-4C5Lxeu6nFAYpWgQqN9lCEKhLGTT3zcaXDSKj0iUnZv6UPo_GargUVQx5F-wOPUtJ/pub") : "Fair Play Agreement (FPA)";
                    : ", a system that can be invoked in the event of technical issues. If playing on BizHawk, it is a strong, strong suggestion to make sure you enable backup saves as documented ";
                    a(href = "https://wiki.ootrandomizer.com/index.php?title=Bizhawk#Enable_Save_Backup_In_Case_of_Crashes") : "here";
                    : ".";
                }
                h2 : "Seed Settings";
                p {
                    : "Starting with Swiss round 2, all tournament matches will be played on ";
                    a(href = "https://ootrandomizer.com/generatorDev?version=dev_6.2.205") : "version 6.2.205";
                    : " of the randomizer. (The qualifier async and Swiss round 1 were played on ";
                    a(href = "https://ootrandomizer.com/generatorDev?version=dev_6.2.181") : "version 6.2.181";
                    : ".)";
                }
                p : "The default settings for each race have the following differences to the S5 tournament preset:";
                ul {
                    li : "Forest: Open";
                    li : "Scrub Shuffle: On (Affordable)";
                    li : "Shopsanity: 4 Items per Shop";
                    li : "Starting Inventory: Ocarina, FW, Lens of Truth, Consumables, Rupees (no Deku Shield)";
                    li : "Free Scarecrow's Song";
                    li : "Starting Age: Adult";
                    li : "Randomize Overworld Spawns: Off";
                    li : "Excluded Locations: Kak 40/50 Gold Skulltula Reward";
                    li : "Adult Trade Quest: Claim Check Only";
                    li : "Enable Tricks: Dodongo's Cavern Scarecrow GS with Armos Statue";
                    li : "Chest Appearance Matches Contents: Both Size and Texture";
                    li : "Maps and Compasses Give Information: On";
                }
                p : "You can use the “Multiworld Tournament Season 3” preset to load these settings.";
                p : "However, in every race several of the settings may be modified by the teams. During Swiss, the team that placed higher in the qualifier async gets to pick who starts the procedure. For the first game of a top 8 match, this choice is made by the team with the higher seed in the bracket, and for subsequent games of a match, by the team that lost the previous game. Ties are broken by coin flip. The draft itself follows this pattern:";
                ul {
                    li(class = "sheikah") : "Ban";
                    li(class = "gerudo") : "Ban";
                    li(class = "sheikah") : "Pick";
                    li(class = "gerudo") : "2x Pick";
                    li(class = "sheikah") : "Pick";
                }
                p {
                    : "A ";
                    em : "ban";
                    : " allows a team to lock in a setting of their choice to the default. A ";
                    em : "pick";
                    : " will function just like last season, allowing a team to change a setting or lock it to the default as well. This drafting procedure takes place in the scheduling thread for the match and must be completed at least 30 minutes before the scheduled starting time so the seed can be rolled.";
                }
                p {
                    : "The settings that can be modified and their respective options (";
                    strong : "first";
                    : " being the default) are, roughly ordered by impact on how drastically seeds and their length can change:";
                }
                ul {
                    li {
                        : "Win Condition:";
                        ul {
                            li {
                                strong : "6 Medallion Rainbow Bridge + Remove Ganon’s Boss Key";
                            }
                            li : "3 Stone Rainbow Bridge + Vanilla LACS Ganon’s Boss Key";
                            li : "Triforce Hunt with 25/30 Triforce pieces per world + 4 Reward Rainbow Bridge";
                        }
                    }
                    li {
                        : "Dungeons:";
                        ul {
                            li {
                                strong : "Small Keys and Boss Keys shuffled inside their own dungeons";
                            }
                            li : "Small Keys and Boss Keys in their vanilla chests + dungeon Tokensanity";
                            li : "Boss Keys in their vanilla chests + Small Keys anywhere + Keyrings on";
                        }
                    }
                    li {
                        : "Shuffle Dungeon Entrances:";
                        ul {
                            li {
                                strong : "Off";
                            }
                            li : "On";
                        }
                    }
                    li {
                        : "Ganon's Trials:";
                        ul {
                            li {
                                strong : "0";
                            }
                            li : "2";
                        }
                    }
                    li {
                        : "Shopsanity:";
                        ul {
                            li {
                                strong : "4 Items per Shop + Random Prices";
                            }
                            li : "Off";
                        }
                    }
                    li {
                        : "Scrub Shuffle:";
                        ul {
                            li {
                                strong : "On (Affordable)";
                            }
                            li : "Off";
                        }
                    }
                    li {
                        : "Zora's Fountain:";
                        ul {
                            li {
                                strong : "Default Behavior (Closed)";
                            }
                            li : "Always Open";
                        }
                    }
                    li {
                        : "Starting Age/Randomize Overworld Spawns:";
                        ul {
                            li {
                                strong : "Adult/Off";
                            }
                            li : "Random/On";
                        }
                    }
                }
                h2 : "Hint Distribution";
                p : "Because of the somewhat unique settings of multiworld, there will be a custom hint distribution for this tournament. With 40 hint stones, the hint distribution will be as follows, with each hint having one duplicated hint:";
                ul {
                    li {
                        : "7 Goal/Path hints:";
                        ul {
                            li : "No dungeon limit";
                            li : "Zelda's Lullaby is never directly hinted";
                        }
                    }
                    li : "0 Foolish hints";
                    li {
                        : "5-8 “Always” Hints (Settings Dependent):";
                        ul {
                            li : "2 active Trials (if enabled)";
                            li : "Song from Ocarina of Time";
                            li : "Sheik in Kakariko";
                            li : "Deku Theater Skull Mask";
                            li : "Kak 30 Gold Skulltula Reward";
                            li : "ZR Frogs Ocarina Game";
                            li : "DMC Deku Scrub (if Scrubsanity enabled)";
                        }
                    }
                }
                p : "The remainder of the hints will be filled out with selections from the “Sometimes” hint pool for a total of 20 paired hints. The following additional locations are Sometimes hints (if dungeon Tokensanity is enabled):";
                ul {
                    li : "Deku Tree GS Basement Back Room";
                    li : "Water Temple GS River";
                    li : "Spirit Temple GS Hall After Sun Block Room";
                }
                p : "Always and Sometimes hint are upgraded to Dual hints where available.";
                p : "The following Sometimes hints have been removed:";
                ul {
                    li : "Sheik in Crater";
                    li : "Song from Royal Familys Tomb";
                    li : "Sheik in Forest";
                    li : "Sheik at Temple";
                    li : "Sheik at Colossus";
                    li : "LH Sun";
                    li : "GC Maze Left Chest";
                    li : "GV Chest";
                    li : "Graveyard Royal Familys Tomb Chest";
                    li : "GC Pot Freestanding PoH";
                    li : "LH Lab Dive";
                    li : "Fire Temple Megaton Hammer Chest";
                    li : "Fire Temple Scarecrow Chest";
                    li : "Water Temple Boss Key Chest";
                    li : "Water Temple GS Behind Gate";
                    li : "Gerudo Training Ground Maze Path Final Chest";
                    li : "Spirit Temple Silver Gauntlets Chest";
                    li : "Spirit Temple Mirror Shield Chest";
                    li : "Shadow Temple Freestanding Key";
                    li : "Ganons Castle Shadow Trial Golden Gauntlets Chest";
                }
                h2 : "Rules";
                p {
                    : "This tournament will take place under the ";
                    a(href = "https://wiki.ootrandomizer.com/index.php?title=Standard") : "Standard";
                    : " racing ruleset, with some slight modifications:";
                }
                ul {
                    li : "Fire Arrow Entry is allowed";
                    li : "Playing Treasure Chest Game without magic and lens is banned";
                    li : "DMC “pot push” is banned";
                    li : "All custom models are banned";
                }
                h2 : "Multiworld Plugins";
                p {
                    : "There are two plugins that can be used for the item sharing: ";
                    a(href = "https://github.com/TestRunnerSRL/bizhawk-co-op#readme") : "bizhawk-co-op";
                    : " (also known as Multiworld 1.0) and ";
                    a(href = uri!(http::mw).to_string()) : "Mido's House Multiworld";
                    : ". While we recommend using the Mido's House plugin since it supports Project64 in addition to BizHawk and is easier to use (see ";
                    a(href = "https://wiki.ootrandomizer.com/index.php?title=Multiworld#Feature_comparison") : "feature comparison";
                    : "), both plugins are legal in this tournament.";
                }
                p : "We were hopeful to host this season of the tournament on Multiworld 2.0, but there have been further delays with its release. In the event that it does release during this tournament, the plan is to allow Multiworld 2.0 to be used after being cleared by the tournament staff. However, be aware that by using this your team accepts the risks with using it and must abide by the standard FPA rules.";
            }
        }),
        _ => None,
    })
}

#[derive(Clone, Copy, PartialEq, Eq, FromFormField, Sequence)]
pub(crate) enum Role {
    #[field(value = "power")]
    Power,
    #[field(value = "wisdom")]
    Wisdom,
    #[field(value = "courage")]
    Courage,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Power => write!(f, "player 1"),
            Self::Wisdom => write!(f, "player 2"),
            Self::Courage => write!(f, "player 3"),
        }
    }
}

impl ToHtml for Role {
    fn to_html(&self) -> RawHtml<String> {
        match self {
            Self::Power => html! {
                span(class = "power") : "player 1";
            },
            Self::Wisdom => html! {
                span(class = "wisdom") : "player 2";
            },
            Self::Courage => html! {
                span(class = "courage") : "player 3";
            },
        }
    }
}

impl TryFrom<crate::event::Role> for Role {
    type Error = ();

    fn try_from(role: crate::event::Role) -> Result<Self, ()> {
        match role {
            crate::event::Role::Power => Ok(Self::Power),
            crate::event::Role::Wisdom => Ok(Self::Wisdom),
            crate::event::Role::Courage => Ok(Self::Courage),
            _ => Err(()),
        }
    }
}

impl From<Role> for crate::event::Role {
    fn from(role: Role) -> Self {
        match role {
            Role::Power => Self::Power,
            Role::Wisdom => Self::Wisdom,
            Role::Courage => Self::Courage,
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct RaceTimeUser {
    pub(crate) teams: Vec<RaceTimeTeam>,
}

#[derive(Deserialize)]
pub(crate) struct RaceTimeTeam {
    name: String,
    pub(crate) slug: String,
}

#[derive(Deserialize)]
pub(crate) struct RaceTimeTeamData {
    pub(crate) name: String,
    pub(crate) slug: String,
    pub(crate) members: Vec<RaceTimeTeamMember>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct RaceTimeTeamMember {
    pub(crate) id: String,
    pub(crate) name: String,
}

pub(crate) async fn enter_form(mut transaction: Transaction<'_, Postgres>, env: Environment, me: Option<User>, uri: Origin<'_>, csrf: Option<&CsrfToken>, data: Data<'_>, ctx: Context<'_>, client: &reqwest::Client) -> Result<RawHtml<String>, Error> {
    let header = data.header(&mut transaction, env, me.as_ref(), Tab::Enter, false).await?;
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests().await, ..PageStyle::default() }, &format!("Enter — {}", data.display_name), if let Some(ref me) = me {
        if let Some(ref racetime) = me.racetime {
            let racetime_user = client.get(format!("https://racetime.gg/user/{}/data", racetime.id))
                .send().await?
                .detailed_error_for_status().await?
                .json_with_text_in_error::<RaceTimeUser>().await?;
            let mut errors = ctx.errors().collect_vec();
            if racetime_user.teams.is_empty() {
                html! {
                    : header;
                    article {
                        p {
                            a(href = "https://racetime.gg/account/teams/create") : "Create a racetime.gg team";
                            : " to enter this event.";
                        }
                    }
                }
            } else {
                html! {
                    : header;
                    : full_form(uri!(enter::post(data.series, &*data.event)), csrf, html! {
                        : form_field("racetime_team", &mut errors, html! {
                            label(for = "racetime_team") : "racetime.gg Team:";
                            select(name = "racetime_team") {
                                @for team in racetime_user.teams {
                                    option(value = team.slug) : team.name;
                                }
                            }
                            label(class = "help") {
                                : "(Or ";
                                a(href = "https://racetime.gg/account/teams/create") : "create a new team";
                                : ", then come back here.)";
                            }
                        });
                    }, errors, "Next");
                }
            }
        } else {
            html! {
                : header;
                article {
                    p {
                        a(href = uri!(crate::auth::racetime_login(Some(uri!(enter::get(data.series, &*data.event, _, _))))).to_string()) : "Connect a racetime.gg account to your Mido's House account";
                        : " to enter this event.";
                    }
                }
            }
        }
    } else {
        html! {
            : header;
            article {
                p {
                    a(href = uri!(auth::login(Some(uri!(enter::get(data.series, &*data.event, _, _))))).to_string()) : "Sign in or create a Mido's House account";
                    : " to enter this event.";
                }
            }
        }
    }).await?)
}

//TODO this is no longer needed since the forms have been merged
pub(crate) enum EnterFormStep2Defaults<'a> {
    Context(Context<'a>),
    Values {
        racetime_team: RaceTimeTeamData,
    },
}

impl<'v> EnterFormStep2Defaults<'v> {
    pub(crate) fn errors(&self) -> Vec<&form::Error<'v>> {
        match self {
            Self::Context(ctx) => ctx.errors().collect(),
            Self::Values { .. } => Vec::default(),
        }
    }

    pub(crate) fn racetime_team_name(&self) -> Option<&str> {
        match self {
            Self::Context(ctx) => ctx.field_value("racetime_team_name"),
            Self::Values { racetime_team: RaceTimeTeamData { name, .. } } => Some(name),
        }
    }

    pub(crate) fn racetime_team_slug(&self) -> Option<&str> {
        match self {
            Self::Context(ctx) => ctx.field_value("racetime_team"),
            Self::Values { racetime_team: RaceTimeTeamData { slug, .. } } => Some(slug),
        }
    }

    pub(crate) fn racetime_members(&self, client: &reqwest::Client) -> impl Future<Output = Result<Vec<RaceTimeTeamMember>, Error>> {
        match self {
            Self::Context(ctx) => if let Some(team_slug) = ctx.field_value("racetime_team") {
                let client = client.clone();
                let url = format!("https://racetime.gg/team/{team_slug}/data");
                async move {
                    Ok(client.get(url)
                        .send().await?
                        .detailed_error_for_status().await?
                        .json_with_text_in_error::<RaceTimeTeamData>().await?
                        .members
                    )
                }.boxed()
            } else {
                future::ok(Vec::default()).boxed()
            }
            Self::Values { racetime_team } => future::ok(racetime_team.members.clone()).boxed(),
        }
    }

    pub(crate) fn role(&self, racetime_id: &str) -> Option<crate::event::Role> {
        match self {
            Self::Context(ctx) => ctx.field_value(&*format!("roles[{racetime_id}]")).and_then(crate::event::Role::from_css_class),
            Self::Values { .. } => None,
        }
    }

    pub(crate) fn startgg_id(&self, racetime_id: &str) -> Option<&str> {
        match self {
            Self::Context(ctx) => ctx.field_value(&*format!("startgg_id[{racetime_id}]")),
            Self::Values { .. } => None,
        }
    }

    pub(crate) fn restream_consent(&self) -> bool {
        match self {
            Self::Context(ctx) => ctx.field_value("restream_consent") == Some("on"),
            Self::Values { .. } => false,
        }
    }
}

pub(crate) async fn find_team_form(mut transaction: Transaction<'_, Postgres>, env: Environment, me: Option<User>, uri: Origin<'_>, csrf: Option<&CsrfToken>, data: Data<'_>, ctx: Context<'_>) -> Result<RawHtml<String>, FindTeamError> {
    let header = data.header(&mut transaction, env, me.as_ref(), Tab::FindTeam, false).await?;
    let mut me_listed = false;
    let mut looking_for_team = Vec::default();
    for row in sqlx::query!(r#"SELECT user_id AS "user!: Id<Users>", availability, notes FROM looking_for_team WHERE series = $1 AND event = $2"#, data.series as _, &data.event).fetch_all(&mut *transaction).await? {
        let user = User::from_id(&mut *transaction, row.user).await?.ok_or(FindTeamError::UnknownUser)?;
        if me.as_ref().map_or(false, |me| user.id == me.id) { me_listed = true }
        looking_for_team.push((user, row.availability, row.notes));
    }
    let form = if me.is_some() {
        let mut errors = ctx.errors().collect_vec();
        if me_listed {
            None
        } else {
            Some(full_form(uri!(event::find_team_post(data.series, &*data.event)), csrf, html! {
                @if data.is_single_race() {
                    legend {
                        : "Click this button to add yourself to the list below.";
                    }
                } else {
                    legend {
                        : "Fill out this form to add yourself to the list below.";
                    }
                    : form_field("availability", &mut errors, html! {
                        label(for = "availability") : "Timezone/Availability/Commitment:";
                        input(type = "text", name = "availability", value? = ctx.field_value("availability"));
                    });
                    : form_field("notes", &mut errors, html! {
                        label(for = "notes") : "Any Other Notes?";
                        input(type = "text", name = "notes", value? = ctx.field_value("notes"));
                    });
                }
            }, errors, if data.is_single_race() { "Looking for Team" } else { "Submit" }))
        }
    } else {
        Some(html! {
            article {
                p {
                    a(href = uri!(auth::login(Some(uri!(event::find_team(data.series, &*data.event))))).to_string()) : "Sign in or create a Mido's House account";
                    : " to add yourself to this list.";
                }
            }
        })
    };
    Ok(page(transaction, &me, &uri, PageStyle { chests: data.chests().await, ..PageStyle::default() }, &format!("Find Teammates — {}", data.display_name), html! {
        : header;
        : form;
        table {
            thead {
                tr {
                    th : "User";
                    @if !data.is_single_race() {
                        th : "Timezone/Availability/Commitment";
                        th : "Notes";
                    }
                }
            }
            tbody {
                @if looking_for_team.is_empty() {
                    tr {
                        td(colspan = if data.is_single_race() { "1" } else { "3" }) {
                            i : "(no one currently looking for teammates)";
                        }
                    }
                } else {
                    @for (user, availability, notes) in looking_for_team {
                        tr {
                            td : user;
                            @if !data.is_single_race() {
                                td : availability;
                                td : notes;
                            }
                        }
                    }
                }
            }
        }
    }).await?)
}

pub(crate) async fn status(transaction: &mut Transaction<'_, Postgres>, discord_ctx: &DiscordCtx, csrf: Option<&CsrfToken>, data: &Data<'_>, team_id: Id<Teams>, ctx: &mut StatusContext<'_>) -> Result<RawHtml<String>, Error> {
    Ok(if let Some(async_kind) = data.active_async(&mut *transaction, Some(team_id)).await? {
        let async_row = sqlx::query!(r#"SELECT discord_channel AS "discord_channel: PgSnowflake<ChannelId>", tfb_uuid, web_id, web_gen_time, file_stem, hash1 AS "hash1: HashIcon", hash2 AS "hash2: HashIcon", hash3 AS "hash3: HashIcon", hash4 AS "hash4: HashIcon", hash5 AS "hash5: HashIcon" FROM asyncs WHERE series = $1 AND event = $2 AND kind = $3"#, data.series as _, &data.event, async_kind as _).fetch_one(&mut **transaction).await?;
        if let Some(team_row) = sqlx::query!(r#"SELECT requested AS "requested!", submitted FROM async_teams WHERE team = $1 AND KIND = $2 AND requested IS NOT NULL"#, team_id as _, async_kind as _).fetch_optional(&mut **transaction).await? {
            if team_row.submitted.is_some() {
                if data.is_started(transaction).await? {
                    //TODO adjust for other match data sources?
                    //TODO get this team's known matchup(s) from start.gg
                    html! {
                        p : "Please schedule your matches using Discord threads in the scheduling channel.";
                    }
                    //TODO form to submit matches
                } else {
                    //TODO if any vods are still missing, show form to add them
                    html! {
                        p : "Waiting for the start of the tournament and round 1 pairings. Keep an eye out for an announcement on Discord."; //TODO include start date?
                    }
                }
            } else {
                let seed = seed::Data {
                    file_hash: match (async_row.hash1, async_row.hash2, async_row.hash3, async_row.hash4, async_row.hash5) {
                        (Some(hash1), Some(hash2), Some(hash3), Some(hash4), Some(hash5)) => Some([hash1, hash2, hash3, hash4, hash5]),
                        (None, None, None, None, None) => None,
                        _ => unreachable!("only some hash icons present, should be prevented by SQL constraint"),
                    },
                    files: Some(match (async_row.tfb_uuid, async_row.web_id, async_row.web_gen_time, async_row.file_stem.as_ref()) {
                        (Some(uuid), _, _, _) => seed::Files::TriforceBlitz { uuid },
                        (None, Some(id), Some(gen_time), Some(file_stem)) => seed::Files::OotrWeb {
                            file_stem: Cow::Owned(file_stem.clone()),
                            id, gen_time,
                        },
                        (None, None, None, Some(file_stem)) => seed::Files::MidosHouse { file_stem: Cow::Owned(file_stem.clone()), locked_spoiler_log_path: None },
                        _ => unreachable!("only some web data present, should be prevented by SQL constraint"),
                    }),
                };
                let seed_table = seed::table(stream::iter(iter::once(seed)), false).await?;
                let ctx = ctx.take_submit_async();
                let mut errors = ctx.errors().collect_vec();
                html! {
                    div(class = "info") {
                        p {
                            : "You requested the qualifier async on ";
                            : format_datetime(team_row.requested, DateTimeFormat { long: true, running_text: true });
                            : ".";
                        };
                        : seed_table;
                        p : "After playing the async, fill out the form below.";
                        : full_form(uri!(event::submit_async(data.series, &*data.event)), csrf, html! {
                            : form_field("time1", &mut errors, html! {
                                label(for = "time1", class = "power") : "Player 1 Finishing Time:";
                                input(type = "text", name = "time1", value? = ctx.field_value("time1")); //TODO h:m:s fields?
                                label(class = "help") : "(If player 1 did not finish, leave this field blank.)";
                            });
                            : form_field("vod1", &mut errors, html! {
                                label(for = "vod1", class = "power") : "Player 1 VoD:";
                                input(type = "text", name = "vod1", value? = ctx.field_value("vod1"));
                                label(class = "help") {
                                    : "(If you plan on uploading the VoD to YouTube later, leave this field blank and ";
                                    @if let Some(PgSnowflake(discord_channel)) = async_row.discord_channel {
                                        : "post it in ";
                                        @if let Some(discord_channel) = discord_channel.to_channel_cached(discord_ctx).and_then(|c| c.guild()) {
                                            : "#";
                                            : discord_channel.name;
                                        } else {
                                            : "the results channel for this async";
                                        }
                                    } else {
                                        : "DM an admin";
                                    }
                                    : " once it is ready.)";
                                    //TODO form to submit vods later
                                }
                            });
                            : form_field("time2", &mut errors, html! {
                                label(for = "time2", class = "wisdom") : "Player 2 Finishing Time:";
                                input(type = "text", name = "time2", value? = ctx.field_value("time2")); //TODO h:m:s fields?
                                label(class = "help") : "(If player 2 did not finish, leave this field blank.)";
                            });
                            : form_field("vod2", &mut errors, html! {
                                label(for = "vod2", class = "wisdom") : "Player 2 VoD:";
                                input(type = "text", name = "vod2", value? = ctx.field_value("vod2"));
                                label(class = "help") {
                                    : "(If you plan on uploading the VoD to YouTube later, leave this field blank and ";
                                    @if let Some(PgSnowflake(discord_channel)) = async_row.discord_channel {
                                        : "post it in ";
                                        @if let Some(discord_channel) = discord_channel.to_channel_cached(discord_ctx).and_then(|c| c.guild()) {
                                            : "#";
                                            : discord_channel.name;
                                        } else {
                                            : "the results channel for this async";
                                        }
                                    } else {
                                        : "DM an admin";
                                    }
                                    : " once it is ready.)";
                                    //TODO form to submit vods later
                                }
                            });
                            : form_field("time3", &mut errors, html! {
                                label(for = "time3", class = "courage") : "Player 3 Finishing Time:";
                                input(type = "text", name = "time3", value? = ctx.field_value("time3")); //TODO h:m:s fields?
                                label(class = "help") : "(If player 3 did not finish, leave this field blank.)";
                            });
                            : form_field("vod3", &mut errors, html! {
                                label(for = "vod3", class = "courage") : "Player 3 VoD:";
                                input(type = "text", name = "vod3", value? = ctx.field_value("vod3"));
                                label(class = "help") {
                                    : "(If you plan on uploading the VoD to YouTube later, leave this field blank and ";
                                    @if let Some(PgSnowflake(discord_channel)) = async_row.discord_channel {
                                        : "post it in ";
                                        @if let Some(discord_channel) = discord_channel.to_channel_cached(discord_ctx).and_then(|c| c.guild()) {
                                            : "#";
                                            : discord_channel.name;
                                        } else {
                                            : "the results channel for this async";
                                        }
                                    } else {
                                        : "DM an admin";
                                    }
                                    : " once it is ready.)";
                                    //TODO form to submit vods later
                                }
                            });
                            : form_field("fpa", &mut errors, html! {
                                label(for = "fpa") {
                                    : "If you would like to invoke the ";
                                    a(href = "https://docs.google.com/document/d/e/2PACX-1vQd3S28r8SOBy-4C5Lxeu6nFAYpWgQqN9lCEKhLGTT3zcaXDSKj0iUnZv6UPo_GargUVQx5F-wOPUtJ/pub") : "Fair Play Agreement";
                                    : ", describe the break(s) you took below. Include the reason, starting time, and duration.";
                                }
                                textarea(name = "fpa"); //TODO fill from form context
                            });
                        }, errors, "Submit");
                    }
                }
            }
        } else {
            let ctx = ctx.take_request_async();
            let mut errors = ctx.errors().collect_vec();
            html! {
                div(class = "info") {
                    @match async_kind {
                        AsyncKind::Qualifier => p : "Play the qualifier async to qualify for the tournament.";
                        AsyncKind::Tiebreaker1 | AsyncKind::Tiebreaker2 => p : "Play the tiebreaker async to qualify for the bracket stage of the tournament.";
                    }
                    p : "Rules:";
                    ol {
                        @match async_kind {
                            AsyncKind::Qualifier => li : "In order to play in the tournament, your team must make a reasonable attempt at completing this seed. In the event of a forfeit, you can still participate, but will be considered the bottom seed for settings draft purposes.";
                            AsyncKind::Tiebreaker1 => li : "In order to play in the top 8 bracket, your team must make a reasonable attempt at completing this seed. In the event of a forfeit, you can still participate, but will be placed at the bottom of your Swiss point group for matchup and settings draft purposes.";
                            AsyncKind::Tiebreaker2 => li : "In order to play in the top 8 bracket, your team must race the other teams of your Swiss point group on this seed.";
                        }
                        @if let AsyncKind::Qualifier = async_kind {
                            li {
                                @if let Some(base_start) = data.base_start {
                                    : "The time must be submitted by ";
                                    : format_datetime(base_start, DateTimeFormat { long: true, running_text: true });
                                    : ". In the event that an odd number of teams is qualified at the time of the deadline, one additional team may qualify within 24 hours.";
                                } else {
                                    : "The time must be submitted by the starting time of the tournament, which is yet to be announced.";
                                }
                            }
                        } else {
                            //TODO give deadline of tiebreaker async
                        }
                        li : "You must start the seed within 30 minutes of obtaining it and submit your time within 30 minutes of the last finish. Any additional time taken will be added to your final time. If anything prevents you from obtaining the seed/submitting your time, please DM an admin (or ping the Discord role) to get it sorted out.";
                        @if let AsyncKind::Qualifier = async_kind {
                            li : "While required for the tournament, the results from the qualifier seed will only determine which team chooses who goes first in the settings draft. Swiss pairings will be seeded randomly.";
                            li : "While you are not strictly required to stream, you must have video proof of your run. Feel free to simply record your run and upload it to YouTube and provide a link. If you do stream or make your upload public, please make sure it is clearly marked so people can avoid spoilers. If you're a big streamer, be extra sure to note what is happening, as several of your viewers are likely going to want to participate as well.";
                        } else {
                            li : "The results from the tiebreaker seed will determine your seeding in the bracket, as well as which team chooses who goes first in the settings draft for the first race of each match.";
                            li : "You must have video proof of your run. Streaming is allowed but discouraged. Feel free to simply record your run and upload it to YouTube and provide a link. If you do stream or make your upload public, please make sure is is clearly marked so people can avoid spoilers.";
                        }
                        li : "Do not spoil yourself on this seed by watching another playthrough. If you do stream, you are responsible for what your chat says, so either do not read chat, set it to emote only, or take the risk at your own discretion. If you do get spoiled, please report it to the admins, we will try to work out something equitable.";
                        li {
                            : "You must use the world numbers with which you entered the tournament for this seed.";
                            @if let AsyncKind::Qualifier = async_kind {
                                : " Once you request the seed, the world numbers you selected are the world numbers you play with for the rest of the tournament. If you wish to change your player order, do not request the qualifier and contact an admin."; //TODO allow changing player order in options below
                            }
                        }
                        li {
                            : "This should be run like an actual race. In the event of a technical issue, teams are allowed to invoke the ";
                            a(href = "https://docs.google.com/document/d/e/2PACX-1vQd3S28r8SOBy-4C5Lxeu6nFAYpWgQqN9lCEKhLGTT3zcaXDSKj0iUnZv6UPo_GargUVQx5F-wOPUtJ/pub") : "Fair Play Agreement";
                            : " and have up to a 15 minute time where the affected runner can try to catch back up. If you do this, you must fill out the appropriate field when submitting your time so it can be authenticated.";
                        }
                    }
                    : full_form(uri!(event::request_async(data.series, &*data.event)), csrf, html! {
                        : form_field("confirm", &mut errors, html! {
                            input(type = "checkbox", id = "confirm", name = "confirm");
                            label(for = "confirm") : "We have read the above and are ready to play the seed";
                        });
                    }, errors, "Request Now");
                }
            }
        }
    } else {
        html! {
            p : "Waiting for the qualifier async to be published. Keep an eye out for an announcement on Discord.";
        }
    })
}

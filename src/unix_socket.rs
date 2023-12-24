use {
    async_proto::{
        ReadError,
        ReadErrorKind,
    },
    serde_json::Value as Json,
    tokio::net::UnixListener,
    crate::{
        prelude::*,
        racetime_bot::{
            Goal,
            PrerollMode,
            RollError,
            SeedCommandParseResult,
            SeedRollUpdate,
            UnlockSpoilerLog,
            VersionedBranch,
            VersionedRslPreset,
        },
    },
};

pub(crate) const PATH: &str = "/usr/local/share/midos-house/sock";

fn json_arg(arg: &str) -> serde_json::Result<Json> {
    serde_json::from_str(arg)
}

#[derive(clap::Subcommand, Protocol)]
pub(crate) enum ClientMessage {
    PrepareStop {
        #[clap(long)]
        no_new_rooms: bool,
    },
    Roll {
        version: ootr_utils::Version,
        #[clap(value_parser = json_arg)]
        settings: Json,
        #[clap(short = 'l', long)]
        spoiler_log: bool,
    },
    RollRsl {
        preset: Option<String>,
        #[clap(short, long, default_value = "xopar")]
        branch: String,
        #[clap(long)]
        rsl_version: Option<ootr_utils::Version>,
        #[clap(short = 'n', long, default_value_t = 1)]
        worlds: u8,
        #[clap(short = 'l', long)]
        spoiler_log: bool,
    },
    Seed {
        #[clap(short = 'l', long)]
        spoiler_log: bool,
        goal: Goal,
        args: Vec<String>,
    },
}

pub(crate) async fn listen(mut shutdown: rocket::Shutdown, clean_shutdown: Arc<Mutex<racetime_bot::CleanShutdown>>, global_state: Arc<racetime_bot::GlobalState>) -> wheel::Result<()> {
    fs::remove_file(PATH).await.missing_ok()?;
    let listener = UnixListener::bind(PATH).at(PATH)?;
    loop {
        select! {
            () = &mut shutdown => break,
            res = listener.accept() => {
                let (mut sock, _) = res.at_unknown()?;
                let clean_shutdown = clean_shutdown.clone();
                let global_state = global_state.clone();
                tokio::spawn(async move {
                    loop {
                        match ClientMessage::read(&mut sock).await {
                            Ok(ClientMessage::PrepareStop { no_new_rooms }) => {
                                println!("preparing to stop Mido's House: acquiring clean shutdown mutex");
                                let mut clean_shutdown = lock!(clean_shutdown);
                                clean_shutdown.requested = true;
                                if no_new_rooms { clean_shutdown.block_new = true }
                                if !clean_shutdown.open_rooms.is_empty() {
                                    println!("preparing to stop Mido's House: waiting for {} rooms to close:", clean_shutdown.open_rooms.len());
                                    for room_url in &clean_shutdown.open_rooms {
                                        println!("https://{}{room_url}", global_state.env.racetime_host());
                                    }
                                    let notifier = Arc::clone(&clean_shutdown.notifier);
                                    drop(clean_shutdown);
                                    notifier.notified().await;
                                }
                                println!("preparing to stop Mido's House: sending reply");
                                0u8.write(&mut sock).await.expect("error writing to UNIX socket");
                                println!("preparing to stop Mido's House: done");
                                break
                            }
                            Ok(ClientMessage::Roll { version, settings, spoiler_log }) => if let Json::Object(settings) = settings {
                                let mut rx = global_state.clone().roll_seed(PrerollMode::Medium, None, VersionedBranch::Pinned(version), settings, if spoiler_log { UnlockSpoilerLog::Now } else { UnlockSpoilerLog::Never });
                                loop {
                                    let update = rx.recv().await;
                                    update.write(&mut sock).await.expect("error writing to UNIX socket");
                                    if update.is_none() { break }
                                }
                            } else {
                                Some(SeedRollUpdate::Error(RollError::NonObjectSettings)).write(&mut sock).await.expect("error writing to UNIX socket");
                                None::<SeedRollUpdate>.write(&mut sock).await.expect("error writing to UNIX socket");
                                break
                            },
                            Ok(ClientMessage::RollRsl { preset, branch, rsl_version, worlds, spoiler_log }) => {
                                let preset = if let Some(rsl_version) = rsl_version {
                                    VersionedRslPreset::new_versioned(rsl_version, preset.as_deref())
                                } else {
                                    VersionedRslPreset::new_unversioned(&branch, preset.as_deref())
                                };
                                if let Ok(preset) = preset {
                                    let mut rx = global_state.clone().roll_rsl_seed(None, preset, worlds, if spoiler_log { UnlockSpoilerLog::Now } else { UnlockSpoilerLog::Never });
                                    loop {
                                        let update = rx.recv().await;
                                        update.write(&mut sock).await.expect("error writing to UNIX socket");
                                        if update.is_none() { break }
                                    }
                                } else {
                                    Some(SeedRollUpdate::Error(RollError::RslVersion)).write(&mut sock).await.expect("error writing to UNIX socket");
                                    None::<SeedRollUpdate>.write(&mut sock).await.expect("error writing to UNIX socket");
                                    break
                                }
                            }
                            Ok(ClientMessage::Seed { goal, spoiler_log, args }) => {
                                let mut rx = match goal.parse_seed_command(&global_state.http_client, spoiler_log, &args).await {
                                    Ok(SeedCommandParseResult::Regular { settings, spoiler_log, description, .. }) => {
                                        Some(SeedRollUpdate::Message(description)).write(&mut sock).await.expect("error writing to UNIX socket");
                                        global_state.clone().roll_seed(goal.preroll_seeds(), None, goal.rando_version(), settings, goal.unlock_spoiler_log(false, spoiler_log))
                                    }
                                    Ok(SeedCommandParseResult::Rsl { preset, world_count, spoiler_log, description, .. }) => {
                                        Some(SeedRollUpdate::Message(description)).write(&mut sock).await.expect("error writing to UNIX socket");
                                        global_state.clone().roll_rsl_seed(None, preset, world_count, goal.unlock_spoiler_log(false, spoiler_log))
                                    }
                                    Ok(SeedCommandParseResult::Tfb { version, spoiler_log, description, .. }) => {
                                        Some(SeedRollUpdate::Message(description)).write(&mut sock).await.expect("error writing to UNIX socket");
                                        global_state.clone().roll_tfb_seed(None, version, None, goal.unlock_spoiler_log(false, spoiler_log))
                                    }
                                    Ok(SeedCommandParseResult::QueueExisting { data, description, .. }) => {
                                        Some(SeedRollUpdate::Message(description)).write(&mut sock).await.expect("error writing to UNIX socket");
                                        Some(SeedRollUpdate::Done { rsl_preset: None, send_spoiler_log: false, seed: data }).write(&mut sock).await.expect("error writing to UNIX socket");
                                        None::<SeedRollUpdate>.write(&mut sock).await.expect("error writing to UNIX socket");
                                        break
                                    }
                                    Ok(SeedCommandParseResult::SendPresets { msg, .. }) => {
                                        Some(SeedRollUpdate::Error(RollError::Cloned { debug: String::default(), display: msg.to_owned() })).write(&mut sock).await.expect("error writing to UNIX socket");
                                        None::<SeedRollUpdate>.write(&mut sock).await.expect("error writing to UNIX socket");
                                        break
                                    }
                                    Ok(SeedCommandParseResult::SendSettings { msg, .. } | SeedCommandParseResult::Error { msg, .. }) => {
                                        Some(SeedRollUpdate::Error(RollError::Cloned { debug: String::default(), display: msg.into_owned() })).write(&mut sock).await.expect("error writing to UNIX socket");
                                        None::<SeedRollUpdate>.write(&mut sock).await.expect("error writing to UNIX socket");
                                        break
                                    }
                                    Ok(SeedCommandParseResult::StartDraft { .. }) => unimplemented!(), //TODO
                                    Err(e) => {
                                        Some(SeedRollUpdate::Error(e.into())).write(&mut sock).await.expect("error writing to UNIX socket");
                                        None::<SeedRollUpdate>.write(&mut sock).await.expect("error writing to UNIX socket");
                                        break
                                    }
                                };
                                loop {
                                    let update = rx.recv().await;
                                    update.write(&mut sock).await.expect("error writing to UNIX socket");
                                    if update.is_none() { break }
                                }
                            }
                            Err(ReadError { kind: ReadErrorKind::Io(e), .. }) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                            Err(e) => panic!("error reading from UNIX socket: {e} ({e:?})"),
                        }
                    }
                });
            }
        }
    }
    Ok(())
}

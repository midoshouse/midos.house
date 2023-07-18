use {
    std::sync::Arc,
    async_proto::{
        Protocol,
        ReadError,
    },
    log_lock::{
        Mutex,
        lock,
    },
    serde_json::Value as Json,
    tokio::{
        io,
        net::UnixListener,
        select,
    },
    wheel::{
        fs,
        traits::IoResultExt as _,
    },
    crate::racetime_bot::{
        self,
        RollError,
        SeedRollUpdate,
        VersionedBranch,
        VersionedRslPreset,
    },
};

pub(crate) const PATH: &str = "/usr/local/share/midos-house/sock";

#[derive(clap::Subcommand, Protocol)]
pub(crate) enum ClientMessage {
    PrepareStop,
    Roll {
        version: ootr_utils::Version,
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
                            Ok(ClientMessage::PrepareStop) => {
                                println!("preparing to stop Mido's House: acquiring clean shutdown mutex");
                                let mut clean_shutdown = lock!(clean_shutdown);
                                clean_shutdown.requested = true;
                                if !clean_shutdown.open_rooms.is_empty() {
                                    println!("preparing to stop Mido's House: waiting for {} rooms to close:", clean_shutdown.open_rooms.len());
                                    for room_url in &clean_shutdown.open_rooms {
                                        println!("https://{}{room_url}", global_state.host);
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
                                let mut rx = global_state.clone().roll_seed(None, VersionedBranch::Pinned(version), settings, spoiler_log);
                                loop {
                                    let update = rx.recv().await;
                                    update.write(&mut sock).await.expect("error writing to UNIX socket");
                                    if update.is_none() { break }
                                }
                            } else {
                                Some(SeedRollUpdate::Error(RollError::NonObjectSettings)).write(&mut sock).await.expect("error writing to UNIX socket");
                            },
                            Ok(ClientMessage::RollRsl { preset, branch, rsl_version, worlds, spoiler_log }) => {
                                let preset = if let Some(rsl_version) = rsl_version {
                                    VersionedRslPreset::new_versioned(rsl_version, preset.as_deref())
                                } else {
                                    VersionedRslPreset::new_unversioned(&branch, preset.as_deref())
                                };
                                if let Ok(preset) = preset {
                                    let mut rx = global_state.clone().roll_rsl_seed(None, preset, worlds, spoiler_log);
                                    loop {
                                        let update = rx.recv().await;
                                        update.write(&mut sock).await.expect("error writing to UNIX socket");
                                        if update.is_none() { break }
                                    }
                                } else {
                                    Some(SeedRollUpdate::Error(RollError::RslVersion)).write(&mut sock).await.expect("error writing to UNIX socket");
                                }
                            }
                            Err(ReadError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                            Err(e) => panic!("error reading from UNIX socket: {e} ({e:?})"),
                        }
                    }
                });
            }
        }
    }
    Ok(())
}

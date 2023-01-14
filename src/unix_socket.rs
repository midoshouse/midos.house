use {
    std::sync::Arc,
    async_proto::{
        Protocol,
        ReadError,
    },
    tokio::{
        io,
        net::UnixListener,
        select,
        sync::Mutex,
    },
    wheel::{
        fs,
        traits::IoResultExt as _,
    },
    crate::racetime_bot,
};

pub(crate) const PATH: &str = "/usr/local/share/midos-house/sock";

#[derive(Protocol)]
pub(crate) enum ClientMessage {
    PrepareStop,
}

pub(crate) async fn listen(mut shutdown: rocket::Shutdown, clean_shutdown: Arc<Mutex<racetime_bot::CleanShutdown>>) -> wheel::Result<()> {
    fs::remove_file(PATH).await.missing_ok()?;
    let listener = UnixListener::bind(PATH).at(PATH)?;
    loop {
        select! {
            () = &mut shutdown => break,
            res = listener.accept() => {
                let (mut sock, _) = res.at_unknown()?;
                let clean_shutdown = clean_shutdown.clone();
                tokio::spawn(async move {
                    loop {
                        match ClientMessage::read(&mut sock).await {
                            Ok(ClientMessage::PrepareStop) => {
                                println!("preparing to stop Mido's House: acquiring clean shutdown mutex");
                                let mut clean_shutdown = clean_shutdown.lock().await;
                                clean_shutdown.requested = true;
                                if !clean_shutdown.open_rooms.is_empty() {
                                    println!("preparing to stop Mido's House: waiting for {} rooms to close:", clean_shutdown.open_rooms.len());
                                    for room_url in &clean_shutdown.open_rooms {
                                        println!("{room_url}");
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

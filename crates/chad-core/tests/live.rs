//! The whole core against the dev stack (`deploy/README.md`, seeded):
//! `cargo test -p chad-core --test live -- --ignored`.
//! Needs `SSL_CERT_FILE` trusting Caddy's CA (`docs/notes/livekit-sdk.md`), and
//! an origin glibc resolves: `IRCHAD_HOST=localhost` for the stack, and
//! `IRCHAD_ORIGIN` here if it's not `localhost:8443`.

use std::sync::Arc;
use std::time::Duration;

use chad_core::secrets::MemorySecrets;
use chad_core::{Auth, Command, Config, ConnState, Event, NetCommand, SendStatus};
use tokio::sync::mpsc::UnboundedReceiver;

async fn until<T>(rx: &mut UnboundedReceiver<Event>, mut f: impl FnMut(&Event) -> Option<T>) -> T {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let e = rx.recv().await.expect("core stopped");
            if let Event::JoinFailed { error, .. }
            | Event::Network(chad_core::NetworkInfo {
                state: ConnState::Failed { error },
                ..
            }) = &e
            {
                panic!("{error}");
            }
            if let Some(t) = f(&e) {
                return t;
            }
        }
    })
    .await
    .expect("timed out")
}

fn connected(e: &Event) -> Option<String> {
    match e {
        Event::Network(n) if n.state == ConnState::Connected => Some(n.nick.clone()),
        _ => None,
    }
}

#[tokio::test]
#[ignore]
async fn register_chat_restart_history() {
    let origin = std::env::var("IRCHAD_ORIGIN").unwrap_or("localhost:8443".into());
    let network = format!("https://{origin}");
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let config = || Config {
        data_dir: dir.path().to_owned(),
        secrets: secrets.clone(),
        notify: Arc::new(|| {}),
        media: None,
    };
    let account = format!("lc{}", std::time::UNIX_EPOCH.elapsed().unwrap().as_secs());

    // Sign up from an invite; first login joins everything in LIST.
    let (core, mut rx) = chad_core::start(config());
    core.send(Command::Join {
        invite: format!("https://{origin}/invite?channel=%23dev"),
        auth: Auth::Register {
            account: account.clone(),
            password: "livecorepass".into(),
            email: None,
        },
    });
    assert_eq!(until(&mut rx, connected).await, account);
    until(&mut rx, |e| {
        matches!(e, Event::Buffer { buffer, .. } if buffer.target == "#dev").then_some(())
    })
    .await;
    until(&mut rx, |e| {
        matches!(e, Event::Buffer { buffer, .. } if buffer.target == "#general").then_some(())
    })
    .await;

    let text = format!("hello from {account}");
    core.send(Command::Net {
        network: network.clone(),
        cmd: NetCommand::Send {
            target: "#dev".into(),
            text: text.clone(),
            reply_to: None,
        },
    });
    let msgid = until(&mut rx, |e| match e {
        Event::Messages { messages, .. } => messages
            .iter()
            .find(|m| m.text == text && m.local_id.is_some() && m.status == SendStatus::Sent)
            .and_then(|m| m.msgid.clone()),
        _ => None,
    })
    .await;
    drop(core);
    // The close handshake waits behind whatever the server is still sending.
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // Restart: the cache shows the message before the connection is up.
    let (core, mut rx) = chad_core::start(config());
    until(&mut rx, |e| match e {
        Event::Messages { messages, .. } => messages
            .iter()
            .any(|m| m.msgid.as_ref() == Some(&msgid))
            .then_some(()),
        _ => None,
    })
    .await;
    assert_eq!(until(&mut rx, connected).await, account);
    drop(core);

    // alice, on another install, gets it through chathistory.
    let alice_dir = tempfile::tempdir().unwrap();
    let (alice, mut arx) = chad_core::start(Config {
        data_dir: alice_dir.path().to_owned(),
        secrets: Arc::new(MemorySecrets::default()),
        notify: Arc::new(|| {}),
        media: None,
    });
    alice.send(Command::Join {
        invite: origin.clone(),
        auth: Auth::Login {
            account: "alice".into(),
            password: "alicepass".into(),
        },
    });
    until(&mut arx, connected).await;
    until(&mut arx, |e| match e {
        Event::Messages {
            target, messages, ..
        } if target == "#dev" => messages
            .iter()
            .any(|m| m.msgid.as_ref() == Some(&msgid) && m.text == text)
            .then_some(()),
        _ => None,
    })
    .await;
}

#[tokio::test]
#[ignore]
async fn guest_join_collision_restart() {
    let origin = std::env::var("IRCHAD_ORIGIN").unwrap_or("localhost:8443".into());
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let config = |d: &std::path::Path| Config {
        data_dir: d.to_owned(),
        secrets: Arc::new(MemorySecrets::default()),
        notify: Arc::new(|| {}),
        media: None,
    };
    let nick = format!("g{}", std::time::UNIX_EPOCH.elapsed().unwrap().as_secs());
    let join = || Command::Join {
        invite: format!("https://{origin}/invite?channel=%23dev"),
        auth: Auth::Guest { nick: nick.clone() },
    };
    let dev = |e: &Event| {
        matches!(e, Event::Buffer { buffer, .. } if buffer.target == "#dev").then_some(())
    };

    let (core, mut rx) = chad_core::start(config(dir.path()));
    core.send(join());
    assert_eq!(until(&mut rx, connected).await, nick);
    until(&mut rx, dev).await;

    // Same nick from another install: it takes the next free one.
    let (core2, mut rx2) = chad_core::start(config(other.path()));
    core2.send(join());
    assert_eq!(until(&mut rx2, connected).await, format!("{nick}1"));
    drop(core2);
    drop(core);
    // The close handshake waits behind whatever the server is still sending.
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // Restart: no password needed, and #dev is rejoined.
    let (core, mut rx) = chad_core::start(config(dir.path()));
    assert_eq!(until(&mut rx, connected).await, nick);
    until(&mut rx, dev).await; // our JOIN came back
    core.send(Command::Net {
        network: format!("https://{origin}"),
        cmd: NetCommand::Members {
            channel: "#dev".into(),
        },
    });
    until(&mut rx, |e| match e {
        Event::Members {
            channel, members, ..
        } if channel == "#dev" => members.iter().any(|m| m.nick == nick).then_some(()),
        _ => None,
    })
    .await;
}

/// Records the join and reports connected right away.
struct FakeMedia(std::sync::Mutex<Vec<(String, String)>>);

impl chad_core::Media for FakeMedia {
    fn join(&self, url: String, token: String) -> UnboundedReceiver<chad_core::MediaEvent> {
        self.0.lock().unwrap().push((url, token));
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let _ = tx.send(chad_core::MediaEvent::Connected);
        // Keep the sender alive until the core hangs up.
        tokio::spawn(async move { tx.closed().await });
        rx
    }
}

#[tokio::test]
#[ignore]
async fn voice_token_and_metadata() {
    use chad_core::VoiceState;
    let origin = std::env::var("IRCHAD_ORIGIN").unwrap_or("localhost:8443".into());
    let network = format!("https://{origin}");
    let media = Arc::new(FakeMedia(Default::default()));
    let config = |d: &std::path::Path, media: Option<Arc<FakeMedia>>| Config {
        data_dir: d.to_owned(),
        secrets: Arc::new(MemorySecrets::default()),
        notify: Arc::new(|| {}),
        media: media.map(|m| m as Arc<dyn chad_core::Media>),
    };
    let login = |account: &str| Command::Join {
        invite: origin.clone(),
        auth: Auth::Login {
            account: account.into(),
            password: format!("{account}pass"),
        },
    };
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (alice, mut arx) = chad_core::start(config(a.path(), Some(media.clone())));
    alice.send(login("alice"));
    until(&mut arx, connected).await;
    let (bob, mut brx) = chad_core::start(config(b.path(), None));
    bob.send(login("bob"));
    until(&mut brx, connected).await;
    // Both synced (the burst is in) before anyone joins.
    for rx in [&mut arx, &mut brx] {
        until(rx, |e| {
            matches!(e, Event::Buffer { buffer, .. } if buffer.target == "#dev").then_some(())
        })
        .await;
    }

    alice.send(Command::Net {
        network: network.clone(),
        cmd: NetCommand::JoinVoice {
            channel: "#dev".into(),
        },
    });
    until(&mut arx, |e| match e {
        Event::Voice {
            state: VoiceState::Connected { channel },
            ..
        } => (channel == "#dev").then_some(()),
        _ => None,
    })
    .await;
    let (url, token) = media.0.lock().unwrap()[0].clone();
    assert!(url.ends_with("/livekit"), "{url}");
    assert_eq!(token.split('.').count(), 3, "a JWT");

    // bob, a plain metadata subscriber, sees alice in #dev's voice room.
    let in_voice = |want: bool| {
        move |e: &Event| match e {
            Event::VoiceMembers { channel, nicks, .. } if channel == "#dev" => {
                (nicks.iter().any(|n| n == "alice") == want).then_some(())
            }
            _ => None,
        }
    };
    until(&mut brx, in_voice(true)).await;

    alice.send(Command::Net {
        network: network.clone(),
        cmd: NetCommand::LeaveVoice,
    });
    until(&mut brx, in_voice(false)).await;

    // bob has no media: voice is refused, not attempted.
    bob.send(Command::Net {
        network,
        cmd: NetCommand::JoinVoice {
            channel: "#dev".into(),
        },
    });
    until(&mut brx, |e| match e {
        Event::Error { error, .. } => Some(error.clone()),
        _ => None,
    })
    .await;
}

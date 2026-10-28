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
    tokio::time::sleep(Duration::from_millis(300)).await;

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
    tokio::time::sleep(Duration::from_millis(300)).await;

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

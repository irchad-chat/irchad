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
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
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
        version: "test".into(),
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
        version: "test".into(),
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
        version: "test".into(),
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

#[tokio::test]
#[ignore]
async fn guest_signs_up() {
    let origin = std::env::var("IRCHAD_ORIGIN").unwrap_or("localhost:8443".into());
    let network = format!("https://{origin}");
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let config = || Config {
        data_dir: dir.path().to_owned(),
        secrets: secrets.clone(),
        notify: Arc::new(|| {}),
        media: None,
        version: "test".into(),
    };
    let nick = format!("gs{}", std::time::UNIX_EPOCH.elapsed().unwrap().as_secs());
    let register = |account: &str| Command::Net {
        network: network.clone(),
        cmd: NetCommand::Register {
            account: account.into(),
            password: "guestsignup".into(),
            email: None,
        },
    };

    let (core, mut rx) = chad_core::start(config());
    core.send(Command::Join {
        invite: format!("https://{origin}/invite?channel=%23dev"),
        auth: Auth::Guest { nick: nick.clone() },
    });
    assert_eq!(until(&mut rx, connected).await, nick);
    until(&mut rx, |e| {
        matches!(e, Event::Buffer { buffer, .. } if buffer.target == "#dev").then_some(())
    })
    .await;

    // A taken name: still a guest, still connected.
    core.send(register("alice"));
    until(&mut rx, |e| {
        matches!(e, Event::SignUpFailed { .. }).then_some(())
    })
    .await;
    until(&mut rx, |e| match e {
        Event::Network(n) if n.state == ConnState::Connected && n.guest => Some(()),
        _ => None,
    })
    .await;

    // Our own nick: an account now, and back in #dev as it.
    core.send(register(&nick));
    until(&mut rx, |e| match e {
        Event::Network(n) if n.state == ConnState::Connected && !n.guest => Some(()),
        _ => None,
    })
    .await;
    in_dev_as_account(&mut rx, &core, &network, &nick).await;
    drop(core);
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // Restart: logs in with the saved password; always-on kept #dev.
    let (core, mut rx) = chad_core::start(config());
    until(&mut rx, |e| match e {
        Event::Network(n) if n.state == ConnState::Connected && !n.guest => Some(()),
        _ => None,
    })
    .await;
    in_dev_as_account(&mut rx, &core, &network, &nick).await;
}

async fn in_dev_as_account(
    rx: &mut UnboundedReceiver<Event>,
    core: &chad_core::Core,
    network: &str,
    nick: &str,
) {
    core.send(Command::Net {
        network: network.into(),
        cmd: NetCommand::Members {
            channel: "#dev".into(),
        },
    });
    until(rx, |e| match e {
        Event::Members {
            channel, members, ..
        } if channel == "#dev" => members
            .iter()
            .any(|m| m.nick == nick && m.account.as_deref() == Some(nick))
            .then_some(()),
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
        version: "test".into(),
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

/// One core on both dev networks at once (`chat.localhost` and
/// `chat2.localhost`), chatting on each, plus an upload and a profile on the
/// first. Override the hosts with `IRCHAD_ORIGIN` / `IRCHAD_ORIGIN2`.
#[tokio::test]
#[ignore]
async fn two_networks_upload_profile() {
    let origins = [
        std::env::var("IRCHAD_ORIGIN").unwrap_or("localhost:8443".into()),
        std::env::var("IRCHAD_ORIGIN2").unwrap_or("chat2.localhost:8443".into()),
    ];
    let dir = tempfile::tempdir().unwrap();
    let (core, mut rx) = chad_core::start(Config {
        data_dir: dir.path().to_owned(),
        secrets: Arc::new(MemorySecrets::default()),
        notify: Arc::new(|| {}),
        media: None,
        version: "test".into(),
    });
    for origin in &origins {
        core.send(Command::Join {
            invite: origin.clone(),
            auth: Auth::Login {
                account: "alice".into(),
                password: "alicepass".into(),
            },
        });
        until(&mut rx, connected).await;
    }
    for origin in &origins {
        let network = format!("https://{origin}");
        let text = format!("hi from two networks, on {origin}");
        core.send(Command::Net {
            network: network.clone(),
            cmd: NetCommand::Send {
                target: "#general".into(),
                text: text.clone(),
                reply_to: None,
            },
        });
        until(&mut rx, |e| match e {
            Event::Messages {
                network: n,
                messages,
                ..
            } if *n == network => messages
                .iter()
                .any(|m| m.text == text && m.status == SendStatus::Sent)
                .then_some(()),
            _ => None,
        })
        .await;
    }

    let network = format!("https://{}", origins[0]);
    let png = b"\x89PNG\r\n\x1a\nnot really".to_vec();
    core.send(Command::Net {
        network: network.clone(),
        cmd: NetCommand::Upload {
            target: Some("#general".into()),
            name: "tiny pic.png".into(),
            bytes: chad_core::Blob(png.clone().into()),
        },
    });
    let url = until(&mut rx, |e| match e {
        Event::Uploaded { result, .. } => Some(result.clone().expect("upload")),
        _ => None,
    })
    .await;
    assert!(url.path().ends_with("/tiny_pic.png"), "{url}");
    // Our own upload comes back directly; the bytes are what we sent.
    core.send(Command::Net {
        network: network.clone(),
        cmd: NetCommand::Media {
            url: url.to_string(),
        },
    });
    let got = until(&mut rx, |e| match e {
        Event::Media { result, .. } => Some(result.clone().expect("media")),
        _ => None,
    })
    .await;
    assert_eq!(&got.0[..], &png[..]);

    core.send(Command::Net {
        network: network.clone(),
        cmd: NetCommand::SetDisplayName {
            name: "Alice Live".into(),
        },
    });
    until(&mut rx, |e| match e {
        Event::Profile {
            nick, display_name, ..
        } if nick == "alice" => (display_name.as_deref() == Some("Alice Live")).then_some(()),
        _ => None,
    })
    .await;
    core.send(Command::Net {
        network,
        cmd: NetCommand::SetDisplayName {
            name: String::new(),
        },
    });
}

async fn state(rx: &mut UnboundedReceiver<Event>, nick: &str) -> chad_core::VerifyState {
    until(rx, |e| match e {
        Event::Verification { nick: n, state, .. } if n == nick => Some(state.clone()),
        _ => None,
    })
    .await
}

/// Phase 4's done-when: alice on two installs and bob on one. A DM sent while
/// bob is offline decrypts on all three, his reply too, and they verify with
/// emoji. A plain client's DM shows as not encrypted. Fresh accounts every run,
/// so device lists don't pile up on the server.
#[tokio::test]
#[ignore]
async fn encrypted_dms_across_devices() {
    use chad_core::{Crypt, Encryption, VerifyState, VerifyStep};
    let origin = std::env::var("IRCHAD_ORIGIN").unwrap_or("localhost:8443".into());
    let network = format!("https://{origin}");
    let stamp = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs();
    let (alice, bob) = (format!("ea{stamp}"), format!("eb{stamp}"));
    let config = |d: &std::path::Path, secrets: Arc<MemorySecrets>| Config {
        data_dir: d.to_owned(),
        secrets,
        notify: Arc::new(|| {}),
        media: None,
        version: "test".into(),
    };
    let auth = |account: &str, register: bool| {
        let (account, password) = (account.to_owned(), "livecorepass".to_owned());
        if register {
            Auth::Register {
                account,
                password,
                email: None,
            }
        } else {
            Auth::Login { account, password }
        }
    };
    let net = |cmd: NetCommand| Command::Net {
        network: network.clone(),
        cmd,
    };
    let dirs = [(); 4].map(|()| tempfile::tempdir().unwrap());
    let bob_secrets = Arc::new(MemorySecrets::default());

    let (a1, mut a1rx) = chad_core::start(config(dirs[0].path(), Default::default()));
    a1.send(Command::Join {
        invite: origin.clone(),
        auth: auth(&alice, true),
    });
    until(&mut a1rx, connected).await;
    let (b, mut brx) = chad_core::start(config(dirs[1].path(), bob_secrets.clone()));
    b.send(Command::Join {
        invite: origin.clone(),
        auth: auth(&bob, true),
    });
    until(&mut brx, connected).await;
    let (a2, mut a2rx) = chad_core::start(config(dirs[2].path(), Default::default()));
    a2.send(Command::Join {
        invite: origin.clone(),
        auth: auth(&alice, false),
    });
    until(&mut a2rx, connected).await;

    // Both of alice's devices are published, and a1 knows about a2.
    let devices = |nick: String, n: usize| {
        move |e: &Event| match e {
            Event::Devices {
                nick: who, devices, ..
            } if *who == nick && devices.len() == n => Some(()),
            _ => None,
        }
    };
    for _ in 0..20 {
        a1.send(net(NetCommand::Devices {
            nick: alice.clone(),
        }));
        let got = tokio::time::timeout(
            Duration::from_secs(1),
            until(&mut a1rx, devices(alice.clone(), 2)),
        );
        if got.await.is_ok() {
            break;
        }
    }
    until(&mut brx, |e| match e {
        Event::Devices { nick, devices, .. } if *nick == bob => {
            devices.iter().any(|d| d.this).then_some(())
        }
        _ => None,
    })
    .await;
    a1.send(net(NetCommand::OpenDm { nick: bob.clone() }));
    until(&mut a1rx, devices(bob.clone(), 1)).await;

    // bob is offline when alice writes.
    drop(b);
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let secret = format!("the secret is {stamp}");
    a1.send(net(NetCommand::Send {
        target: bob.clone(),
        text: secret.clone(),
        reply_to: None,
    }));
    let has = |target: String, text: String| {
        move |e: &Event| match e {
            Event::Messages {
                target: t,
                messages,
                ..
            } if *t == target => messages
                .iter()
                .find(|m| m.text == text && m.msgid.is_some())
                .map(|m| m.crypt),
            _ => None,
        }
    };
    assert_eq!(
        until(&mut a1rx, has(bob.clone(), secret.clone())).await,
        Crypt::Encrypted
    );
    assert_eq!(
        until(&mut a2rx, has(bob.clone(), secret.clone())).await,
        Crypt::Encrypted
    );

    // bob comes back and reads it from history, then answers.
    let (b, mut brx) = chad_core::start(config(dirs[1].path(), bob_secrets.clone()));
    until(&mut brx, connected).await;
    assert_eq!(
        until(&mut brx, has(alice.clone(), secret.clone())).await,
        Crypt::Encrypted
    );
    let reply = format!("got it {stamp}");
    b.send(net(NetCommand::Send {
        target: alice.clone(),
        text: reply.clone(),
        reply_to: None,
    }));
    for rx in [&mut a1rx, &mut a2rx] {
        assert_eq!(
            until(rx, has(bob.clone(), reply.clone())).await,
            Crypt::Encrypted
        );
    }

    // a1 and bob compare emoji.
    a1.send(net(NetCommand::Verification {
        nick: bob.clone(),
        step: VerifyStep::Start,
    }));
    assert_eq!(state(&mut brx, &alice).await, VerifyState::Incoming);
    b.send(net(NetCommand::Verification {
        nick: alice.clone(),
        step: VerifyStep::Accept,
    }));
    let compare = |s: VerifyState| match s {
        VerifyState::Compare(e) => e,
        other => panic!("{other:?}"),
    };
    let mut seen = Vec::new();
    for (rx, nick) in [(&mut a1rx, bob.clone()), (&mut brx, alice.clone())] {
        loop {
            match state(rx, &nick).await {
                VerifyState::Waiting => continue,
                s => break seen.push(compare(s)),
            }
        }
    }
    assert_eq!(seen[0], seen[1]);
    a1.send(net(NetCommand::Verification {
        nick: bob.clone(),
        step: VerifyStep::Match,
    }));
    b.send(net(NetCommand::Verification {
        nick: alice.clone(),
        step: VerifyStep::Match,
    }));
    for (rx, nick) in [(&mut a1rx, bob.clone()), (&mut brx, alice.clone())] {
        loop {
            match state(rx, &nick).await {
                VerifyState::Waiting => continue,
                s => break assert_eq!(s, VerifyState::Done),
            }
        }
    }
    until(&mut a1rx, |e| match e {
        Event::Buffer { buffer, .. } if buffer.target == bob => {
            (buffer.encryption == Encryption::Verified).then_some(())
        }
        _ => None,
    })
    .await;

    // A plain client (a guest here) gets no encryption, and it shows.
    let guest = format!("gp{stamp}");
    let (g, mut grx) = chad_core::start(config(dirs[3].path(), Default::default()));
    g.send(Command::Join {
        invite: origin.clone(),
        auth: Auth::Guest {
            nick: guest.clone(),
        },
    });
    until(&mut grx, connected).await;
    g.send(net(NetCommand::Send {
        target: bob.clone(),
        text: "plain hi".into(),
        reply_to: None,
    }));
    assert_eq!(
        until(&mut brx, has(guest.clone(), "plain hi".into())).await,
        Crypt::Plain
    );
    until(&mut brx, |e| match e {
        Event::Buffer { buffer, .. } if buffer.target == guest => {
            (buffer.encryption == Encryption::Off).then_some(())
        }
        _ => None,
    })
    .await;

    // Logging out unpublishes the device.
    a2.send(Command::Remove {
        network: network.clone(),
    });
    until(&mut a2rx, |e| {
        matches!(e, Event::NetworkRemoved { .. }).then_some(())
    })
    .await;
    for _ in 0..20 {
        a1.send(net(NetCommand::Devices {
            nick: alice.clone(),
        }));
        let got = tokio::time::timeout(
            Duration::from_secs(1),
            until(&mut a1rx, devices(alice.clone(), 1)),
        );
        if got.await.is_ok() {
            return;
        }
    }
    panic!("a2 is still listed");
}

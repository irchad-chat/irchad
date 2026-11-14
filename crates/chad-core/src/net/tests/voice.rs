//! Voice rooms, profiles and renames.

use super::*;

fn voice_members(events: &[Event], channel: &str) -> Option<Vec<String>> {
    events.iter().rev().find_map(|e| match e {
        Event::VoiceMembers {
            channel: c, nicks, ..
        } if c == channel => Some(nicks.clone()),
        _ => None,
    })
}

fn voice_state(events: &[Event]) -> Option<VoiceState> {
    events.iter().rev().find_map(|e| match e {
        Event::Voice { state, .. } => Some(state.clone()),
        _ => None,
    })
}

#[test]
fn voice_metadata_and_calls() {
    let mut n = net();
    n.caps.handle(&line("CAP * LS :draft/metadata-2"));
    n.caps.handle(&line("CAP * ACK :draft/metadata-2"));
    assert!(n.caps.enabled("draft/metadata-2"));
    n.attach("bob");
    let label = out(&mut n)[1]
        .strip_prefix("@label=")
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .to_owned();
    feed(&mut n, ":bob!u@h JOIN #dev bob :bob");
    // Our own value in the reattach burst is left over from a crash.
    feed(&mut n, ":s 761 * bob chad/voice * #dev");
    assert!(out(&mut n).contains(&"METADATA * SET chad/voice".to_owned()));
    feed(&mut n, ":s 761 * alice chad/voice * #Dev");
    feed(&mut n, ":s 761 * carol chad/voice * :not a channel");
    feed(&mut n, &format!("@label={label} :s PONG s sync"));
    let ev = events(&mut n);
    assert_eq!(voice_members(&ev, "#dev").unwrap(), ["alice", "bob"]);
    out(&mut n);

    // After sync our own value is left alone (another device may be in a call).
    feed(&mut n, ":s 761 * bob chad/voice * #dev");
    assert!(out(&mut n).is_empty());
    feed(&mut n, ":s 766 * bob chad/voice :Key deleted");
    feed(&mut n, ":alice!u@h NICK alicia");
    assert_eq!(voice_members(&events(&mut n), "#dev").unwrap(), ["alicia"]);
    // Every device of carol's is gone: ergo auto-aways her, and her value is stale.
    feed(&mut n, ":s 761 * carol chad/voice * #dev");
    feed(&mut n, ":carol!u@h AWAY :User is currently disconnected");
    assert_eq!(voice_members(&events(&mut n), "#dev").unwrap(), ["alicia"]);
    feed(&mut n, ":alicia!u@h AWAY :brb");
    assert!(voice_members(&events(&mut n), "#dev").is_none());
    feed(&mut n, ":alicia!u@h QUIT :bye");
    assert_eq!(
        voice_members(&events(&mut n), "#dev").unwrap(),
        Vec::<String>::new()
    );

    // Join: conn fetches a token, connects, then we publish.
    n.command(NetCommand::JoinVoice {
        channel: "#DEV".into(),
    });
    assert_eq!(n.voice_request.take().as_deref(), Some("#dev"));
    assert_eq!(
        voice_state(&events(&mut n)),
        Some(VoiceState::Connecting {
            channel: "#dev".into()
        })
    );
    n.voice_connected();
    assert_eq!(out(&mut n), ["METADATA * SET chad/voice #dev"]);
    // Something else cleared it while we're in: put it back.
    feed(&mut n, ":s 766 * bob chad/voice :Key deleted");
    assert_eq!(out(&mut n), ["METADATA * SET chad/voice #dev"]);

    // Ops turn voice off: we leave, and clear our value.
    feed(&mut n, ":s 761 * #dev chad/voice * off");
    let ev = events(&mut n);
    assert!(!buffer(&ev, "#dev").unwrap().voice);
    assert_eq!(voice_state(&ev), Some(VoiceState::Idle));
    assert_eq!(out(&mut n), ["METADATA * SET chad/voice"]);
    assert!(n.voice.is_none());
    n.command(NetCommand::JoinVoice {
        channel: "#dev".into(),
    });
    assert!(n.voice_request.is_none());

    // Dropping the connection ends the call without touching the server.
    feed(&mut n, ":s 766 * #dev chad/voice :Key deleted");
    n.command(NetCommand::JoinVoice {
        channel: "#dev".into(),
    });
    n.voice_connected();
    out(&mut n);
    events(&mut n);
    n.detach();
    assert!(n.out.is_empty());
    assert_eq!(voice_state(&events(&mut n)), Some(VoiceState::Idle));
}

#[test]
fn guests_get_no_voice() {
    let mut n = net();
    attach(&mut n, &["#dev"]);
    n.guest = true;
    n.command(NetCommand::JoinVoice {
        channel: "#dev".into(),
    });
    assert!(n.voice_request.is_none());
    assert!(
        events(&mut n)
            .iter()
            .any(|e| matches!(e, Event::Error { .. }))
    );
}

const MEET: &str = "#meet-0123456789abcdefghijklmnopqrstuv0123";

#[test]
fn guests_get_voice_in_meetings() {
    let mut n = net();
    attach(&mut n, &["#dev", MEET]);
    n.guest = true;
    n.command(NetCommand::JoinVoice {
        channel: MEET.into(),
    });
    assert_eq!(n.voice_request.take().as_deref(), Some(MEET));
}

#[test]
fn meetings_join_their_call_and_hanging_up_leaves() {
    let mut n = net();
    attach(&mut n, &["#dev"]);
    out(&mut n);
    n.command(NetCommand::JoinMeeting {
        channel: "#dev".into(),
        new: true,
    });
    assert!(out(&mut n).is_empty(), "not a meeting");

    n.command(NetCommand::JoinMeeting {
        channel: MEET.into(),
        new: true,
    });
    assert_eq!(out(&mut n), [format!("JOIN {MEET}")]);
    feed(&mut n, &format!(":bob!u@h JOIN {MEET} bob :bob"));
    assert!(out(&mut n).contains(&format!("MODE {MEET} +s")));
    assert_eq!(n.voice_request.take().as_deref(), Some(MEET));
    n.voice_connected();
    out(&mut n);

    // Switching calls leaves the meeting; so does hanging up.
    n.command(NetCommand::JoinVoice {
        channel: "#dev".into(),
    });
    assert!(out(&mut n).contains(&format!("PART {MEET}")));
    n.command(NetCommand::LeaveVoice);
    assert!(!out(&mut n).iter().any(|l| l.starts_with("PART")));

    // Joining someone else's: no +s, and a dropped call keeps the chat.
    n.command(NetCommand::JoinMeeting {
        channel: MEET.into(),
        new: false,
    });
    feed(&mut n, &format!(":bob!u@h JOIN {MEET} bob :bob"));
    assert!(!out(&mut n).iter().any(|l| l.starts_with("MODE")));
    n.voice_ended(Some("Lost the voice connection.".into()));
    assert!(!out(&mut n).iter().any(|l| l.starts_with("PART")));
    n.command(NetCommand::JoinVoice {
        channel: MEET.into(),
    });
    n.command(NetCommand::LeaveVoice);
    assert!(out(&mut n).contains(&format!("PART {MEET}")));
}

#[test]
fn profiles_and_away() {
    let mut n = net();
    n.caps.handle(&line("CAP * LS :draft/metadata-2"));
    n.caps.handle(&line("CAP * ACK :draft/metadata-2"));
    n.command(NetCommand::SetAway {
        message: Some("lunch".into()),
    });
    n.attach("bob");
    let sent = out(&mut n);
    assert_eq!(sent[0], "AWAY lunch");
    feed(&mut n, ":s 761 * Alice display-name * :Alice \x07 Smith");
    feed(&mut n, ":s 761 * alice avatar * :javascript:alert(1)");
    let ev = events(&mut n);
    let profile = |ev: &[Event]| {
        ev.iter()
            .rev()
            .find_map(|e| match e {
                Event::Profile {
                    display_name,
                    avatar,
                    ..
                } => Some((display_name.clone(), avatar.clone())),
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(profile(&ev), (Some("Alice  Smith".into()), None));
    feed(&mut n, ":s 761 * alice avatar * https://x/a.png");
    feed(&mut n, ":s 766 * alice display-name :not set");
    let (name, avatar) = profile(&events(&mut n));
    assert_eq!((name, avatar.unwrap().as_str()), (None, "https://x/a.png"));

    // Someone joining later: ask once.
    feed(&mut n, ":bob!u@h JOIN #dev bob :bob");
    out(&mut n);
    feed(&mut n, ":carol!u@h JOIN #dev * :c");
    feed(&mut n, ":carol!u@h PART #dev");
    feed(&mut n, ":carol!u@h JOIN #dev * :c");
    assert_eq!(
        out(&mut n),
        ["METADATA carol GET display-name avatar chad/activity chad/music"]
    );

    n.command(NetCommand::SetDisplayName {
        name: "  Bobby  ".into(),
    });
    n.command(NetCommand::SetAvatar { url: String::new() });
    n.command(NetCommand::SetAvatar {
        url: "file:///etc/passwd".into(),
    });
    n.command(NetCommand::SetAway { message: None });
    assert_eq!(
        out(&mut n),
        [
            "METADATA * SET display-name Bobby",
            "METADATA * SET avatar",
            "AWAY"
        ]
    );
}

#[test]
fn activity() {
    let mut n = net();
    n.caps.handle(&line("CAP * LS :draft/metadata-2"));
    n.caps.handle(&line("CAP * ACK :draft/metadata-2"));
    // Not managing it (phone, or sharing off): another device's value is left alone.
    n.attach("bob");
    out(&mut n);
    feed(&mut n, ":s 761 * bob chad/activity * :Playing Elsewhere");
    assert!(out(&mut n).is_empty());

    feed(
        &mut n,
        ":s 761 * alice chad/activity * :Playing \x1b[31mCeleste",
    );
    let activity = events(&mut n).iter().rev().find_map(|e| match e {
        Event::Profile { activity, .. } => Some(activity.clone()),
        _ => None,
    });
    assert_eq!(activity, Some(Some("Playing [31mCeleste".into())));

    let set = |n: &mut Net, t: Option<&str>| {
        n.command(NetCommand::SetActivity {
            text: t.map(str::to_owned),
            music: None,
        });
    };
    // Turning sharing on clears whatever a crash left; then it follows the game.
    set(&mut n, Some(""));
    set(&mut n, Some(" Playing Celeste "));
    set(&mut n, Some("Playing Celeste"));
    set(&mut n, Some(""));
    set(&mut n, Some("Playing Celeste"));
    assert_eq!(
        out(&mut n),
        [
            "METADATA * SET chad/music",
            "METADATA * SET chad/activity",
            "METADATA * SET chad/activity :Playing Celeste",
            "METADATA * SET chad/activity",
            "METADATA * SET chad/activity :Playing Celeste",
        ]
    );
    // Kept across reconnects.
    n.attach("bob");
    assert!(out(&mut n).contains(&"METADATA * SET chad/activity :Playing Celeste".to_owned()));
    // Turning sharing off takes ours down, once.
    set(&mut n, None);
    set(&mut n, None);
    assert_eq!(out(&mut n), ["METADATA * SET chad/activity"]);
    n.attach("bob");
    assert!(!out(&mut n).iter().any(|l| l.contains("chad/activity")));
}

#[test]
fn music() {
    let mut n = net();
    n.caps.handle(&line("CAP * LS :draft/metadata-2"));
    n.caps.handle(&line("CAP * ACK :draft/metadata-2"));
    n.attach("bob");
    out(&mut n);
    let song = crate::Music::parse("100 200 https://open.spotify.com/track/x");
    let set = |n: &mut Net, t: &str, m: &Option<crate::Music>| {
        n.command(NetCommand::SetActivity {
            text: Some(t.to_owned()),
            music: m.clone(),
        });
    };
    set(&mut n, "Listening to X", &song);
    set(&mut n, "Listening to X", &song);
    // A game takes over: its line goes up without the song's progress.
    set(&mut n, "Playing Celeste", &song.clone().filter(|_| false));
    assert_eq!(
        out(&mut n),
        [
            "METADATA * SET chad/music :100 200 https://open.spotify.com/track/x",
            "METADATA * SET chad/activity :Listening to X",
            "METADATA * SET chad/music",
            "METADATA * SET chad/activity :Playing Celeste",
        ]
    );
    // Theirs is checked: a bad link is dropped, junk is no music at all.
    let music = |n: &mut Net| {
        events(n).iter().rev().find_map(|e| match e {
            Event::Profile { music, .. } => Some(music.clone()),
            _ => None,
        })
    };
    feed(
        &mut n,
        ":s 761 * alice chad/music * :5 60 https://evil.example/",
    );
    assert_eq!(music(&mut n), Some(crate::Music::parse("5 60")));
    feed(&mut n, ":s 761 * alice chad/music * :lol");
    assert_eq!(music(&mut n), Some(None));
}

#[test]
fn channel_rename_keeps_history_and_ends_the_call() {
    let mut n = net();
    n.caps.handle(&line("CAP * LS :draft/metadata-2"));
    n.caps.handle(&line("CAP * ACK :draft/metadata-2"));
    n.store.insert("#old", &msg("o1", 1000, "alice", "before"));
    attach(&mut n, &["#old", "#taken"]);
    n.command(NetCommand::JoinVoice {
        channel: "#old".into(),
    });
    n.voice_connected();
    out(&mut n);
    events(&mut n);

    // Onto a channel we already hold: ignored.
    feed(&mut n, ":s RENAME #old #Taken :x");
    assert!(events(&mut n).is_empty());

    feed(&mut n, ":s RENAME #old #New :tidy");
    let ev = events(&mut n);
    assert!(
        matches!(&ev[0], Event::BufferRenamed { from, to, .. } if from == "#old" && to == "#New")
    );
    assert!(buffer(&ev, "#New").is_some());
    assert_eq!(voice_state(&ev), Some(VoiceState::Idle));
    assert_eq!(out(&mut n), ["METADATA * SET chad/voice"]);
    assert_eq!(n.store.recent("#new", 10)[0].text, "before");
    assert!(
        n.store
            .buffers()
            .iter()
            .any(|b| b.key == "#new" && b.name == "#New")
    );
    assert!(n.joined.contains("#new") && !n.joined.contains("#old"));

    // Unknown channel, or a new name that isn't a channel: nothing happens.
    feed(&mut n, ":s RENAME #nope #other :x");
    feed(&mut n, ":s RENAME #new alice :x");
    assert!(events(&mut n).is_empty());
}

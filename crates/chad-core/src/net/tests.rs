use super::*;
use crate::Encryption;
use crate::store::msg;

fn net() -> Net {
    let info = NetworkInfo {
        network: "https://x".into(),
        name: "X".into(),
        icon: None,
        nick: "bob".into(),
        state: ConnState::Connecting,
        uploads: None,
        gifs: None,
    };
    let mut n = Net::new(Store::memory(), info);
    let caps = "batch echo-message labeled-response draft/chathistory draft/read-marker \
                draft/persistence message-tags server-time draft/multiline";
    n.caps.handle(&line(&format!(
        "CAP * LS :{caps}=max-bytes=4096,max-lines=100"
    )));
    n.caps.handle(&line(&format!("CAP * ACK :{caps}")));
    n.isupport.handle(&line(
        ":s 005 bob CASEMAPPING=ascii CHANTYPES=# CHATHISTORY=1000 MONITOR=100 \
         PREFIX=(qaohv)~&@%+ STATUSMSG=~&@%+ :are supported",
    ));
    n
}

fn line(s: &str) -> Message {
    Message::parse(s).unwrap()
}

fn feed(n: &mut Net, s: &str) {
    n.handle(line(s)).unwrap();
}

fn out(n: &mut Net) -> Vec<String> {
    n.out.drain(..).map(|m| m.to_line().unwrap()).collect()
}

fn events(n: &mut Net) -> Vec<Event> {
    std::mem::take(&mut n.events)
}

fn ts(ms: i64) -> String {
    time::format(ms)
}

/// Attaches as bob, joined to `channels` by the server's burst, then syncs.
fn attach(n: &mut Net, channels: &[&str]) -> Vec<String> {
    n.attach("bob");
    let sync = out(n);
    assert_eq!(sync[0], "PERSISTENCE GET");
    let label = sync[1]
        .strip_prefix("@label=")
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .to_owned();
    assert!(sync[1].ends_with("PING sync"));
    feed(n, ":s PERSISTENCE STATUS DEFAULT ON");
    for c in channels {
        feed(n, &format!(":bob!u@h JOIN {c} bob :bob"));
    }
    feed(n, &format!("@label={label} :s PONG s sync"));
    out(n)
}

fn buffer(events: &[Event], target: &str) -> Option<BufferInfo> {
    events.iter().rev().find_map(|e| match e {
        Event::Buffer { buffer, .. } if buffer.target == target => Some(buffer.clone()),
        _ => None,
    })
}

fn messages(events: &[Event]) -> Vec<ChatMessage> {
    events
        .iter()
        .flat_map(|e| match e {
            Event::Messages { messages, .. } => messages.clone(),
            _ => Vec::new(),
        })
        .collect()
}

fn label_of(l: &str) -> &str {
    l.strip_prefix("@label=")
        .unwrap()
        .split([' ', ';'])
        .next()
        .unwrap()
}

#[test]
fn first_login_with_no_channels_joins_everything_listed() {
    let mut n = net();
    let sent = attach(&mut n, &[]);
    let list = sent.iter().find(|l| l.ends_with(" LIST")).expect("LIST");
    let label = label_of(list).to_owned();
    for l in [
        format!("@label={label} :s BATCH +L labeled-response"),
        "@batch=L :s 321 bob Channel :Users Name".into(),
        "@batch=L :s 322 bob #general 3 :General".into(),
        "@batch=L :s 322 bob #dev 2 :Dev".into(),
        "@batch=L :s 322 bob bad,name 2 :x".into(),
        "@batch=L :s 323 bob :End".into(),
        ":s BATCH -L".into(),
    ] {
        feed(&mut n, &l);
    }
    assert_eq!(out(&mut n), ["JOIN #general,#dev"]);

    // Once only, even if the user later leaves everything.
    n.detach();
    assert!(!attach(&mut n, &[]).iter().any(|l| l.ends_with(" LIST")));
}

#[test]
fn reattach_trusts_the_server_channel_list_and_fills_gaps() {
    let mut n = net();
    n.store
        .put_buffer("#general", "#general", BufferKind::Channel);
    n.store.put_buffer("#left", "#left", BufferKind::Channel);
    n.store.put_buffer("alice", "alice", BufferKind::Dm);
    n.store.insert("#general", &msg("g1", 1000, "alice", "old"));
    n.store.set_meta("autojoined", "1");
    n.load_cache();
    let ev = events(&mut n);
    assert_eq!(messages(&ev)[0].text, "old");

    let sent = attach(&mut n, &["#general"]);
    let ev = events(&mut n);
    assert!(
        ev.iter()
            .any(|e| matches!(e, Event::BufferRemoved { target, .. } if target == "#left"))
    );
    // No JOINs sent: always-on already has us in our channels.
    assert!(!sent.iter().any(|l| l.contains("JOIN")), "{sent:?}");
    assert!(
        sent.iter()
            .any(|l| l.ends_with("CHATHISTORY AFTER #general msgid=g1 100"))
    );
    assert!(
        sent.iter()
            .any(|l| l.ends_with("CHATHISTORY LATEST alice * 100"))
    );
    assert!(sent.iter().any(|l| l.contains(&format!(
        "CHATHISTORY TARGETS timestamp={} timestamp=",
        ts(1000)
    ))));
    assert!(sent.contains(&"MONITOR + alice".to_owned()));

    // TARGETS names a DM that arrived while offline.
    let targets = sent.iter().find(|l| l.contains("TARGETS")).unwrap();
    let label = label_of(targets).to_owned();
    for l in [
        format!("@label={label} :s BATCH +T draft/chathistory-targets"),
        format!("@batch=T :s CHATHISTORY TARGETS carol {}", ts(2000)),
        format!("@batch=T :s CHATHISTORY TARGETS #general {}", ts(2000)),
        ":s BATCH -T".into(),
    ] {
        feed(&mut n, &l);
    }
    let sent = out(&mut n);
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0], "MONITOR + carol");
    assert!(sent[1].ends_with("CHATHISTORY LATEST carol * 100"));
}

#[test]
fn without_always_on_cached_channels_are_rejoined() {
    let mut n = net();
    n.store
        .put_buffer("#general", "#general", BufferKind::Channel);
    n.store.set_meta("autojoined", "1");
    n.load_cache();
    n.attach("bob");
    let label = label_of(&out(&mut n)[1]).to_owned();
    feed(&mut n, ":s PERSISTENCE STATUS OFF OFF");
    feed(&mut n, &format!("@label={label} :s PONG s sync"));
    assert!(out(&mut n).contains(&"JOIN #general".to_owned()));
}

#[test]
fn send_echo_and_failure() {
    let mut n = net();
    attach(&mut n, &["#c"]);
    events(&mut n);

    n.command(NetCommand::Send {
        target: "#c".into(),
        text: "hello".into(),
        reply_to: Some("p1".into()),
    });
    let pending = messages(&events(&mut n));
    assert_eq!(pending[0].status, SendStatus::Pending);
    let sent = out(&mut n);
    assert!(sent[0].starts_with("@+draft/reply=p1;label="), "{sent:?}");
    assert!(sent[0].ends_with("PRIVMSG #c hello"));
    let label = sent[0]
        .split("label=")
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap();
    feed(
        &mut n,
        &format!(
            "@label={label};msgid=m1;time={};+draft/reply=p1 :bob!u@h PRIVMSG #c hello",
            ts(5)
        ),
    );
    let echo = messages(&events(&mut n));
    assert_eq!(echo[0].local_id, pending[0].local_id);
    assert_eq!(echo[0].msgid.as_deref(), Some("m1"));
    assert_eq!(echo[0].reply_to.as_deref(), Some("p1"));
    assert!(echo[0].own && echo[0].status == SendStatus::Sent);

    n.command(NetCommand::Send {
        target: "#c".into(),
        text: "x".into(),
        reply_to: None,
    });
    let label = label_of(&out(&mut n)[0]).to_owned();
    feed(
        &mut n,
        &format!("@label={label} :s 404 bob #c :Cannot send to channel"),
    );
    let failed = messages(&events(&mut n));
    assert_eq!(
        failed[1].status,
        SendStatus::Failed("You can't send messages to #c right now.".into())
    );

    // Unanswered sends fail when the connection drops; offline sends fail at once.
    n.command(NetCommand::Send {
        target: "#c".into(),
        text: "lost".into(),
        reply_to: None,
    });
    n.detach();
    let ev = messages(&events(&mut n));
    assert!(matches!(ev.last().unwrap().status, SendStatus::Failed(_)));
    n.command(NetCommand::Send {
        target: "#c".into(),
        text: "offline".into(),
        reply_to: None,
    });
    assert!(out(&mut n).is_empty());
    assert!(matches!(
        messages(&events(&mut n))[0].status,
        SendStatus::Failed(_)
    ));
}

#[test]
fn multiline_both_ways() {
    let mut n = net();
    attach(&mut n, &["#c"]);
    let long = "y".repeat(MAX_PIECE + 10);
    n.command(NetCommand::Send {
        target: "#c".into(),
        text: format!("one\n\n{long}\n"),
        reply_to: None,
    });
    let sent = out(&mut n);
    assert!(sent[0].starts_with("@label=") && sent[0].ends_with("BATCH +ml1 draft/multiline #c"));
    assert_eq!(sent[1], "@batch=ml1 PRIVMSG #c one");
    assert_eq!(sent[2], "@batch=ml1 PRIVMSG #c :");
    assert!(sent[3].starts_with("@batch=ml1 PRIVMSG #c yyy"));
    assert_eq!(
        sent[4],
        "@batch=ml1;draft/multiline-concat PRIVMSG #c yyyyyyyyyy"
    );
    assert_eq!(sent[5], "BATCH -ml1");

    events(&mut n);
    for l in [
        format!(
            "@msgid=ml;time={} :alice!u@h BATCH +b draft/multiline #c",
            ts(9)
        ),
        "@batch=b :alice!u@h PRIVMSG #c :hi bob".into(),
        "@batch=b :alice!u@h PRIVMSG #c :how".into(),
        "@batch=b;draft/multiline-concat :alice!u@h PRIVMSG #c : are you".into(),
        ":alice!u@h BATCH -b".into(),
    ] {
        feed(&mut n, &l);
    }
    let ev = events(&mut n);
    let m = &messages(&ev)[0];
    assert_eq!(m.text, "hi bob\nhow are you");
    assert!(m.mention && m.msgid.as_deref() == Some("ml"));
    assert_eq!(buffer(&ev, "#c").unwrap().mentions, 1);

    // Over the server's limits: fails locally, nothing sent.
    n.command(NetCommand::Send {
        target: "#c".into(),
        text: "x\n".repeat(150),
        reply_to: None,
    });
    assert!(out(&mut n).is_empty());
}

#[test]
fn history_pages_dedupe_and_stop() {
    let mut n = net();
    let sent = attach(&mut n, &["#c"]);
    let latest = sent.iter().find(|l| l.contains("LATEST #c")).unwrap();
    let label = label_of(latest).to_owned();
    let page = |label: &str, from: usize, count: usize| {
        let mut lines = vec![format!("@label={label} :s BATCH +h chathistory #c")];
        for i in from..from + count {
            lines.push(format!(
                "@batch=h;msgid=m{i};time={} :alice!u@h PRIVMSG #c :msg {i}",
                ts(i as i64 * 1000)
            ));
        }
        lines.push(":s BATCH -h".into());
        lines
    };
    for l in page(&label, 50, 100) {
        feed(&mut n, &l);
    }
    events(&mut n);

    n.command(NetCommand::LoadOlder {
        target: "#c".into(),
    });
    let before = out(&mut n);
    assert!(before[0].ends_with("CHATHISTORY BEFORE #c msgid=m50 100"));
    // 40 older plus one we already have.
    for l in page(label_of(&before[0]), 10, 41) {
        feed(&mut n, &l);
    }
    let ev = events(&mut n);
    assert!(ev.iter().any(|e| matches!(e, Event::HistoryStart { .. })));
    assert_eq!(n.store.recent("#c", 1000).len(), 140);
    assert_eq!(buffer(&ev, "#c").unwrap().unread, 140);

    // A full AFTER page means the gap may go on.
    n.detach();
    let sent = attach(&mut n, &["#c"]);
    let after = sent
        .iter()
        .find(|l| l.contains("AFTER #c msgid=m149"))
        .unwrap();
    for l in page(label_of(after), 150, 100) {
        feed(&mut n, &l);
    }
    assert!(out(&mut n)[0].ends_with("CHATHISTORY AFTER #c msgid=m249 100"));
}

#[test]
fn read_markers_and_mentions() {
    let mut n = net();
    attach(&mut n, &["#c"]);
    feed(
        &mut n,
        &format!("@msgid=a;time={} :alice!u@h PRIVMSG #c :hey Bob!", ts(1000)),
    );
    feed(
        &mut n,
        &format!("@msgid=b;time={} :alice!u@h PRIVMSG #c :bobby no", ts(2000)),
    );
    feed(
        &mut n,
        &format!("@msgid=c;time={} :bob!u@h PRIVMSG #c :mine", ts(3000)),
    );
    let b = buffer(&events(&mut n), "#c").unwrap();
    assert_eq!((b.unread, b.mentions), (2, 1));

    feed(&mut n, &format!(":s MARKREAD #c timestamp={}", ts(1000)));
    let b = buffer(&events(&mut n), "#c").unwrap();
    assert_eq!((b.unread, b.mentions), (1, 0));
    // Markers never go backwards.
    feed(&mut n, ":s MARKREAD #c *");
    feed(&mut n, &format!(":s MARKREAD #c timestamp={}", ts(10)));
    assert_eq!(n.buffers["#c"].read, Some(1000));

    n.command(NetCommand::MarkRead {
        target: "#c".into(),
    });
    assert_eq!(out(&mut n), [format!("MARKREAD #c timestamp={}", ts(3000))]);
    assert_eq!(buffer(&events(&mut n), "#c").unwrap().unread, 0);
}

#[test]
fn dms_presence_and_ctcp() {
    let mut n = net();
    attach(&mut n, &[]);
    out(&mut n);
    feed(&mut n, "@msgid=d1 :alice!u@h PRIVMSG bob :psst");
    let ev = events(&mut n);
    let b = buffer(&ev, "alice").unwrap();
    assert_eq!((b.kind, b.unread, b.mentions), (BufferKind::Dm, 1, 1));
    assert_eq!(out(&mut n), ["MONITOR + alice"]);

    feed(&mut n, ":s 731 bob :alice");
    assert_eq!(
        buffer(&events(&mut n), "alice").unwrap().online,
        Some(false)
    );
    feed(&mut n, ":s 730 bob :alice!u@h");
    feed(&mut n, ":alice!u@h AWAY :lunch");
    let b = buffer(&events(&mut n), "alice").unwrap();
    assert_eq!((b.online, b.away), (Some(true), true));

    feed(&mut n, ":alice!u@h PRIVMSG bob :\x01ACTION waves\x01");
    feed(&mut n, ":alice!u@h PRIVMSG bob :\x01VERSION\x01");
    let m = messages(&events(&mut n));
    assert_eq!(m.len(), 1);
    assert_eq!(
        (m[0].kind, m[0].text.as_str()),
        (MessageKind::Action, "waves")
    );
    assert!(out(&mut n).is_empty(), "never answers CTCP");

    // Our own echo to a DM files under the other side.
    feed(&mut n, "@msgid=d2 :bob!u@h PRIVMSG carol :hi");
    assert!(buffer(&events(&mut n), "carol").is_some());
    // Server notices aren't conversations.
    feed(&mut n, ":s NOTICE bob :*** server stuff");
    assert!(buffer(&events(&mut n), "s").is_none());

    out(&mut n);
    n.command(NetCommand::CloseDm {
        nick: "Alice".into(),
    });
    assert!(
        events(&mut n)
            .iter()
            .any(|e| matches!(e, Event::BufferRemoved { target, .. } if target == "alice"))
    );
    assert_eq!(out(&mut n), ["MONITOR - Alice"]);
}

#[test]
fn members_wait_for_join() {
    let mut n = net();
    n.attach("bob");
    out(&mut n);
    n.command(NetCommand::Members {
        channel: "#c".into(),
    });
    assert!(out(&mut n).is_empty());
    feed(&mut n, ":bob!u@h JOIN #c bob :bob");
    assert!(out(&mut n)[0].ends_with("WHO #c %tcnfa,1"));
}

#[test]
fn members_and_presence() {
    let mut n = net();
    attach(&mut n, &["#c"]);
    n.command(NetCommand::Members {
        channel: "#c".into(),
    });
    let who = out(&mut n);
    assert!(who[0].ends_with("WHO #c %tcnfa,1"));
    for l in [
        format!("@label={} :s BATCH +w labeled-response", label_of(&who[0])),
        "@batch=w :s 354 bob 1 #c alice H+@ alice".into(),
        "@batch=w :s 354 bob 1 #c carol G 0".into(),
        "@batch=w :s 315 bob #c :End".into(),
        ":s BATCH -w".into(),
    ] {
        feed(&mut n, &l);
    }
    let members = |n: &mut Net| {
        events(n)
            .into_iter()
            .rev()
            .find_map(|e| match e {
                Event::Members { members, .. } => Some(members),
                _ => None,
            })
            .unwrap()
    };
    let m = members(&mut n);
    assert_eq!(m[0].prefix, "@");
    assert_eq!(m[1].account, None);
    assert!(m[1].away);

    feed(&mut n, ":dave!u@h JOIN #c dave :Dave");
    assert_eq!(members(&mut n)[2].account.as_deref(), Some("dave"));
    // ergo grants a founder's +q after the JOIN; only prefix modes refresh the list.
    feed(&mut n, ":s MODE #c +q dave");
    assert!(out(&mut n)[0].ends_with("WHO #c %tcnfa,1"));
    feed(&mut n, ":s MODE #c +t");
    assert!(out(&mut n).is_empty());
    feed(&mut n, ":carol!u@h AWAY");
    assert!(!members(&mut n)[1].away);
    feed(&mut n, ":dave!u@h NICK dan");
    assert_eq!(members(&mut n)[2].nick, "dan");
    feed(&mut n, ":dan!u@h QUIT :bye");
    feed(&mut n, ":alice!u@h PART #c");
    assert_eq!(members(&mut n).len(), 1);
    // A DM learns away from the member list; MONITOR only says online.
    feed(&mut n, "@msgid=d1 :carol!u@h PRIVMSG bob :hi");
    feed(&mut n, ":s 730 bob :carol!u@h");
    out(&mut n);
    n.command(NetCommand::Members {
        channel: "#c".into(),
    });
    let who = out(&mut n);
    assert!(!buffer(&events(&mut n), "carol").unwrap().away);
    for l in [
        format!("@label={} :s BATCH +w labeled-response", label_of(&who[0])),
        "@batch=w :s 354 bob 1 #c carol G 0".into(),
        "@batch=w :s 315 bob #c :End".into(),
        ":s BATCH -w".into(),
    ] {
        feed(&mut n, &l);
    }
    assert!(buffer(&events(&mut n), "carol").unwrap().away);

    feed(&mut n, ":carol!u@h KICK #c bob :out");
    assert!(
        events(&mut n)
            .iter()
            .any(|e| matches!(e, Event::BufferRemoved { .. }))
    );
}

#[test]
fn reactions_redactions_typing() {
    let mut n = net();
    attach(&mut n, &["#c"]);
    feed(&mut n, "@msgid=m1 :alice!u@h PRIVMSG #c :hi");
    events(&mut n);

    n.command(NetCommand::React {
        target: "#c".into(),
        msgid: "m1".into(),
        emoji: "👍".into(),
    });
    assert_eq!(out(&mut n), ["@+draft/react=👍;+draft/reply=m1 TAGMSG #c"]);
    feed(
        &mut n,
        "@+draft/react=👍;+draft/reply=m1 :bob!u@h TAGMSG #c",
    );
    feed(
        &mut n,
        "@+draft/react=👍;+draft/reply=m1 :carol!u@h TAGMSG #c",
    );
    let m = messages(&events(&mut n));
    assert_eq!(m.last().unwrap().reactions[0].nicks, ["bob", "carol"]);

    n.command(NetCommand::Unreact {
        target: "#c".into(),
        msgid: "m1".into(),
        emoji: "👍".into(),
    });
    assert_eq!(
        out(&mut n),
        ["@+draft/unreact=👍;+draft/reply=m1 TAGMSG #c"]
    );
    feed(
        &mut n,
        "@+draft/unreact=👍;+draft/reply=m1 :carol!u@h TAGMSG #c",
    );
    let m = messages(&events(&mut n));
    assert_eq!(m.last().unwrap().reactions[0].nicks, ["bob"]);

    feed(&mut n, ":alice!u@h REDACT #c m1");
    let m = messages(&events(&mut n));
    assert!(m[0].redacted && m[0].text.is_empty());

    feed(&mut n, "@+typing=active :alice!u@h TAGMSG #c");
    assert!(events(&mut n).iter().any(
        |e| matches!(e, Event::Typing { nick, state: Typing::Active, .. } if nick == "alice")
    ));
    let typing = |n: &mut Net, state| {
        n.command(NetCommand::Typing {
            target: "#c".into(),
            state,
        });
        out(n)
    };
    assert_eq!(
        typing(&mut n, Typing::Active),
        ["@+typing=active TAGMSG #c"]
    );
    assert!(typing(&mut n, Typing::Active).is_empty(), "throttled");
    assert_eq!(typing(&mut n, Typing::Done), ["@+typing=done TAGMSG #c"]);
    assert!(typing(&mut n, Typing::Done).is_empty());
}

#[test]
fn caps_changes_reach_the_ui() {
    let mut n = net();
    attach(&mut n, &[]);
    events(&mut n);
    feed(&mut n, ":s CAP bob NEW :away-notify");
    assert_eq!(out(&mut n), ["CAP REQ away-notify"]);
    feed(&mut n, ":s CAP bob ACK :away-notify");
    feed(&mut n, ":s CAP bob DEL :away-notify draft/multiline");
    let ev = events(&mut n);
    assert!(matches!(&ev[0], Event::CapsChanged { added, .. } if added == &["away-notify"]));
    assert!(
        matches!(&ev[1], Event::CapsChanged { removed, .. } if removed == &["away-notify", "draft/multiline"])
    );
}

#[test]
fn hostile_input_is_bounded() {
    let mut n = net();
    attach(&mut n, &[]);
    for i in 0..MAX_BUFFERS + 10 {
        feed(&mut n, &format!(":u{i}!u@h PRIVMSG bob :spam"));
    }
    assert_eq!(n.buffers.len(), MAX_BUFFERS);
    let huge = "x".repeat(MAX_TEXT * 2);
    feed(&mut n, &format!("@msgid=big :u1!u@h PRIVMSG bob :{huge}"));
    assert_eq!(n.store.get("big").unwrap().1.text.len(), MAX_TEXT);
    // Weird names never become buffers or outgoing lines.
    feed(&mut n, ":bob!u@h JOIN :#a\u{7}b");
    assert!(!n.buffers.contains_key("#a\u{7}b"));
    assert_eq!(
        failure(&Item::Message(line(
            ":s 433 * bob :Nickname is already in use"
        )))
        .unwrap(),
        "That username is taken."
    );
    assert!(mentions("hi bob.", "bob") && mentions("BOB: hi", "bob") && !mentions("bobby", "bob"));
}

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
    assert_eq!(out(&mut n), ["METADATA carol GET display-name avatar"]);

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

/// Answers the labeled request in `sent` that contains `needle` with a batch of `lines`.
fn answer(n: &mut Net, sent: &[String], needle: &str, lines: &[String]) {
    let l = sent
        .iter()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no {needle:?} in {sent:?}"));
    let label = l
        .strip_prefix("@label=")
        .unwrap()
        .split(' ')
        .next()
        .unwrap();
    feed(n, &format!("@label={label} :s BATCH +k metadata x"));
    for x in lines {
        feed(n, &format!("@batch=k {x}"));
    }
    feed(n, ":s BATCH -k");
}

/// bob with encrypted DMs on, synced, published, and a DM with alice (`a1`).
fn e2e_bob(alice: &chad_e2ee::Olm) -> (Net, chad_e2ee::Device) {
    let mut n = net();
    n.caps.handle(&line("CAP * ACK :draft/metadata-2"));
    n.e2e_init([7; 32], "b1");
    n.command(NetCommand::OpenDm {
        nick: "alice".into(),
    });
    n.out.clear(); // as `conn` does offline
    let sent = attach(&mut n, &[]);
    answer(&mut n, &sent, "METADATA bob LIST", &[]);
    let set = out(&mut n);
    let record = line(
        set.iter()
            .find(|l| l.contains("SET chad/e2e/d/b1"))
            .unwrap(),
    );
    assert!(
        set.iter()
            .any(|l| l == "METADATA * SET chad/e2e/devices b1"),
        "{set:?}"
    );
    let bob = chad_e2ee::Device::parse("bob", "b1", record.param(3).unwrap()).unwrap();
    let rec = alice.record("alice", "a1");
    answer(
        &mut n,
        &sent,
        "METADATA alice LIST",
        &[format!(":s 761 bob alice chad/e2e/d/a1 * :{rec}")],
    );
    (n, bob)
}

fn e2e_line(msgid: &str, ct: &str, to: &str) -> String {
    format!(
        "@msgid={msgid};time={};+chad/e2e={ct};+chad/e2e-from=a1;+chad/e2e-to={to} \
         :alice!u@h PRIVMSG bob :locked",
        ts(1000)
    )
}

#[test]
fn e2e_decrypts_once_ignores_other_copies_and_flags_tampering() {
    let mut alice = chad_e2ee::Olm::new();
    let (mut n, bob) = e2e_bob(&alice);
    assert_eq!(
        buffer(&events(&mut n), "alice").unwrap().encryption,
        Encryption::On
    );

    let ct = alice.encrypt(&bob, "hi bob").unwrap();
    feed(&mut n, &e2e_line("m1", &ct, "b1"));
    let got = messages(&events(&mut n));
    assert_eq!(
        (got[0].text.as_str(), got[0].crypt),
        ("hi bob", Crypt::Encrypted)
    );
    // History replays it: the stored copy, not a second (failing) decrypt.
    feed(&mut n, &e2e_line("m1", &ct, "b1"));
    assert!(messages(&events(&mut n)).is_empty());
    assert_eq!(n.store.get("m1").unwrap().1.text, "hi bob");
    // Someone else's copy isn't shown at all.
    feed(&mut n, &e2e_line("m2", &ct, "zz"));
    assert!(messages(&events(&mut n)).is_empty());

    let mut bad = alice.encrypt(&bob, "evil").unwrap().into_bytes();
    let i = bad.len() - 4;
    bad[i] = if bad[i] == b'A' { b'B' } else { b'A' };
    feed(
        &mut n,
        &e2e_line("m3", std::str::from_utf8(&bad).unwrap(), "b1"),
    );
    let got = messages(&events(&mut n));
    assert_eq!((got[0].text.as_str(), got[0].crypt), ("", Crypt::Failed));

    feed(&mut n, ":carol!u@h PRIVMSG bob :plain hi");
    assert_eq!(messages(&events(&mut n))[0].crypt, Crypt::Plain);
}

#[test]
fn e2e_sends_ciphertext_and_shows_the_echo_as_plaintext() {
    let mut alice = chad_e2ee::Olm::new();
    let (mut n, bob) = e2e_bob(&alice);
    n.command(NetCommand::Send {
        target: "alice".into(),
        text: "secret".into(),
        reply_to: None,
    });
    let sent = out(&mut n);
    assert_eq!(sent.len(), 1, "one device: {sent:?}");
    assert!(sent[0].ends_with("PRIVMSG alice :🔒 Encrypted message, open IrChad to read"));
    let m = line(&sent[0]);
    assert_eq!(m.tag("+chad/e2e-to"), Some("a1"));
    let ct = m.tag("+chad/e2e").unwrap();
    assert_eq!(alice.decrypt(bob.curve, ct).as_deref(), Some("secret"));

    // The echo carries alice's ciphertext; bob shows what he wrote.
    let label = m.tag("label").unwrap();
    let echo = format!(
        "@label={label};msgid=s1;time={};+chad/e2e={};+chad/e2e-from=b1;+chad/e2e-to=a1 \
         :bob!u@h PRIVMSG alice :locked",
        ts(2000),
        m.tag("+chad/e2e").unwrap()
    );
    feed(&mut n, &echo);
    let got = messages(&events(&mut n));
    let last = got.last().unwrap();
    assert_eq!(
        (last.text.as_str(), last.crypt, last.msgid.as_deref()),
        ("secret", Crypt::Encrypted, Some("s1"))
    );
}

#[test]
fn e2e_key_change_warns_until_accepted() {
    let alice = chad_e2ee::Olm::new();
    let (mut n, _) = e2e_bob(&alice);
    let reinstalled = chad_e2ee::Olm::new();
    n.command(NetCommand::Devices {
        nick: "alice".into(),
    });
    let sent = out(&mut n);
    let rec = reinstalled.record("alice", "a1");
    answer(
        &mut n,
        &sent,
        "METADATA alice LIST",
        &[format!(":s 761 bob alice chad/e2e/d/a1 * :{rec}")],
    );
    assert_eq!(
        buffer(&events(&mut n), "alice").unwrap().encryption,
        Encryption::Changed
    );
    n.command(NetCommand::AcceptKeys {
        nick: "alice".into(),
    });
    assert_eq!(
        buffer(&events(&mut n), "alice").unwrap().encryption,
        Encryption::On
    );
    // A record signed for someone else is ignored: alice has no devices then.
    n.command(NetCommand::Devices {
        nick: "alice".into(),
    });
    let sent = out(&mut n);
    let rec = reinstalled.record("mallory", "a1");
    answer(
        &mut n,
        &sent,
        "METADATA alice LIST",
        &[format!(":s 761 bob alice chad/e2e/d/a1 * :{rec}")],
    );
    assert_eq!(
        buffer(&events(&mut n), "alice").unwrap().encryption,
        Encryption::Off
    );
}

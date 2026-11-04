use super::*;
use crate::Encryption;
use crate::store::msg;

mod chat;
mod e2e;
mod voice;

fn net() -> Net {
    let info = NetworkInfo {
        network: "https://x".into(),
        name: "X".into(),
        icon: None,
        nick: "bob".into(),
        state: ConnState::Connecting,
        guest: false,
        uploads: None,
        gifs: None,
        oper: false,
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
fn after_a_guest_signs_up_their_channels_are_rejoined_once() {
    let mut n = net();
    n.store
        .put_buffer("#general", "#general", BufferKind::Channel);
    n.store.set_meta("autojoined", "1");
    n.store.set_meta("rejoin", "1");
    n.load_cache();
    // Always-on, but the new account is in nothing yet.
    let sent = attach(&mut n, &[]);
    assert!(sent.contains(&"JOIN #general".to_owned()), "{sent:?}");
    assert!(buffer(&events(&mut n), "#general").is_some());
    assert!(n.store.meta("rejoin").is_none());

    // Next time the server's list is the truth again.
    n.detach();
    attach(&mut n, &[]);
    assert!(
        events(&mut n)
            .iter()
            .any(|e| matches!(e, Event::BufferRemoved { target, .. } if target == "#general"))
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

#[test]
fn web_push_registers_once_per_connection() {
    let mut n = net();
    n.caps.handle(&line("CAP * NEW :draft/webpush"));
    n.caps.handle(&line("CAP * ACK :draft/webpush"));
    let vapid =
        "BCx1-gdmg_SOyTmh2jWxBSCEXyQqTBQqkm5fhpoG4nnlBVLf2dg6Uhb7xTZ3p3nM4EqfKwBe1_VDZb-8Wg2cYTE";
    n.isupport
        .handle(&line(&format!(":s 005 bob VAPID={vapid} :are supported")));
    let sub = PushSubscription {
        endpoint: "https://ntfy.sh/upAbc?up=1".into(),
        p256dh: "BNcR-key_".into(),
        auth: "c2VjcmV0".into(),
    };
    // Offline: kept for later, nothing sent.
    n.command(NetCommand::Push {
        subscription: Some(sub.clone()),
    });
    assert!(n.out.is_empty());

    n.attach("bob");
    assert!(
        events(&mut n)
            .iter()
            .any(|e| matches!(e, Event::PushAvailable { vapid: v, .. } if v == vapid))
    );
    out(&mut n);
    let register = "WEBPUSH REGISTER https://ntfy.sh/upAbc?up=1 p256dh=BNcR-key_;auth=c2VjcmV0";
    for _ in 0..2 {
        n.command(NetCommand::Push {
            subscription: Some(sub.clone()),
        });
    }
    assert_eq!(out(&mut n), [register]);

    // A new connection renews it.
    n.detach();
    n.attach("bob");
    out(&mut n);
    n.command(NetCommand::Push {
        subscription: Some(sub.clone()),
    });
    assert_eq!(out(&mut n), [register]);

    // ergo's test push failed (ntfy's 507 race): retried quietly, then reported.
    let fail = ":s FAIL WEBPUSH INVALID_PARAMS REGISTER https://ntfy.sh/upAbc?up=1 :Test push message failed to send";
    events(&mut n);
    for _ in 0..4 {
        feed(&mut n, fail);
        assert!(n.push_retry_at().is_some() && events(&mut n).is_empty());
        n.retry_push();
        assert_eq!(out(&mut n), [register]);
    }
    feed(&mut n, fail);
    assert!(n.push_retry_at().is_none());
    assert!(
        events(&mut n)
            .iter()
            .any(|e| matches!(e, Event::Error { .. }))
    );

    // D14: a param that would break the line is refused, and the old one stops.
    n.command(NetCommand::Push {
        subscription: Some(PushSubscription {
            endpoint: "https://a b".into(),
            ..sub.clone()
        }),
    });
    assert_eq!(
        out(&mut n),
        ["WEBPUSH UNREGISTER https://ntfy.sh/upAbc?up=1"]
    );
    n.command(NetCommand::Push { subscription: None });
    assert!(out(&mut n).is_empty());
}

#[test]
fn the_home_channels_list_joins_and_parts_official_channels() {
    let mut n = net();
    n.caps.handle(&line("CAP * NEW :draft/metadata-2"));
    n.caps.handle(&line("CAP * ACK :draft/metadata-2"));
    n.home = Some("#lobby".into());
    // First login: the home channel instead of everything in LIST.
    assert!(attach(&mut n, &[]).contains(&"JOIN #lobby".to_owned()));
    feed(&mut n, ":bob!u@h JOIN #lobby bob :bob");
    assert!(out(&mut n).contains(&"METADATA #lobby SYNC".to_owned()));

    let list = |n: &mut Net, target: &str, v: &str| {
        feed(n, &format!(":s 761 * {target} chad/channels * :{v}"));
        out(n)
    };
    assert_eq!(list(&mut n, "#lobby", "#a #b #lobby #A"), ["JOIN #a,#b"]);
    for c in ["#a", "#b"] {
        feed(&mut n, &format!(":bob!u@h JOIN {c} bob :bob"));
    }
    out(&mut n);
    // Left by hand: stays left when the list changes.
    feed(&mut n, ":bob!u@h PART #b");
    assert_eq!(list(&mut n, "#lobby", "#a #b #c"), ["JOIN #c"]);
    feed(&mut n, ":bob!u@h JOIN #c bob :bob");
    out(&mut n);
    assert_eq!(list(&mut n, "#lobby", "#c"), ["PART #a"]);
    // Unset, another channel's list, or junk: nothing.
    feed(&mut n, ":s 766 * #lobby chad/channels :Key deleted");
    assert!(out(&mut n).is_empty());
    assert!(list(&mut n, "#other", "#evil").is_empty());
    assert_eq!(
        list(&mut n, "#lobby", "#c 0 #x,0 :y nick"),
        Vec::<String>::new()
    );

    // Reconnect: fetch whatever changed meanwhile.
    n.detach();
    let sent = attach(&mut n, &["#lobby", "#c"]);
    assert!(
        sent.contains(&"METADATA #lobby GET chad/channels".to_owned()),
        "{sent:?}"
    );
}

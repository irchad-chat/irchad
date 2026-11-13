//! Read receipts in DMs (D68).

use super::*;
use crate::Seen;

/// Attached as bob in `#c`, with metadata, a DM with alice holding `m1` (ours,
/// at 1000), `a1` (theirs, 1500) and `m2` (ours, 2000).
fn dm() -> Net {
    let mut n = net();
    n.caps.handle(&line("CAP * NEW :draft/metadata-2"));
    n.caps.handle(&line("CAP * ACK :draft/metadata-2"));
    attach(&mut n, &["#c"]);
    for (id, from, to, t) in [
        ("m1", "bob", "alice", 1000),
        ("a1", "alice", "bob", 1500),
        ("m2", "bob", "alice", 2000),
    ] {
        feed(
            &mut n,
            &format!("@msgid={id};time={} :{from}!u@h PRIVMSG {to} :hi", ts(t)),
        );
    }
    out(&mut n);
    events(&mut n);
    n
}

fn seen(n: &mut Net) -> Option<Seen> {
    n.buffers["alice"].seen
}

fn receipts_on(n: &mut Net) {
    feed(n, ":s 761 bob bob chad/receipts * :1");
}

#[test]
fn sent_only_while_both_have_them_on() {
    let mut n = dm();
    let read = |n: &mut Net, t: &str| {
        n.read_sent.clear(); // not testing READ_EVERY here
        n.command(NetCommand::MarkRead { target: t.into() });
        out(n)
    };
    let markread = |t| format!("MARKREAD alice timestamp={}", ts(t));
    assert_eq!(read(&mut n, "alice"), [markread(2000)], "off by default");

    // Another of our devices turns them on: we ask about everyone we DM.
    receipts_on(&mut n);
    assert!(n.info.receipts);
    assert_eq!(n.store.meta("receipts").as_deref(), Some("1"));
    assert!(
        events(&mut n)
            .iter()
            .any(|e| matches!(e, Event::Network(i) if i.receipts))
    );
    assert_eq!(out(&mut n), ["METADATA alice GET chad/receipts"]);
    feed(
        &mut n,
        &format!("@msgid=a2;time={} :alice!u@h PRIVMSG bob :x", ts(3000)),
    );
    assert_eq!(read(&mut n, "alice"), [markread(3000)], "theirs unknown");

    feed(&mut n, ":s 761 bob alice chad/receipts * :1");
    feed(
        &mut n,
        &format!("@msgid=a3;time={} :alice!u@h PRIVMSG bob :x", ts(4000)),
    );
    assert_eq!(
        read(&mut n, "alice"),
        [markread(4000), "@+chad/read=a3 TAGMSG alice".into()]
    );
    // They follow a nick change.
    feed(&mut n, ":alice!u@h NICK alicia");
    assert!(n.receipts_of.contains("alicia"));
    feed(&mut n, ":alicia!u@h NICK alice");

    // Never in channels.
    feed(
        &mut n,
        &format!("@msgid=c1;time={} :alice!u@h PRIVMSG #c :x", ts(5000)),
    );
    assert_eq!(
        read(&mut n, "#c"),
        [format!("MARKREAD #c timestamp={}", ts(5000))]
    );

    // They turn theirs off.
    feed(&mut n, ":s 766 bob alice chad/receipts :key not set");
    feed(
        &mut n,
        &format!("@msgid=a4;time={} :alice!u@h PRIVMSG bob :x", ts(6000)),
    );
    assert_eq!(read(&mut n, "alice"), [markread(6000)]);

    // We turn ours off: the metadata goes, and nothing more is asked.
    feed(&mut n, ":s 761 bob alice chad/receipts * :1");
    n.command(NetCommand::SetReceipts { on: false });
    assert_eq!(out(&mut n), ["METADATA * SET chad/receipts"]);
    assert!(!n.info.receipts);
    assert_eq!(n.store.meta("receipts"), None);
    feed(
        &mut n,
        &format!("@msgid=a5;time={} :alice!u@h PRIVMSG bob :x", ts(7000)),
    );
    assert_eq!(read(&mut n, "alice"), [markread(7000)]);

    n.command(NetCommand::SetReceipts { on: true });
    assert_eq!(
        out(&mut n),
        [
            "METADATA * SET chad/receipts 1",
            "METADATA alice GET chad/receipts"
        ]
    );
}

#[test]
fn guests_cant_turn_them_on() {
    let mut n = dm();
    n.guest = true;
    n.command(NetCommand::SetReceipts { on: true });
    assert!(out(&mut n).is_empty());
    assert!(!n.info.receipts);
}

#[test]
fn only_their_receipts_for_this_dm_move_forward() {
    let mut n = dm();
    let receipt = |n: &mut Net, s: &str| {
        feed(n, s);
        seen(n)
    };
    let at = ts(5000);
    // Shown only while ours are on; kept regardless.
    let r = format!("@time={at};+chad/read=m1 :alice!u@h TAGMSG bob");
    assert_eq!(receipt(&mut n, &r), None, "ours are off");
    receipts_on(&mut n);
    events(&mut n);
    let first = Some(Seen {
        upto: 1000,
        at: 5000,
    });
    assert_eq!(receipt(&mut n, &r), first);
    let b = buffer(&events(&mut n), "alice").unwrap();
    assert_eq!(b.seen, first);

    for bad in [
        // Backwards, or not moving.
        format!("@time={at};+chad/read=m1 :alice!u@h TAGMSG bob"),
        // Not a message we hold, or too long to be one.
        "@+chad/read=nope :alice!u@h TAGMSG bob".into(),
        format!("@+chad/read={} :alice!u@h TAGMSG bob", "m".repeat(200)),
        "@+chad/read= :alice!u@h TAGMSG bob".into(),
        // Someone else saying they read alice's DM.
        "@+chad/read=m2 :carol!u@h TAGMSG bob".into(),
        "@+chad/read=m2 :carol!u@h TAGMSG alice".into(),
        // In a channel, or from ourselves (another device's receipt to alice).
        "@+chad/read=m2 :alice!u@h TAGMSG #c".into(),
        "@+chad/read=m2 :bob!u@h TAGMSG alice".into(),
        // Not even a DM peer's own nick.
        "@+chad/read=m2 :s TAGMSG bob".into(),
    ] {
        assert_eq!(receipt(&mut n, &bad), first, "{bad}");
    }
    assert!(!n.buffers.contains_key("carol"));

    // A time from the future is now; before the message is the message's.
    let r = format!(
        "@time={};+chad/read=m2 :alice!u@h TAGMSG bob",
        ts(i64::MAX / 4)
    );
    let s = receipt(&mut n, &r).unwrap();
    assert_eq!(s.upto, 2000);
    assert!(s.at <= time::now_ms());
    assert_eq!(
        n.store
            .buffers()
            .into_iter()
            .find(|b| b.key == "alice")
            .unwrap()
            .seen,
        Some(s)
    );

    // They turn theirs off: their old receipt goes too.
    feed(&mut n, ":s 766 bob alice chad/receipts :key not set");
    assert_eq!(seen(&mut n), None);
    assert_eq!(buffer(&events(&mut n), "alice").unwrap().seen, None);
    let stored = n.store.buffers().into_iter().find(|b| b.key == "alice");
    assert_eq!(stored.unwrap().seen, None);
}

#[test]
fn hidden_while_ours_are_off() {
    let mut n = dm();
    receipts_on(&mut n);
    feed(&mut n, "@+chad/read=m2 :alice!u@h TAGMSG bob");
    events(&mut n);
    feed(&mut n, ":s 766 bob bob chad/receipts :key not set");
    assert!(!n.info.receipts);
    assert_eq!(buffer(&events(&mut n), "alice").unwrap().seen, None);
    assert!(seen(&mut n).is_some(), "kept for when they're back on");
}

#[test]
fn from_history_after_the_page_is_stored() {
    let mut n = dm();
    receipts_on(&mut n);
    let page = [
        format!("@msgid=m9;time={} :bob!u@h PRIVMSG alice :later", ts(9000)),
        format!("@time={};+chad/read=m9 :alice!u@h TAGMSG bob", ts(9500)),
        format!("@time={};+chad/read=m9 :carol!u@h TAGMSG bob", ts(9600)),
    ];
    let items = page.iter().map(|l| Item::Message(line(l))).collect();
    n.store_history("alice", items);
    assert_eq!(
        seen(&mut n),
        Some(Seen {
            upto: 9000,
            at: 9500
        })
    );
}

#[test]
fn blocked_peers_send_none() {
    let mut n = dm();
    receipts_on(&mut n);
    n.command(NetCommand::Block {
        nick: "alice".into(),
        account: Some("alice".into()),
    });
    feed(&mut n, "@account=alice;+chad/read=m2 :alice!u@h TAGMSG bob");
    assert_eq!(seen(&mut n), None);
}

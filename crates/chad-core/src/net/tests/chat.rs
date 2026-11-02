//! Messages, history, presence, reactions and edits.

use super::*;

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
    assert_eq!(out(&mut n), ["NOTICE alice :\x01VERSION IrChad\x01"]);
    // Not again so soon, nor to a channel, nor anything else.
    feed(&mut n, ":carol!u@h PRIVMSG bob :\x01VERSION\x01");
    assert!(out(&mut n).is_empty());
    n.ctcp_at = None;
    feed(&mut n, ":alice!u@h PRIVMSG #c :\x01VERSION\x01");
    feed(&mut n, ":alice!u@h PRIVMSG bob :\x01PING 1\x01");
    feed(&mut n, ":alice!u@h NOTICE bob :\x01VERSION\x01");
    feed(&mut n, ":s PRIVMSG bob :\x01VERSION\x01");
    assert!(out(&mut n).is_empty(), "only answers VERSION");
    assert!(messages(&events(&mut n)).is_empty());

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
fn edits_apply_only_from_the_author_and_never_go_backwards() {
    let mut n = net();
    attach(&mut n, &["#c"]);
    feed(
        &mut n,
        &format!(
            "@msgid=m1;account=alice;time={} :alice!u@h PRIVMSG #c :helo",
            ts(1)
        ),
    );
    events(&mut n);

    // Someone else, or a guest with alice's nick, can't edit it.
    feed(
        &mut n,
        &format!(
            "@msgid=x1;account=eve;time={};+chad/edit=m1 :alice!u@h PRIVMSG #c :* pwned",
            ts(2)
        ),
    );
    feed(
        &mut n,
        &format!(
            "@msgid=x2;time={};+chad/edit=m1 :alice!u@h PRIVMSG #c :* pwned",
            ts(2)
        ),
    );
    assert!(messages(&events(&mut n)).is_empty());

    feed(
        &mut n,
        &format!(
            "@msgid=e2;account=alice;time={};+chad/edit=m1 :alice!u@h PRIVMSG #c :* hello!",
            ts(3)
        ),
    );
    let m = messages(&events(&mut n));
    assert_eq!(m.len(), 1, "the edit itself isn't a message");
    assert_eq!(
        (m[0].msgid.as_deref(), m[0].text.as_str(), m[0].edited),
        (Some("m1"), "hello!", true)
    );
    // An older edit replayed later doesn't undo the newer one.
    feed(
        &mut n,
        &format!(
            "@msgid=e1;account=alice;time={};+chad/edit=m1 :alice!u@h PRIVMSG #c :* hello",
            ts(2)
        ),
    );
    assert!(messages(&events(&mut n)).is_empty());
    assert_eq!(n.store.get("m1").unwrap().1.text, "hello!");

    // Our own edit goes out tagged with the fallback prefix and shows no pending message.
    feed(
        &mut n,
        &format!("@msgid=b1;time={} :bob!u@h PRIVMSG #c :typo", ts(4)),
    );
    events(&mut n);
    n.command(NetCommand::Edit {
        target: "#c".into(),
        msgid: "b1".into(),
        text: "fixed".into(),
    });
    assert!(messages(&events(&mut n)).is_empty());
    let sent = out(&mut n);
    assert!(sent[0].starts_with("@+chad/edit=b1;label="), "{sent:?}");
    assert!(sent[0].ends_with("PRIVMSG #c :* fixed"), "{sent:?}");
    let label = sent[0]
        .split([';', ' '])
        .nth(1)
        .unwrap()
        .strip_prefix("label=")
        .unwrap()
        .to_owned();
    feed(
        &mut n,
        &format!(
            "@label={label};msgid=b2;time={};+chad/edit=b1 :bob!u@h PRIVMSG #c :* fixed",
            ts(5)
        ),
    );
    let m = messages(&events(&mut n));
    assert_eq!(
        (m[0].msgid.as_deref(), m[0].text.as_str()),
        (Some("b1"), "fixed")
    );
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

//! Classic networks (D72): no labels, no persistence, no chadd.

use super::*;

/// A solanum-like network: echo-message but no labeled-response.
fn classic() -> Net {
    let mut info = net().info;
    info.network = "ircs://irc.x:6697".into();
    let mut n = Net::new(Store::memory().unwrap(), info);
    n.caps
        .handle(&line("CAP * LS :echo-message server-time multi-prefix"));
    n.caps
        .handle(&line("CAP * ACK :echo-message server-time multi-prefix"));
    n.isupport.handle(&line(
        ":s 005 bob CASEMAPPING=rfc1459 CHANTYPES=# PREFIX=(ov)@+ :are supported",
    ));
    n
}

#[test]
fn rejoins_cached_channels_without_listing() {
    let mut n = classic();
    n.store.put_buffer("#old", "#old", BufferKind::Channel);
    n.load_cache();
    n.attach("bob");
    let sent = out(&mut n);
    assert!(sent.contains(&"JOIN #old".to_owned()), "{sent:?}");
    assert!(
        !sent
            .iter()
            .any(|l| l.contains("LIST") || l.contains("label")),
        "{sent:?}"
    );
}

#[test]
fn names_echo_list_and_server_buffer() {
    let mut n = classic();
    n.attach("bob");
    feed(&mut n, ":bob!u@h JOIN #c");
    feed(&mut n, ":s 353 bob = #c :@bob +alice carol!c@h");
    feed(&mut n, ":s 366 bob #c :End of /NAMES list.");
    let ev = events(&mut n);
    let members = ev
        .iter()
        .find_map(|e| match e {
            Event::Members { members, .. } => Some(members.clone()),
            _ => None,
        })
        .expect("members");
    let mut got: Vec<(String, String)> = members
        .iter()
        .map(|m| (m.nick.clone(), m.prefix.clone()))
        .collect();
    got.sort();
    assert_eq!(
        got,
        [
            ("alice".into(), "+".into()),
            ("bob".into(), "@".into()),
            ("carol".into(), String::new())
        ]
    );

    // A send completes on its unlabeled echo, once.
    out(&mut n);
    n.command(NetCommand::Send {
        target: "#c".into(),
        text: "hi".into(),
        reply_to: None,
        mentions: Vec::new(),
    });
    assert_eq!(out(&mut n), ["PRIVMSG #c hi"]);
    events(&mut n);
    feed(&mut n, ":bob!u@h PRIVMSG #c :hi");
    let msgs = messages(&events(&mut n));
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].local_id.is_some() && msgs[0].status == SendStatus::Sent);
    assert_eq!(n.store.recent("#c", 10).len(), 1, "stored without a msgid");

    n.command(NetCommand::ListChannels);
    assert_eq!(out(&mut n), ["LIST"]);
    for l in [
        ":s 321 bob Channel :Users Name",
        ":s 322 bob #small 2 :Small",
        ":s 322 bob #big 300 :Big",
        ":s 322 bob bad,name 9 :x",
        ":s 323 bob :End of /LIST",
    ] {
        feed(&mut n, l);
    }
    let list = events(&mut n)
        .into_iter()
        .find_map(|e| match e {
            Event::ChannelList { channels, .. } => Some(channels),
            _ => None,
        })
        .expect("list");
    let names: Vec<_> = list.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["#big", "#small"]);

    feed(&mut n, ":irc.x NOTICE bob :*** server notice");
    feed(&mut n, ":irc.x 372 bob :- message of the day");
    let ev = events(&mut n);
    assert_eq!(buffer(&ev, SERVER_BUFFER).unwrap().kind, BufferKind::Server);
    let texts: Vec<_> = messages(&ev).into_iter().map(|m| m.text).collect();
    assert_eq!(texts, ["*** server notice", "- message of the day"]);

    n.command(NetCommand::Raw {
        line: "/quote WHOIS alice".into(),
    });
    assert_eq!(out(&mut n), ["WHOIS alice"]);
}

#[test]
fn irchad_servers_get_no_server_buffer() {
    let mut n = net();
    attach(&mut n, &[]);
    feed(&mut n, ":s NOTICE bob :*** server notice");
    assert!(buffer(&events(&mut n), SERVER_BUFFER).is_none());
}

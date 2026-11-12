use super::*;

fn texts(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Messages { messages, .. } => Some(messages.iter().map(|m| m.text.clone())),
            _ => None,
        })
        .flatten()
        .collect()
}

fn blocks(events: &[Event]) -> Option<Vec<Block>> {
    events.iter().rev().find_map(|e| match e {
        Event::Blocks { blocks, .. } => Some(blocks.clone()),
        _ => None,
    })
}

#[test]
fn blocks_by_account_and_by_host_and_unblocks() {
    let mut n = net();
    attach(&mut n, &["#c"]);
    events(&mut n);

    n.command(NetCommand::Block {
        nick: "Al".into(),
        account: Some("Alice".into()),
    });
    let b = blocks(&events(&mut n)).unwrap();
    assert_eq!(b[0].mask, "$a:alice");
    feed(&mut n, "@account=alice :al2!u@h1 PRIVMSG #c :hi");
    feed(&mut n, "@account=alice :al2!u@h1 PRIVMSG bob :psst");
    feed(&mut n, "@account=alice;+typing=active :al2!u@h1 TAGMSG #c");
    feed(&mut n, "@account=carol :carol!u@h1 PRIVMSG #c :shown");
    let ev = events(&mut n);
    assert_eq!(texts(&ev), ["shown"]);
    assert!(!ev.iter().any(|e| matches!(e, Event::Typing { .. })));
    assert!(buffer(&ev, "al2").is_none(), "no DM buffer for them");

    // A guest: WHOX finds their host.
    n.command(NetCommand::Block {
        nick: "gus".into(),
        account: None,
    });
    let who = out(&mut n);
    let label = who[0]
        .strip_prefix("@label=")
        .unwrap()
        .split(' ')
        .next()
        .unwrap();
    assert!(who[0].ends_with("WHO gus %tnha,2"));
    feed(
        &mut n,
        &format!("@label={label} :s 354 bob 2 Cloak.Host gus 0"),
    );
    assert_eq!(blocks(&events(&mut n)).unwrap()[1].mask, "*!*@cloak.host");
    feed(&mut n, ":gus2!u@cloak.host PRIVMSG #c :nope");
    assert!(texts(&events(&mut n)).is_empty());

    n.command(NetCommand::Unblock {
        mask: "$a:alice".into(),
    });
    assert_eq!(blocks(&events(&mut n)).unwrap().len(), 1);
    assert_eq!(n.store.blocks().len(), 1);
    feed(&mut n, "@account=alice :al2!u@h1 PRIVMSG #c :back");
    assert_eq!(texts(&events(&mut n)), ["back"]);
}

#[test]
fn hides_cached_messages_and_reactions() {
    let mut n = net();
    let mut m = msg("1", 1, "al", "old");
    m.account = Some("Alice".into());
    n.store.put_buffer("#c", "#c", BufferKind::Channel);
    n.store.insert("#c", &m);
    n.store.insert("#c", &msg("2", 2, "carol", "kept"));
    n.store.react("2", "al", "👍", true);
    n.store.put_block(&Block {
        mask: "$a:alice".into(),
        nick: "al".into(),
    });
    n = Net::new(
        std::mem::replace(&mut n.store, Store::memory().unwrap()),
        n.info.clone(),
    );
    n.load_cache();
    let ev = events(&mut n);
    assert!(matches!(ev[0], Event::Blocks { .. }));
    let msgs: Vec<&ChatMessage> = ev
        .iter()
        .filter_map(|e| match e {
            Event::Messages { messages, .. } => Some(messages),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].text, "kept");
    assert!(msgs[0].reactions.is_empty());
}

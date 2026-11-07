//! Encrypted DMs.

use super::*;

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
        mentions: Vec::new(),
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

#[test]
fn e2e_removes_a_stale_device_of_ours() {
    let alice = chad_e2ee::Olm::new();
    let (mut n, _) = e2e_bob(&alice);
    n.command(NetCommand::Devices { nick: "bob".into() });
    let sent = out(&mut n);
    let stale = chad_e2ee::Olm::new().record("bob", "b2");
    answer(
        &mut n,
        &sent,
        "METADATA bob LIST",
        &[format!(":s 761 bob bob chad/e2e/d/b2 * :{stale}")],
    );
    out(&mut n);
    // Not ours to remove: nothing happens.
    n.command(NetCommand::RemoveDevice { id: "a1".into() });
    assert!(out(&mut n).is_empty());
    n.command(NetCommand::RemoveDevice { id: "b2".into() });
    let sent = out(&mut n);
    assert!(
        sent.iter().any(|l| l == "METADATA * SET chad/e2e/d/b2"),
        "{sent:?}"
    );
    assert!(
        sent.iter().any(|l| l.contains("METADATA bob LIST")),
        "{sent:?}"
    );
}

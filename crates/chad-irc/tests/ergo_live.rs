//! Against the dev stack (`deploy/README.md`, seeded). Run with
//! `cargo test -p chad-irc --test ergo_live -- --ignored`.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

use chad_irc::Message;
use chad_irc::cap::Negotiator;
use chad_irc::register::{self, Reply};
use chad_irc::sasl::{self, Mechanism, Progress, Sasl};

struct Conn {
    r: BufReader<TcpStream>,
    w: TcpStream,
    caps: Negotiator,
}

impl Conn {
    /// Connects and negotiates caps, stopping before `CAP END`.
    fn open(nick: &str) -> Self {
        let w = TcpStream::connect("127.0.0.1:6667").expect("dev stack running?");
        let mut c = Conn {
            r: BufReader::new(w.try_clone().unwrap()),
            w,
            caps: Negotiator::new(),
        };
        c.send(&Negotiator::start());
        c.send(&Message::new("NICK", [nick]));
        c.send(&Message::new("USER", [nick, "0", "*", nick]));
        while !c.caps.ready() {
            let m = c.recv();
            for out in c.caps.handle(&m) {
                c.send(&out);
            }
        }
        c
    }

    fn send(&mut self, m: &Message) {
        writeln!(self.w, "{}\r", m.to_line().unwrap()).unwrap();
    }

    fn recv(&mut self) -> Message {
        let mut line = String::new();
        assert!(self.r.read_line(&mut line).unwrap() > 0, "server closed");
        Message::parse(&line).unwrap()
    }

    fn login(&mut self, user: &str, password: &str, mechs: &str) -> Result<Mechanism, sasl::Error> {
        let mut s = Sasl::new(user, password, mechs)?;
        self.send(&s.start());
        loop {
            let m = self.recv();
            match s.handle(&m) {
                Progress::Continue(out) => out.iter().for_each(|o| self.send(o)),
                Progress::LoggedIn => return Ok(s.mechanism()),
                Progress::Failed(e) => return Err(e),
            }
        }
    }

    fn register(&mut self, m: &Message) -> Reply {
        self.send(m);
        loop {
            if let Some(r) = Reply::parse(&self.recv()) {
                return r;
            }
        }
    }
}

#[test]
#[ignore]
fn sasl_against_ergo() {
    let mut c = Conn::open("livetest1");
    let mechs = c.caps.value("sasl").unwrap().to_owned();
    assert!(c.caps.enabled("sasl") && c.caps.enabled("cap-notify"));
    assert_eq!(
        c.login("alice@livetest", "alicepass", &mechs),
        Ok(Mechanism::ScramSha256)
    );

    let mut c = Conn::open("livetest2");
    assert_eq!(
        c.login("bob@livetest", "bobpass", "PLAIN"),
        Ok(Mechanism::Plain)
    );

    let mut c = Conn::open("livetest3");
    assert_eq!(
        c.login("alice", "wrong", &mechs),
        Err(sasl::Error::Rejected)
    );
}

#[test]
#[ignore]
fn register_against_ergo() {
    let secs = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs();
    let account = format!("lt{secs}");
    let mut c = Conn::open(&account);
    assert_eq!(
        c.register(&register::register("*", None, "livetestpass")),
        Reply::Registered {
            account: account.clone()
        }
    );

    // ergo only registers your current nick, so the join flow sets NICK first.
    let mut c = Conn::open("livetest4");
    let r = c.register(&register::register(&account, None, "livetestpass"));
    assert!(
        matches!(&r, Reply::Failed { code, .. } if code == "ACCOUNT_NAME_MUST_BE_NICK"),
        "{r:?}"
    );
}

/// Over Caddy, so trust its CA: see `docs/notes/livekit-sdk.md` for `SSL_CERT_FILE`.
#[tokio::test]
#[ignore]
async fn websocket_isupport_and_labels() {
    use chad_irc::batch::Assembler;
    use chad_irc::isupport::{Casemapping, Isupport};
    use chad_irc::label::Labels;
    use chad_irc::ws::Connection;

    let url = std::env::var("IRCHAD_WS").unwrap_or("wss://localhost:8443/irc".into());
    let mut ws = Connection::connect(&url)
        .await
        .expect("dev stack running, CA trusted?");
    let mut caps = Negotiator::new();
    let mut isupport = Isupport::new();
    let mut batches = Assembler::new();
    let mut labels = Labels::new();
    ws.send(&Negotiator::start()).await.unwrap();
    ws.send(&Message::new("NICK", ["livetest5"])).await.unwrap();
    ws.send(&Message::new("USER", ["livetest5", "0", "*", "lt"]))
        .await
        .unwrap();

    loop {
        let m = ws.recv().await.unwrap().expect("server closed");
        for out in caps.handle(&m) {
            ws.send(&out).await.unwrap();
        }
        if caps.ready() {
            ws.send(&caps.end()).await.unwrap();
        }
        isupport.handle(&m);
        if m.command == "376" || m.command == "422" {
            break;
        }
    }
    assert!(caps.enabled("labeled-response"));
    assert_eq!(isupport.casemapping(), Casemapping::Ascii);
    assert_eq!(isupport.chathistory_limit(), Some(1000));

    ws.send(&labels.tag(Message::new("PING", ["probe"]), "ping"))
        .await
        .unwrap();
    loop {
        let m = ws.recv().await.unwrap().expect("server closed");
        if let Some(item) = batches.feed(m).unwrap()
            && let Ok((token, _)) = labels.take(item)
        {
            assert_eq!(token, "ping");
            break;
        }
    }
    ws.close().await.unwrap();
}

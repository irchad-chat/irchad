//! One task per network: discovery, connect, CAP/SASL/REGISTER, then pump lines
//! between the socket and [`Net`]; reconnect with backoff when it drops.

use std::path::PathBuf;
use std::sync::Arc;

use chad_irc::Message;
use chad_irc::cap::Negotiator;
use chad_irc::register::{self, Reply};
use chad_irc::sasl::{self, Progress, Sasl};
use chad_irc::ws::{self, Backoff, Connection};
use chad_proto::discovery::{Discovery, Registration};
use tokio::sync::mpsc;

use crate::manager::Sink;
use crate::net::{Net, failure};
use crate::secrets::SecretStore;
use crate::{ConnState, Event, NetCommand, discovery};

pub enum ToNet {
    Cmd(NetCommand),
    /// Forget the network: its cache file and its password.
    Remove,
}

pub struct Params {
    pub account: String,
    pub password: Option<String>,
    pub device: String,
    /// `Some(email)` while signing up.
    pub register: Option<Option<String>>,
    /// Set until the first login on a new network succeeds; a failure before
    /// then removes the network again.
    pub joining: Option<String>,
    pub db_path: PathBuf,
    pub secrets: Arc<dyn SecretStore>,
}

enum End {
    /// Dropped; retry after a backoff.
    Lost(String),
    /// Retrying won't help until the user acts.
    Fatal(String),
    /// Signed up: log in again so the device ID is in place (D11).
    Registered,
    Stop,
    Remove,
}

pub async fn run(mut net: Net, mut p: Params, mut rx: mpsc::UnboundedReceiver<ToNet>, sink: Sink) {
    let mut backoff = Backoff::new();
    loop {
        let end = match p.password.clone() {
            Some(password) => {
                session(&mut net, &mut p, &password, &mut rx, &sink, &mut backoff).await
            }
            None => End::Fatal("Sign in to this network again.".into()),
        };
        net.detach();
        match end {
            End::Registered => {
                p.register = None;
                continue;
            }
            End::Lost(error) if p.joining.is_none() => {
                net.set_state(ConnState::Disconnected { error });
                sink.send(net.events.drain(..));
                let delay = backoff.next_delay();
                tracing::info!("{}: reconnecting in {delay:?}", net.info.network);
                let sleep = tokio::time::sleep(delay);
                tokio::pin!(sleep);
                loop {
                    tokio::select! {
                        () = &mut sleep => break,
                        cmd = rx.recv() => match cmd {
                            Some(ToNet::Cmd(c)) => offline(&mut net, c, &sink),
                            Some(ToNet::Remove) => return remove(net, &p, &sink).await,
                            None => return,
                        },
                    }
                }
            }
            End::Lost(error) | End::Fatal(error) => {
                if let Some(invite) = p.joining.take() {
                    sink.send([Event::JoinFailed { invite, error }]);
                    return remove(net, &p, &sink).await;
                }
                net.set_state(ConnState::Failed { error });
                sink.send(net.events.drain(..));
                // Stay listed with the cache readable until removed or re-joined.
                loop {
                    match rx.recv().await {
                        Some(ToNet::Cmd(c)) => offline(&mut net, c, &sink),
                        Some(ToNet::Remove) => return remove(net, &p, &sink).await,
                        None => return,
                    }
                }
            }
            End::Remove => return remove(net, &p, &sink).await,
            End::Stop => return,
        }
    }
}

fn offline(net: &mut Net, cmd: NetCommand, sink: &Sink) {
    net.command(cmd);
    net.out.clear();
    sink.send(net.events.drain(..));
}

async fn remove(net: Net, p: &Params, sink: &Sink) {
    let network = net.info.network.clone();
    drop(net);
    for ext in ["", "-wal", "-shm"] {
        let mut path = p.db_path.clone().into_os_string();
        path.push(ext);
        let _ = std::fs::remove_file(path);
    }
    if let Err(e) = p.secrets.delete(&network).await {
        tracing::warn!("forgetting the password for {network}: {e}");
    }
    sink.send([Event::NetworkRemoved { network }]);
}

/// Fresh discovery if the server answers, else the cached copy.
async fn discover(net: &mut Net) -> Result<Discovery, String> {
    let origin = net.info.network.clone();
    let cached = net
        .store
        .meta("discovery")
        .and_then(|d| discovery::parse(d.as_bytes()));
    let d = match discovery::fetch(&origin).await {
        Ok(d) => {
            if let Ok(json) = serde_json::to_string(&d) {
                net.store.set_meta("discovery", &json);
            }
            d
        }
        Err(e) => cached.ok_or(e)?,
    };
    net.info.name = d.name.clone();
    net.info.icon = d.icon.clone();
    Ok(d)
}

async fn session(
    net: &mut Net,
    p: &mut Params,
    password: &str,
    rx: &mut mpsc::UnboundedReceiver<ToNet>,
    sink: &Sink,
    backoff: &mut Backoff,
) -> End {
    net.set_state(ConnState::Connecting);
    sink.send(net.events.drain(..));
    let d = match discover(net).await {
        Ok(d) => d,
        Err(e) if p.joining.is_some() => return End::Fatal(e),
        Err(e) => return End::Lost(e),
    };
    if p.register.is_some() && d.registration == Registration::Closed {
        return End::Fatal(format!("{} isn't taking sign-ups.", d.name));
    }
    let mut ws = match Connection::connect(d.irc.websocket.as_str()).await {
        Ok(ws) => ws,
        Err(e) => {
            tracing::warn!("{}: connect: {e}", net.info.network);
            return End::Lost(format!("Couldn't connect to {}.", d.name));
        }
    };

    net.caps = Negotiator::new();
    net.isupport.clear();
    let account = p.account.clone();
    for m in [
        Negotiator::start(),
        Message::new("NICK", [account.as_str()]),
        Message::new("USER", [account.as_str(), "0", "*", account.as_str()]),
    ] {
        if let Err(e) = ws.send(&m).await {
            return lost(e);
        }
    }

    let mut auth_started = false;
    let mut sasl: Option<Sasl> = None;
    let nick = loop {
        let m = tokio::select! {
            r = ws.recv() => match r {
                Ok(Some(m)) => m,
                Ok(None) => return End::Lost("The server closed the connection.".into()),
                Err(e) => return lost(e),
            },
            cmd = rx.recv() => {
                match cmd {
                    Some(ToNet::Cmd(NetCommand::Verify { code })) => {
                        let _ = ws.send(&register::verify(&account, code.trim())).await;
                    }
                    Some(ToNet::Cmd(c)) => offline(net, c, sink),
                    Some(ToNet::Remove) => return End::Remove,
                    None => return End::Stop,
                }
                continue;
            }
        };
        let mut out = net.caps.handle(&m);
        net.isupport.handle(&m);

        if net.caps.ready() && !auth_started {
            auth_started = true;
            if let Some(email) = &p.register {
                if !net.caps.enabled("draft/account-registration") {
                    return End::Fatal(format!("{} isn't taking sign-ups.", d.name));
                }
                out.push(register::register("*", email.as_deref(), password));
            } else {
                if !net.caps.enabled("sasl") {
                    return End::Fatal(format!("{} doesn't support logging in.", d.name));
                }
                let user = format!("{account}@{}", p.device);
                let mechs = net.caps.value("sasl").unwrap_or_default();
                match Sasl::new(&user, password, mechs) {
                    Ok(mut s) => {
                        out.push(s.start());
                        sasl = Some(s);
                    }
                    Err(e) => return End::Fatal(sasl_error(e)),
                }
            }
        }
        if let Some(s) = &mut sasl {
            match s.handle(&m) {
                Progress::Continue(more) => out.extend(more),
                Progress::LoggedIn => {
                    sasl = None;
                    out.push(net.caps.end());
                    logged_in(net, p, password, sink).await;
                }
                Progress::Failed(e) => return End::Fatal(sasl_error(e)),
            }
        }
        match Reply::parse(&m) {
            Some(Reply::Registered { .. } | Reply::Verified { .. }) => {
                let _ = ws.send(&Message::new("QUIT", ["signed up"])).await;
                return End::Registered;
            }
            Some(Reply::NeedsVerification { message, .. }) => {
                net.set_state(ConnState::NeedsVerification { message });
                sink.send(net.events.drain(..));
            }
            Some(Reply::Failed { command, .. }) if command == "VERIFY" => {
                net.error("That code didn't work. Check the email and try again.");
                sink.send(net.events.drain(..));
            }
            Some(Reply::Failed { message, .. }) => {
                let plain = failure(&chad_irc::batch::Item::Message(m.clone()));
                return End::Fatal(plain.unwrap_or(message));
            }
            None => {}
        }
        match m.command.as_str() {
            "001" => net.info.nick = m.param(0).unwrap_or(&account).to_owned(),
            "376" | "422" => break net.info.nick.clone(),
            "433" => return End::Fatal("That username is taken.".into()),
            "ERROR" => return End::Lost(m.params.last().cloned().unwrap_or_default()),
            _ => {}
        }
        for o in out {
            if let Err(e) = send(&mut ws, &o).await {
                return lost(e);
            }
        }
    };

    net.attach(&nick);
    backoff.reset();
    loop {
        for m in std::mem::take(&mut net.out) {
            if let Err(e) = send(&mut ws, &m).await {
                return lost(e);
            }
        }
        sink.send(net.events.drain(..));
        tokio::select! {
            r = ws.recv() => match r {
                Ok(Some(m)) => {
                    if m.command == "ERROR" {
                        return End::Lost(m.params.last().cloned().unwrap_or_default());
                    }
                    if net.handle(m).is_err() {
                        return End::Lost("The server sent more than we can hold.".into());
                    }
                }
                Ok(None) => return End::Lost("The server closed the connection.".into()),
                Err(e) => return lost(e),
            },
            cmd = rx.recv() => match cmd {
                Some(ToNet::Cmd(c)) => net.command(c),
                Some(ToNet::Remove) => {
                    let _ = ws.send(&Message::new("QUIT", ["bye"])).await;
                    return End::Remove;
                }
                None => {
                    // App exit: always-on keeps us in our channels.
                    let _ = ws.close().await;
                    return End::Stop;
                }
            },
        }
    }
}

/// First login on a new network: now it's worth keeping.
async fn logged_in(net: &mut Net, p: &mut Params, password: &str, sink: &Sink) {
    if p.joining.take().is_none() {
        return;
    }
    let origin = net.info.network.clone();
    net.store.set_meta("origin", &origin);
    net.store.set_meta("account", &p.account);
    if let Err(e) = p.secrets.save(&origin, &p.account, password).await {
        net.error(format!("{e} You'll need to sign in again next time."));
        sink.send(net.events.drain(..));
    }
}

/// A line we built from server-supplied names may not serialize; skip it
/// rather than drop the connection.
async fn send(ws: &mut Connection, m: &Message) -> Result<(), ws::Error> {
    match ws.send(m).await {
        Err(ws::Error::Message(e)) => {
            tracing::warn!("not sending {:?}: {e}", m.command);
            Ok(())
        }
        r => r,
    }
}

fn lost(e: ws::Error) -> End {
    tracing::warn!("connection lost: {e}");
    End::Lost(match e {
        ws::Error::Timeout => "The server stopped responding.".into(),
        _ => "Lost the connection to the server.".into(),
    })
}

fn sasl_error(e: sasl::Error) -> String {
    match e {
        sasl::Error::Rejected => "Wrong username or password.".into(),
        sasl::Error::NoMechanism => "This server's login method isn't supported.".into(),
        other => {
            tracing::warn!("SASL: {other}");
            "Logging in failed. Try again later.".into()
        }
    }
}

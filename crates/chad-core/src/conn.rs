//! One task per network: discovery, connect, [`Login`], then pump lines
//! between the socket and [`Net`]; reconnect with backoff when it drops.
//! The IRC is in [`login`] and [`Net`]; this is the tokio half, shared with
//! nothing (chad-web has its own).

use std::path::PathBuf;
use std::sync::Arc;

use chad_irc::Message;
use chad_irc::keepalive::Backoff;
use chad_irc::ws::{self, Connection};
use chad_proto::chadd::VoiceTokenResponse;
use chad_proto::discovery::{self as proto_discovery, Discovery, Registration};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use url::Url;

use crate::chadd::{self, Session};
use crate::login::{self, Account, Login, Step};
use crate::manager::Sink;
use crate::net::Net;
use crate::secrets::SecretStore;
use crate::{Blob, ConnState, Event, Media, MediaEvent, NetCommand, StreamLink, discovery};

pub enum ToNet {
    Cmd(NetCommand),
    /// Forget the network: its cache file and its password.
    Remove,
    /// The app is in the background (`true`) or back (D39): drop the socket
    /// until it's back, once out of voice. Always-on keeps us in our
    /// channels, and reconnecting catches up through chathistory.
    Background(bool),
}

pub struct Params {
    /// The nick, for a guest.
    pub account: String,
    pub guest: bool,
    pub password: Option<String>,
    pub device: String,
    /// `Some(email)` while signing up.
    pub register: Option<Option<String>>,
    /// Signed up and waiting on the emailed code: the server's message, to
    /// show again after a reconnect without signing up a second time.
    pub verifying: Option<String>,
    /// A guest signing up: their nick, to go back to if it fails.
    pub upgrading: Option<String>,
    /// Set until the first login on a new network succeeds; a failure before
    /// then removes the network again.
    pub joining: Option<String>,
    /// Save the password after the next login (it came from `SignIn`).
    pub save_password: bool,
    /// Signs in with Discord (D96): `password` is chadd's refresh token, and
    /// each connect trades it for a JWT and a new one.
    pub discord: bool,
    pub db_path: PathBuf,
    pub secrets: Arc<dyn SecretStore>,
    pub media: Option<Arc<dyn Media>>,
    /// [`crate::Config::version`].
    pub version: String,
    /// chadd session, reused until it expires.
    pub session: Arc<tokio::sync::Mutex<Option<Session>>>,
    /// [`ToNet::Background`]: what the app asked for last.
    pub background: bool,
}

enum End {
    /// Dropped; retry after a backoff.
    Lost(String),
    /// Retrying won't help until the user acts.
    Fatal(String),
    /// Signed up: log in again so the device ID is in place (D11).
    Registered,
    /// A guest asked to sign up: reconnect to do it.
    Upgrade {
        account: String,
        password: String,
        email: Option<String>,
    },
    Stop,
    Remove,
    /// Closed for [`ToNet::Background`].
    Paused,
}

pub async fn run(mut net: Net, mut p: Params, mut rx: mpsc::UnboundedReceiver<ToNet>, sink: Sink) {
    if !p.guest {
        init_e2e(&mut net, &p).await;
        sink.send(net.drain_events());
    }
    let mut backoff = Backoff::new();
    loop {
        if p.background {
            net.set_state(ConnState::Disconnected {
                error: "Paused while IrChad is in the background.".into(),
            });
            sink.send(net.drain_events());
            loop {
                match rx.recv().await {
                    Some(ToNet::Background(false)) => break,
                    Some(ToNet::Background(true)) => {}
                    Some(ToNet::Cmd(c)) => offline(&mut net, c, &sink),
                    Some(ToNet::Remove) => return remove(net, &p, &sink).await,
                    None => return,
                }
            }
            p.background = false;
            backoff.reset();
        }
        let password = if p.guest {
            Some(String::new())
        } else {
            p.password.clone()
        };
        let end = match password {
            Some(password) => {
                session(&mut net, &mut p, &password, &mut rx, &sink, &mut backoff).await
            }
            None => End::Fatal("Sign in to this network again.".into()),
        };
        net.detach();
        // ergo drops a connection still unregistered after its registration
        // timeout (a minute by default), but the code stays good for hours.
        let retry_verify = matches!(end, End::Lost(_)) && p.verifying.is_some();
        // A guest's sign-up that didn't work: back to being a guest.
        let end = match end {
            End::Lost(error) | End::Fatal(error) if p.upgrading.is_some() && !retry_verify => {
                p.account = p.upgrading.take().expect("guarded");
                p.guest = true;
                p.password = None;
                p.register = None;
                p.verifying = None;
                p.session = Default::default();
                sink.send([Event::SignUpFailed {
                    network: net.info.network.clone(),
                    error,
                }]);
                continue;
            }
            end => end,
        };
        // A guest has no password to fix, so after the first join only retrying helps.
        let end = match end {
            End::Fatal(e) if p.guest && p.joining.is_none() => End::Lost(e),
            end => end,
        };
        match end {
            End::Registered => {
                p.register = None;
                p.verifying = None;
                continue;
            }
            End::Paused => {
                tracing::info!("{}: paused in the background", net.info.network);
                continue;
            }
            End::Upgrade {
                account,
                password,
                email,
            } => {
                p.upgrading = Some(std::mem::replace(&mut p.account, account));
                p.guest = false;
                p.password = Some(password);
                p.register = Some(email);
                p.session = Default::default();
                backoff.reset();
                continue;
            }
            End::Lost(error) if p.joining.is_none() || retry_verify => {
                tracing::info!("{}: connection lost: {error}", net.info.network);
                // Keep the code prompt up across the reconnect.
                if !retry_verify {
                    net.set_state(ConnState::Disconnected { error });
                    sink.send(net.drain_events());
                }
                let delay = backoff.next_delay();
                tracing::info!("{}: reconnecting in {delay:?}", net.info.network);
                let sleep = tokio::time::sleep(delay);
                tokio::pin!(sleep);
                loop {
                    tokio::select! {
                        () = &mut sleep => break,
                        cmd = rx.recv() => match cmd {
                            Some(ToNet::Cmd(c)) => offline(&mut net, c, &sink),
                            // Back in front (the socket likely died while frozen): retry now, not after the wait.
                            Some(ToNet::Background(false)) => {
                                p.background = false;
                                backoff.reset();
                                break;
                            }
                            Some(ToNet::Background(true)) => p.background = true,
                            Some(ToNet::Remove) => return remove(net, &p, &sink).await,
                            None => return,
                        },
                    }
                }
            }
            End::Lost(error) | End::Fatal(error) => {
                p.verifying = None;
                if let Some(invite) = p.joining.take() {
                    sink.send([Event::JoinFailed { invite, error }]);
                    return remove(net, &p, &sink).await;
                }
                net.set_state(ConnState::Failed { error });
                sink.send(net.drain_events());
                // Stay listed with the cache readable until removed, or
                // until the user signs in again.
                loop {
                    match rx.recv().await {
                        Some(ToNet::Cmd(NetCommand::SignIn { password })) => {
                            p.password = Some(password);
                            p.session = Default::default();
                            p.save_password = true;
                            backoff.reset();
                            break;
                        }
                        Some(ToNet::Cmd(c)) => offline(&mut net, c, &sink),
                        // Failed: nothing is connected to pause.
                        Some(ToNet::Background(on)) => p.background = on,
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

/// The code in NickServ's "To confirm, run this command: /NS UNREGISTER <account> <code>".
fn unregister_code(m: &Message, account: &str) -> Option<String> {
    if m.command != "NOTICE" || !m.source_nick()?.eq_ignore_ascii_case("NickServ") {
        return None;
    }
    let mut words = m
        .params
        .last()?
        .split_whitespace()
        .skip_while(|w| *w != "UNREGISTER")
        .skip(1);
    let (name, code) = (words.next()?, words.next()?);
    let ok = name.eq_ignore_ascii_case(account)
        && (1..=32).contains(&code.len())
        && code.bytes().all(|b| b.is_ascii_alphanumeric());
    ok.then(|| code.to_owned())
}

#[test]
fn finds_unregister_code() {
    let m = |l: &str| Message::parse(l).unwrap();
    let ok = ":NickServ!NickServ@localhost NOTICE zed :To confirm, run this command: /NS UNREGISTER zed 5zrg8";
    assert_eq!(unregister_code(&m(ok), "Zed").as_deref(), Some("5zrg8"));
    let other = ":NickServ!NickServ@localhost NOTICE zed :To confirm, run this command: /NS UNREGISTER bob 5zrg8";
    assert_eq!(unregister_code(&m(other), "zed"), None);
    let spoof = ":mallory!m@h NOTICE zed :/NS UNREGISTER zed 5zrg8";
    assert_eq!(unregister_code(&m(spoof), "zed"), None);
}

fn offline(net: &mut Net, cmd: NetCommand, sink: &Sink) {
    net.command(cmd);
    net.out.clear();
    sink.send(net.drain_events());
}

async fn remove(net: Net, p: &Params, sink: &Sink) {
    let network = net.info.network.clone();
    drop(net);
    for ext in ["", "-wal", "-shm"] {
        let mut path = p.db_path.clone().into_os_string();
        path.push(ext);
        let _ = std::fs::remove_file(path);
    }
    for name in [network.clone(), e2e_secret(&network)] {
        if !p.guest
            && let Err(e) = p.secrets.delete(&name).await
        {
            tracing::warn!("forgetting {name}: {e}");
        }
    }
    sink.send([Event::NetworkRemoved { network }]);
}

/// Fresh discovery if the server answers, else the cached copy.
async fn discover(net: &mut Net, guest: bool) -> Result<Discovery, String> {
    let origin = net.info.network.clone();
    if net.classic() {
        let d = classic_discovery(&origin)?;
        net.info.name.clone_from(&d.name);
        return Ok(d);
    }
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
    login::discovered(net, &d, guest);
    Ok(d)
}

/// A classic network has no discovery document (D72). The chadd and LiveKit
/// URLs are required fields, so they hold the IRC origin; nothing calls them
/// (`chadd::Client::call` refuses non-HTTP URLs, `join_voice` refuses classic).
fn classic_discovery(origin: &str) -> Result<Discovery, String> {
    let url = Url::parse(origin).map_err(|_| format!("“{origin}” isn't a server URL"))?;
    Ok(Discovery {
        version: proto_discovery::VERSION,
        name: url.host_str().unwrap_or(origin).to_owned(),
        icon: None,
        home: None,
        irc: proto_discovery::Irc {
            websocket: url.clone(),
        },
        chadd: proto_discovery::Chadd {
            api: url.clone(),
            uploads: None,
            gifs: None,
            games: false,
            emoji: false,
            delete: false,
            meetings: false,
            discord: false,
        },
        livekit: proto_discovery::Livekit { url },
        registration: Registration::Closed,
    })
}

async fn session(
    net: &mut Net,
    p: &mut Params,
    password: &str,
    rx: &mut mpsc::UnboundedReceiver<ToNet>,
    sink: &Sink,
    backoff: &mut Backoff,
) -> End {
    if p.verifying.is_none() {
        net.set_state(ConnState::Connecting);
        sink.send(net.drain_events());
    }
    let d = match discover(net, p.guest).await {
        Ok(d) => d,
        Err(e) if p.joining.is_some() => return End::Fatal(e),
        Err(e) => return End::Lost(e),
    };
    let bearer = if p.discord {
        match chadd::oauth_refresh(&d.chadd.api, password).await {
            Ok((r, session)) => {
                *p.session.lock().await = Some(session);
                // The old one stays good until it expires, so a failed save
                // only means the 90 days don't slide this time.
                if let Err(e) = p
                    .secrets
                    .save(&net.info.network, &p.account, &r.refresh)
                    .await
                {
                    tracing::warn!("{}: saving the refresh token: {e}", net.info.network);
                }
                p.password = Some(r.refresh);
                Some(r.irc_token)
            }
            Err(chadd::Error::Unauthorized(_)) => {
                return End::Fatal("Sign in with Discord again.".into());
            }
            Err(e) if p.joining.is_some() => return End::Fatal(e.message()),
            Err(e) => return End::Lost(e.message()),
        }
    } else {
        None
    };
    let acct = Account {
        name: p.account.clone(),
        password: password.to_owned(),
        guest: p.guest,
        device: p.device.clone(),
        sign_up: p.register.clone(),
        verifying: p.verifying.is_some(),
        bearer,
    };
    let login = match Login::start(net, &d, acct) {
        Ok(l) => l,
        Err(e) => return End::Fatal(e),
    };
    let mut ws = match Connection::connect(d.irc.websocket.as_str()).await {
        Ok(ws) => ws,
        Err(e) => {
            tracing::warn!("{}: connect: {e}", net.info.network);
            return End::Lost(format!("Couldn't connect to {}.", d.name));
        }
    };
    let nick = match register(net, p, password, rx, sink, backoff, login, &mut ws).await {
        Ok(nick) => nick,
        Err(end) => return end,
    };
    net.attach(&nick);
    backoff.reset();
    live(net, p, password, rx, sink, &d, ws).await
}

/// Feeds the connection to [`Login`] until it's done. Returns our nick.
#[allow(clippy::too_many_arguments)]
async fn register(
    net: &mut Net,
    p: &mut Params,
    password: &str,
    rx: &mut mpsc::UnboundedReceiver<ToNet>,
    sink: &Sink,
    backoff: &mut Backoff,
    mut login: Login,
    ws: &mut Connection,
) -> Result<String, End> {
    loop {
        for m in std::mem::take(&mut login.out) {
            if let Err(e) = send(ws, &m).await {
                return Err(lost(e));
            }
        }
        sink.send(net.drain_events());
        let m = tokio::select! {
            r = ws.recv() => match r {
                Ok(Some(m)) => m,
                Ok(None) => return Err(End::Lost("The server closed the connection.".into())),
                Err(e) => return Err(lost(e)),
            },
            cmd = rx.recv() => {
                match cmd {
                    Some(ToNet::Cmd(NetCommand::Verify { code })) => login.verify(&code),
                    Some(ToNet::Cmd(c)) => offline(net, c, sink),
                    // Paused once logged in, so a sign-up isn't cut short.
                    Some(ToNet::Background(on)) => p.background = on,
                    Some(ToNet::Remove) => return Err(End::Remove),
                    None => return Err(End::Stop),
                }
                continue;
            }
        };
        match login.handle(net, m) {
            Step::Continue => {}
            Step::LoggedIn => logged_in(net, p, password, sink).await,
            Step::NeedsVerification(message) => {
                p.verifying = Some(message);
                // The server's fine; only its registration timeout drops us.
                backoff.reset();
            }
            Step::Registered => {
                for m in login.out.drain(..) {
                    let _ = ws.send(&m).await;
                }
                return Err(End::Registered);
            }
            Step::Done(nick) => {
                if p.guest && p.joining.take().is_some() {
                    net.store.set_meta("origin", &net.info.network.clone());
                    net.store.set_meta("guest", &p.account);
                }
                return Ok(nick);
            }
            Step::Lost(e) => return Err(End::Lost(e)),
            Step::Fatal(e) => return Err(End::Fatal(e)),
        }
    }
}

/// Registered: IRC, voice and UI commands until the connection ends.
async fn live(
    net: &mut Net,
    p: &mut Params,
    password: &str,
    rx: &mut mpsc::UnboundedReceiver<ToNet>,
    sink: &Sink,
    d: &Discovery,
    mut ws: Connection,
) -> End {
    let mut client = chadd::Client::new(
        d.chadd.api.clone(),
        p.account.clone(),
        (!p.guest).then(|| password.to_owned()),
        p.session.clone(),
    );
    client.discord = p.discord;
    let client = Arc::new(client);
    fetch(NetCommand::Emoji, net, &client, sink);
    // Dropping `call` hangs up.
    let mut call: Option<mpsc::UnboundedReceiver<MediaEvent>> = None;
    let mut token: Option<JoinHandle<TokenResult>> = None;
    // Deleting the account: when to give up, and whether we sent the confirmation code.
    let mut deleting: Option<(tokio::time::Instant, bool)> = None;
    // Messages chadd deleted with other accounts while we were away (D69).
    let mut redactions = d.chadd.delete.then(|| {
        let api = d.chadd.api.clone();
        let since = net.redacted_since();
        tokio::spawn(async move { chadd::redacted(&api, since).await })
    });
    let mut lag = tokio::time::interval(std::time::Duration::from_secs(30));
    lag.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        // A call keeps the connection (it needs the voice metadata) until it ends.
        if p.background && net.voice.is_none() && call.is_none() && token.is_none() {
            for m in std::mem::take(&mut net.out) {
                let _ = send(&mut ws, &m).await;
            }
            let _ = ws.close().await;
            return End::Paused;
        }
        if let Some(channel) = net.voice_request.take() {
            call = None;
            if p.media.is_some() {
                let guest = p.guest.then(|| net.info.nick.clone());
                token = Some(tokio::spawn(voice_token(client.clone(), channel, guest)));
            } else {
                net.voice_ended(Some("Voice isn't available in this app.".into()));
            }
        }
        if net.voice.is_none() {
            call = None;
            if let Some(t) = token.take() {
                t.abort();
            }
        }
        for m in std::mem::take(&mut net.out) {
            if let Err(e) = send(&mut ws, &m).await {
                return lost(e);
            }
        }
        sink.send(net.drain_events());
        let push_retry = net.push_retry_at();
        let read_due = net.read_due_at();
        tokio::select! {
            () = tokio::time::sleep_until(read_due.unwrap_or_else(std::time::Instant::now).into()), if read_due.is_some() => net.flush_reads(),
            () = tokio::time::sleep_until(push_retry.unwrap_or_else(std::time::Instant::now).into()), if push_retry.is_some() => net.retry_push(),
            () = tokio::time::sleep_until(deleting.map_or_else(tokio::time::Instant::now, |d| d.0)), if deleting.is_some() => {
                deleting = None;
                sink.send([Event::Error {
                    network: net.info.network.clone(),
                    error: "The server didn't delete your account.".into(),
                }]);
            }
            _ = lag.tick() => net.ping_lag(),
            // ergo drops the connection once the account is gone.
            r = ws.recv() => match r {
                Ok(Some(m)) if m.command == "ERROR" && deleting.is_some_and(|d| d.1) => return End::Remove,
                Ok(None) | Err(_) if deleting.is_some_and(|d| d.1) => return End::Remove,
                Ok(Some(m)) => {
                    if let Some(d) = deleting.as_mut()
                        && !d.1
                        && let Some(code) = unregister_code(&m, &p.account)
                    {
                        d.1 = true;
                        let text = format!("UNREGISTER {} {code}", p.account);
                        net.out.push(Message::new("PRIVMSG", ["NickServ", text.as_str()]));
                    }
                    if let Err(e) = login::live(net, m) {
                        return End::Lost(e);
                    }
                }
                Ok(None) => return End::Lost("The server closed the connection.".into()),
                Err(e) => return lost(e),
            },
            r = async { redactions.as_mut().expect("guarded").await }, if redactions.is_some() => {
                redactions = None;
                if let Ok(Some((ids, next))) = r {
                    net.chadd_redacted(&ids, next);
                }
            }
            r = async { token.as_mut().expect("guarded").await }, if token.is_some() => {
                token = None;
                let Ok((channel, res)) = r else { continue };
                let wanted = net.voice.as_ref().is_some_and(|v| v.channel == channel);
                match (res, &p.media) {
                    (Ok(t), Some(media)) if wanted => {
                        call = Some(media.join(d.livekit.url.to_string(), t.token));
                    }
                    (Err(e), _) if wanted => net.voice_ended(Some(e)),
                    _ => {}
                }
            }
            ev = async { call.as_mut().expect("guarded").recv().await }, if call.is_some() => {
                match ev {
                    Some(MediaEvent::Connected) => net.voice_connected(),
                    Some(MediaEvent::Ended(e)) => {
                        call = None;
                        net.voice_ended(e);
                    }
                    None => {
                        call = None;
                        net.voice_ended(Some("Lost the voice connection.".into()));
                    }
                }
            }
            cmd = rx.recv() => match cmd {
                Some(ToNet::Cmd(NetCommand::DeleteAccount)) if net.classic() => {
                    net.error("Ask this network's services (NickServ) to drop your account.");
                }
                // chadd deletes the messages and uploads too (D69).
                Some(ToNet::Cmd(NetCommand::DeleteAccount)) if !p.guest && d.chadd.delete => {
                    let res = if p.discord {
                        // No password to ask for again (D96): the session vouches.
                        let account = p.account.clone();
                        client
                            .call(|api, t| {
                                let account = account.clone();
                                async move { chadd::delete_account(&api, &account, "", Some(&t)).await }
                            })
                            .await
                    } else {
                        chadd::delete_account(&d.chadd.api, &p.account, password, None)
                            .await
                            .map_err(chadd::Error::message)
                    };
                    match res {
                        Ok(()) => {
                            let _ = ws.send(&Message::new("QUIT", ["account deleted"])).await;
                            return End::Remove;
                        }
                        Err(e) => net.error(e),
                    }
                }
                Some(ToNet::Cmd(NetCommand::DeleteAccount)) if !p.guest => {
                    let text = format!("UNREGISTER {}", p.account);
                    net.out.push(Message::new("PRIVMSG", ["NickServ", text.as_str()]));
                    deleting = Some((tokio::time::Instant::now() + std::time::Duration::from_secs(20), false));
                }
                Some(ToNet::Cmd(NetCommand::Register { account, password, email })) if p.guest => {
                    let account = account.trim().to_owned();
                    if !login::valid_nick(&account) {
                        sink.send([Event::SignUpFailed {
                            network: net.info.network.clone(),
                            error: "Names can't have spaces or start with # or :.".into(),
                        }]);
                        continue;
                    }
                    // Gone before the new connection asks for the nick, if it's the same.
                    let _ = ws.send(&Message::new("QUIT", ["signing up"])).await;
                    let _ = ws.close().await;
                    return End::Upgrade { account, password, email };
                }
                Some(ToNet::Cmd(c)) => {
                    if let Some(c) = fetch(c, net, &client, sink) {
                        net.command(c);
                    }
                }
                Some(ToNet::Background(on)) => {
                    // Every return to the front, even within the pause delay or with "Stay connected".
                    if !on {
                        tracing::info!("{}: back in front, catching up", net.info.network);
                        net.resumed();
                    }
                    p.background = on;
                }
                Some(ToNet::Remove) => {
                    net.unpublish();
                    for m in std::mem::take(&mut net.out) {
                        let _ = send(&mut ws, &m).await;
                    }
                    let _ = ws.send(&Message::new("QUIT", ["bye"])).await;
                    return End::Remove;
                }
                None => {
                    // App exit: always-on keeps us in our channels, but not in voice.
                    net.voice_ended(None);
                    for m in std::mem::take(&mut net.out) {
                        let _ = send(&mut ws, &m).await;
                    }
                    let _ = ws.close().await;
                    return End::Stop;
                }
            },
        }
    }
}

/// First login on a new network: now it's worth keeping. After a `SignIn`,
/// the password that worked is saved again.
async fn logged_in(net: &mut Net, p: &mut Params, password: &str, sink: &Sink) {
    let first = p.joining.take().is_some();
    let upgraded = p.upgrading.take().is_some();
    if !first && !upgraded && !std::mem::take(&mut p.save_password) {
        return;
    }
    let origin = net.info.network.clone();
    if first || upgraded {
        net.store.set_meta("origin", &origin);
        net.store.set_meta("account", &p.account);
    }
    if p.discord {
        // Its refresh token was saved before connecting (D96); `password`
        // may be the one it replaced.
        net.store.set_meta(DISCORD_META.0, DISCORD_META.1);
        return;
    }
    if upgraded {
        net.store.delete_meta("guest");
        net.store.set_meta("rejoin", "1");
        net.guest = false;
        net.info.guest = false;
        init_e2e(net, p).await;
        sink.send(net.drain_events());
    }
    if let Err(e) = p.secrets.save(&origin, &p.account, password).await {
        net.error(format!("{e} You'll need to sign in again next time."));
        sink.send(net.drain_events());
    }
}

/// Store meta marking a network that signs in with Discord (D96).
pub const DISCORD_META: (&str, &str) = ("auth", "discord");

/// The secret store entry holding the key to a network's E2EE pickles.
fn e2e_secret(origin: &str) -> String {
    format!("{origin}#e2e")
}

/// Opens this device's E2EE keys, making them on first use. Without a keyring
/// encrypted DMs stay off rather than keep keys in the clear.
async fn init_e2e(net: &mut Net, p: &Params) {
    let name = e2e_secret(&net.info.network);
    let parse = |hex: &str| -> Option<[u8; 32]> {
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
            .collect::<Option<_>>()?;
        bytes.try_into().ok()
    };
    let key = match p.secrets.load(&name).await {
        Ok(Some((_, hex))) => parse(&hex),
        Ok(None) => None,
        Err(e) => return net.error(format!("{e} Encrypted DMs are off for now.")),
    };
    let key = match key {
        Some(k) => k,
        None => {
            let k = chad_e2ee::new_pickle_key();
            let hex = hex::encode(k);
            if let Err(e) = p.secrets.save(&name, "e2e", &hex).await {
                return net.error(format!("{e} Encrypted DMs are off for now."));
            }
            k
        }
    };
    net.e2e_init(key, &p.device);
}

type TokenResult = (String, Result<VoiceTokenResponse, String>);

/// A LiveKit token for `channel`.
/// `guest` is our nick when we have no account: then only a meeting's call (D86).
async fn voice_token(
    client: Arc<chadd::Client>,
    channel: String,
    guest: Option<String>,
) -> TokenResult {
    let r = client.voice_token(&channel, guest.as_deref()).await;
    (channel, r)
}

/// Starts the chadd calls in the background, so a big upload doesn't hold up
/// the chat; any other command comes back.
fn fetch(
    cmd: NetCommand,
    net: &Net,
    client: &Arc<chadd::Client>,
    sink: &Sink,
) -> Option<NetCommand> {
    let network = net.info.network.clone();
    let (client, sink) = (client.clone(), sink.clone());
    match cmd {
        NetCommand::Upload {
            target,
            name,
            bytes,
        } => {
            let max = net.info.uploads;
            tokio::spawn(async move {
                let result = match max {
                    None => Err("This server doesn't take uploads.".into()),
                    Some(max) if bytes.0.len() as u64 > max => Err(format!(
                        "That file is too big. The limit here is {} MB.",
                        max >> 20
                    )),
                    Some(_) => {
                        client
                            .call(|api, t| {
                                let (name, bytes) = (name.clone(), bytes.0.to_vec());
                                async move { chadd::upload(&api, &t, &name, bytes).await }
                            })
                            .await
                    }
                };
                sink.send([Event::Uploaded {
                    network,
                    target,
                    result,
                }]);
            });
        }
        NetCommand::CreateMeeting => {
            tokio::spawn(async move {
                let result = client
                    .call(|api, t| async move { chadd::create_meeting(&api, &t).await })
                    .await
                    .map(|r| (r.channel, r.url));
                sink.send([Event::MeetingCreated { network, result }]);
            });
        }
        NetCommand::CreateStream => {
            // Viewers see who's streaming: our display name, if we set one.
            let req = chad_proto::stream::CreateStreamRequest {
                name: net.own_display_name().map(str::to_owned),
            };
            tokio::spawn(async move {
                let result = client
                    .call(|api, t| {
                        let req = req.clone();
                        async move { chadd::create_stream(&api, &t, &req).await }
                    })
                    .await
                    .map(|r| StreamLink {
                        url: r.url,
                        room: r.room,
                        livekit: r.livekit.to_string(),
                        token: r.token,
                    });
                sink.send([Event::StreamCreated { network, result }]);
            });
        }
        NetCommand::StreamToken { room } => {
            tokio::spawn(async move {
                let result = match chad_proto::stream::code_of_room(&room) {
                    Some(code) => client
                        .call(|api, t| {
                            let code = code.clone();
                            async move { chadd::stream_token(&api, &t, &code).await }
                        })
                        .await
                        .map(|r| (r.livekit.to_string(), r.token)),
                    None => Err("That isn't a stream.".into()),
                };
                sink.send([Event::StreamToken {
                    network,
                    room,
                    result,
                }]);
            });
        }
        NetCommand::Preview { url } => {
            tokio::spawn(async move {
                let preview = client
                    .call(|api, t| {
                        let url = url.clone();
                        async move { chadd::preview(&api, &t, &url).await }
                    })
                    .await
                    .ok();
                sink.send([Event::Preview {
                    network,
                    url,
                    preview,
                }]);
            });
        }
        NetCommand::Media { url } => {
            tokio::spawn(async move {
                // Our own uploads: same server, nothing new learns our IP.
                let own = Url::parse(&url)
                    .ok()
                    .zip(Url::parse(&network).ok())
                    .is_some_and(|(u, n)| u.origin() == n.origin());
                let result = if own {
                    discovery::get(&url, chadd::MAX_MEDIA)
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|b| b.ok_or_else(|| "That image is too big.".into()))
                } else {
                    client
                        .call(|api, t| {
                            let url = url.clone();
                            async move { chadd::media(&api, &t, &url).await }
                        })
                        .await
                };
                let result = result.map(|b| Blob(b.into()));
                sink.send([Event::Media {
                    network,
                    url,
                    result,
                }]);
            });
        }
        NetCommand::Gifs { query, pos } => {
            tokio::spawn(async move {
                let result = client
                    .call(|api, t| {
                        let (query, pos) = (query.clone(), pos.clone());
                        async move { chadd::gifs(&api, &t, query, pos).await }
                    })
                    .await;
                sink.send([Event::Gifs {
                    network,
                    query,
                    pos,
                    result,
                }]);
            });
        }
        NetCommand::Games { etag } => {
            tokio::spawn(async move {
                let result = client
                    .call(|api, t| {
                        let etag = etag.clone();
                        async move { chadd::games(&api, &t, etag.as_deref()).await }
                    })
                    .await;
                sink.send([Event::Games { network, result }]);
            });
        }
        NetCommand::Emoji => {
            if net.info.emoji {
                tokio::spawn(async move {
                    // On failure the set we have stays.
                    if let Ok(list) = chadd::emoji(&client.api).await {
                        let emoji = crate::emoji::custom(list, &network);
                        sink.send([Event::Emoji { network, emoji }]);
                    }
                });
            }
        }
        NetCommand::SetPassword { password } => {
            tokio::spawn(async move {
                let result = client
                    .call(|api, t| {
                        let password = password.clone();
                        async move { chadd::set_password(&api, &t, &password).await }
                    })
                    .await;
                sink.send([Event::PasswordSet { network, result }]);
            });
        }
        NetCommand::Sounds => {
            tokio::spawn(async move {
                let result = client
                    .call(|api, t| async move { chadd::sounds(&api, &t).await })
                    .await;
                sink.send([Event::Sounds { network, result }]);
            });
        }
        NetCommand::Sound { file } => {
            tokio::spawn(async move {
                let result = client
                    .call(|api, t| {
                        let file = file.clone();
                        async move { chadd::sound(&api, &t, &file).await }
                    })
                    .await
                    .map(|b| Blob(b.into()));
                sink.send([Event::Sound {
                    network,
                    file,
                    result,
                }]);
            });
        }
        cmd => return Some(cmd),
    }
    None
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

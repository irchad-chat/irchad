//! One task per network: discovery, connect, CAP/SASL/REGISTER, then pump lines
//! between the socket and [`Net`]; reconnect with backoff when it drops.

use std::path::PathBuf;
use std::sync::Arc;

use chad_irc::Message;
use chad_irc::cap::Negotiator;
use chad_irc::register::{self, Reply};
use chad_irc::sasl::{self, Progress, Sasl};
use chad_irc::ws::{self, Backoff, Connection};
use chad_proto::chadd::VoiceTokenResponse;
use chad_proto::discovery::{Discovery, Registration};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use url::Url;

use crate::chadd::{self, Session};
use crate::manager::Sink;
use crate::net::{
    AVATAR_KEY, CHANNELS_KEY, DEVICES_KEY, DISPLAY_NAME_KEY, Net, VOICE_KEY, failure,
};
use crate::secrets::SecretStore;
use crate::{Blob, ConnState, Event, Media, MediaEvent, NetCommand, discovery};

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
            End::Paused => continue,
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
                            Some(ToNet::Background(on)) => p.background = on,
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
    net.info.name.clone_from(&d.name);
    net.info.icon.clone_from(&d.icon);
    net.home = d.home.clone().filter(|h| net.is_home_name(h));
    net.info.uploads = d
        .chadd
        .uploads
        .as_ref()
        .filter(|_| !guest)
        .map(|u| u.max_bytes);
    net.info.gifs = d
        .chadd
        .gifs
        .as_ref()
        .filter(|_| !guest)
        .map(|g| g.provider.chars().take(40).collect());
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
    if p.verifying.is_none() {
        net.set_state(ConnState::Connecting);
        sink.send(net.drain_events());
    }
    let d = match discover(net, p.guest).await {
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

    let nick = match register(net, p, password, rx, sink, backoff, &d, &mut ws).await {
        Ok(nick) => nick,
        Err(end) => return end,
    };
    net.attach(&nick);
    backoff.reset();
    live(net, p, password, rx, sink, &d, ws).await
}

/// CAP, SASL or sign-up, until the end of the MOTD. Returns our nick.
#[allow(clippy::too_many_arguments)]
async fn register(
    net: &mut Net,
    p: &mut Params,
    password: &str,
    rx: &mut mpsc::UnboundedReceiver<ToNet>,
    sink: &Sink,
    backoff: &mut Backoff,
    d: &Discovery,
    ws: &mut Connection,
) -> Result<String, End> {
    let account = p.account.clone();
    let mut auth_started = false;
    let mut nick_tries = 0;
    let mut sasl: Option<Sasl> = None;
    loop {
        let m = tokio::select! {
            r = ws.recv() => match r {
                Ok(Some(m)) => m,
                Ok(None) => return Err(End::Lost("The server closed the connection.".into())),
                Err(e) => return Err(lost(e)),
            },
            cmd = rx.recv() => {
                match cmd {
                    Some(ToNet::Cmd(NetCommand::Verify { code })) => {
                        let _ = ws.send(&register::verify(&account, code.trim())).await;
                    }
                    Some(ToNet::Cmd(c)) => offline(net, c, sink),
                    // Paused once logged in, so a sign-up isn't cut short.
                    Some(ToNet::Background(on)) => p.background = on,
                    Some(ToNet::Remove) => return Err(End::Remove),
                    None => return Err(End::Stop),
                }
                continue;
            }
        };
        let mut out = net.caps.handle(&m);
        net.isupport.handle(&m);

        if net.caps.ready() && !auth_started {
            auth_started = true;
            sasl = start_auth(net, p, password, d, &mut out)?;
        }
        if let Some(s) = &mut sasl {
            match s.handle(&m) {
                Progress::Continue(more) => out.extend(more),
                Progress::LoggedIn => {
                    sasl = None;
                    out.push(net.caps.end());
                    logged_in(net, p, password, sink).await;
                }
                Progress::Failed(e) => return Err(End::Fatal(sasl_error(e))),
            }
        }
        match Reply::parse(&m) {
            Some(Reply::Registered { .. } | Reply::Verified { .. }) => {
                let _ = ws.send(&Message::new("QUIT", ["signed up"])).await;
                return Err(End::Registered);
            }
            Some(Reply::NeedsVerification { message, .. }) => {
                p.verifying = Some(message.clone());
                // The server's fine; only its registration timeout drops us.
                backoff.reset();
                net.set_state(ConnState::NeedsVerification { message });
                sink.send(net.drain_events());
            }
            Some(Reply::Failed { command, .. }) if command == "VERIFY" => {
                net.error("That code didn't work. Check the email and try again.");
                sink.send(net.drain_events());
            }
            Some(Reply::Failed { message, .. }) => {
                let plain = failure(&chad_irc::batch::Item::Message(m.clone()));
                return Err(End::Fatal(plain.unwrap_or(message)));
            }
            None => {}
        }
        match m.command.as_str() {
            "001" => m
                .param(0)
                .unwrap_or(&account)
                .clone_into(&mut net.info.nick),
            "376" | "422" => {
                if p.guest && p.joining.take().is_some() {
                    net.store.set_meta("origin", &net.info.network.clone());
                    net.store.set_meta("guest", &account);
                }
                return Ok(net.info.nick.clone());
            }
            // Guests take the next free `nick1`..`nick9`.
            "433" if p.guest && nick_tries < 9 => {
                nick_tries += 1;
                out.push(Message::new("NICK", [format!("{account}{nick_tries}")]));
            }
            "433" if p.guest => return Err(End::Fatal("That nickname is taken.".into())),
            "433" => return Err(End::Fatal("That username is taken.".into())),
            "432" => return Err(End::Fatal("That name isn't allowed here.".into())),
            "MODE" | "221" => {
                let _ = net.handle(m);
            }
            "ERROR" => return Err(End::Lost(m.params.last().cloned().unwrap_or_default())),
            _ => {}
        }
        for o in out {
            if let Err(e) = send(ws, &o).await {
                return Err(lost(e));
            }
        }
    }
}

/// Once CAP LS is in: subscribe to metadata, then start SASL or a sign-up.
/// Returns the SASL exchange, if one started.
fn start_auth(
    net: &mut Net,
    p: &Params,
    password: &str,
    d: &Discovery,
    out: &mut Vec<Message>,
) -> Result<Option<Sasl>, End> {
    // Before CAP END, so the reattach burst includes everyone's value.
    if net.caps.enabled("draft/metadata-2") {
        let keys = [
            VOICE_KEY,
            DISPLAY_NAME_KEY,
            AVATAR_KEY,
            DEVICES_KEY,
            CHANNELS_KEY,
        ];
        out.push(Message::new(
            "METADATA",
            ["*", "SUB"].into_iter().chain(keys),
        ));
    }
    if p.guest {
        out.push(net.caps.end());
        return Ok(None);
    }
    if let Some(email) = &p.register {
        if !net.caps.enabled("draft/account-registration") {
            return Err(End::Fatal(format!("{} isn't taking sign-ups.", d.name)));
        }
        // Already signed up: VERIFY works from a fresh connection.
        if p.verifying.is_none() {
            out.push(register::register("*", email.as_deref(), password));
        }
        return Ok(None);
    }
    if !net.caps.enabled("sasl") {
        return Err(End::Fatal(format!(
            "{} doesn't support logging in.",
            d.name
        )));
    }
    let user = format!("{}@{}", p.account, p.device);
    let mechs = net.caps.value("sasl").unwrap_or_default();
    let mut s = Sasl::new(&user, password, mechs).map_err(|e| End::Fatal(sasl_error(e)))?;
    out.push(s.start());
    Ok(Some(s))
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
    let client = Arc::new(chadd::Client::new(
        d.chadd.api.clone(),
        p.account.clone(),
        (!p.guest).then(|| password.to_owned()),
        p.session.clone(),
    ));
    // Dropping `call` hangs up.
    let mut call: Option<mpsc::UnboundedReceiver<MediaEvent>> = None;
    let mut token: Option<JoinHandle<TokenResult>> = None;
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
                token = Some(tokio::spawn(voice_token(client.clone(), channel)));
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
        tokio::select! {
            () = tokio::time::sleep_until(push_retry.unwrap_or_else(std::time::Instant::now).into()), if push_retry.is_some() => net.retry_push(),
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
                Some(ToNet::Cmd(NetCommand::Register { account, password, email })) if p.guest => {
                    let account = account.trim().to_owned();
                    if !crate::manager::valid_nick(&account) {
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
                Some(ToNet::Background(on)) => p.background = on,
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
async fn voice_token(client: Arc<chadd::Client>, channel: String) -> TokenResult {
    let r = client
        .call(|api, t| {
            let channel = channel.clone();
            async move { chadd::voice_token(&api, &t, &channel).await }
        })
        .await;
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

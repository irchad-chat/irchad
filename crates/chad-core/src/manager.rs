//! Owns the set of networks: loads them at startup, starts joins, routes commands.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::conn::{self, Params, ToNet};
use crate::login::valid_nick;
use crate::net::Net;
use crate::oauth::{self, Flow};
use crate::store::Store;
use crate::{
    Auth, Command, Config, ConnState, Core, Event, NetCommand, NetworkInfo, chadd, invite,
};

#[derive(Clone)]
pub struct Sink {
    tx: mpsc::UnboundedSender<Event>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl Sink {
    pub fn send(&self, events: impl IntoIterator<Item = Event>) {
        let mut any = false;
        for e in events {
            any = true;
            let _ = self.tx.send(e);
        }
        if any {
            (self.notify)();
        }
    }
}

/// Starts the core on the current tokio runtime. Cached networks show up as
/// events right away and connect in the background.
pub fn start(cfg: Config) -> (Core, mpsc::UnboundedReceiver<Event>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let (etx, erx) = mpsc::unbounded_channel();
    let sink = Sink {
        tx: etx,
        notify: cfg.notify.clone(),
    };
    tokio::spawn(manager(cfg, rx, sink));
    (Core { tx }, erx)
}

async fn manager(cfg: Config, mut rx: mpsc::UnboundedReceiver<Command>, sink: Sink) {
    let dir = cfg.data_dir.join("networks");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::error!("creating {}: {e}", dir.display());
    }
    let device = device_id(&cfg.data_dir);
    let params = |account: String, password, db_path| Params {
        account,
        guest: false,
        password,
        device: device.clone(),
        register: None,
        verifying: None,
        upgrading: None,
        joining: None,
        save_password: false,
        discord: false,
        db_path,
        secrets: cfg.secrets.clone(),
        media: cfg.media.clone(),
        version: cfg.version.clone(),
        session: Default::default(),
        background: false,
    };

    let mut nets: HashMap<String, mpsc::UnboundedSender<ToNet>> = HashMap::new();
    for path in std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "sqlite"))
    {
        let store = match Store::open(&path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("skipping {}: {e}", path.display());
                continue;
            }
        };
        let Some(origin) = store.meta("origin") else {
            continue; // a join that never finished
        };
        if let Some(nick) = store.meta("guest") {
            let mut p = params(nick, None, path);
            p.guest = true;
            nets.insert(origin.clone(), spawn(origin, store, p, &sink));
            continue;
        }
        let Some(account) = store.meta("account") else {
            continue;
        };
        let password = match cfg.secrets.load(&origin).await {
            Ok(Some((a, pw))) if a == account => Some(pw),
            Ok(_) => None,
            Err(e) => {
                sink.send([Event::Error {
                    network: origin.clone(),
                    error: e,
                }]);
                None
            }
        };
        let mut p = params(account, password, path);
        p.discord = store.meta(conn::DISCORD_META.0).as_deref() == Some(conn::DISCORD_META.1);
        nets.insert(origin.clone(), spawn(origin, store, p, &sink));
    }

    // A "Sign in with Discord" waiting on the browser or a name (D96).
    let mut flow: Option<Flow> = None;
    while let Some(cmd) = rx.recv().await {
        // shortcut: chadd is called inline, so other networks' commands wait
        // the second or so it takes; spawn it if that's ever noticeable.
        let cmd = match cmd {
            Command::DiscordStart { invite } => {
                flow = None;
                match discord_start(&invite).await {
                    Ok(f) => {
                        sink.send([Event::OpenUrl { url: f.start_url() }]);
                        flow = Some(f);
                    }
                    Err(error) => sink.send([Event::JoinFailed { invite, error }]),
                }
                continue;
            }
            Command::DiscordCallback { url } => {
                let Some(cb) = oauth::parse_callback(&url) else {
                    tracing::warn!("ignoring a malformed sign-in redirect");
                    continue;
                };
                // Not ours, or not now: someone else's link (D96).
                let Some(f) = flow.take_if(|f| f.origin == cb.origin && f.ticket.is_none()) else {
                    tracing::warn!("ignoring a sign-in redirect we didn't start");
                    continue;
                };
                let outcome = match cb.result {
                    Ok(code) => chadd::oauth_exchange(&f.api, &code, &f.verifier)
                        .await
                        .map_err(chadd::Error::message),
                    Err(e) => Err(e),
                };
                match discord_outcome(outcome, f, &mut flow, &sink) {
                    Some(cmd) => cmd,
                    None => continue,
                }
            }
            Command::DiscordName { name } => {
                let Some(f) = flow.take_if(|f| f.ticket.is_some()) else {
                    continue;
                };
                let ticket = f.ticket.as_deref().unwrap_or_default();
                match chadd::oauth_register(&f.api, ticket, name.trim()).await {
                    // The name can't be had, or the server's busy: ask again.
                    Err(chadd::Error::Other(error)) => {
                        sink.send([Event::DiscordNeedsName {
                            invite: f.invite.clone(),
                            suggestion: name,
                            error: Some(error),
                        }]);
                        flow = Some(f);
                        continue;
                    }
                    outcome => match discord_outcome(
                        outcome.map_err(chadd::Error::message),
                        f,
                        &mut flow,
                        &sink,
                    ) {
                        Some(cmd) => cmd,
                        None => continue,
                    },
                }
            }
            Command::DiscordCancel => {
                flow = None;
                continue;
            }
            cmd => cmd,
        };
        match cmd {
            // Signed in with Discord on a network we have: its new login (D96).
            Command::Join {
                invite,
                auth: Auth::Discord { refresh, .. },
            } if invite::parse(&invite)
                .ok()
                .and_then(|i| nets.get(&i.origin))
                .is_some_and(|t| !t.is_closed()) =>
            {
                let origin = invite::parse(&invite).expect("guarded").origin;
                let cmd = NetCommand::SignIn { password: refresh };
                let _ = nets[&origin].send(ToNet::Cmd(cmd));
            }
            Command::Join { invite, auth } => {
                let inv = match invite::parse(&invite) {
                    Ok(i) => i,
                    Err(error) => {
                        sink.send([Event::JoinFailed { invite, error }]);
                        continue;
                    }
                };
                if nets.get(&inv.origin).is_some_and(|t| !t.is_closed()) {
                    let error = "You've already joined this network.".into();
                    sink.send([Event::JoinFailed { invite, error }]);
                    continue;
                }
                let discord = matches!(auth, Auth::Discord { .. });
                let (account, password, register) = match auth {
                    Auth::Login { account, password } => (account, Some(password), None),
                    Auth::Register {
                        account,
                        password,
                        email,
                    } => (account, Some(password), Some(email)),
                    Auth::Guest { nick } => (nick, None, None),
                    Auth::Discord { account, refresh } => (account, Some(refresh), None),
                };
                let account = account.trim().to_owned();
                if !valid_nick(&account) {
                    let error = "Names can't have spaces or start with # or :.".into();
                    sink.send([Event::JoinFailed { invite, error }]);
                    continue;
                }
                let path = dir.join(file_name(&inv.origin));
                let store = match Store::open(&path) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!("{}: {e}", path.display());
                        let error = "Couldn't save anything to disk.".into();
                        sink.send([Event::JoinFailed { invite, error }]);
                        continue;
                    }
                };
                let guest = password.is_none();
                let mut p = params(account, password, path);
                p.guest = guest;
                p.register = register;
                p.discord = discord;
                p.joining = Some(invite);
                let tx = spawn_with(inv.origin.clone(), store, p, &sink, inv.channel);
                nets.insert(inv.origin, tx);
            }
            // Taken care of before this match.
            Command::DiscordStart { .. }
            | Command::DiscordCallback { .. }
            | Command::DiscordName { .. }
            | Command::DiscordCancel => {}
            Command::Background { on } => {
                for tx in nets.values() {
                    let _ = tx.send(ToNet::Background(on));
                }
            }
            Command::Remove { network } => {
                if let Some(tx) = nets.remove(&network) {
                    let _ = tx.send(ToNet::Remove);
                }
            }
            Command::Net { network, cmd } => {
                // One call at a time, across every network.
                if matches!(cmd, NetCommand::JoinVoice { .. }) {
                    for (_, tx) in nets.iter().filter(|(n, _)| **n != network) {
                        let _ = tx.send(ToNet::Cmd(NetCommand::LeaveVoice));
                    }
                }
                if let Some(tx) = nets.get(&network) {
                    let _ = tx.send(ToNet::Cmd(cmd));
                }
            }
        }
    }
}

fn spawn(origin: String, store: Store, p: Params, sink: &Sink) -> mpsc::UnboundedSender<ToNet> {
    spawn_with(origin, store, p, sink, None)
}

fn spawn_with(
    origin: String,
    store: Store,
    p: Params,
    sink: &Sink,
    invite_channel: Option<String>,
) -> mpsc::UnboundedSender<ToNet> {
    let info = NetworkInfo {
        name: origin
            .split_once("://")
            .map_or(origin.as_str(), |(_, rest)| rest)
            .to_owned(),
        network: origin,
        icon: None,
        nick: p.account.clone(),
        state: ConnState::Connecting,
        guest: p.guest,
        discord: p.discord,
        uploads: None,
        gifs: None,
        games: false,
        meetings: false,
        emoji: false,
        receipts: false,
        lag_ms: None,
        oper: false,
    };
    let mut net = Net::new(store, info);
    net.invite_channel = invite_channel;
    net.guest = p.guest;
    net.version.clone_from(&p.version);
    if let Some(d) = net
        .store
        .meta("discovery")
        .and_then(|d| crate::discovery::parse(d.as_bytes()))
    {
        net.info.name = d.name;
        net.info.icon = d.icon;
    }
    net.set_state(ConnState::Connecting);
    net.load_cache();
    sink.send(net.drain_events());
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(conn::run(net, p, rx, sink.clone()));
    tx
}

/// A new sign-in for `invite`, if its server does Discord.
async fn discord_start(invite: &str) -> Result<Flow, String> {
    let inv = invite::parse(invite)?;
    if invite::classic(&inv.origin) {
        return Err("That network doesn't sign in with Discord.".into());
    }
    let d = crate::discovery::fetch(&inv.origin).await?;
    if !d.chadd.discord {
        return Err(format!("{} doesn't sign in with Discord.", d.name));
    }
    Ok(Flow::new(invite.to_owned(), inv.origin, d.chadd.api))
}

/// chadd's answer: a join to run, a name to ask for, or a failure to show.
fn discord_outcome(
    outcome: Result<chad_proto::chadd::OAuthOutcome, String>,
    mut f: Flow,
    flow: &mut Option<Flow>,
    sink: &Sink,
) -> Option<Command> {
    use chad_proto::chadd::OAuthOutcome;
    match outcome {
        Ok(OAuthOutcome::SignedIn { account, refresh }) => Some(Command::Join {
            invite: f.invite,
            auth: Auth::Discord { account, refresh },
        }),
        Ok(OAuthOutcome::NeedsName { suggestion, ticket }) => {
            sink.send([Event::DiscordNeedsName {
                invite: f.invite.clone(),
                suggestion,
                error: None,
            }]);
            f.ticket = Some(ticket);
            *flow = Some(f);
            None
        }
        Err(error) => {
            sink.send([Event::JoinFailed {
                invite: f.invite,
                error,
            }]);
            None
        }
    }
}

/// Enough to keep the name a single IRC parameter; the server judges the rest.
fn file_name(origin: &str) -> PathBuf {
    // Classic ids keep their scheme, so `ircs://` and `irc://` to one host differ.
    let safe: String = origin
        .trim_start_matches("https://")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    PathBuf::from(format!("{safe}.sqlite"))
}

/// Stable per install: the `@device` in SASL logins (D11), and Phase 4's E2EE device.
fn device_id(data_dir: &Path) -> String {
    let path = data_dir.join("device-id");
    let valid =
        |s: &str| !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_alphanumeric());
    if let Ok(id) = std::fs::read_to_string(&path)
        && valid(id.trim())
    {
        return id.trim().to_owned();
    }
    let mut bytes = [0u8; 8];
    let _ = getrandom::fill(&mut bytes);
    let id = hex::encode(bytes);
    if let Err(e) = std::fs::write(&path, &id) {
        tracing::error!("saving device id: {e}");
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_and_device_id() {
        assert_eq!(
            file_name("https://chat.localhost:8443"),
            PathBuf::from("chat.localhost_8443.sqlite")
        );
        let dir = tempfile::tempdir().unwrap();
        let id = device_id(dir.path());
        assert_eq!(id.len(), 16);
        assert_eq!(device_id(dir.path()), id);
    }

    #[test]
    fn nicks() {
        assert!(valid_nick("carol") && valid_nick("[x]_y"));
        for bad in ["", "a b", "#chan", ":x", "a\rb"] {
            assert!(!valid_nick(bad), "{bad:?}");
        }
    }
}

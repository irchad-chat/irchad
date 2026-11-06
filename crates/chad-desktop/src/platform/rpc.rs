//! Discord Rich Presence, received: we listen where games look for Discord's
//! IPC socket (`discord-ipc-N`) and keep what their `SET_ACTIVITY` says.
//! Opt-in (`Settings::share_activity`). Protocol as in arRPC: frames are
//! `u32 LE opcode, u32 LE length, JSON`. The socket or pipe itself is per OS.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chad_core::one_line;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::watch;
use tokio::task::JoinSet;

const HANDSHAKE: u32 = 0;
const FRAME: u32 = 1;
const CLOSE: u32 = 2;
const PING: u32 = 3;
const PONG: u32 = 4;
/// Any local process can connect: bound what one can make us hold.
const MAX_FRAME: usize = 64 << 10;
const MAX_CLIENTS: usize = 16;
/// Games stay connected and quiet for hours, but say hello at once.
const HANDSHAKE_WAIT: Duration = Duration::from_secs(10);
const MAX_TEXT: usize = 128;
/// Discord's SDKs try `discord-ipc-0` up to this, first answer wins.
pub(super) const SLOTS: u32 = 10;

/// What a game last said it's doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    /// The Discord application id it handshook with.
    pub app: String,
    /// Discord's activity type: 0 playing, 2 listening, 3 watching, 5 competing.
    pub kind: u64,
    pub details: Option<String>,
}

impl Activity {
    fn parse(app: &str, v: &Value) -> Option<Self> {
        v.is_object().then(|| Self {
            app: app.to_owned(),
            kind: v["type"].as_u64().unwrap_or(0),
            details: v["details"]
                .as_str()
                .map(|d| one_line(d, MAX_TEXT))
                .filter(|d| !d.is_empty()),
        })
    }

    /// One line for `chad/activity`, naming the game from Discord's list when
    /// we have it.
    pub fn text(&self, games: Option<&crate::presence::Games>) -> String {
        let verb = match self.kind {
            2 => "Listening to",
            3 => "Watching",
            5 => "Competing in",
            _ => "Playing",
        };
        let name = games.and_then(|g| g.name(&self.app));
        let line = match (name, &self.details) {
            (Some(n), Some(d)) => format!("{verb} {n}: {d}"),
            (Some(n), None) => format!("{verb} {n}"),
            (None, Some(d)) => format!("{verb} {d}"),
            (None, None) => format!("{verb} something"),
        };
        line.chars().take(MAX_TEXT).collect()
    }
}

/// The listener while sharing is on. Dropping it stops listening and hangs up
/// on every game, so they find Discord next time they look.
pub struct Rpc {
    /// The most recent activity of any connected game.
    pub rx: watch::Receiver<Option<Activity>>,
    _listener: Option<Listener>,
}

impl Rpc {
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (tx, rx) = watch::channel(None);
        let shared = Arc::new(Shared {
            live: Mutex::default(),
            tx,
            wake: Box::new(wake),
        });
        let listener = listen(shared);
        if listener.is_none() {
            tracing::warn!("rich presence: no free discord-ipc slot");
        }
        Self {
            rx,
            _listener: listener,
        }
    }
}

#[cfg(target_os = "linux")]
use super::rpc_linux::{Listener, listen};
#[cfg(windows)]
use super::rpc_windows::{Listener, listen};

#[cfg(not(any(target_os = "linux", windows)))]
struct Listener;
#[cfg(not(any(target_os = "linux", windows)))]
fn listen(_shared: Arc<Shared>) -> Option<Listener> {
    None
}

pub(super) struct Shared {
    /// (connection, activity), most recently set last.
    live: Mutex<Vec<(u64, Activity)>>,
    tx: watch::Sender<Option<Activity>>,
    wake: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn set(&self, conn: u64, a: Option<Activity>) {
        let mut live = chad_core::lock(&self.live);
        live.retain(|(c, _)| *c != conn);
        live.extend(a.map(|a| (conn, a)));
        let now = live.last().map(|(_, a)| a.clone());
        if self
            .tx
            .send_if_modified(|cur| std::mem::replace(cur, now.clone()) != now)
        {
            (self.wake)();
        }
    }
}

/// The connected games, owned by the accept loop: when it's aborted, this
/// drops and aborts every connection with it.
pub(super) struct Clients {
    shared: Arc<Shared>,
    set: JoinSet<()>,
    next: u64,
}

impl Clients {
    pub(super) fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            set: JoinSet::new(),
            next: 0,
        }
    }

    pub(super) fn add<S>(&mut self, stream: S)
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        while self.set.try_join_next().is_some() {}
        if self.set.len() >= MAX_CLIENTS {
            tracing::warn!("rich presence: {MAX_CLIENTS} clients already, refusing another");
            return;
        }
        let (conn, shared) = (self.next, self.shared.clone());
        self.next += 1;
        self.set.spawn(async move {
            let r = serve(stream, conn, &shared).await;
            tracing::debug!("rich presence client {conn} gone: {r:?}");
            shared.set(conn, None);
        });
    }
}

async fn read_frame<S: AsyncRead + Unpin>(s: &mut S) -> std::io::Result<(u32, Vec<u8>)> {
    let mut head = [0u8; 8];
    s.read_exact(&mut head).await?;
    let op = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
    let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    if len > MAX_FRAME {
        return Err(bad("frame too big"));
    }
    let mut body = vec![0; len];
    s.read_exact(&mut body).await?;
    Ok((op, body))
}

fn bad(m: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, m.to_owned())
}

async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
    mut s: S,
    conn: u64,
    shared: &Shared,
) -> std::io::Result<()> {
    let mut app: Option<String> = None;
    loop {
        let (op, body) = if app.is_none() {
            tokio::time::timeout(HANDSHAKE_WAIT, read_frame(&mut s))
                .await
                .map_err(|_| bad("no handshake"))??
        } else {
            read_frame(&mut s).await?
        };
        match (op, &app) {
            (HANDSHAKE, None) => {
                let v: Value = serde_json::from_slice(&body)?;
                let id = v["client_id"]
                    .as_str()
                    .filter(|id| (1..=20).contains(&id.len()))
                    .filter(|id| id.bytes().all(|b| b.is_ascii_digit()))
                    .ok_or_else(|| bad("no client_id"))?;
                app = Some(id.to_owned());
                write(&mut s, FRAME, &ready()).await?;
            }
            (FRAME, Some(id)) => {
                let v: Value = serde_json::from_slice(&body)?;
                let (cmd, nonce) = (&v["cmd"], &v["nonce"]);
                let reply = if cmd == "SET_ACTIVITY" {
                    let a = &v["args"]["activity"];
                    shared.set(conn, Activity::parse(id, a));
                    json!({ "cmd": cmd, "data": a, "evt": null, "nonce": nonce })
                } else {
                    let data = json!({ "code": 4000, "message": "Not supported" });
                    json!({ "cmd": cmd, "data": data, "evt": "ERROR", "nonce": nonce })
                };
                write(&mut s, FRAME, &reply).await?;
            }
            (PING, _) => {
                let v: Value = serde_json::from_slice(&body)?;
                write(&mut s, PONG, &v).await?;
            }
            (CLOSE, _) => return Ok(()),
            _ => return Err(bad("unexpected opcode")),
        }
    }
}

async fn write<S: AsyncWrite + Unpin>(s: &mut S, op: u32, v: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(v)?;
    let len = u32::try_from(body.len()).map_err(std::io::Error::other)?;
    let mut frame = Vec::with_capacity(8 + body.len());
    frame.extend_from_slice(&op.to_le_bytes());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(&body);
    s.write_all(&frame).await
}

/// Discord's answer to a handshake; SDKs want a user in it.
fn ready() -> Value {
    json!({
        "cmd": "DISPATCH",
        "evt": "READY",
        "nonce": null,
        "data": {
            "v": 1,
            "config": {
                "cdn_host": "cdn.discordapp.com",
                "api_endpoint": "//discord.com/api",
                "environment": "production",
            },
            "user": {
                "id": "0",
                "username": "irchad",
                "discriminator": "0",
                "global_name": "IrChad",
                "avatar": null,
                "bot": false,
                "flags": 0,
                "premium_type": 0,
            },
        },
    })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(in crate::platform) fn shared_with(tx: watch::Sender<Option<Activity>>) -> Arc<Shared> {
        Arc::new(Shared {
            live: Mutex::default(),
            tx,
            wake: Box::new(|| {}),
        })
    }

    fn shared() -> (Arc<Shared>, watch::Receiver<Option<Activity>>) {
        let (tx, rx) = watch::channel(None);
        (shared_with(tx), rx)
    }

    async fn read(s: &mut tokio::io::DuplexStream) -> (u32, Value) {
        let (op, body) = read_frame(s).await.unwrap();
        (op, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn handshake_set_activity_and_clear_on_close() {
        let (shared, mut rx) = shared();
        let mut clients = Clients::new(shared);
        let (mut game, ours) = tokio::io::duplex(4096);
        clients.add(ours);

        write(&mut game, HANDSHAKE, &json!({"v": 1, "client_id": "123"}))
            .await
            .unwrap();
        assert_eq!(read(&mut game).await.1["evt"], "READY");
        let set = json!({"cmd": "SET_ACTIVITY", "nonce": "n1", "args": {"pid": 1, "activity": {
            "details": "Chapter \u{7}3", "state": "Dying", "type": 0}}});
        write(&mut game, FRAME, &set).await.unwrap();
        assert_eq!(read(&mut game).await.1["nonce"], "n1");
        let a = rx.borrow_and_update().clone().unwrap();
        assert_eq!(a.text(None), "Playing Chapter 3");
        let games = crate::presence::Games::new(vec![chad_core::Game {
            id: "123".into(),
            name: "Celeste".into(),
            executables: Vec::new(),
        }]);
        assert_eq!(a.text(Some(&games)), "Playing Celeste: Chapter 3");

        write(&mut game, CLOSE, &json!({})).await.unwrap();
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), None);
    }

    #[tokio::test]
    async fn dropping_clients_hangs_up_on_games() {
        let (shared, _rx) = shared();
        let mut clients = Clients::new(shared);
        let (mut game, ours) = tokio::io::duplex(4096);
        clients.add(ours);
        write(&mut game, HANDSHAKE, &json!({"v": 1, "client_id": "1"}))
            .await
            .unwrap();
        read(&mut game).await;
        drop(clients);
        let mut buf = [0u8; 1];
        assert_eq!(game.read(&mut buf).await.unwrap(), 0, "EOF once dropped");
    }

    #[tokio::test]
    async fn oversized_frame_drops_the_client() {
        let (shared, _rx) = shared();
        let (mut game, ours) = tokio::io::duplex(64);
        let mut head = HANDSHAKE.to_le_bytes().to_vec();
        head.extend_from_slice(&u32::MAX.to_le_bytes());
        game.write_all(&head).await.unwrap();
        assert!(serve(ours, 1, &shared).await.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn silent_client_is_dropped() {
        let (shared, _rx) = shared();
        let (_game, ours) = tokio::io::duplex(64);
        let e = serve(ours, 1, &shared).await.unwrap_err();
        assert_eq!(e.to_string(), "no handshake");
    }
}

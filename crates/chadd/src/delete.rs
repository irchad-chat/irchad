//! Account deletion (D69). `POST /api/account/delete` queues a job file; one
//! worker runs the jobs in turn over a short-lived oper connection to ergo:
//! unregister the account, `HISTSERV EXPORT` its messages, REDACT the channel
//! ones (`HISTSERV DELETE` for anything REDACT refuses), `HISTSERV FORGET` the
//! rest (DMs, which EXPORT leaves out), then delete the uploads it owns. Every
//! exported msgid goes in `redacted.log` first, which `POST /api/redacted`
//! serves so clients drop their local copies too.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use anyhow::{Context, bail};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use chad_irc::Message;
use chad_proto::chadd::{
    ApiError, LoginRequest, REDACTED_PAGE, RedactedRequest, RedactedResponse, valid_msgid,
};
use serde::Deserialize;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout, timeout_at};

use crate::api::{self, ApiResult, fail};
use crate::config::Delete;
use crate::routes::AppState;

/// ergo's lines are at most 8191 bytes of tags plus 512.
const MAX_IRC_LINE: usize = 16 << 10;
/// One exported message: its text, tags and split lines, as JSON.
const MAX_EXPORT_LINE: usize = 1 << 20;
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);
const EXPORT_TIMEOUT: Duration = Duration::from_secs(600);
/// REDACTs fan out to every channel member; this keeps their sendqs safe.
const PACE: Duration = Duration::from_millis(50);
/// Failed jobs are retried this often.
const RETRY: Duration = Duration::from_secs(600);
/// The upload folder xattr naming the account that uploaded it.
const OWNER: &str = "user.chad.owner";

pub async fn delete_account(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let Some(cfg) = &state.config.delete else {
        return Err(fail(
            StatusCode::NOT_FOUND,
            "This server doesn't delete accounts.",
        ));
    };
    let account = match &state.oauth {
        // A Discord account has no password to ask for again (D96).
        Some(o) if headers.contains_key(axum::http::header::AUTHORIZATION) => {
            let account = api::session_account(&state, &headers)?;
            if !o.linked(&account).await {
                return Err(fail(
                    StatusCode::FORBIDDEN,
                    "Enter your password to delete your account.",
                ));
            }
            account
        }
        _ => api::check_password(&state, &headers, &req.account, &req.password).await?,
    };
    // The name is the job: hex keeps any account name a safe file name.
    let job = cfg.state_dir.join("jobs").join(hex::encode(&account));
    if let Err(e) = tokio::fs::write(&job, b"").await {
        tracing::error!("queueing deletion of {account} at {}: {e}", job.display());
        return Err(fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Couldn't delete your account. Try again later.",
        ));
    }
    tracing::info!(%account, "account deletion queued");
    state.deletions.notify_one();
    Ok(StatusCode::ACCEPTED)
}

pub async fn redacted(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<RedactedRequest>,
) -> ApiResult<RedactedResponse> {
    if !state.redacted_throttle.allow(api::client_ip(&headers)) {
        return Err(fail(StatusCode::TOO_MANY_REQUESTS, "Slow down."));
    }
    let ids = state
        .redacted
        .read()
        .unwrap_or_else(PoisonError::into_inner);
    Ok(Json(page(&ids, req.since)))
}

/// No `since`: just the cursor. Otherwise up to a page from there, clamped.
fn page(ids: &[String], since: Option<u64>) -> RedactedResponse {
    let Some(since) = since else {
        return RedactedResponse {
            ids: Vec::new(),
            next: ids.len() as u64,
        };
    };
    let start = usize::try_from(since).unwrap_or(usize::MAX).min(ids.len());
    let end = (start + REDACTED_PAGE).min(ids.len());
    RedactedResponse {
        ids: ids[start..end].to_vec(),
        next: end as u64,
    }
}

/// `redacted.log`, skipping anything that isn't a msgid.
pub fn load_redacted(cfg: Option<&Delete>) -> Vec<String> {
    let Some(cfg) = cfg else { return Vec::new() };
    match std::fs::read_to_string(cfg.state_dir.join("redacted.log")) {
        Ok(s) => s
            .lines()
            .filter(|l| valid_msgid(l))
            .map(str::to_owned)
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            tracing::error!("reading redacted.log: {e}");
            Vec::new()
        }
    }
}

#[cfg(target_os = "linux")]
pub fn set_owner(dir: &Path, account: &str) -> std::io::Result<()> {
    Ok(rustix::fs::setxattr(
        dir,
        OWNER,
        account.as_bytes(),
        rustix::fs::XattrFlags::empty(),
    )?)
}

#[cfg(not(target_os = "linux"))]
pub fn set_owner(_dir: &Path, _account: &str) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

#[cfg(target_os = "linux")]
fn owner(dir: &Path) -> Option<String> {
    let mut buf = [0u8; 256];
    let n = rustix::fs::getxattr(dir, OWNER, &mut buf[..]).ok()?;
    String::from_utf8(buf.get(..n)?.to_vec()).ok()
}

#[cfg(not(target_os = "linux"))]
fn owner(_dir: &Path) -> Option<String> {
    None
}

/// Deletes every upload folder `account` owns; how many went.
fn delete_uploads(dir: &Path, account: &str) -> std::io::Result<usize> {
    let account = account.to_lowercase();
    let mut n = 0;
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if owner(&path).is_some_and(|o| o.to_lowercase() == account) {
            std::fs::remove_dir_all(&path)?;
            n += 1;
        }
    }
    Ok(n)
}

/// Runs queued jobs, then waits for more. A failed job stays queued.
pub async fn worker(state: Arc<AppState>) {
    let Some(cfg) = &state.config.delete else {
        return;
    };
    let jobs = cfg.state_dir.join("jobs");
    if let Err(e) = tokio::fs::create_dir_all(&jobs).await {
        tracing::error!("account deletion is off: creating {}: {e}", jobs.display());
        return;
    }
    loop {
        for (path, account) in queued(&jobs).await {
            match run(&state, cfg, &account).await {
                Ok(()) => {
                    if let Err(e) = tokio::fs::remove_file(&path).await {
                        tracing::error!("removing {}: {e}", path.display());
                    }
                    tracing::info!(%account, "account deleted");
                }
                Err(e) => tracing::error!(%account, "deleting account (retrying later): {e:#}"),
            }
        }
        tokio::select! {
            () = state.deletions.notified() => {}
            () = tokio::time::sleep(RETRY) => {}
        }
    }
}

async fn queued(jobs: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let Ok(mut dir) = tokio::fs::read_dir(jobs).await else {
        return out;
    };
    while let Ok(Some(e)) = dir.next_entry().await {
        let name = e.file_name();
        let account = name
            .to_str()
            .and_then(|n| hex::decode(n).ok())
            .and_then(|b| String::from_utf8(b).ok());
        match account {
            Some(a) if !a.is_empty() && a.len() <= 64 && !a.contains([' ', '\r', '\n', '\0']) => {
                out.push((e.path(), a));
            }
            _ => tracing::warn!("ignoring job file {}", e.path().display()),
        }
    }
    out
}

async fn run(state: &AppState, cfg: &Delete, account: &str) -> anyhow::Result<()> {
    let mut irc = Irc::connect(cfg).await?;
    unregister(&mut irc, account).await?;
    if let Some(o) = &state.oauth {
        o.unlink(account).await;
    }
    let file = export(&mut irc, account).await?;
    let path = cfg.export_dir.join(&file);
    let messages = read_export(&path).await?;
    redact(state, cfg, &mut irc, messages).await?;
    forget(&mut irc, account).await?;
    let _ = irc.send(Message::new("QUIT", ["deleted"])).await;
    tokio::fs::remove_file(&path)
        .await
        .with_context(|| format!("removing {}", path.display()))?;
    if let Some(u) = &state.config.uploads {
        let (dir, account) = (u.dir.clone(), account.to_owned());
        let n = tokio::task::spawn_blocking(move || delete_uploads(&dir, &account)).await??;
        tracing::info!(n, "deleted uploads");
    }
    Ok(())
}

/// An oper connection. Lines arrive through a task, so reads are cancel-safe.
struct Irc {
    rx: mpsc::Receiver<Message>,
    w: OwnedWriteHalf,
}

impl Irc {
    async fn connect(cfg: &Delete) -> anyhow::Result<Self> {
        let sock = timeout(REPLY_TIMEOUT, tokio::net::TcpStream::connect(&cfg.irc))
            .await
            .context("connecting to ergo")??;
        let (r, w) = sock.into_split();
        let (tx, rx) = mpsc::channel(256);
        tokio::spawn(async move {
            let mut r = BufReader::new(r);
            while let Ok(Some(line)) = read_line(&mut r, MAX_IRC_LINE).await {
                let Ok(m) = std::str::from_utf8(&line).map(Message::parse) else {
                    continue;
                };
                if let Ok(m) = m
                    && tx.send(m).await.is_err()
                {
                    break;
                }
            }
        });
        let mut irc = Self { rx, w };
        let mut nick = [0u8; 4];
        getrandom::fill(&mut nick).map_err(|e| anyhow::anyhow!("getrandom: {e}"))?;
        let nick = format!("chadd-{}", hex::encode(nick));
        irc.send(Message::new("NICK", [nick.as_str()])).await?;
        irc.send(Message::new("USER", ["chadd", "0", "*", "chadd"]))
            .await?;
        let deadline = Instant::now() + REPLY_TIMEOUT;
        loop {
            let m = irc.recv(deadline).await?;
            match m.command.as_str() {
                "001" => break,
                "432" | "433" => bail!("ergo refused nick {nick}"),
                _ => {}
            }
        }
        irc.send(Message::new(
            "OPER",
            [cfg.oper.as_str(), cfg.oper_password.as_str()],
        ))
        .await?;
        loop {
            let m = irc.recv(deadline).await?;
            match m.command.as_str() {
                "381" => return Ok(irc),
                "464" | "491" => bail!("OPER {} refused", cfg.oper),
                _ => {}
            }
        }
    }

    async fn send(&mut self, m: Message) -> anyhow::Result<()> {
        let mut line = m.to_line()?;
        line.push_str("\r\n");
        self.w.write_all(line.as_bytes()).await?;
        Ok(())
    }

    /// The next message that isn't a PING. Fails at `deadline`, or on ERROR.
    async fn recv(&mut self, deadline: Instant) -> anyhow::Result<Message> {
        loop {
            let m = timeout_at(deadline, self.rx.recv())
                .await
                .context("ergo didn't answer in time")?
                .context("ergo closed the connection")?;
            tracing::debug!(command = %m.command, params = ?m.params, "from ergo");
            match m.command.as_str() {
                "PING" => {
                    let token = m.param(0).unwrap_or_default().to_owned();
                    self.send(Message::new("PONG", [token.as_str()])).await?;
                }
                "ERROR" => bail!("ergo: {}", m.param(0).unwrap_or_default()),
                _ => return Ok(m),
            }
        }
    }

    /// The text of the next NOTICE from `service`.
    async fn notice(&mut self, service: &str, deadline: Instant) -> anyhow::Result<String> {
        loop {
            let m = self.recv(deadline).await?;
            if m.command == "NOTICE"
                && m.source_nick()
                    .is_some_and(|n| n.eq_ignore_ascii_case(service))
                && let Some(text) = m.param(1)
            {
                return Ok(text.to_owned());
            }
        }
    }
}

/// One line, at most `max` bytes. A longer line is skipped and comes back
/// empty. `None` at end of input.
async fn read_line<R: AsyncBufRead + Unpin>(
    r: &mut R,
    max: usize,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    let n = (&mut *r)
        .take(max as u64)
        .read_until(b'\n', &mut buf)
        .await?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() != Some(&b'\n') && buf.len() == max {
        loop {
            let chunk = r.fill_buf().await?;
            if chunk.is_empty() {
                break;
            }
            if let Some(i) = chunk.iter().position(|&b| b == b'\n') {
                r.consume(i + 1);
                break;
            }
            let len = chunk.len();
            r.consume(len);
        }
        buf.clear();
    }
    Ok(Some(buf))
}

/// ergo's two-step `UNREGISTER`, as an oper with `accreg`.
async fn unregister(irc: &mut Irc, account: &str) -> anyhow::Result<()> {
    let ask = format!("UNREGISTER {account}");
    irc.send(Message::new("PRIVMSG", ["NickServ", ask.as_str()]))
        .await?;
    let deadline = Instant::now() + REPLY_TIMEOUT;
    loop {
        let text = irc.notice("NickServ", deadline).await?;
        if text.starts_with("Successfully unregistered account") {
            return Ok(());
        }
        // A job retried after the account went.
        if text == "Invalid account name" || text == "Account does not exist" {
            tracing::info!(%account, "already unregistered");
            return Ok(());
        }
        if let Some(code) = confirm_code(&text, account) {
            let confirm = format!("UNREGISTER {account} {code}");
            irc.send(Message::new("PRIVMSG", ["NickServ", confirm.as_str()]))
                .await?;
        } else if text.contains("privs") || text.starts_with("Error") {
            bail!("NickServ: {text}");
        }
    }
}

/// The code in "To confirm, run this command: /NS UNREGISTER <account> <code>".
fn confirm_code<'a>(text: &'a str, account: &str) -> Option<&'a str> {
    let cmd = text.strip_prefix("To confirm, run this command: ")?;
    let mut w = cmd
        .split(' ')
        .skip_while(|w| !w.eq_ignore_ascii_case("UNREGISTER"))
        .skip(1);
    let (name, code) = (w.next()?, w.next()?);
    let ok = name.eq_ignore_ascii_case(account)
        && (1..=32).contains(&code.len())
        && code.bytes().all(|b| b.is_ascii_alphanumeric());
    ok.then_some(code)
}

/// Asks for an export and waits for its file name.
async fn export(irc: &mut Irc, account: &str) -> anyhow::Result<String> {
    let ask = format!("EXPORT {account}");
    irc.send(Message::new("PRIVMSG", ["HistServ", ask.as_str()]))
        .await?;
    let deadline = Instant::now() + EXPORT_TIMEOUT;
    loop {
        let text = irc.notice("HistServ", deadline).await?;
        if text.starts_with("Started exporting") {
            continue;
        }
        if text.starts_with("Data export for")
            && let Some(file) = text.rsplit(' ').next()
            && export_name(file)
        {
            return Ok(file.to_owned());
        }
        bail!("HistServ: {text}");
    }
}

/// What ergo names exports (`<token>-<timestamp>.json`): no way out of the dir.
fn export_name(f: &str) -> bool {
    f.len() <= 128
        && f.strip_suffix(".json").is_some_and(|stem| {
            !stem.is_empty()
                && !stem.starts_with('.')
                && stem
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
        })
}

/// One exported `history.Item`, only the parts we need.
#[derive(Deserialize)]
struct Item {
    /// ergo's `history.ItemType`.
    #[serde(rename = "Type", default)]
    kind: u8,
    #[serde(rename = "Message")]
    message: ItemMessage,
    #[serde(rename = "CfCorrespondent", default)]
    target: String,
}

#[derive(Deserialize)]
struct ItemMessage {
    #[serde(rename = "Msgid", default)]
    msgid: String,
}

/// `(target, msgid)` per exported message, deduplicated and checked.
async fn read_export(path: &Path) -> anyhow::Result<Vec<(String, String)>> {
    let f = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("opening {}", path.display()))?;
    let mut r = BufReader::new(f);
    let (mut out, mut seen) = (Vec::new(), HashSet::new());
    while let Some(line) = read_line(&mut r, MAX_EXPORT_LINE).await? {
        if let Some(m) = parse_item(&line)
            && seen.insert(m.1.clone())
        {
            out.push(m);
        }
    }
    Ok(out)
}

fn parse_item(line: &[u8]) -> Option<(String, String)> {
    let item: Item = serde_json::from_slice(line).ok()?;
    // PRIVMSG, NOTICE and TAGMSG (reactions); joins, topics and the like aren't messages.
    let message = matches!(item.kind, 1 | 2 | 8);
    (message && valid_target(&item.target) && valid_msgid(&item.message.msgid))
        .then_some((item.target, item.message.msgid))
}

/// A channel or nick that's safe as one IRC param (ergo allows UTF-8 names).
fn valid_target(t: &str) -> bool {
    (1..=200).contains(&t.len())
        && !t.starts_with(':')
        && t.bytes().all(|b| b.is_ascii_graphic() || b >= 0x80)
}

/// Tombstones each msgid, then REDACTs it (channels) or deletes it from
/// history (DMs, and whatever REDACT refuses), paced network-wide.
async fn redact(
    state: &AppState,
    cfg: &Delete,
    irc: &mut Irc,
    messages: Vec<(String, String)>,
) -> anyhow::Result<()> {
    let log_path = cfg.state_dir.join("redacted.log");
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("opening {}", log_path.display()))?;
    let mut queue: std::collections::VecDeque<Message> = std::collections::VecDeque::new();
    let mut messages = messages.into_iter();
    let mut tick = tokio::time::interval(PACE);
    let total = messages.len();
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if let Some(m) = queue.pop_front() {
                    irc.send(m).await?;
                    continue;
                }
                let Some((target, msgid)) = messages.next() else { break };
                writeln!(log, "{msgid}").context("writing redacted.log")?;
                state.redacted.write().unwrap_or_else(PoisonError::into_inner).push(msgid.clone());
                irc.send(if target.starts_with('#') {
                    Message::new("REDACT", [target.as_str(), msgid.as_str()])
                } else {
                    hs_delete(&target, &msgid)
                })
                .await?;
            }
            m = irc.recv(Instant::now() + REPLY_TIMEOUT) => {
                if let Some(m) = fallback(&m?) {
                    queue.push_back(m);
                }
            }
        }
    }
    // Late FAILs for the last REDACTs.
    let deadline = Instant::now() + Duration::from_secs(3);
    while let Ok(m) = irc.recv(deadline).await {
        if let Some(m) = fallback(&m) {
            irc.send(m).await?;
        }
    }
    tracing::info!(total, "redacted messages");
    Ok(())
}

/// Deletes what's left of the account's history: its DMs, which ergo's EXPORT
/// leaves out (it only joins channel history), so they get no REDACT or tombstone.
async fn forget(irc: &mut Irc, account: &str) -> anyhow::Result<()> {
    let ask = format!("FORGET {account}");
    irc.send(Message::new("PRIVMSG", ["HistServ", ask.as_str()]))
        .await?;
    let deadline = Instant::now() + REPLY_TIMEOUT;
    loop {
        let text = irc.notice("HistServ", deadline).await?;
        if text.starts_with("Enqueued account") {
            return Ok(());
        }
        // Expected: the account is already unregistered.
        if !text.starts_with("Could not look up account name") {
            bail!("HistServ: {text}");
        }
    }
}

fn hs_delete(target: &str, msgid: &str) -> Message {
    let text = format!("DELETE {target} {msgid}");
    Message::new("PRIVMSG", ["HistServ", text.as_str()])
}

/// `FAIL REDACT <code> <target> <msgid> :…`: delete it from history instead.
fn fallback(m: &Message) -> Option<Message> {
    if m.command != "FAIL" || m.param(0) != Some("REDACT") {
        return None;
    }
    let (target, msgid) = (m.param(2)?, m.param(3)?);
    (valid_target(target) && valid_msgid(msgid)).then(|| hs_delete(target, msgid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages() {
        let ids: Vec<String> = (0..2500).map(|i| format!("m{i}")).collect();
        let p = page(&ids, None);
        assert!(p.ids.is_empty());
        assert_eq!(p.next, 2500);
        let p = page(&ids, Some(0));
        assert_eq!((p.ids.len(), p.next), (REDACTED_PAGE, 1000));
        assert_eq!(p.ids[0], "m0");
        let p = page(&ids, Some(2000));
        assert_eq!((p.ids.len(), p.next), (500, 2500));
        let p = page(&ids, Some(u64::MAX));
        assert_eq!((p.ids.len(), p.next), (0, 2500));
    }

    #[test]
    fn confirm_codes() {
        let t = "To confirm, run this command: /NS UNREGISTER Alice 5zrg8";
        assert_eq!(confirm_code(t, "alice"), Some("5zrg8"));
        assert_eq!(confirm_code(t, "bob"), None);
        assert_eq!(
            confirm_code(
                "To confirm, run this command: /NS UNREGISTER alice ../x",
                "alice"
            ),
            None
        );
        assert_eq!(confirm_code("Warning: unregistering", "alice"), None);
    }

    #[test]
    fn export_names() {
        assert!(export_name("abc123-2026-11-07T10:00:00.000Z.json"));
        assert!(!export_name("../etc/passwd.json"));
        assert!(!export_name("a/b.json"));
        assert!(!export_name(".json"));
        assert!(!export_name("abc.txt"));
    }

    #[test]
    fn export_items() {
        let ok = br##"{"Type":1,"Message":{"Message":"hi","Msgid":"abc123"},"CfCorrespondent":"#general"}"##;
        assert_eq!(parse_item(ok), Some(("#general".into(), "abc123".into())));
        let join = br##"{"Type":3,"Message":{"Msgid":"j1"},"CfCorrespondent":"#general"}"##;
        assert_eq!(parse_item(join), None);
        assert_eq!(
            parse_item(br#"{"Type":1,"Message":{"Msgid":"a b"},"CfCorrespondent":"bob"}"#),
            None
        );
        assert_eq!(
            parse_item(br#"{"Type":1,"Message":{"Msgid":"x"},"CfCorrespondent":":bob"}"#),
            None
        );
        assert_eq!(
            parse_item(br#"{"Type":1,"Message":{},"CfCorrespondent":"bob"}"#),
            None
        );
        assert_eq!(parse_item(b"not json"), None);
    }

    #[test]
    fn falls_back_to_histserv() {
        let m = Message::parse("FAIL REDACT UNKNOWN_MSGID #c abc :no").unwrap();
        let d = fallback(&m).unwrap();
        assert_eq!(d.params, ["HistServ", "DELETE #c abc"]);
        assert!(fallback(&Message::parse("FAIL REDACT X #c a:b :no").unwrap()).is_none());
        assert!(fallback(&Message::parse("NOTICE x :hi").unwrap()).is_none());
    }

    #[tokio::test]
    async fn long_lines_are_skipped() {
        let input = format!("short\n{}\nafter\n", "x".repeat(100));
        let mut r = BufReader::new(input.as_bytes());
        assert_eq!(read_line(&mut r, 16).await.unwrap().unwrap(), b"short\n");
        assert_eq!(read_line(&mut r, 16).await.unwrap().unwrap(), b"");
        assert_eq!(read_line(&mut r, 16).await.unwrap().unwrap(), b"after\n");
        assert!(read_line(&mut r, 16).await.unwrap().is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn deletes_only_owned_uploads() {
        let dir = tempfile::tempdir().unwrap();
        for (id, who) in [("a", "Alice"), ("b", "bob"), ("c", "alice")] {
            let d = dir.path().join(id);
            std::fs::create_dir(&d).unwrap();
            std::fs::write(d.join("f.png"), b"x").unwrap();
            set_owner(&d, who).unwrap();
        }
        std::fs::create_dir(dir.path().join("unowned")).unwrap();
        assert_eq!(delete_uploads(dir.path(), "ALICE").unwrap(), 2);
        let mut left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["b", "unowned"]);
    }
}

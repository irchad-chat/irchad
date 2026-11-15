//! "Sign in with Discord" (D96). chadd is the Discord OAuth2 client; Discord
//! only tells it who someone is. Which Discord user owns which account lives in
//! `discord.json`, and ergo only ever sees chadd's own JWTs (`accounts.jwt-auth`,
//! SASL `IRCV3BEARER`), so ergo stays the account authority.
//!
//! Browser: `start` → Discord → `callback` → `irchad://oauth?origin&code`.
//! App: `exchange` (code + PKCE verifier) → `register` if there's no account
//! yet → `refresh` before every connect.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64URL};
use chad_proto::chadd::{
    self as proto, ApiError, OAuthExchangeRequest, OAuthOutcome, OAuthRegisterRequest,
    PasswordRequest, RefreshRequest, RefreshResponse,
};
use chad_proto::discovery::Registration;
use ed25519_dalek::{Signer as _, SigningKey};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::api::{self, ApiResult, fail};
use crate::config::Discord;
use crate::routes::AppState;

/// The `aud` ergo's `jwt-auth` is told to accept.
const AUDIENCE: &str = "irchad-ergo";
/// IRC JWTs are fetched right before each connect; ergo can't stop a replay
/// before `exp`, so keep it short.
const IRC_TTL: Duration = Duration::from_secs(60);
/// Slides: every refresh hands out a new one.
const REFRESH_TTL: Duration = Duration::from_secs(90 * 24 * 3600);
const TICKET_TTL: Duration = Duration::from_secs(15 * 60);
/// From `start` to Discord's callback: someone reading Discord's consent page.
const PENDING_TTL: Duration = Duration::from_secs(10 * 60);
/// From the callback to the app's `exchange`: just the app waking up.
const CODE_TTL: Duration = Duration::from_secs(2 * 60);
/// Unfinished sign-ins kept at once, so nobody can fill chadd's memory.
const MAX_PENDING: usize = 10_000;
/// Discord's answers are a few hundred bytes (D14).
const MAX_DISCORD_BODY: usize = 64 << 10;
/// What ergo's `CasefoldName` refuses anywhere in a name.
const BAD_CHARS: &[char] = &[' ', ',', '*', '?', '.', '!', '@', ':'];
/// The longest account name; ergo's `nicklen` (ergo.yaml) is the same.
const MAX_ACCOUNT: usize = 32;

type HmacSha256 = Hmac<Sha256>;

pub struct OAuth {
    client_id: String,
    secret: String,
    api: Url,
    callback: Url,
    signing: SigningKey,
    refresh_key: [u8; 32],
    map_path: PathBuf,
    /// Discord user id → account. One lock: registrations are rare.
    map: tokio::sync::Mutex<HashMap<String, Link>>,
    /// `state` → the sign-in it belongs to.
    pending: Mutex<HashMap<String, Pending>>,
    /// One-time codes handed to the app.
    codes: Mutex<HashMap<String, Code>>,
    /// Per IP, across all of these endpoints.
    pub throttle: api::Throttle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Link {
    account: String,
    /// ergo's `registeredAt` when the link was made: a different one means the
    /// name was dropped and taken by someone else.
    registered_at: String,
    /// Bumped to sign every device out.
    #[serde(default)]
    generation: u64,
}

struct Pending {
    discord_verifier: String,
    /// The app's PKCE challenge, checked at `exchange`.
    challenge: String,
    at: Instant,
}

struct Code {
    who: Who,
    challenge: String,
    at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Who {
    id: String,
    username: String,
}

/// What a ticket or refresh token says, signed with `refresh_key`.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum Claims {
    Ticket {
        who: Who,
        exp: u64,
    },
    Refresh {
        id: String,
        generation: u64,
        exp: u64,
    },
}

impl OAuth {
    pub fn load(cfg: &Discord, api_base: &Url) -> anyhow::Result<Self> {
        let secret = std::fs::read_to_string(&cfg.client_secret_file)
            .with_context(|| format!("reading {}", cfg.client_secret_file.display()))?
            .trim()
            .to_owned();
        anyhow::ensure!(!secret.is_empty(), "discord.client_secret_file is empty");
        let pem = std::fs::read_to_string(&cfg.signing_key)
            .with_context(|| format!("reading {}", cfg.signing_key.display()))?;
        let signing = SigningKey::from_bytes(&ed25519_seed(&pem)?);
        std::fs::create_dir_all(&cfg.state_dir)
            .with_context(|| format!("creating {}", cfg.state_dir.display()))?;
        let refresh_key = load_or_make_key(&cfg.state_dir.join("refresh.key"))?;
        let map_path = cfg.state_dir.join("discord.json");
        let map = match std::fs::read(&map_path) {
            Ok(b) => serde_json::from_slice(&b)
                .with_context(|| format!("parsing {}", map_path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", map_path.display())),
        };
        Ok(Self {
            client_id: cfg.client_id.clone(),
            secret,
            api: cfg.api.clone(),
            callback: Url::parse(&proto::endpoint(api_base, "oauth/discord/callback"))?,
            signing,
            refresh_key,
            map_path,
            map: tokio::sync::Mutex::new(map),
            pending: Mutex::default(),
            codes: Mutex::default(),
            throttle: api::Throttle::new(30),
        })
    }

    fn sign(&self, claims: &Claims) -> String {
        let body = B64URL.encode(serde_json::to_vec(claims).expect("plain struct"));
        let mut mac = HmacSha256::new_from_slice(&self.refresh_key).expect("any key length");
        mac.update(body.as_bytes());
        format!("{body}.{}", B64URL.encode(mac.finalize().into_bytes()))
    }

    /// The claims, if we signed them and they haven't expired.
    fn verify(&self, token: &str) -> Option<Claims> {
        if token.len() > 2048 {
            return None;
        }
        let (body, sig) = token.split_once('.')?;
        let mut mac = HmacSha256::new_from_slice(&self.refresh_key).expect("any key length");
        mac.update(body.as_bytes());
        mac.verify_slice(&B64URL.decode(sig).ok()?).ok()?;
        let claims: Claims = serde_json::from_slice(&B64URL.decode(body).ok()?).ok()?;
        let exp = match &claims {
            Claims::Ticket { exp, .. } | Claims::Refresh { exp, .. } => *exp,
        };
        (exp > now()).then_some(claims)
    }

    fn refresh_token(&self, id: &str, generation: u64) -> String {
        self.sign(&Claims::Refresh {
            id: id.to_owned(),
            generation,
            exp: now() + REFRESH_TTL.as_secs(),
        })
    }

    /// An EdDSA JWT for ergo's `jwt-auth`: `account` names the account.
    fn irc_token(&self, account: &str) -> String {
        let header = B64URL.encode(br#"{"alg":"EdDSA","typ":"JWT"}"#);
        let claims = serde_json::json!({
            "account": account,
            "aud": AUDIENCE,
            "exp": now() + IRC_TTL.as_secs(),
        });
        let payload = B64URL.encode(claims.to_string());
        let signed = format!("{header}.{payload}");
        let sig = self.signing.sign(signed.as_bytes());
        format!("{signed}.{}", B64URL.encode(sig.to_bytes()))
    }

    async fn save(&self, map: &HashMap<String, Link>) -> Result<(), (StatusCode, Json<ApiError>)> {
        let tmp = self.map_path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(map).expect("plain map");
        let res = async {
            tokio::fs::write(&tmp, bytes).await?;
            tokio::fs::rename(&tmp, &self.map_path).await
        };
        res.await.map_err(|e| {
            tracing::error!("saving {}: {e}", self.map_path.display());
            fail(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong.")
        })
    }

    /// Forgets every link to `account`; the deletion worker calls it (D69).
    pub async fn unlink(&self, account: &str) {
        let mut map = self.map.lock().await;
        let before = map.len();
        map.retain(|_, l| !l.account.eq_ignore_ascii_case(account));
        if map.len() != before && self.save(&map).await.is_err() {
            tracing::error!(%account, "couldn't drop its Discord link");
        }
    }

    /// Whether `account` signs in with Discord.
    pub async fn linked(&self, account: &str) -> bool {
        let map = self.map.lock().await;
        map.values()
            .any(|l| l.account.eq_ignore_ascii_case(account))
    }
}

/// The 32-byte seed in an Ed25519 PKCS#8 PEM, as `openssl genpkey` writes it.
fn ed25519_seed(pem: &str) -> anyhow::Result<[u8; 32]> {
    // SEQUENCE { INTEGER 0, SEQUENCE { OID 1.3.101.112 }, OCTET STRING { OCTET STRING (32) } }
    const PREFIX: [u8; 16] = [
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
        0x20,
    ];
    let b64: String = pem
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("-----"))
        .collect();
    let der = B64.decode(b64).context("signing_key isn't PEM")?;
    der.strip_prefix(&PREFIX[..])
        .and_then(|seed| seed.try_into().ok())
        .context("signing_key isn't an Ed25519 PKCS#8 private key")
}

fn load_or_make_key(path: &Path) -> anyhow::Result<[u8; 32]> {
    match std::fs::read(path) {
        Ok(b) => b
            .try_into()
            .map_err(|_| anyhow::anyhow!("{} isn't 32 bytes", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = random::<32>();
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
            std::io::Write::write_all(&mut opts.open(path)?, &key)
                .with_context(|| format!("writing {}", path.display()))?;
            Ok(key)
        }
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("the OS has randomness");
    b
}

fn token() -> String {
    B64URL.encode(random::<32>())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// PKCE S256 (RFC 7636).
pub fn challenge(verifier: &str) -> String {
    B64URL.encode(Sha256::digest(verifier.as_bytes()))
}

/// A PKCE value as RFC 7636 allows: 43 to 128 of `A-Z a-z 0-9 - . _ ~`.
fn pkce_ok(s: &str) -> bool {
    (43..=128).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

/// A first guess at an account name from a Discord username: ergo refuses `.`.
pub fn suggest(username: &str) -> String {
    let s: String = username
        .chars()
        .filter_map(|c| match c {
            '.' => Some('_'),
            c if c.is_ascii_alphanumeric() || c == '_' => Some(c.to_ascii_lowercase()),
            _ => None,
        })
        .take(MAX_ACCOUNT)
        .collect();
    if s.is_empty() { "chad".into() } else { s }
}

/// What ergo's `CasefoldName` would take, kept to ASCII so the name looks the
/// same everywhere. ergo still refuses reserved names and lookalikes itself.
pub fn valid_account(name: &str) -> bool {
    (1..=MAX_ACCOUNT).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_graphic() && !BAD_CHARS.contains(&c))
        && !name.starts_with(['#', '~', '&', '@', '%', '+', '-'])
}

fn oauth(state: &AppState) -> Result<&OAuth, (StatusCode, Json<ApiError>)> {
    state.oauth.as_ref().ok_or_else(|| {
        fail(
            StatusCode::NOT_FOUND,
            "This server doesn't sign in with Discord.",
        )
    })
}

fn throttled(o: &OAuth, headers: &HeaderMap) -> Result<(), (StatusCode, Json<ApiError>)> {
    if o.throttle.allow(api::client_ip(headers)) {
        Ok(())
    } else {
        Err(fail(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many tries. Wait a minute and try again.",
        ))
    }
}

/// Drops entries older than `ttl`; false if it's still full.
fn prune<T>(map: &mut HashMap<String, T>, at: impl Fn(&T) -> Instant, ttl: Duration) -> bool {
    map.retain(|_, v| at(v).elapsed() < ttl);
    map.len() < MAX_PENDING
}

#[derive(Deserialize)]
pub struct StartQuery {
    challenge: String,
}

pub async fn start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<StartQuery>,
) -> Response {
    let o = match oauth(&state) {
        Ok(o) => o,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = throttled(o, &headers) {
        return e.into_response();
    }
    if !pkce_ok(&q.challenge) {
        return fail(StatusCode::BAD_REQUEST, "Bad sign-in link.").into_response();
    }
    let (state_id, verifier) = (token(), token());
    {
        let mut pending = crate::lock(&o.pending);
        if !prune(&mut pending, |p| p.at, PENDING_TTL) {
            return fail(StatusCode::SERVICE_UNAVAILABLE, "Busy. Try again soon.").into_response();
        }
        pending.insert(
            state_id.clone(),
            Pending {
                discord_verifier: verifier.clone(),
                challenge: q.challenge,
                at: Instant::now(),
            },
        );
    }
    let mut url = Url::parse("https://discord.com/oauth2/authorize").expect("static URL");
    url.query_pairs_mut()
        .append_pair("client_id", &o.client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", o.callback.as_str())
        .append_pair("scope", "identify")
        .append_pair("state", &state_id)
        .append_pair("code_challenge", &challenge(&verifier))
        .append_pair("code_challenge_method", "S256");
    tracing::info!("Discord sign-in started");
    Redirect::to(url.as_str()).into_response()
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// Always a page sending the browser on to the app, with a code or an error.
pub async fn callback(
    State(state): State<Arc<AppState>>,
    Query(q): Query<CallbackQuery>,
) -> Html<String> {
    let origin = state.config.origin.origin().ascii_serialization();
    let mut back = Url::parse("irchad://oauth").expect("static URL");
    back.query_pairs_mut().append_pair("origin", &origin);
    match finish(&state, q).await {
        Ok(code) => {
            tracing::info!("Discord sign-in: back to the app");
            back.query_pairs_mut().append_pair("code", &code);
        }
        Err(error) => {
            tracing::warn!(error, "Discord sign-in failed");
            back.query_pairs_mut().append_pair("error", error);
        }
    }
    Html(back_to_app(back.as_str()))
}

async fn finish(state: &AppState, q: CallbackQuery) -> Result<String, &'static str> {
    let o = state
        .oauth
        .as_ref()
        .ok_or("This server doesn't sign in with Discord.")?;
    let expired = "That sign-in took too long. Try again.";
    let pending = {
        let id = q.state.as_deref().ok_or(expired)?;
        crate::lock(&o.pending).remove(id).ok_or(expired)?
    };
    if pending.at.elapsed() > PENDING_TTL {
        return Err(expired);
    }
    if q.error.is_some() {
        return Err("Discord sign-in was cancelled.");
    }
    let code = q
        .code
        .filter(|c| (1..=512).contains(&c.len()))
        .ok_or(expired)?;
    let who = identify(state, o, &code, &pending.discord_verifier).await?;
    let mut codes = crate::lock(&o.codes);
    if !prune(&mut codes, |c| c.at, CODE_TTL) {
        return Err("Busy. Try again soon.");
    }
    let ours = token();
    codes.insert(
        ours.clone(),
        Code {
            who,
            challenge: pending.challenge,
            at: Instant::now(),
        },
    );
    Ok(ours)
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct Me {
    id: String,
    username: String,
}

/// Swaps Discord's code for who it is, then lets go of Discord's token.
async fn identify(
    state: &AppState,
    o: &OAuth,
    code: &str,
    verifier: &str,
) -> Result<Who, &'static str> {
    let trouble = "Couldn't reach Discord. Try again in a minute.";
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", o.callback.as_str()),
        ("code_verifier", verifier),
    ];
    let res = state
        .http
        .post(o.api.join("oauth2/token").map_err(|_| trouble)?)
        .basic_auth(&o.client_id, Some(&o.secret))
        .form(&form)
        .send()
        .await;
    let token: TokenResponse = discord_json(res, "token").await.ok_or(trouble)?;
    let res = state
        .http
        .get(o.api.join("users/@me").map_err(|_| trouble)?)
        .bearer_auth(&token.access_token)
        .send()
        .await;
    let me: Option<Me> = discord_json(res, "users/@me").await;
    // We only needed it once (D96): no Discord tokens are kept.
    let revoke = state
        .http
        .post(o.api.join("oauth2/token/revoke").map_err(|_| trouble)?)
        .basic_auth(&o.client_id, Some(&o.secret))
        .form(&[
            ("token", token.access_token.as_str()),
            ("token_type_hint", "access_token"),
        ]);
    tokio::spawn(async move {
        if let Err(e) = revoke.send().await {
            tracing::warn!("revoking a Discord token: {e}");
        }
    });
    let me = me.ok_or(trouble)?;
    // A snowflake, and a username as Discord allows them (D14).
    let ok = (1..=20).contains(&me.id.len())
        && me.id.bytes().all(|b| b.is_ascii_digit())
        && (1..=32).contains(&me.username.len())
        && me.username.bytes().all(|b| b.is_ascii_graphic());
    ok.then_some(Who {
        id: me.id,
        username: me.username,
    })
    .ok_or(trouble)
}

/// A capped JSON body from Discord, or `None` (logged). A 429 isn't retried:
/// the user is waiting, and Discord bans IPs that keep knocking.
async fn discord_json<T: serde::de::DeserializeOwned>(
    res: reqwest::Result<reqwest::Response>,
    what: &str,
) -> Option<T> {
    let mut res = res
        .map_err(|e| tracing::warn!("Discord {what}: {e}"))
        .ok()?;
    let status = res.status();
    let body = crate::fetch::capped(&mut res, MAX_DISCORD_BODY)
        .await
        .map_err(|e| tracing::warn!("Discord {what}: {e}"))
        .ok()?;
    if !status.is_success() {
        tracing::warn!("Discord {what}: HTTP {status}");
        return None;
    }
    serde_json::from_slice(&body)
        .map_err(|e| tracing::warn!("Discord {what}: {e}"))
        .ok()
}

fn back_to_app(url: &str) -> String {
    let url = crate::routes::escape(url);
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="refresh" content="0; url={url}">
<title>IrChad</title>
<style>body{{font:16px system-ui,sans-serif;margin:3em auto;max-width:28em;padding:0 1em;text-align:center}}</style>
</head>
<body>
<p>Back to IrChad…</p>
<p><a href="{url}">Open IrChad</a> if it didn't open by itself. You can close this tab.</p>
</body>
</html>
"#
    )
}

pub async fn exchange(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<OAuthExchangeRequest>,
) -> ApiResult<OAuthOutcome> {
    let o = oauth(&state)?;
    throttled(o, &headers)?;
    let expired = || fail(StatusCode::UNAUTHORIZED, "That sign-in expired. Try again.");
    let code = crate::lock(&o.codes)
        .remove(&req.code)
        .filter(|c| c.at.elapsed() < CODE_TTL)
        .ok_or_else(expired)?;
    // Only the app that started this sign-in knows the verifier (PKCE).
    if !pkce_ok(&req.verifier) || challenge(&req.verifier) != code.challenge {
        return Err(expired());
    }
    let link = o.map.lock().await.get(&code.who.id).cloned();
    if let Some(link) = link
        && registered_at(&state, &link.account).await? == Some(link.registered_at)
    {
        return Ok(Json(OAuthOutcome::SignedIn {
            refresh: o.refresh_token(&code.who.id, link.generation),
            account: link.account,
        }));
    }
    Ok(Json(OAuthOutcome::NeedsName {
        suggestion: suggest(&code.who.username),
        ticket: o.sign(&Claims::Ticket {
            who: code.who,
            exp: now() + TICKET_TTL.as_secs(),
        }),
    }))
}

pub async fn register(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<OAuthRegisterRequest>,
) -> ApiResult<OAuthOutcome> {
    let o = oauth(&state)?;
    throttled(o, &headers)?;
    let Some(Claims::Ticket { who, .. }) = o.verify(&req.ticket) else {
        return Err(fail(
            StatusCode::UNAUTHORIZED,
            "That sign-in expired. Try again.",
        ));
    };
    if state.config.registration == Registration::Closed {
        return Err(fail(
            StatusCode::FORBIDDEN,
            &format!("{} isn't taking sign-ups.", state.config.name),
        ));
    }
    let name = req.account.trim();
    if !valid_account(name) {
        return Err(fail(
            StatusCode::CONFLICT,
            "Names are up to 32 letters, digits and symbols, with no spaces or . , * ? ! @ :",
        ));
    }
    let mut map = o.map.lock().await;
    let r: ErgoReply = ergo(
        &state,
        "v1/ns/saregister",
        serde_json::json!({ "accountName": name, "passphrase": "" }),
    )
    .await?;
    if !r.success {
        let error = match r.error_code.as_deref() {
            Some("ACCOUNT_EXISTS") => "That name is taken. Try another.",
            _ => "That name isn't allowed here. Try another.",
        };
        return Err(fail(StatusCode::CONFLICT, error));
    }
    let info = account_info(&state, name).await?.ok_or_else(|| {
        tracing::error!(%name, "registered, but ns/info doesn't know it");
        fail(StatusCode::BAD_GATEWAY, "Something went wrong.")
    })?;
    // A stale link to a dropped account is replaced, generation and all.
    let generation = map.get(&who.id).map_or(0, |l| l.generation + 1);
    let link = Link {
        account: info.account_name.clone(),
        registered_at: info.registered_at,
        generation,
    };
    map.insert(who.id.clone(), link);
    o.save(&map).await?;
    tracing::info!(account = %info.account_name, "signed up with Discord");
    Ok(Json(OAuthOutcome::SignedIn {
        refresh: o.refresh_token(&who.id, generation),
        account: info.account_name,
    }))
}

pub async fn refresh(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<RefreshRequest>,
) -> ApiResult<RefreshResponse> {
    let o = oauth(&state)?;
    throttled(o, &headers)?;
    let again = || fail(StatusCode::UNAUTHORIZED, "Sign in with Discord again.");
    let Some(Claims::Refresh { id, generation, .. }) = o.verify(&req.refresh) else {
        return Err(again());
    };
    let link = o
        .map
        .lock()
        .await
        .get(&id)
        .filter(|l| l.generation == generation)
        .cloned()
        .ok_or_else(again)?;
    // Unregistered, suspended or someone else's by now: no token for it.
    if registered_at(&state, &link.account).await? != Some(link.registered_at) {
        return Err(again());
    }
    let (session, expires_in) = api::mint_session(&state, &link.account)?;
    Ok(Json(RefreshResponse {
        irc_token: o.irc_token(&link.account),
        session,
        expires_in,
        refresh: o.refresh_token(&id, generation),
        account: link.account,
    }))
}

/// Sets a password for other IRC clients and FILEHOST. ergo won't let a
/// passwordless account set its own first one (`docs/notes/oauth.md`).
pub async fn password(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<PasswordRequest>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let o = oauth(&state)?;
    let account = api::session_account(&state, &headers)?;
    // Password accounts change theirs with NS PASSWD, which asks for the old one.
    if !o.linked(&account).await {
        return Err(fail(
            StatusCode::FORBIDDEN,
            "Change your password from your IRC client.",
        ));
    }
    if !(8..=256).contains(&req.password.len()) || req.password.chars().any(char::is_control) {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "Passwords are 8 to 256 characters.",
        ));
    }
    let r: ErgoReply = ergo(
        &state,
        "v1/ns/passwd",
        serde_json::json!({ "accountName": account, "passphrase": req.password }),
    )
    .await?;
    if r.success {
        Ok(StatusCode::NO_CONTENT)
    } else {
        tracing::warn!(%account, code = ?r.error_code, "ns/passwd refused");
        Err(fail(
            StatusCode::BAD_REQUEST,
            "The server refused that password.",
        ))
    }
}

#[derive(Deserialize)]
struct ErgoReply {
    success: bool,
    #[serde(rename = "errorCode")]
    error_code: Option<String>,
}

#[derive(Deserialize)]
struct AccountInfo {
    success: bool,
    #[serde(rename = "accountName", default)]
    account_name: String,
    #[serde(rename = "registeredAt", default)]
    registered_at: String,
}

struct Info {
    account_name: String,
    registered_at: String,
}

/// A call to ergo's API; any failure to get an answer is a 502.
async fn ergo<T: serde::de::DeserializeOwned>(
    state: &AppState,
    path: &str,
    body: serde_json::Value,
) -> Result<T, (StatusCode, Json<ApiError>)> {
    let down = || {
        fail(
            StatusCode::BAD_GATEWAY,
            "Couldn't reach the server. Try again later.",
        )
    };
    let e = &state.config.ergo;
    let url = e.api.join(path).map_err(|_| down())?;
    let res = state
        .http
        .post(url)
        .bearer_auth(&e.token)
        .json(&body)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|err| {
            tracing::error!("ergo {path}: {err}");
            down()
        })?;
    res.json().await.map_err(|err| {
        tracing::error!("ergo {path}: {err}");
        down()
    })
}

/// `None` if ergo doesn't know the account (or it's suspended).
async fn account_info(
    state: &AppState,
    account: &str,
) -> Result<Option<Info>, (StatusCode, Json<ApiError>)> {
    let r: AccountInfo = ergo(
        state,
        "v1/ns/info",
        serde_json::json!({ "accountName": account }),
    )
    .await?;
    Ok((r.success && !r.registered_at.is_empty()).then_some(Info {
        account_name: r.account_name,
        registered_at: r.registered_at,
    }))
}

async fn registered_at(
    state: &AppState,
    account: &str,
) -> Result<Option<String>, (StatusCode, Json<ApiError>)> {
    Ok(account_info(state, account).await?.map(|i| i.registered_at))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// RFC 8032 section 7.1 test 1, as `openssl genpkey` would write it.
    pub const TEST_KEY: &str = "-----BEGIN PRIVATE KEY-----
MC4CAQAwBQYDK2VwBCIEIJ1hsZ3v/VpguoRK9JLsLMREScVpezJpGXA7rAMcrn9g
-----END PRIVATE KEY-----
";

    fn oauth(dir: &Path) -> OAuth {
        std::fs::write(dir.join("secret"), "s3cret\n").unwrap();
        std::fs::write(dir.join("key.pem"), TEST_KEY).unwrap();
        let cfg = Discord {
            client_id: "1".into(),
            client_secret_file: dir.join("secret"),
            signing_key: dir.join("key.pem"),
            state_dir: dir.join("state"),
            api: Url::parse("http://discord.invalid/api/").unwrap(),
        };
        OAuth::load(&cfg, &Url::parse("https://chat.example.com/api").unwrap()).unwrap()
    }

    #[test]
    fn pkce_s256() {
        // Cross-checked with Python: urlsafe_b64encode(sha256(v)).rstrip('=').
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWjOEjXk"),
            "VYFANLqdx_HDV6BqEhluZJ63rtrIPROSSFdB3P6G83I"
        );
        assert!(pkce_ok("VYFANLqdx_HDV6BqEhluZJ63rtrIPROSSFdB3P6G83I"));
        assert!(!pkce_ok("short") && !pkce_ok(&"a".repeat(129)) && !pkce_ok(&"a/".repeat(30)));
    }

    #[test]
    fn names() {
        assert_eq!(suggest("sam.hoffman"), "sam_hoffman");
        assert_eq!(suggest("Ünïcode"), "ncode");
        assert_eq!(suggest("..."), "___");
        assert_eq!(suggest("é"), "chad");
        assert_eq!(suggest(&"a".repeat(40)).len(), 32);
        for ok in ["sam_h", "Bob", "x", "a-b", "[w]", &"a".repeat(32)] {
            assert!(valid_account(ok), "{ok}");
        }
        for bad in [
            "",
            "sam.h",
            "a b",
            "a,b",
            "a*",
            "a?",
            "a!",
            "a@b",
            "a:b",
            "#c",
            "-a",
            "+a",
            "~a",
            "é",
            &"a".repeat(33),
        ] {
            assert!(!valid_account(bad), "{bad}");
        }
    }

    #[test]
    fn signed_claims_round_trip_and_reject_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let o = oauth(dir.path());
        let t = o.refresh_token("123", 4);
        assert!(matches!(
            o.verify(&t),
            Some(Claims::Refresh { id, generation: 4, .. }) if id == "123"
        ));
        let (body, sig) = t.split_once('.').unwrap();
        let forged = B64URL.encode(
            serde_json::to_vec(&Claims::Refresh {
                id: "999".into(),
                generation: 4,
                exp: u64::MAX,
            })
            .unwrap(),
        );
        assert_eq!(o.verify(&format!("{forged}.{sig}")), None);
        assert_eq!(o.verify(&format!("{body}.AAAA")), None);
        assert_eq!(o.verify("garbage"), None);
        let old = o.sign(&Claims::Refresh {
            id: "1".into(),
            generation: 0,
            exp: now() - 1,
        });
        assert_eq!(o.verify(&old), None);

        // The key survives a restart; a new one would sign everyone out.
        let again = oauth(dir.path());
        assert!(again.verify(&t).is_some());
    }

    #[test]
    fn irc_token_is_an_eddsa_jwt_ergo_can_check() {
        use ed25519_dalek::{Signature, Verifier as _};
        let dir = tempfile::tempdir().unwrap();
        let o = oauth(dir.path());
        let jwt = o.irc_token("bob");
        let mut parts = jwt.split('.');
        let (h, p, s) = (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        );
        let header: serde_json::Value = serde_json::from_slice(&B64URL.decode(h).unwrap()).unwrap();
        assert_eq!(header["alg"], "EdDSA");
        let claims: serde_json::Value = serde_json::from_slice(&B64URL.decode(p).unwrap()).unwrap();
        assert_eq!(claims["account"], "bob");
        assert_eq!(claims["aud"], AUDIENCE);
        assert!(claims["exp"].as_u64().unwrap() <= now() + 60);
        let sig = Signature::from_slice(&B64URL.decode(s).unwrap()).unwrap();
        o.signing
            .verifying_key()
            .verify(format!("{h}.{p}").as_bytes(), &sig)
            .unwrap();
        // RFC 8032 test 1's public key: the PEM parse took the right bytes.
        assert_eq!(
            hex::encode(o.signing.verifying_key().as_bytes()),
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
        );
    }

    #[test]
    fn rejects_other_keys() {
        assert!(
            ed25519_seed("-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----").is_err()
        );
        assert!(ed25519_seed("not pem at all!").is_err());
    }

    #[test]
    fn prune_caps_and_expires() {
        let mut m: HashMap<String, Instant> = HashMap::new();
        let old = Instant::now() - Duration::from_secs(3600);
        m.insert("old".into(), old);
        assert!(prune(&mut m, |t| *t, Duration::from_secs(60)));
        assert!(m.is_empty());
        for i in 0..MAX_PENDING {
            m.insert(i.to_string(), Instant::now());
        }
        assert!(!prune(&mut m, |t| *t, Duration::from_secs(60)));
    }

    /// Discord and ergo, faked: just what chadd calls.
    mod fake {
        use std::collections::HashMap;
        use std::sync::{Arc, Mutex};

        use axum::{
            Json, Router,
            extract::State,
            http::StatusCode,
            routing::{get, post},
        };
        use serde_json::{Value, json};

        #[derive(Default)]
        pub struct World {
            /// account → registeredAt
            pub accounts: Mutex<HashMap<String, String>>,
            pub passwords: Mutex<HashMap<String, String>>,
            pub me_status: Mutex<u16>,
            pub revoked: Mutex<u32>,
        }

        pub async fn serve(world: Arc<World>) -> String {
            let app = Router::new()
                .route(
                    "/discord/oauth2/token",
                    post(|body: String| async move {
                        assert!(body.contains("code_verifier=") && body.contains("code=dcode"));
                        Json(json!({ "access_token": "at", "token_type": "Bearer" }))
                    }),
                )
                .route(
                    "/discord/oauth2/token/revoke",
                    post(|State(w): State<Arc<World>>| async move {
                        *w.revoked.lock().unwrap() += 1;
                        StatusCode::OK
                    }),
                )
                .route(
                    "/discord/users/@me",
                    get(|State(w): State<Arc<World>>| async move {
                        let status = *w.me_status.lock().unwrap();
                        let s = StatusCode::from_u16(status).unwrap();
                        (s, Json(json!({ "id": "42", "username": "sam.h", "extra": [1, 2] })))
                    }),
                )
                .route(
                    "/v1/ns/saregister",
                    post(|State(w): State<Arc<World>>, Json(b): Json<Value>| async move {
                        let name = b["accountName"].as_str().unwrap().to_owned();
                        let mut a = w.accounts.lock().unwrap();
                        if a.contains_key(&name.to_lowercase()) || name == "nickserv" {
                            return Json(json!({ "success": false, "errorCode": "ACCOUNT_EXISTS" }));
                        }
                        let n = a.len();
                        a.insert(name.to_lowercase(), format!("2026-11-14T00:00:0{n}.000Z"));
                        Json(json!({ "success": true }))
                    }),
                )
                .route(
                    "/v1/ns/info",
                    post(|State(w): State<Arc<World>>, Json(b): Json<Value>| async move {
                        let name = b["accountName"].as_str().unwrap().to_lowercase();
                        match w.accounts.lock().unwrap().get(&name) {
                            Some(at) => Json(json!({ "success": true, "accountName": name, "registeredAt": at })),
                            None => Json(json!({ "success": false })),
                        }
                    }),
                )
                .route(
                    "/v1/ns/passwd",
                    post(|State(w): State<Arc<World>>, Json(b): Json<Value>| async move {
                        let name = b["accountName"].as_str().unwrap().to_owned();
                        let pass = b["passphrase"].as_str().unwrap().to_owned();
                        w.passwords.lock().unwrap().insert(name, pass);
                        Json(json!({ "success": true }))
                    }),
                )
                .with_state(world);
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = l.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
            format!("http://{addr}")
        }
    }

    mod flow {
        use std::sync::Arc;

        use axum::{body::Body, http::Request};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        use super::super::*;
        use super::{TEST_KEY, fake};
        use crate::config::Config;
        use crate::routes::{AppState, router};

        struct T {
            state: Arc<AppState>,
            world: Arc<fake::World>,
            _dir: tempfile::TempDir,
        }

        impl T {
            async fn new(registration: &str) -> Self {
                let world = Arc::new(fake::World::default());
                *world.me_status.lock().unwrap() = 200;
                let base = fake::serve(world.clone()).await;
                let dir = tempfile::tempdir().unwrap();
                let d = dir.path();
                std::fs::write(d.join("secret"), "s3cret").unwrap();
                std::fs::write(d.join("key.pem"), TEST_KEY).unwrap();
                let config = Config::parse(&format!(
                    r#"
                    origin = "https://chat.example.com"
                    name = "Chad"
                    registration = "{registration}"
                    [ergo]
                    api = "{base}/"
                    token = "t"
                    [livekit]
                    key = "devkey"
                    secret = "0123456789abcdef0123456789abcdef"
                    [discord]
                    client_id = "1571365831345185029"
                    client_secret_file = "{secret}"
                    signing_key = "{key}"
                    state_dir = "{state}"
                    api = "{base}/discord/"
                    "#,
                    secret = d.join("secret").display(),
                    key = d.join("key.pem").display(),
                    state = d.join("state").display(),
                ))
                .unwrap();
                assert!(config.discovery().unwrap().chadd.discord);
                let state = Arc::new(AppState::new(config).unwrap());
                Self {
                    state,
                    world,
                    _dir: dir,
                }
            }

            async fn call(&self, req: Request<Body>) -> (u16, axum::http::HeaderMap, String) {
                let res = router(self.state.clone()).oneshot(req).await.unwrap();
                let (status, headers) = (res.status().as_u16(), res.headers().clone());
                let body = res.into_body().collect().await.unwrap().to_bytes();
                (status, headers, String::from_utf8(body.to_vec()).unwrap())
            }

            async fn post(&self, path: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
                self.post_as(path, body, None).await
            }

            async fn post_as(
                &self,
                path: &str,
                body: serde_json::Value,
                bearer: Option<&str>,
            ) -> (u16, serde_json::Value) {
                let mut req = Request::post(path).header("content-type", "application/json");
                if let Some(b) = bearer {
                    req = req.header("authorization", format!("Bearer {b}"));
                }
                let (s, _, body) = self
                    .call(req.body(Body::from(body.to_string())).unwrap())
                    .await;
                (s, serde_json::from_str(&body).unwrap_or_default())
            }

            /// The browser part: start, Discord, callback. Returns the irchad:// URL.
            async fn browser(&self, challenge: &str) -> Url {
                let (s, h, _) = self
                    .call(
                        Request::get(format!("/api/oauth/discord/start?challenge={challenge}"))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await;
                assert_eq!(s, 303);
                let to = Url::parse(h["location"].to_str().unwrap()).unwrap();
                assert_eq!(to.host_str(), Some("discord.com"));
                let q: HashMap<_, _> = to.query_pairs().into_owned().collect();
                assert_eq!(
                    q["redirect_uri"],
                    "https://chat.example.com/api/oauth/discord/callback"
                );
                assert_eq!(q["code_challenge_method"], "S256");
                assert_eq!(q["scope"], "identify");
                let (s, _, page) = self
                    .call(
                        Request::get(format!(
                            "/api/oauth/discord/callback?code=dcode&state={}",
                            q["state"]
                        ))
                        .body(Body::empty())
                        .unwrap(),
                    )
                    .await;
                assert_eq!(s, 200);
                let href = page
                    .split("href=\"")
                    .nth(1)
                    .unwrap()
                    .split('"')
                    .next()
                    .unwrap();
                Url::parse(&href.replace("&amp;", "&")).unwrap()
            }
        }

        fn param(u: &Url, k: &str) -> Option<String> {
            u.query_pairs()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.into_owned())
        }

        #[tokio::test]
        async fn sign_up_then_sign_in_then_refresh() {
            let t = T::new("open").await;
            let verifier = token();
            let back = t.browser(&challenge(&verifier)).await;
            assert_eq!(back.host_str(), Some("oauth"));
            assert_eq!(param(&back, "origin").unwrap(), "https://chat.example.com");
            let code = param(&back, "code").unwrap();
            // Revoked in the background, after the page is served.
            for _ in 0..100 {
                if *t.world.revoked.lock().unwrap() == 1 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            assert_eq!(
                *t.world.revoked.lock().unwrap(),
                1,
                "Discord's token is revoked"
            );

            // Someone who intercepted the code but not the verifier gets nothing,
            // and the code is spent.
            let (s, _) = t
                .post(
                    "/api/oauth/exchange",
                    serde_json::json!({ "code": code, "verifier": token() }),
                )
                .await;
            assert_eq!(s, 401);
            let (s, _) = t
                .post(
                    "/api/oauth/exchange",
                    serde_json::json!({ "code": code, "verifier": verifier }),
                )
                .await;
            assert_eq!(s, 401, "one-time");

            let verifier = token();
            let code = param(&t.browser(&challenge(&verifier)).await, "code").unwrap();
            let (s, out) = t
                .post(
                    "/api/oauth/exchange",
                    serde_json::json!({ "code": code, "verifier": verifier }),
                )
                .await;
            assert_eq!(s, 200);
            assert_eq!(out["kind"], "needs_name");
            assert_eq!(out["suggestion"], "sam_h");
            let ticket = out["ticket"].as_str().unwrap().to_owned();

            for (name, status) in [("sam.h", 409), ("nickserv", 409)] {
                let (s, e) = t
                    .post(
                        "/api/oauth/register",
                        serde_json::json!({ "ticket": ticket, "account": name }),
                    )
                    .await;
                assert_eq!(s, status, "{name}: {e}");
            }
            let (s, out) = t
                .post(
                    "/api/oauth/register",
                    serde_json::json!({ "ticket": ticket, "account": "sam_h" }),
                )
                .await;
            assert_eq!(s, 200, "{out}");
            assert_eq!(out["kind"], "signed_in");
            assert_eq!(out["account"], "sam_h");
            let refresh = out["refresh"].as_str().unwrap().to_owned();

            let (s, r) = t
                .post(
                    "/api/oauth/refresh",
                    serde_json::json!({ "refresh": refresh }),
                )
                .await;
            assert_eq!(s, 200, "{r}");
            assert_eq!(r["account"], "sam_h");
            assert_eq!(r["irc_token"].as_str().unwrap().split('.').count(), 3);
            let session = r["session"].as_str().unwrap().to_owned();

            // Next time, Discord sign-in goes straight to the account.
            let verifier = token();
            let code = param(&t.browser(&challenge(&verifier)).await, "code").unwrap();
            let (_, out) = t
                .post(
                    "/api/oauth/exchange",
                    serde_json::json!({ "code": code, "verifier": verifier }),
                )
                .await;
            assert_eq!(out["kind"], "signed_in");
            assert_eq!(out["account"], "sam_h");

            // A password for other IRC clients.
            let (s, _) = t
                .post_as(
                    "/api/oauth/password",
                    serde_json::json!({ "password": "hunter22" }),
                    Some(&session),
                )
                .await;
            assert_eq!(s, 204);
            assert_eq!(t.world.passwords.lock().unwrap()["sam_h"], "hunter22");
            let (s, _) = t
                .post_as(
                    "/api/oauth/password",
                    serde_json::json!({ "password": "short" }),
                    Some(&session),
                )
                .await;
            assert_eq!(s, 400);

            // The name is dropped and someone else takes it: the old link is dead.
            t.world
                .accounts
                .lock()
                .unwrap()
                .insert("sam_h".into(), "2027-01-01T00:00:00.000Z".into());
            let (s, _) = t
                .post(
                    "/api/oauth/refresh",
                    serde_json::json!({ "refresh": refresh }),
                )
                .await;
            assert_eq!(s, 401);
            let verifier = token();
            let code = param(&t.browser(&challenge(&verifier)).await, "code").unwrap();
            let (_, out) = t
                .post(
                    "/api/oauth/exchange",
                    serde_json::json!({ "code": code, "verifier": verifier }),
                )
                .await;
            assert_eq!(out["kind"], "needs_name");
        }

        #[tokio::test]
        async fn discord_trouble_and_closed_sign_ups() {
            let t = T::new("closed").await;
            *t.world.me_status.lock().unwrap() = 429;
            let back = t.browser(&challenge(&token())).await;
            assert!(param(&back, "code").is_none());
            assert!(param(&back, "error").unwrap().contains("Discord"));

            *t.world.me_status.lock().unwrap() = 200;
            let verifier = token();
            let code = param(&t.browser(&challenge(&verifier)).await, "code").unwrap();
            let (_, out) = t
                .post(
                    "/api/oauth/exchange",
                    serde_json::json!({ "code": code, "verifier": verifier }),
                )
                .await;
            let (s, _) = t
                .post(
                    "/api/oauth/register",
                    serde_json::json!({ "ticket": out["ticket"], "account": "sam_h" }),
                )
                .await;
            assert_eq!(s, 403);

            // Unknown state, bad challenge, forged ticket and refresh.
            let (s, _, page) = t
                .call(
                    Request::get("/api/oauth/discord/callback?code=x&state=nope")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await;
            assert!(s == 200 && page.contains("error="));
            let (s, _, _) = t
                .call(
                    Request::get("/api/oauth/discord/start?challenge=short")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await;
            assert_eq!(s, 400);
            let (s, _) = t
                .post(
                    "/api/oauth/register",
                    serde_json::json!({ "ticket": "a.b", "account": "x" }),
                )
                .await;
            assert_eq!(s, 401);
            let (s, _) = t
                .post(
                    "/api/oauth/refresh",
                    serde_json::json!({ "refresh": "a.b" }),
                )
                .await;
            assert_eq!(s, 401);
        }
    }
}

# Shared Discord sign-in (broker) — plan, not built

Status: planned, deferred. Today each server needs its own Discord app (D96), so
"Sign in with Discord" only shows up where an operator went through the Discord
developer portal. This plan lets any server offer it by trusting one shared
IrChad sign-in service (the "broker"), e.g. `https://auth.irchad.chat`.

## Shape

The broker does only the Discord half of what chadd's `oauth.rs` does today
(`start`, `callback`, `identify`). Everything after "this is Discord user X"
stays on each server: the account map, the name prompt, `register`, refresh
tokens and ergo JWTs. **The client doesn't change.**

```
app ──► server /api/oauth/discord/start?challenge=c
server ─303─► broker /start?challenge=c&aud=<server origin>
broker: pending[state] = {discord_verifier, c, aud} ─302─► Discord authorize
Discord ─302─► broker /callback?code&state
broker: token exchange, /users/@me, revoke, codes[k] = {who, c, aud}
        ─► irchad://oauth?origin=<aud>&code=k           (same page as today)
app ──► server /api/oauth/exchange {code: k, verifier: v}  (unchanged)
server ──► broker POST /redeem {code: k, verifier: v, aud: <own origin>}
broker: one-time, S256(v) == c, aud matches → {sub, username}
server: continues exactly as today (map lookup → SignedIn | NeedsName)
```

The `/redeem` call is server to broker over TLS, so the reply needs no
signature and the server pins no broker key. A dishonest server can claim any
`aud`, but it still needs a code and verifier that only the user's app holds.
The app sends those only to the origin it started the flow with (manager.rs),
so this is no weaker than today.

## Pieces

- **chadd broker mode.** A `[discord_broker]` config section (`client_id`,
  `client_secret_file`) mounts `/start`, `/callback` and `/redeem`. Those are
  the existing `start`/`callback`/`finish`/`identify`, with `aud` added to
  `Pending` and `Code`. Same binary, so chadd stays one thing. The broker
  deployment is a chadd with only that section set.
- **chadd server side.** `[discord]` gets `broker = "https://…"` as an
  alternative to `client_id` + `client_secret_file`. `start` redirects to the
  broker instead of Discord, and `exchange` calls `/redeem` instead of reading
  its own `codes`. `signing_key` and `state_dir` stay: the ergo JWT and the
  refresh tokens are still the server's own.
- **Validate `aud`** on the broker: it must be an `https://host[:port]` origin,
  with no path, userinfo or query string. Throttle per IP as `start` does now,
  plus a global cap so one abuser can't get the shared Discord app
  rate-limited for everyone.
- **Pairwise subject.** The broker returns `sub = HMAC(broker_key, discord_id ‖ aud)`
  instead of the raw Discord ID, so servers can't match users across
  networks. `username` still goes along, but only as the name suggestion.
  `broker_key` must be backed up: losing it orphans every broker-made link.
- **Operator setup becomes:** generate the ergo JWT keypair (fold it into
  `deploy/native` setup and compose, so it's automatic), turn on `jwt-auth` in
  ergo, and set `broker = …` in chadd. No Discord portal, no secret.

## Trade-offs to state in the operator docs

- **Trust.** Whoever runs the broker can sign in as any Discord user on every
  server that trusts it. It's opt-in per server, and never the default.
- **Availability.** If the broker is down, new Discord sign-ins fail on every
  server that uses it. Devices already signed in keep working, because
  refresh never touches the broker.
- **Privacy.** The broker learns which Discord users sign in to which servers
  (`aud`). Log counts only, not who or where.
- **One Discord app for everyone.** Discord's OAuth2 rate limits apply across
  all networks combined. Check Discord's current limits and any verification
  rules for large apps before launch.

## Open questions

- **Moving a server between modes.** Links made with its own app are keyed by
  the raw Discord ID, while broker links use the pairwise `sub`. Options: the
  broker also returns the raw ID to servers that set
  `legacy_ids = true`, or a server keeps one mode for life. Keyed map entries
  (`discord:<id>` / `broker:<sub>`) would let both coexist.
- **Who runs `auth.irchad.chat`**, and its key backup and rotation story.
- **Discord consent.** Should the broker force `prompt=consent`, so a reused
  approval isn't silent? This matters more once the app spans many networks.

## Tests (when built)

A fake broker plus a fake Discord in chadd's tests: start → broker →
callback → exchange → redeem; a wrong `aud`; a reused code; a wrong
verifier; an oversized or malformed `/redeem` reply (D14: the broker is a
remote server too).

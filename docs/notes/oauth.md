# Sign in with Discord: ergo JWT spike

Verified against ergo **v2.19.1** (official image), 2026-11-14. Script: [`oauth-jwt-spike.py`](oauth-jwt-spike.py), 24/24 checks pass.

## Chosen shape (option A)

chadd is the confidential Discord OAuth2 client, and it issues short-lived EdDSA JWTs that ergo's `accounts.jwt-auth` accepts over SASL `IRCV3BEARER`.
Discord is plain OAuth2 with no ID token or introspection, so ergo can't check Discord tokens itself. chadd keeps a map from Discord user ID to account.
Alternatives considered: an external IdP (Authentik or Keycloak, with Discord as an upstream source) behind `accounts.oauth2` introspection, which is too heavy to self-host; or `auth-script` with PLAIN, which is a hack now that bearer exists.

## ergo config

```yaml
accounts:
    jwt-auth:
        enabled: true
        autocreate: false          # chadd creates accounts; see below
        tokens:
            - algorithm: "eddsa"
              key-file: "jwt_pub.pem"
              account-claims: ["account"]
              validate-aud: ["irchad-ergo"]
```

## Findings

- **Device ID (D11) works.** `IRCV3BEARER` ignores its authzid (`handlers.go` `authIRCv3BearerHandler`), but `USER bob@laptop 0 * :x` sets the session's device ID (`handlers.go` USER, #843).
  Two JWT logins as `bob@laptop` and `bob@phone` both show in `NS CLIENTS LIST`, and the always-on client stays after both disconnect.
  The client has to send `USER <account>@<device>` on the bearer path; `login.rs` sends a bare name today. With `force-nick-equals-account`, the client also needs the account name before connecting, so chadd's code exchange returns it.
- **`autocreate: false` is needed.** With it, a valid JWT for an account that doesn't exist is refused. chadd creates the account with `POST /v1/ns/saregister {accountName, passphrase: ""}`.
- **ergo's checks hold:** it rejects expired tokens, a missing `exp` (`WithExpirationRequired`), a wrong or missing `aud`, the wrong key, `alg: none`, and HS256 signed with the public key (algorithm confusion). The `oauth2` token type is refused while `accounts.oauth2` is off.
- **No replay protection.** The same JWT logs in twice within its `exp`. Keep the TTL around 60 s, since the client fetches one per connect.
- **A passwordless account can't set its own first password.** ergo treats empty credentials as externally managed: `NS PASSWD * new new` answers "Your account credentials are managed externally" (`accounts.go` `setPassword`, `!hasPrivs && creds.Empty()`). The same goes for adding a CertFP.
  chadd's `POST /v1/ns/passwd` (privileged) does set it. After that `check_auth` works (FILEHOST Basic auth and plain clients), JWT login still works, and the user can change it with `NS PASSWD`.
  So "Set a password for other IRC clients" has to be a chadd endpoint, authenticated by the session token.
- **Reused names can be detected.** After `NS UNREGISTER`, a JWT for that name is refused. If someone re-registers the name, `/v1/ns/info`'s `registeredAt` changes.
  chadd stores `registeredAt` in the Discord-ID → account map and checks it before every JWT is issued, so a stale link can't log into the new owner's account.

## Discord probe

Real Discord application, 2026-11-14. Script: [`oauth-discord-probe.py`](oauth-discord-probe.py) (`python3 -I oauth-discord-probe.py <client_id> <secret_file>`, redirect `http://127.0.0.1:8765/callback`).

- **Discord enforces PKCE.** With `code_challenge` (S256) on authorize, a token exchange with the wrong `code_verifier` gets `400 invalid_grant "Code challenge failed."`. The right verifier then succeeds with the same code, so a failed attempt doesn't burn the code.
  chadd sends PKCE as well as `state` and its client secret.
- **Token response:** `access_token`, `refresh_token`, `expires_in: 604800` (7 days), `scope`. chadd only needs the access token once, for `/users/@me`, then revokes it (`/oauth2/token/revoke` returns 200), and keeps no Discord tokens.
- **`/users/@me`** returns `id`, `username`, `global_name`, `avatar`, `email`, `verified` and more (`discriminator`, `flags`, `locale`, `mfa_enabled`, `premium_type`, `clan`, `primary_guild`, ...). Only read `id`, `username`, `global_name`, `avatar`, `email` and `verified`; ignore the rest.
- **Usernames contain `.` in practice** (the test account's did). ergo refuses `.` in account names (`protocolBreakingNameCharacters = " ,*?.!@:"`, `irc/strings.go`), and it also refuses reserved service names and confusable "skeletons" of existing accounts (`Register`).
  So chadd suggests a cleaned-up name (`.` → `_`), checks it with saregister, and lets the user pick another if it's refused or taken.
- **Rate limits:** `/oauth2/token` returned no `X-RateLimit-*` headers. `/users/@me` returned `limit: 149`, `reset-after: 0.033` with a bucket hash, which is generous for one call per login.
  chadd honours `429` + `Retry-After` and caps invalid requests (Discord's Cloudflare layer bans IPs for bursts of invalid requests), tested against a fake Discord, not the real one.

## Outcome

Built as D96: the session-token path for account deletion, PKCE between the client and chadd, links made only at the code exchange, and stateless refresh tokens with a per-link `generation`.

Next (deferred): a shared broker so servers need no Discord app of their own, in [oauth-broker.md](oauth-broker.md).

# ergo HTTP API: credential check

Verified against ergo **v2.19.1** on the dev stack (2026-10-27). Source: `irc/api.go`, `docs/API.md`.

**Use `POST /v1/check_auth`.** The old Go chadd's `/v1/account_details` is a different endpoint (now `/v1/ns/info`, legacy name still accepted).
It returns account info and does **not** check a password.

## Request

```
POST http://ergo:8089/v1/check_auth
Authorization: Bearer <token from api.bearer-tokens>
Content-Type: application/json

{"accountName": "alice", "passphrase": "alicepass"}
```

`certfp` (hex SHA-256 of the client cert) may be sent instead of `passphrase`. We don't use it.

## Responses (observed)

| Input | Response |
|---|---|
| `Alice` / correct password | `{"success":true,"accountName":"alice"}` (case-insensitive; returns canonical name) |
| `alice` / wrong password | `{"success":false}` |
| nonexistent account | `{"success":false}` |
| `alice@laptop` / correct password | `{"success":false}` (no device-ID stripping here, unlike SASL) |
| bad bearer token | HTTP 401 |

Unexpected internal errors come back as `{"success":false,"error":"..."}` with HTTP 200, so chadd must check `success`, not just the status.
Unverified and suspended accounts get `success:false` with no error.

## Implications for chadd

- Strip any `@device` suffix before calling check_auth. Use the returned `accountName` as the canonical identity: it becomes the LiveKit participant identity and the session token subject.
- Every API endpoint is fully privileged, so the API listener must only be reachable on the compose network. It isn't published, and Caddy doesn't route to it.
- check_auth has no rate limit of its own. chadd needs to throttle `/api/login` per IP and per account, because ergo's login throttle only covers IRC connections.
- `/v1/ns/info` (alias `/v1/account_details`) is useful for "does this account exist", and it returns registered channels.

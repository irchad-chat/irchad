# Phase 0: Foundations

**Goal:** a workspace that builds on Linux and Windows, a one-command dev server stack, and the three riskiest unknowns retired before any feature work.

## Workspace

- [ ] Cargo workspace with the crates from `CLAUDE.md` (empty libs and bins are fine). Use edition 2024 and shared `[workspace.dependencies]`.
- [ ] `rustfmt.toml`, clippy config, `.gitignore`.
- [ ] Fill in the Commands section of `CLAUDE.md`.

## Dev server stack (`deploy/`)

- [ ] `compose.yaml` with ergo (pinned ≥ 2.19), livekit-server, chadd and caddy. A local self-signed origin such as `chat.localhost` is fine.
- [ ] `ergo.yaml`:
  - websocket listener, accounts and registration enabled, nick = account (`force-nick-equals-account`)
  - multiclient on (`accounts.multiclient.enabled: true`, `allowed-by-default: true`)
  - always-on on by default: `accounts.multiclient.always-on: opt-out` (see D11). Check the pinned `default.yaml` for auto-away and always-on expiration keys, and set auto-away so users with no connected device show as away
  - persistent history (check which backends the pinned version supports)
  - `accounts.metadata` block, HTTP API with a bearer token
- [ ] `livekit.yaml`: API key and secret, plus a TURN/UDP port range documented.
- [ ] `Caddyfile` routing per `ARCHITECTURE.md`.
- [ ] Seed script: create two test accounts and a few channels.

## chadd v0

- [ ] axum server: `GET /.well-known/irchad.json`, built from config (TOML or env), using the shared discovery types.
- [ ] `GET /invite[/*]`: minimal static landing page with an `irchad://join?origin=…` button.
- [ ] Verify the ergo HTTP API credential-check endpoint and write it up in `docs/notes/ergo-api.md`. The old code used `/v1/account_details`, but it's probably `/v1/check_auth`.

## CI

Hosting is self-hosted Gitea at `git.gentoo.party` (user `sam`), with **Gitea Actions**, not GitHub. Use `tea --login git.gentoo.party` for repo, PR and release operations.

- [ ] Create the repo on `git.gentoo.party` under `sam` with `tea`, add it as `origin`, and push `main`.
- [ ] Gitea Actions workflows in `.gitea/workflows/`: fmt, clippy and test on Linux, plus build on Windows (MSVC).
- [ ] Confirm which `act_runner` labels the instance offers. The Windows job has a windows-latest runner.
- [ ] Cache the LiveKit/libwebrtc artifacts, because the `webrtc-sys` download is large. Check that the instance's Actions cache server is enabled for `actions/cache`.

## Spikes (each writes `docs/notes/<topic>.md` with a go/no-go)

- [ ] **egui chat log:**
  - 50k messages with variable heights (wrapped text, an inline image, a code block)
  - smooth scroll, plus jump-to-unread and keep-position on prepend (history backfill)
  - text selection across several messages, and IME input in the composer
  - test on Wayland and X11
- [ ] **LiveKit Rust SDK:**
  - join a room on the dev stack and publish a sine tone via `NativeAudioSource`
  - receive and play back via PipeWire
  - build on Windows
  - confirm the APM is reachable from the SDK for AEC with our own I/O
- [ ] **ergo capability check:** connect with a raw client and dump `CAP LS 302` and ISUPPORT. Save them to `docs/notes/ergo-caps.md`.
      Confirm metadata limits (max keys, max value bytes) and that client-only tags on PRIVMSG survive chathistory replay. Phase 4 depends on that.
      Also confirm always-on behaviour: what a reconnecting session receives on attach (JOINs, topics, NAMES), whether a SASL SCRAM username of `account@device` works, and that `CHATHISTORY TARGETS` lists DMs received while offline.

## Done when

- `cargo build` is green on Linux and Windows in CI.
- `docker compose up` gives a working ergo, LiveKit and chadd, and `curl https://chat.localhost/.well-known/irchad.json` works.
- All three spike notes exist, and none is a no-go (or the user has accepted a fallback).

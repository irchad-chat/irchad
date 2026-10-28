# Phase 0: Foundations

**Goal:** a workspace that builds on Linux and Windows, a one-command dev server stack, and the three riskiest unknowns retired before any feature work.

## Workspace

- [x] Cargo workspace with the crates from `CLAUDE.md` (empty libs and bins are fine). Use edition 2024 and shared `[workspace.dependencies]`.
- [x] `rustfmt.toml`, clippy config, `.gitignore`. Clippy levels live in `[workspace.lints]`, and CI adds `-D warnings`.
- [x] Fill in the Commands section of `CLAUDE.md`.

## Dev server stack (`deploy/`)

- [x] `compose.yaml` with ergo (pinned ≥ 2.19), livekit-server, chadd and caddy. A local self-signed origin such as `chat.localhost` is fine.
- [x] `ergo.yaml`:
  - websocket listener, accounts and registration enabled, nick = account (`force-nick-equals-account`)
  - multiclient on (`accounts.multiclient.enabled: true`, `allowed-by-default: true`)
  - always-on on by default: `accounts.multiclient.always-on: opt-out` (see D11). Check the pinned `default.yaml` for auto-away and always-on expiration keys, and set auto-away so users with no connected device show as away
  - persistent history (check which backends the pinned version supports)
  - `accounts.metadata` block, HTTP API with a bearer token
  - Done: in 2.19.1, metadata is a **top-level** `metadata:` block. Persistent history uses SQLite. `always-on-expiration` stays unset (no expiry), and auto-away is `opt-out`.
    `ergo.yaml` is generated from upstream `default.yaml` by `deploy/tools/patch-ergo-default.py`.
- [x] `livekit.yaml`: API key and secret, plus a TURN/UDP port range documented.
- [x] `Caddyfile` routing per `ARCHITECTURE.md`.
- [x] Seed script: create two test accounts and a few channels.

## chadd v0

- [x] axum server: `GET /.well-known/irchad.json`, built from config (TOML or env), using the shared discovery types.
- [x] `GET /invite[/*]`: minimal static landing page with an `irchad://join?origin=…` button.
- [x] Verify the ergo HTTP API credential-check endpoint ([notes/ergo-api.md](../notes/ergo-api.md): it is `/v1/check_auth`) and write it up in `docs/notes/ergo-api.md`. The old code used `/v1/account_details`, but it's probably `/v1/check_auth`.

## CI

Hosting is self-hosted Gitea at `git.gentoo.party` (user `sam`), with **Gitea Actions**, not GitHub. Use `tea --login git.gentoo.party` for repo, PR and release operations.

- [x] Create the repo on `git.gentoo.party` under `sam` with `tea`, add it as `origin`, and push `main`.
- [x] Gitea Actions workflows in `.gitea/workflows/`: fmt, clippy and test on Linux, plus build on Windows (MSVC).
- [x] Confirm which `act_runner` labels the instance offers. The Windows job has a windows-latest runner.
      Instance-wide runners are `act-runner` (`ubuntu-latest`, `ubuntu-24.04`, `ubuntu-22.04`) and `act-runner-win` (`windows-latest`, `windows-2025`, `windows`, host mode).
- [x] Cache the LiveKit/libwebrtc artifacts, because the `webrtc-sys` download is large. Check that the instance's Actions cache server is enabled for `actions/cache`.
      The cache server works: `Swatinem/rust-cache` saved on both runners. libwebrtc extracts into `target/`, so rust-cache covers it.

## Spikes (each writes `docs/notes/<topic>.md` with a go/no-go)

- [ ] **egui chat log:** → [notes/egui-chatlog.md](../notes/egui-chatlog.md). Perf and keep-position are GO. Selection and IME are waiting on the human check below.
  - 50k messages with variable heights (wrapped text, an inline image, a code block)
  - smooth scroll, plus jump-to-unread and keep-position on prepend (history backfill)
  - text selection across several messages, and IME input in the composer
  - test on Wayland and X11
- [ ] **LiveKit Rust SDK:** → [notes/livekit-sdk.md](../notes/livekit-sdk.md). GO on Linux. Waiting on the Windows CI build of the example.
  - join a room on the dev stack and publish a sine tone via `NativeAudioSource`
  - receive and play back via PipeWire
  - build on Windows
  - confirm the APM is reachable from the SDK for AEC with our own I/O
- [x] **ergo capability check:** → [notes/ergo-caps.md](../notes/ergo-caps.md) (GO). connect with a raw client and dump `CAP LS 302` and ISUPPORT. Save them to `docs/notes/ergo-caps.md`.
      Confirm metadata limits (max keys, max value bytes) and that client-only tags on PRIVMSG survive chathistory replay. Phase 4 depends on that.
      Also confirm always-on behaviour: what a reconnecting session receives on attach (JOINs, topics, NAMES), whether a SASL SCRAM username of `account@device` works, and that `CHATHISTORY TARGETS` lists DMs received while offline.

## Discovered

- [x] Rootless podman can't bind :443, so Caddy publishes on 8443 (`IRCHAD_HTTPS_PORT`), and the compose project is named `irchad-ng` to avoid colliding with `../irchad` (D13).
- [x] glibc on this machine doesn't resolve `*.localhost`, so Caddy also serves plain `localhost`. Phase 1: the client must handle a discovery origin it can't resolve. Simplest is to use `IRCHAD_HOST=localhost` for native dev.
- [x] ergo's per-IP connection throttle hits dev tooling (every host connection comes from the compose subnet), so the subnet is exempted in dev.
- [x] libwebrtc's weak PipeWire stubs shadow libpipewire (see `notes/livekit-sdk.md`). The workaround is proven, and Phase 2 picks the long-term fix.
- [ ] Human check on Wayland and X11: cross-message selection, IME and scroll feel in `chatlog_spike` (see `notes/egui-chatlog.md`).
- [ ] Phase 2: decide on the metadata rate limit for `chad/voice` (10 SETs per 2 min by default, see `notes/ergo-caps.md`).

## Done when

- `cargo build` is green on Linux and Windows in CI.
- `docker compose up` gives a working ergo, LiveKit and chadd, and `curl https://chat.localhost/.well-known/irchad.json` works.
- All three spike notes exist, and none is a no-go (or the user has accepted a fallback).

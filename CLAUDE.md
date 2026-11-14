# IrChad (next) — orientation for Claude sessions

IrChad is a Discord-like client for IRC, built for people who have never set up an IRC client ("chads").
It gives them an opinionated, pre-configured experience: they paste an invite URL, log in, and chat.
They also get voice, screen share and encrypted DMs.
Text is IRCv3 on **ergo only**, and media goes through a self-hosted **LiveKit** SFU.
This repo is a fresh start. Earlier attempts live in `../irchad` (Wails/Go/Vue + Go `chadd`), `../irchadx` (Rust/egui) and `../irchadz` (Rust/Slint).
They are reference material only, so don't copy code from them blindly.

## Where to start in a session

For a bug fix or a small, specific change, skip this section. Go straight to the code, and don't read `docs/` unless the task needs it.
For phase or roadmap work:

1. Read `docs/PLAN.md` to find the **current phase**, then open its file in `docs/phases/`.
2. Work through the unchecked items in order. Tick boxes as you go, and add tasks you discover to the same file.
3. Record any new architectural decision in `docs/decisions.md` (it's append-only, and you supersede an entry rather than edit it).
4. Spikes write their findings to `docs/notes/<topic>.md` and link them from the phase file.
5. Each phase file has a **Done when** section, which is the exit criteria. Don't start the next phase until it holds or the user waives it.
6. `docs/ARCHITECTURE.md` describes the target design. If reality diverges, update the doc in the same change.

## Non-negotiables

- **Linux first**, with **Wayland and X11 both first-class**. Windows is second-class: it gets CI builds and a smoke test each phase. macOS working is pure coincidence.
- **Prefer state of the art over legacy.** That means PipeWire, xdg-desktop-portal, D-Bus, StatusNotifierItem, Secret Service, wgpu and rustls.
  Where X11 needs a fallback, it lives behind the same trait as the modern path, so it's never the primary path.
- **ergo is the only supported IRCd.** Draft IRCv3 specs are fair game when ergo implements them.
- **Never trust the server, even though it's ergo** (D14). Admins can modify or swap it.
  Treat every server-sent byte as hostile: no panics, no unbounded buffers or CPU, no path or code injection.
  "ergo wouldn't send that" is not a valid reason to skip a check.
- **Don't degrade the IRC experience for plain IRC clients** on the same network.
  IrChad-only data goes in client tags (`+chad/...`) or metadata (`chad/...`) with readable fallbacks.
- **No admin UI yet.** Admins use a normal IRC client. IrChad-specific server config is ergo channel/user metadata that ops set by hand.
- **Keep chadd small.** It does discovery, auth and token minting, and later uploads. Before you add an endpoint, ask whether it can be IRC-native instead.
- Desktop UI is **egui**. The user hacks on it, so keep it idiomatic eframe.

## Stack at a glance

| Area | Choice |
|---|---|
| Language | Rust (edition 2024), single Cargo workspace |
| Desktop UI | egui / eframe on the wgpu backend |
| Async | tokio |
| IRC | own `chad-irc` crate over WebSocket (`tokio-tungstenite` + rustls, subprotocol `text.ircv3.net`) |
| Media | `livekit` Rust SDK (libwebrtc); LiveKit server self-hosted |
| Linux audio | PipeWire native (`pipewire` crate) |
| Linux capture | xdg-desktop-portal ScreenCast via `ashpd` → PipeWire (DMA-BUF); X11 fallback via libwebrtc's own capturers (D21) |
| Windows capture | Windows.Graphics.Capture (libwebrtc's) + WASAPI process loopback |
| Secrets | `oo7` (Secret Service, portal-aware) / Windows Credential Manager |
| E2EE DMs | `vodozemac` (Olm double ratchet + SAS verification) |
| Server glue | `chadd` in Rust (axum) |
| Android | Kotlin + Compose + LiveKit Android SDK, sharing `chad-core` over UniFFI |
| iOS | SwiftUI + LiveKit Swift SDK over the same UniFFI bindings, built on Linux with xtool (`iOS/README.md`) |

## Layout (target — created in Phase 0)

```
crates/chad-proto     wire types shared by client and chadd (discovery, chadd API)
crates/chad-irc       IRCv3 protocol: parse/serialize, CAP, SASL, batch, labeled-response
crates/chad-core      UI-agnostic client state; commands in, events out
crates/chad-media     LiveKit session, audio I/O, capture backends
crates/chad-e2ee      DM encryption (vodozemac), key publication, local store
crates/chad-desktop   egui app (binary: irchad)
crates/chad-ffi       UniFFI bindings (Phase 6)
crates/chadd          server (binary: chadd)
deploy/               docker compose: ergo + livekit + chadd + caddy
android/              (Phase 6)
iOS/                  (Phase 8)
site/                 project website (Astro, static); user docs are Markdown in site/src/pages/docs/
vendor/webrtc-sys      patched webrtc-sys (NVENC on Windows, D89); see vendor/README.md
docs/
```

## Commands

- `cargo build`, `cargo clippy --all-targets`, `cargo fmt`, `cargo test` (CI runs clippy with `-D warnings`).
  To keep output small, use `-q`, scope to the crate you touched, and use short diagnostics:
  `cargo clippy -q -p <crate> --all-targets --message-format=short`, `cargo test -q -p <crate>`. Building `chad-media` examples needs clang ≥ 21 (webrtc-sys) and PipeWire headers.
- Run the client: `cargo run -p chad-desktop` (binary `irchad`)
- Windows NVENC: set `CUDA_HOME` to the output of `powershell -ExecutionPolicy Bypass -File scripts\cuda-headers.ps1` before building. Without it, webrtc-sys builds without NVENC and a cargo warning says so.
  `cargo run -p chad-media --example encoder_check` shows which encoder actually runs, no server needed.
- Screenshots of the UI (to check visual changes): `cargo test -p chad-desktop shots -- --nocapture` renders the real `App` headless and writes `target/shots/*.png`.
  To add a scenario, put a test in `crates/chad-desktop/src/shots.rs` that drives the UI through accesskit (`get_by_label(..).click()`) and then calls `shot("name")`.
- Dev server stack: `docker compose -f deploy/compose.yaml up -d --build` (podman compose works).
  Then `python3 deploy/seed.py` creates the test accounts and channels. See `deploy/README.md`.
- Android: `cd android && JAVA_HOME=/usr/lib/jvm/openjdk-bin-21 ./gradlew assembleFullDebug` (it runs `build-rust.sh` first). See `android/README.md`.
- iOS builds on Linux with xtool, no Mac needed: `cd iOS && ./build-rust.sh && xtool dev build`. `xtool dev` also installs on a connected device. See `iOS/README.md`.
  Run the iOS build after any change to `chad-ffi`'s API or to `iOS/`.
- Project website: `cd site && pnpm install && pnpm dev` (or `pnpm build` → `site/dist/`).
- Dev origin: `https://chat.localhost:8443` (Caddy internal CA, so pass `curl -k`). Raw IRC: `127.0.0.1:6667`.

## Navigating the code

- `App` (`chad-desktop/src/app.rs`) and `Net` (`chad-core/src/net.rs`) keep their `impl` split by concern across `app/*.rs` and `net/*.rs`.
  Put new methods in the matching file. Still find the spot with grep or the LSP rather than reading whole files.
- Prefer the LSP (go-to-definition, find-references, hover) over reading files to trace types and callers.
- `vendor/webrtc-sys` is a vendored third-party crate (D89): about 280 files and 250k lines of C++, generated bindings and assembly.
  Don't search or read it unless the task is about webrtc-sys itself, such as NVENC, the encoder factories or re-vendoring.
  Scope searches to `crates/` (or the relevant dir) so it stays out of the results. Our changes are listed in `vendor/README.md` and marked `irchad:` in the source.

## Conventions

- `chad-core` never depends on egui, and `chad-desktop` never speaks IRC directly.
- Platform code goes behind traits, with `#[cfg(target_os = ...)]` impls in `platform/linux`, `platform/windows`.
  The Wayland vs X11 choice is made at **runtime**, not compile time.
- Use `tracing` for logs, filtered with `RUST_LOG`.
- IRC-visible names: client tags are `+chad/<name>`, and metadata keys are `chad/<name>`.

# chad-web

IrChad's Rust core compiled to wasm, for the Vue app in [`web/`](../../web).
It speaks no IRC of its own (D78): chad-core's `login::Login` registers, `Net` handles everything after, and chad-irc's `keepalive` answers PING and spaces reconnects, the same code the desktop and mobile apps run.
This crate is only the browser's driver:

- `src/client.rs`: the JS-facing `Client`, shaped like chad-ffi's: methods in (`connect`, `signUp`, `verify`, `send`, `edit`, `react`, …), one `onChange` callback out. It also fetches discovery, drives `Login`/`Net`, reconnects with `Backoff`, and turns a guest into an account in place.
- `src/snapshot.rs`: what the page reads after `onChange` (`state`, `buffers`, `messages`, `members`, `nicks`, `customEmoji`, `replaceShortcodes`), with the TypeScript types for them.
- `src/api.rs`: chadd through chad-core's `chadd` client (D79): `upload`, `preview`, `media`, `gifs` return Promises. Custom emoji and D69 tombstones are fetched on every connect.
- `src/irc.rs`: the browser's WebSocket, standing in for `chad_irc::ws::Connection`.
- `src/timer.rs`: `setTimeout`, for keepalive and reconnects.
- `src/voice.rs`: `joinVoice` (chad-core's checks, then chadd's LiveKit token), `voiceConnected`, `leaveVoice`. The room itself is `web/src/stores/voice.ts` with `livekit-client` (D80).

Socket and timer callbacks never do the work themselves: they queue it with `spawn_local`, because the work may drop the socket or timer whose closure is still running.

```sh
cd web && pnpm install && pnpm dev   # builds this crate, runs wasm-bindgen, starts Vite
```

`pnpm wasm` rebuilds only the Rust side. Vite picks up the new `src/wasm/` output.
The `wasm-bindgen` CLI version must equal the `wasm-bindgen` crate version in `Cargo.lock` (currently 0.2.130).

The network is the page's origin in production. In dev it's `VITE_IRCHAD_NETWORK` in `web/.env.development` (`https://chat.localhost:8443`). If your `deploy/.env` sets another `IRCHAD_HOST`, override it in `web/.env.development.local`.
It must be `https://`, or chad-core would treat it as a classic network (D72).
The browser must trust Caddy's internal CA (`root.crt`) for both the discovery fetch and the `wss://` connection.
The dev stack's ergo has no `allowed-origins`, so any page can connect. Caddy answers `/api/*` for any origin (D79), so chadd works from Vite's origin too.
The network's own uploads and emoji load straight into `<img>`, and any other image goes through chadd (`media`, D25).
Before shipping, serve the web app from the network's own origin (a Caddy `handle`) and set `allowed-origins` in ergo.

On native targets this crate compiles to nothing.
`getrandom`'s browser backend is selected in `/.cargo/config.toml`.

## What builds on wasm32

| Crate | Status |
|---|---|
| `chad-proto` | builds unchanged |
| `chad-e2ee` (vodozemac) | builds; needs `getrandom` 0.3 + 0.4 `wasm_js` features (done in this crate) |
| `chad-irc` | builds with `default-features = false`, which drops `ws` (tokio/rustls/tungstenite) |
| `chad-core` | builds. `conn` and `manager` are compiled out; `chadd` and discovery's fetching run over the browser's `fetch`; `login`, `Net`, `view`, `format` and the rest are in. Its clock is `web-time`. Sqlite is sqlite-wasm-rs, which needs this crate's `wasm-bindgen` feature for its host functions. |
| `chad-media` | **not portable** (libwebrtc, PipeWire, cpal). Use `livekit-client` from the Vue side; Rust only hands over the URL and token. |
| `chad-desktop` | not portable as a whole (tray, PTT, overlay, update, chad-media). Individual widgets can be lifted. |

## Not yet

- **Cache**: none, by choice. `Store::memory()`, so each page load replays history from chathistory.
- **Saved password**: none, there's no keyring. A chadd session token is the likely answer.
- **E2EE DMs**: needs a pickle key, which needs somewhere to keep it (the same question).
- **Publishing a screen or camera, soundboard, whisper, games, avatars and display names**: chadd's client is here now, so these are UI work.
- **`/me`**: chad-core has no way to send an action yet.
- **Several networks**: one `Client` is one network.

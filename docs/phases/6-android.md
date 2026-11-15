# Phase 6: Android

**Goal:** chat, encrypted DMs, joining voice with the mic, and watching screen shares. **Out of scope:** camera and screen-share publishing from mobile.

Decisions: D38 (shared view model), D39 (push and backgrounding), D40 (distribution), D41 (theme).

- [x] Move the desktop's event→state projection (`chad-desktop/src/app.rs`) into chad-core as a view-model API, and port desktop onto it (D38). Done as `chad_core::view`. The desktop keeps notifications, dialogs and pane bookkeeping. Live shots weren't re-run because the dev stack's Caddy wasn't answering on :8443.
- [x] `chad-ffi`: UniFFI bindings for `chad-core` (+ `chad-e2ee`). Async events go to Kotlin Flows.
  Media is **not** shared: Android uses the LiveKit Android SDK directly.
  A `Client` object with commands and snapshot getters over `View<(), ()>`. A `Listener` callback bumps a `StateFlow` in `IrChad.kt`. Upload, profile and sound commands aren't exposed yet. They get added with their UI items below.
- [x] Gradle project in `android/` (application ID `party.gentoo.irchad`, minSdk 29), with cargo-ndk builds for arm64-v8a and x86_64 (emulator).
  Uses `android/build-rust.sh` (NDK clang directly, `-Zbuild-std` without rustup) instead of cargo-ndk. See `android/README.md`. x86_64 runs on the emulator. arm64 builds but hasn't run on a phone.
- [x] Compose UI with Material You dynamic color (D41): network rail as a drawer (multi-network, as on desktop), channel list, chat, member sheet, voice panel, screen-share viewer.
  Done: join screen, drawer (networks and channels, with who's in voice), chat with grouping, reactions shown, long-press react/delete, typing, member sheet, composer, status banner with sign-in and email code, voice panel, screen-share viewer. Also tappable links, inline images and an emoji/GIF picker (composer, and "React…" in the long-press menu). Encryption verification (SAS emoji dialog, "Verify" in the DM top bar, accept-changed-keys banner) is in.
  Formatting: desktop's `format.rs` moved to `chad_core::format` (colours as RGB), and `Message.spans` carries bold, italic, underline, strike, mono, colours and links to Compose. Replies: "Reply" in the long-press menu, a "Replying to" bar over the composer, and a quote line over a reply that jumps to its parent when it's loaded (there's no `LoadAround` yet). Also "Copy text". Tapping an inline image opens a full-screen viewer (pinch, pan, double-tap), showing the 800 px decode.
  Not checked on the emulator (the dev stack couldn't start: `/run/user` full).
- [x] Uploads: composer attach, Android share-sheet target, and the inline image viewer.
  `Client::upload` posts the link when it's done, like desktop (`Network.uploads` is the limit). The attach button (shown when the network takes uploads) picks any files. Sharing text or files to IrChad (`SEND`, `SEND_MULTIPLE`) opens a "Send to…" sheet with every channel and DM. Files are read up to the network's limit, and a bigger one is refused before upload. The viewer is the one above. Not checked on the emulator.
- [x] Link previews and remote images through chadd (D25), GIF search (D30), and the soundboard (D37).
  Done: previews, images and GIF search (`Media.kt`). chad-ffi has `preview`, `media` and `gifs` answered through `Listener`, `emojis(query)`, and `Message.links` (a copy of desktop's link finding and kinds). Images must be PNG, JPEG, GIF or WebP by magic bytes, at most 8192 px a side (2048² when animated), and are decoded by `ImageDecoder` to at most 800 px a side. The cache is capped at about 64 MB. A setting (the gear in the drawer) loads images and previews always, on unmetered networks only, or on a tap. GIF picker thumbnails always load. Not checked on the emulator yet.
  Soundboard: the note button in the voice panel lists the network's clips (`Client::sounds`/`sound`). A clip is decoded by `MediaCodec` (10 s, mono) and mixed into the mic track, because the SDK can't make a second track (D48). It's dropped while muted and plays locally too. Not checked with audio.
- [ ] Invite handling (the `irchad://` intent filter and join prefill are in. chadd's landing page links to `irchad://join?origin=…`, which the intent filter takes and `invite::parse` reads (both tested), but nobody has tapped it on a phone yet): Android App Links for `https://<origin>/invite` won't verify for arbitrary origins, so register the `irchad://` intent filter, and the landing page links to it.
- [x] Voice via the LiveKit Android SDK with a foreground service (`microphone` type), audio focus and Bluetooth headsets. The ongoing-call notification has mute, deafen and leave.
  D43. `Voice.kt`: `Calls` plays the core's calls, `VoiceService` keeps them up. The join button in the top bar asks for the mic first (plus Bluetooth and notifications); without the mic there's no voice, not even listen-only.
  Checked on the emulator against the dev stack: join, mute, deafen and leave from the panel and the notification, and the roster in the drawer. Audio both ways wasn't checked (emulator mic and speaker), and neither were Bluetooth and audio focus. Those are the SDK's defaults.
  Watching only: no screen-share or camera publishing from the phone. The viewer subscribes to a share only while it's open, and sets `watching` (D22).
  Checked with `screen_check` (now takes `ROOM=#dev`) sharing a 720p test pattern into the phone's call.
- [x] Picture-in-picture for the screen-share viewer when leaving the app.
  Enters on Home while watching (`onUserLeaveHint`). Checked on the emulator. On API 31+ it enters by itself (`setAutoEnterEnabled` while watching, so gesture nav works too). The aspect ratio follows the frame (a `VideoSink` on the watched track), clamped to Android's 2.39:1.
- [x] Rotate to landscape or pinch-zoom in the screen-share viewer. A desktop share is small on a portrait phone.
  Both: a rotate button locks the activity to landscape while the viewer is open, and pinch, pan and double-tap zoom work (the renderer is a `TextureView`, which follows the layer's transform). Not checked on the emulator.
- [x] Disconnect shortly after backgrounding, and catch up via chathistory on resume (D39).
  D49: `Command::Background` / `Client::background`. `Notifier.foreground` sends it a minute after the activity stops (not in "Stay connected"), and when the core starts with nothing on screen. A network in voice stays connected until the call ends. Resuming reconnects at once, and the usual reconnect catch-up runs. Live test `background_pauses_and_catches_up` (bob paused, alice talks, bob resumes and gets it) compiles but hasn't run, because the dev stack is down.
- [x] Push via ergo `draft/webpush` → UnifiedPush only, no FCM (D39). Turn on `webpush.enabled` in the dev ergo config. Notification content is decrypted on device for E2EE DMs.
  D47. `Notify.kt` (`Notifier`, `PushService`), `read_push` in chad-ffi, and `NetCommand::Push` / `Event::PushAvailable` in chad-core. Tapping a notification opens its conversation, and viewing it or a `MARKREAD` push clears it.
  E2EE DMs can't be decrypted from the push, because ergo drops `+chad/e2e`, so they show the fallback text (D47).
  Not checked live yet: the dev stack couldn't start (the `/run/user` tmpfs was full). Unit tests cover the REGISTER/renew/UNREGISTER lines and the push parsing.
  To check: emulator + ntfy (ntfy.sh server, since ergo refuses private IPs), `bob` gets a DM from desktop with the app swiped away.
- [x] No distributor: onboarding suggests ntfy. Otherwise the user picks a persistent socket in a foreground service, or offline.
  `BackgroundOnboarding` asks once (it picks push by itself when a distributor is installed). The same choice is in the settings gear. "Stay connected" is `KeepService` (`specialUse`).
  `BootReceiver` restarts "Stay connected" after a reboot. Coming to the front with push picked but no distributor installed resets the choice, so onboarding asks again, with a notice.
- [ ] E2EE: the phone is a new Olm device, trusted the same way as a new desktop device (Phase 4). No mobile-specific flow.
  Nothing Android-specific is left to write: chad-ffi runs the same core (`init_e2e`, the store key in `KeystoreSecrets`), and SAS and accept-keys are in. Open until a DM between the phone and desktop has been checked live.
- [x] Keystore-backed storage for credentials and E2EE keys.
  `KeystoreSecrets` uses AES-GCM with a Keystore key over SharedPreferences. The E2EE store key goes through the same `SecretStore` (`<origin>#e2e`).
- [ ] CI: add the Android SDK, NDK and cargo-ndk to the CI image (D32). Tags build a signed APK and attach it to the Gitea release with Obtainium-friendly asset names (D40).
  The CI image has JDK 21, the SDK and the NDK (no cargo-ndk, see `build-rust.sh`). `ci.yaml`'s `android` job builds a debug APK. `release.yml` builds `irchad-android-arm64.apk`, signed with the `ANDROID_KEYSTORE` secrets.
  The APK's version comes from `Cargo.toml` (`versionCode` = major·10⁶ + minor·10³ + patch). Open until the secrets are in and a tag has produced an APK that installs.
- [x] Edit your own messages from the composer, as on desktop (D51). Android already shows "(edited)"; chad-ffi has `edit` now (Phase 8), so only the Compose UI is left.
  "Edit" in a message's long-press menu puts its text in the composer under "Editing your message". Not checked on the emulator.
- [x] Desktop parity (2026-11-05), besides sharing a screen from the phone. None of it checked on the emulator or a phone yet.
  Chat: tapping a reaction takes yours back, a reply's parent that isn't loaded is fetched (`loadAround`) and scrolled to, typing notices go out, `:shortcode:` turns into emoji, `@` lists nicks (desktop's `complete` and `replace_shortcodes` moved to chad-core, so both share them), and `/gif` opens the GIF search. Avatars show people's pictures, and network chips show the network's icon (`Client::icon`).
  The top bar's ⋮ leaves a channel, closes a DM, or lists their encrypted-DM devices. Your row at the top of the drawer opens Online/Away, "Edit profile…" (display name, picture, your devices with Remove), and "Leave network…".
  Settings: "Notify me about" (every message, mentions and DMs, or nothing; `Client::notify_all`, and pushes only ever bring DMs and mentions), away after idle minutes (since the last touch), and desktop's voice settings (D61).
  Voice panel: connection quality, a speaker route picker, hold-to-talk with push to talk, and holding someone's chip for their voice and screen-share volume.
  Desktop-only on purpose: the quick switcher (the drawer does it), text size (the system's font scale does it), panes, stage layout, tray, autostart, the update check, and picking a microphone (Android picks it with the route).
- [x] Account deletion removes the account's data, for Play's rules (D69): chadd unregisters it, redacts its channel messages live, `HISTSERV FORGET`s its DMs, deletes its uploads, and clients redact their copies from `/api/redacted` after each connect.
  In chad-core, so Android, iOS and desktop all use it with no UI change. Prod needs ergo's new `chadd` oper and export dir, and `upload-owners.py` run once.

- [ ] Sign in with Discord (D96), found during Phase 6. chadd, chad-core and desktop are done; Android, iOS (`ASWebAuthenticationSession`) and web still need the button, the browser hand-off and the `irchad://oauth` redirect (chad-ffi doesn't expose `Command::DiscordStart`/`DiscordCallback`/`DiscordName` yet).
  Not done for any client: linking Discord to an existing password account, and a per-device sign-out.
- [ ] Later: a shared Discord sign-in service, so servers don't each need a Discord app. Plan in [notes/oauth-broker.md](../notes/oauth-broker.md).

## Done when
- The phone joins via an invite link, receives push while backgrounded, talks in voice with desktop users, and watches a desktop screen share.

## Notes
- TLS roots on Android: see D42. The dev-stack setup for the emulator is in `android/README.md`.

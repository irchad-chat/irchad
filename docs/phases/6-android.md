# Phase 6: Android

**Goal:** chat, encrypted DMs, joining voice with the mic, and watching screen shares. **Out of scope:** camera and screen-share publishing from mobile.

Decisions: D38 (shared view model), D39 (push and backgrounding), D40 (distribution), D41 (theme).

- [x] Move the desktop's event→state projection (`chad-desktop/src/app.rs`) into chad-core as a view-model API, and port desktop onto it (D38). Done as `chad_core::view`. The desktop keeps notifications, dialogs and pane bookkeeping. Live shots weren't re-run because the dev stack's Caddy wasn't answering on :8443.
- [x] `chad-ffi`: UniFFI bindings for `chad-core` (+ `chad-e2ee`). Async events go to Kotlin Flows.
  Media is **not** shared: Android uses the LiveKit Android SDK directly.
  A `Client` object with commands and snapshot getters over `View<(), ()>`. A `Listener` callback bumps a `StateFlow` in `IrChad.kt`. Upload, profile, GIF, sound and verification commands aren't exposed yet. They get added with their UI items below.
- [x] Gradle project in `android/` (application ID `party.gentoo.irchad`, minSdk 29), with cargo-ndk builds for arm64-v8a and x86_64 (emulator).
  Uses `android/build-rust.sh` (NDK clang directly, `-Zbuild-std` without rustup) instead of cargo-ndk. See `android/README.md`. x86_64 runs on the emulator. arm64 builds but hasn't run on a phone.
- [ ] Compose UI with Material You dynamic color (D41): network rail as a drawer (multi-network, as on desktop), channel list, chat, member sheet, voice panel, screen-share viewer.
  Done: join screen, drawer (networks and channels, with who's in voice), chat with grouping, reactions shown, long-press react/delete, typing, member sheet, composer, status banner with sign-in and email code, voice panel, screen-share viewer. Missing: formatting (stripped in chad-ffi for now), reply UI, inline images, and encryption verification.
- [ ] Uploads: composer attach, Android share-sheet target, and the inline image viewer.
- [ ] Link previews and remote images through chadd (D25), GIF search (D30), and the soundboard (D37).
- [ ] Invite handling (the `irchad://` intent filter and join prefill are in, the landing-page link isn't checked): Android App Links for `https://<origin>/invite` won't verify for arbitrary origins, so register the `irchad://` intent filter, and the landing page links to it.
- [x] Voice via the LiveKit Android SDK with a foreground service (`microphone` type), audio focus and Bluetooth headsets. The ongoing-call notification has mute, deafen and leave.
  D43. `Voice.kt`: `Calls` plays the core's calls, `VoiceService` keeps them up. The join button in the top bar asks for the mic first (plus Bluetooth and notifications); without the mic there's no voice, not even listen-only.
  Checked on the emulator against the dev stack: join, mute, deafen and leave from the panel and the notification, and the roster in the drawer. Audio both ways wasn't checked (emulator mic and speaker), and neither were Bluetooth and audio focus. Those are the SDK's defaults.
  Watching only: no screen-share or camera publishing from the phone. The viewer subscribes to a share only while it's open, and sets `watching` (D22).
  Checked with `screen_check` (now takes `ROOM=#dev`) sharing a 720p test pattern into the phone's call.
- [x] Picture-in-picture for the screen-share viewer when leaving the app.
  Enters on Home while watching (`onUserLeaveHint`). Checked on the emulator. Not done: auto-enter on gesture nav (API 31 `setAutoEnterEnabled`), and an aspect ratio taken from the frame (it's fixed at 16:9).
- [ ] Rotate to landscape or pinch-zoom in the screen-share viewer. A desktop share is small on a portrait phone.
- [ ] Disconnect shortly after backgrounding, and catch up via chathistory on resume (D39).
- [ ] Push via ergo `draft/webpush` → UnifiedPush only, no FCM (D39). Turn on `webpush.enabled` in the dev ergo config. Notification content is decrypted on device for E2EE DMs.
- [ ] No distributor: onboarding suggests ntfy. Otherwise the user picks a persistent socket in a foreground service, or offline.
- [ ] E2EE: the phone is a new Olm device, trusted the same way as a new desktop device (Phase 4). No mobile-specific flow.
- [x] Keystore-backed storage for credentials and E2EE keys.
  `KeystoreSecrets` uses AES-GCM with a Keystore key over SharedPreferences. The E2EE store key goes through the same `SecretStore` (`<origin>#e2e`).
- [ ] CI: add the Android SDK, NDK and cargo-ndk to the CI image (D32). Tags build a signed APK and attach it to the Gitea release with Obtainium-friendly asset names (D40).
  The CI image has JDK 21, the SDK and the NDK (no cargo-ndk, see `build-rust.sh`). `ci.yaml`'s `android` job builds a debug APK. `release.yml` builds `irchad-android-arm64.apk`, signed with the `ANDROID_KEYSTORE` secrets.
  The APK's version comes from `Cargo.toml` (`versionCode` = major·10⁶ + minor·10³ + patch). Open until the secrets are in and a tag has produced an APK that installs.

## Done when
- The phone joins via an invite link, receives push while backgrounded, talks in voice with desktop users, and watches a desktop screen share.

## Notes
- TLS roots on Android: see D42. The dev-stack setup for the emulator is in `android/README.md`.

# Phase 6: Android

**Goal:** chat, encrypted DMs, joining voice with the mic, and watching screen shares. **Out of scope:** camera and screen-share publishing from mobile.

Decisions: D38 (shared view model), D39 (push and backgrounding), D40 (distribution), D41 (theme).

- [ ] Move the desktop's event→state projection (`chad-desktop/src/app.rs`) into chad-core as a view-model API, and port desktop onto it (D38).
- [ ] `chad-ffi`: UniFFI bindings for `chad-core` (+ `chad-e2ee`). Async events go to Kotlin Flows.
  Media is **not** shared: Android uses the LiveKit Android SDK directly.
- [ ] Gradle project in `android/` (application ID `party.gentoo.irchad`, minSdk 29), with cargo-ndk builds for arm64-v8a and x86_64 (emulator).
- [ ] Compose UI with Material You dynamic color (D41): network rail as a drawer (multi-network, as on desktop), channel list, chat, member sheet, voice panel, screen-share viewer.
- [ ] Uploads: composer attach, Android share-sheet target, and the inline image viewer.
- [ ] Link previews and remote images through chadd (D25), GIF search (D30), and the soundboard (D37).
- [ ] Invite handling: Android App Links for `https://<origin>/invite` won't verify for arbitrary origins, so register the `irchad://` intent filter, and the landing page links to it.
- [ ] Voice via the LiveKit Android SDK with a foreground service (`microphone` type), audio focus and Bluetooth headsets. The ongoing-call notification has mute, deafen and leave.
- [ ] Picture-in-picture for the screen-share viewer when leaving the app.
- [ ] Disconnect shortly after backgrounding, and catch up via chathistory on resume (D39).
- [ ] Push via ergo `draft/webpush` → UnifiedPush only, no FCM (D39). Turn on `webpush.enabled` in the dev ergo config. Notification content is decrypted on device for E2EE DMs.
- [ ] No distributor: onboarding suggests ntfy. Otherwise the user picks a persistent socket in a foreground service, or offline.
- [ ] E2EE: the phone is a new Olm device, trusted the same way as a new desktop device (Phase 4). No mobile-specific flow.
- [ ] Keystore-backed storage for credentials and E2EE keys.
- [ ] CI: add the Android SDK, NDK and cargo-ndk to the CI image (D32). Tags build a signed APK and attach it to the Gitea release with Obtainium-friendly asset names (D40).

## Done when
- The phone joins via an invite link, receives push while backgrounded, talks in voice with desktop users, and watches a desktop screen share.

# Phase 6: Android

**Goal:** chat, encrypted DMs, joining voice with the mic, and watching screen shares. **Out of scope:** camera and screen-share publishing from mobile.

- [ ] `chad-ffi`: UniFFI bindings for `chad-core` (+ `chad-e2ee`). Async events go to Kotlin Flows.
  Media is **not** shared: Android uses the LiveKit Android SDK directly.
- [ ] Gradle project in `android/`, with cargo-ndk builds for arm64-v8a and x86_64 (emulator).
- [ ] Compose UI: network rail as a drawer, channel list, chat, member sheet, voice panel, screen-share viewer.
- [ ] Invite handling: Android App Links for `https://<origin>/invite` won't verify for arbitrary origins, so register the `irchad://` intent filter, and the landing page links to it.
- [ ] Voice via the LiveKit Android SDK with a foreground service (`microphone` type), audio focus and Bluetooth headsets.
- [ ] Push via ergo `draft/webpush` → UnifiedPush (with an FCM distributor fallback). Notification content is decrypted on device for E2EE DMs.
- [ ] Keystore-backed storage for credentials and E2EE keys.

## Done when
- The phone joins via an invite link, receives push while backgrounded, talks in voice with desktop users, and watches a desktop screen share.

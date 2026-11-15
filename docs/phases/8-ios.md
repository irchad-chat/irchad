# Phase 8: iOS and iPadOS

**Goal:** the Android app's feature set on iPhone and iPad, headed for the App Store: chat, encrypted DMs, voice with the mic, and watching screen shares. **Out of scope:** screen-share and camera publishing from iOS.

Decisions: D58 (TLS roots), D59 (shape of the app), D60 (notifications while suspended).

- [x] Build on Linux with xtool: `iOS/build-rust.sh` cross-compiles `chad-ffi` to a static library for `aarch64-apple-ios` (clang plus xtool's iPhoneOS SDK, `-Zbuild-std` without rustup) and generates the Swift bindings. SwiftPM links it through the `chad_ffiFFI` C target. iOS 18 and later.
- [x] TLS: rustls-native-certs finds no roots on iOS, so chad-irc and reqwest use rustls-platform-verifier there (D58).
- [x] chad-ffi gains what desktop has and Android lacked: `edit` (D51), `typing`, `load_around`, `set_display_name`, `upload_avatar`/`clear_avatar`, `set_away`, `fetch_devices`/`devices`/`remove_device` (D52) and `profile`. Android can use them too.
- [x] SwiftUI app (`iOS/Sources/IrChad`): a `NavigationSplitView` (a sidebar on iPad, a stack on iPhone) with every network as a section of channels and DMs, who's in voice, unread and mention badges, and swipe to leave or close.
  Join screen (log in, sign up, guest), status banner with sign-in and email code, chat with grouping, formatting and links (`Span`s), reply quotes that jump to the parent (`load_around` when it isn't loaded), reactions (tap one to add or take back yours), a context menu (reply, react, copy, edit, delete), typing, members sheet, composer with reply and edit bars, and hardware-keyboard Return to send.
- [x] Encrypted DMs: lock icons, "Verify" in a DM's toolbar, the SAS emoji sheet, and the accept-changed-keys banner. Credentials and each network's E2EE store key live in the keychain (this device only, after first unlock), and the data directory is excluded from backups (D59).
- [x] Uploads (photos and videos through the system picker, converted to JPEG/PNG/GIF/MP4 where needed, and any file through Files), limited to the network's `uploads`. Inline images and GIFs (ImageIO, bounded as on Android), link previews, file cards, a full-screen viewer with pinch zoom, and the emoji and GIF pickers. Images load always, on Wi‑Fi only, or on a tap (Settings).
- [x] Profile sheet per network: picture, display name, away (all networks), and this account's encrypted-DM devices with removal.
- [x] Voice via the LiveKit Swift SDK behind chad-ffi's `Voice` (D43): mute, deafen, the audio-route picker (speaker, Bluetooth), who's speaking, and the soundboard through the SDK's `SoundPlayer`, which mixes clips into the mic (D59). The `audio` background mode keeps a call going with the app in the background.
- [x] Watching a screen share full screen, subscribed only while open (D22). Rotate or pinch to zoom, and picture-in-picture when the app goes to the background.
- [x] Backgrounding: connections pause 20 s after the app leaves the screen, inside a background task, except a network in voice (D49). Coming back reconnects and catches up.
- [x] Local notifications for DMs and mentions while the core still runs in the background (in a call, or just after leaving), the app badge as the mention count, and tapping one opens its conversation.
- [x] `irchad://` invites open the join screen (`CFBundleURLTypes`).
- [ ] Run it on an iPhone and an iPad against a real server. Nothing has run on a device yet.
- [ ] Push while suspended (D60): pick and run a Web Push → APNs relay, then a Notification Service Extension that decrypts (RFC 8291, CryptoKit) and calls `read_push`. Needs a paid Apple developer account for the `aps-environment` entitlement.
- [ ] Share extension: send text, photos and files to a channel or DM from other apps (Android's "Send to…" sheet). It runs in its own process, so it needs an app group and a way to hand the upload to the app.
- [ ] Ongoing-call controls outside the app (Android's notification with mute, deafen and leave): a Live Activity or CallKit.
- [ ] App Store: a privacy manifest (`PrivacyInfo.xcprivacy`) for the app's own API use, the export-compliance answer (TLS and Olm), screenshots, and a release build (`PROFILE=release`, `xtool dev build -c release --ipa`).
  - [x] Distribution certificate, App Store profile and App Store Connect API key for `chat.irchad.IrChad` (kept in `~/irchad-signing`, see `iOS/README.md`).
  - [x] `PrivacyInfo.xcprivacy` (UserDefaults, file timestamps, disk space; account ID, messages, media and other content for app functionality).
  - [x] `iOS/release.sh` + `finish-app.py`: xtool can't sign for distribution, writes no DT* keys and stamps the deployment target as the SDK version (xtool #351), so the script fixes these and signs with rcodesign.
  - [ ] `iOS/icon/`: Assets.car built once on a Mac with `actool` (no Linux compiler that App Store processing accepts yet).
  - [ ] First TestFlight upload (`upload.py`, Build Uploads API, since Transporter on Linux needs a Mac-made AppStoreInfo.plist); answer export compliance there, then set `ITSAppUsesNonExemptEncryption`.
- [ ] CI: no job builds iOS. The CI image would need Swift and xtool's SDK, which comes out of Xcode's `.xip`.

## Done when
- An iPhone and an iPad join via an invite link, chat and DM with desktop users (encrypted, verified), talk in voice, and watch a desktop screen share, including in picture-in-picture.

## Notes
- Building and testing: `iOS/README.md`.
- Desktop features still missing on mobile: `:shortcode:` replacement and tab completion in the composer, auto-away when idle, per-person volume, and the soundboard's own volume.

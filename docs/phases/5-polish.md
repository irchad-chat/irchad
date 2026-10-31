# Phase 5: Polish & distribution

**Goal:** something you can hand to a Discord friend.

## Features
- [x] Display names and avatars via metadata (`display-name`, `avatar`). Avatar upload goes through chadd.
- [x] Uploads: `chadd POST /api/upload` with size limits and content-type sniffing, advertised in discovery and via ISUPPORT `FILEHOST` if ergo allows custom tokens.
  Drag-and-drop and paste into the composer. (D26; FILEHOST via `server.additional-isupport`.)
- [x] Inline image, GIF and video-thumbnail rendering for links, with a size cap and a click-to-load setting for privacy.
- [x] Link previews. Decide whether they're fetched client-side, which leaks the user's IP, or by chadd as a proxy, and record a decision. (D25: chadd proxy.)
- [x] Emoji picker and shortcodes (`:smile:`), plus reactions from the picker.
- [x] GIF picker: a GIF tab in the composer's picker (and a GIF button), searching KLIPY through chadd (D30). Clicking one sends its URL.
  Checked by unit tests only. Still to do: a live run against KLIPY to confirm its response matches Tenor v2, a screenshot scenario, and the attribution assets from KLIPY's branding guidelines.
- [x] Away status from idle (ext-idle-notify / XScreenSaver / Windows), and manual status.
- [x] Keyboard navigation (Ctrl+K quick switcher, Alt+↑/↓ for channels), and accessibility via AccessKit.

## Distribution
- [x] **Flatpak** first. Portals come naturally. Mind the manifest permissions: PipeWire socket, D-Bus names for SNI and notifications.
- [x] AppImage or a plain tarball for non-Flatpak users.
- [x] Windows installer (MSI via `cargo-wix` or NSIS) with scheme handler registration. Update checks, at least a notification.
- [x] Release CI (Gitea Actions on `git.gentoo.party`): tagged builds attach the Flatpak bundle and the Windows installer to a Gitea release, and push a chadd container image to the Gitea container registry.
- [x] Pre-release checklist: run every `cargo fuzz` target (e.g. `crates/chad-irc/fuzz`) for a while before tagging. Fuzzing stays out of CI.
- [x] Operator docs: "host your own network in 15 minutes" (compose, DNS, ports for LiveKit UDP/TURN, ergo admin basics).

## Deferred from Phase 1
- [x] The client works in two networks at once. (`two_networks_upload_profile` live test.)
- [ ] Verify on GNOME Wayland and KDE Wayland: notification popups, SNI tray (GNOME needs the AppIndicator extension), and IME (GNOME usually means ibus).
- [ ] `irchad://` links from a browser, and single-instance forwarding, on Linux and Windows.

## Found while building
- [ ] Video "thumbnails" are a card with a play icon, not a decoded frame.
- [ ] Build the Flatpak bundle for real (`flatpak-builder` needs ~GBs of SDKs; not run locally) and check portals, PipeWire, tray and notifications inside it.
- [ ] Run the release workflow on a tag: needs a privileged-container act_runner (Flatpak), a Windows runner with NSIS, and the `REGISTRY_TOKEN` secret. Smoke-test the NSIS installer and its `irchad://` registration.
- [ ] Drag-and-drop on Wayland: winit 0.30 has none, so paste and the paperclip file dialog cover it. Revisit when winit lands it.
- [ ] egui `DragValue`/`Slider` value fields are labelled via `labelled_by`; recheck with Orca.

## Done when
- A non-technical tester installs from a release, joins via an invite link, and uses chat, voice and screen share without help.

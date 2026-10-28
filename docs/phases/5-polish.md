# Phase 5: Polish & distribution

**Goal:** something you can hand to a Discord friend.

## Features
- [ ] Display names and avatars via metadata (`display-name`, `avatar`). Avatar upload goes through chadd.
- [ ] Uploads: `chadd POST /api/upload` with size limits and content-type sniffing, advertised in discovery and via ISUPPORT `FILEHOST` if ergo allows custom tokens.
  Drag-and-drop and paste into the composer.
- [ ] Inline image, GIF and video-thumbnail rendering for links, with a size cap and a click-to-load setting for privacy.
- [ ] Link previews. Decide whether they're fetched client-side, which leaks the user's IP, or by chadd as a proxy, and record a decision.
- [ ] Emoji picker and shortcodes (`:smile:`), plus reactions from the picker.
- [ ] Away status from idle (ext-idle-notify / XScreenSaver / Windows), and manual status.
- [ ] Keyboard navigation (Ctrl+K quick switcher, Alt+↑/↓ for channels), and accessibility via AccessKit.

## Distribution
- [ ] **Flatpak** first. Portals come naturally. Mind the manifest permissions: PipeWire socket, D-Bus names for SNI and notifications.
- [ ] AppImage or a plain tarball for non-Flatpak users.
- [ ] Windows installer (MSI via `cargo-wix` or NSIS) with scheme handler registration. Update checks, at least a notification.
- [ ] Release CI (Gitea Actions on `git.gentoo.party`): tagged builds attach the Flatpak bundle and the Windows installer to a Gitea release, and push a chadd container image to the Gitea container registry.
- [ ] Pre-release checklist: run every `cargo fuzz` target (e.g. `crates/chad-irc/fuzz`) for a while before tagging. Fuzzing stays out of CI.
- [ ] Operator docs: "host your own network in 15 minutes" (compose, DNS, ports for LiveKit UDP/TURN, ergo admin basics).

## Done when
- A non-technical tester installs from a release, joins via an invite link, and uses chat, voice and screen share without help.

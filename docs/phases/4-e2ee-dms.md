# Phase 4: Encrypted DMs

**Goal:** IrChad↔IrChad DMs are end-to-end encrypted by default. They work while the peer is offline and across each user's devices. See decision D8 for why Olm and not OTR or MLS.

## Design (validate against `docs/notes/ergo-caps.md` before building)
- **Device keys:** each device has an Olm account (Curve25519 identity + Ed25519 signing) via `vodozemac`.
  The pickled account is stored in the OS secret store, or encrypted with a key kept there.
- **Publication:**
  - user metadata `chad/e2e/devices` lists the device IDs
  - `chad/e2e/d/<id>` holds the identity keys plus a signed **fallback key**
  If metadata size or key-count limits are too small, move this to a chadd key directory authenticated by the session token, and record a decision.
- **Prekeys:** start with fallback keys only, rotated weekly. That trades some first-message forward secrecy for simplicity, so document it.
  One-time keys via chadd are a later improvement.
- **Transport:** each recipient device, *and each of the sender's own other devices*, gets its own `PRIVMSG <nick>` carrying:
  - `+chad/e2e=<base64 ciphertext>`
  - `+chad/e2e-to=<device id>`
  - body `🔒 Encrypted message, open IrChad to read`, so plain clients get a readable fallback
  Devices ignore messages addressed to another device.
  - Check the client tag size budget (4094 bytes) and split long messages.
  - If tags don't survive chathistory, fall back to the ciphertext in the body with a prefix.
- **History:** an Olm message decrypts only once, so plaintext is persisted locally (encrypted SQLite) at first decrypt. A new device doesn't get old DMs, as in Signal.
- **Trust:** TOFU, with a loud warning when a device key changes. SAS emoji verification (vodozemac `sas`) runs over `TAGMSG +chad/e2e-verify`, and verified peers get a badge.
- **Policy:** encrypt automatically when the peer publishes devices, and show plaintext DMs with a clear "not encrypted" marker. No E2EE in channels.

## Tasks
- [ ] `chad-e2ee` crate: account and session management, session storage, and a "Olm message pending decrypt" retry queue.
- [ ] Key publication and refresh on connect. Handle a device being removed from the device list.
- [ ] Encrypt/decrypt integrated in `chad-core`'s DM send and receive paths.
- [ ] UI: lock indicator per DM, device list in the profile, verification flow, key-change warning.
- [ ] "Log out this device" deletes local keys and unpublishes the device.
- [ ] Threat model note in `docs/notes/e2ee-threat-model.md` covering:
  - the server can see metadata (who, when, size)
  - the server can inject fake devices, which verification mitigates
  - no post-quantum protection

## Done when
- A DM between user A (2 devices) and user B (1 device) decrypts on all three devices.
- This works when B was offline during sending.
- SAS verification works.
- An irssi user DMing an IrChad user gets plaintext with a "not encrypted" marker.
- Tampered ciphertext is rejected visibly.

# E2EE threat model (Phase 4)

What encrypted DMs protect against and what they don't. The design is D8 plus D29.
The adversary that matters is the server operator, or whoever controls the box (D14). They can read, drop, delay, replay, reorder and forge anything on the wire, and they can edit metadata.

## Protected

- **Message text** of a DM between two IrChad users with accounts. Only the devices listed when it was sent can read it: theirs, and our other ones.
- **Tampering.** A changed ciphertext fails Olm's MAC, and the message shows as "couldn't be decrypted" instead of quietly vanishing or showing altered text.
- **Replay.** A replayed copy is either the same msgid (shown once, from the cache) or a message key that's already used (shown as couldn't be decrypted). A pre-key message replayed after its session aged out (more than four sessions with that device since) could start a fresh session and show twice. That's unlikely, and visible.
- **Forward secrecy** after the first exchange: the double ratchet means stealing a device's keys later doesn't open past messages. That doesn't cover the cache, below.

## Not protected

- **Metadata.** The server sees who DMs whom, when, how often and how long each message is (ciphertext length tracks plaintext length).
  It also sees reactions (emoji in the clear), reply links, typing, deletions and read markers, because those are plain tags and commands.
- **Fake devices.** The server can publish a device record of its own under someone's nick. It has to sign it with its own key, so it can't claim the real device's key.
  The client encrypts to every listed device, so a fake device reads everything from then on. TOFU doesn't catch a *new* device, only a *changed* one.
  Emoji verification catches it: the DM shows "Verified" only while every listed device is verified, so a new one drops the badge. People who care should verify, and look again when the badge goes.
- **Changed keys.** Replacing a known device's keys shows a warning in the DM until the user verifies or accepts it. Accepting without verifying is the user trusting the server.
- **Downgrade.** If the server hides someone's records, the client sees no devices and sends in the clear, marked "Not encrypted". There's no "this person used to have keys" check yet.
- **First message.** There are fallback keys only, no one-time keys. The first message to a device that hasn't answered yet has weaker forward secrecy: compromising that device's fallback key within the week (or the one after) opens it.
- **Post-quantum.** None. Olm's handshake is X25519. A recorded conversation could be read by a future quantum computer.
- **The device itself.** Decrypted messages sit in the per-network SQLite cache, unencrypted beyond the disk's own encryption.
  The Olm keys are encrypted with a key from the OS secret store, which any process running as the user can usually ask for.
  Malware on the device wins.
- **Guests and plain clients.** No account, so no published keys: their DMs are always in the clear, and marked.
- **Channels.** Never encrypted.

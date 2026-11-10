---
layout: ../../layouts/Docs.astro
title: Encrypted DMs
description: How end-to-end encrypted DMs work in IrChad, what they protect, and how to verify someone.
order: 5
---

DMs between two people using IrChad with accounts are end-to-end encrypted, automatically. Only your devices and theirs can read them. The network's admin can't.

There's nothing to turn on.

> **Server setting:** `ircd.yaml` → `metadata.enabled: true` and `metadata.operator-only-modification: false`. Each device publishes its public keys as metadata, so `metadata.max-keys` (100 by default) has to leave room for one entry per device.

## What the lock tells you

Every DM shows its state at the top.

| Shows | Means |
|---|---|
| **Encrypted** | End-to-end encrypted. Verify to be sure nobody is in the middle. |
| **Verified** | Encrypted, and you compared emoji with them |
| **Keys changed** | One of their devices has a new key since you last talked. Verify again, or **Accept the new keys** if you trust it. |
| **Not encrypted** | They don't use IrChad here, or they're a guest. The network can read these messages. |

A message that was tampered with on the way shows as "This message couldn't be decrypted."

## Verifying someone

Verifying proves you're talking to their real devices, not one the server slipped in.

1. Open the DM and choose **Verify…**, then **Start**.
2. They accept on their side. You both see the same row of emoji.
3. Compare them over something you trust, such as a voice call or in person.
4. Both press **They match**.

The DM shows **Verified** only while every one of their devices is verified. If the badge goes away, they've added a device (or someone has added one for them), so verify again.

## Your devices

Each install of IrChad is its own device with its own keys. **Edit profile…** lists them, with **Remove** for ones you no longer use.

- A new device can't read DMs from before it existed.
- **Leave network…** deletes this device's keys, so it can't read its old encrypted DMs again.
- On Linux, encryption needs a working keyring (Secret Service). Without one, DMs are sent unencrypted and marked that way.

## What isn't hidden

Encryption covers the text of DMs. The network still sees:

- who you DM, when, and roughly how long each message is,
- reactions, which message a reply points at, typing, deletions and read receipts.

Also not encrypted: **channels**, **voice and video** (encrypted on the way to the server, but not end to end), and decrypted messages stored on your own device.

People on ordinary IRC clients can't read encrypted DMs. They see "🔒 Encrypted message, open IrChad to read". When you DM them, it goes unencrypted and is marked.

The full details are in the [threat model](https://git.gentoo.party/sam/irchad-ng/src/branch/main/docs/notes/e2ee-threat-model.md). In IRC terms this is Olm (the double ratchet Matrix uses), with keys in user metadata and ciphertext in message tags.

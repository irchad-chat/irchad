---
layout: ../../layouts/Docs.astro
title: Docs
description: How to use IrChad, from joining your first network to voice, screen share and encrypted DMs.
order: 0
---

IrChad is a chat app with channels, voice, screen share and encrypted DMs. It connects to **networks** that people host themselves, the way a Discord app connects to servers, except nobody in the middle owns them.

You don't need to know anything else to use it. Start with [Getting started](/docs/getting-started).

## Guides

| Page | What's in it |
|---|---|
| [Getting started](/docs/getting-started) | Install, join a network from an invite link, make an account |
| [Chat](/docs/chat) | Channels, DMs, replies, reactions, uploads, GIFs, notifications, your profile |
| [Voice](/docs/voice) | Voice channels, push to talk, whisper, the soundboard |
| [Screen share and camera](/docs/screen-share) | Sharing a screen or window with sound, and watching |
| [Encrypted DMs](/docs/encrypted-dms) | What's encrypted, and how to verify someone |
| [Phones and tablets](/docs/mobile) | What's different on Android and iOS |
| [Server settings](/docs/server-settings) | Every feature that depends on how the network is set up |

## Words you'll see

| Word | Meaning |
|---|---|
| Network | One community's server, at one web address. Discord calls this a "server". You can be in several. |
| Channel | A room, named with a `#`, like `#general`. Every channel has text and a voice room. |
| DM | A private conversation with one person. |
| Guest | Someone using a network without an account. |
| Admin | Whoever runs the network you joined. Not IrChad's developers. |

## "Server setting" notes

Some features only work if the network's admin has turned them on. Those are marked like this:

> **Server setting:** `chadd.toml` → `[uploads]` must be present.

If a feature is missing for you, that note is what to send your admin. `ircd.yaml` and `chadd.toml` are the two config files of a network, and [Server settings](/docs/server-settings) lists every such note in one table.

## If you know IRC

IrChad is an IRC client underneath. A network is an [ergo](https://ergo.chat) IRC server, plus [LiveKit](https://livekit.io) for voice and video and a small helper called chadd.
Your account is a NickServ account, channels are IRC channels, and people on ordinary IRC clients can sit in the same channels and talk to you.
Where it helps, these pages say what a feature is in IRC terms.

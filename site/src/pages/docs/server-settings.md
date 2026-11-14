---
layout: ../../layouts/Docs.astro
title: Server settings
description: Which IrChad client features depend on a network's ircd.yaml or chadd.toml, and the setting each one needs.
order: 7
---

Some features depend on how the network is set up. If something is missing for you, find it here and send the row to your network's admin.

A network has two config files:

- **`ircd.yaml`** configures ergo, the IRC server.
- **`chadd.toml`** configures chadd, IrChad's helper server.

This page is not a hosting guide. For that, see [Host your own network](https://git.gentoo.party/sam/irchad-ng/src/branch/main/docs/hosting.md). The network needs ergo 2.19 or newer.

## chadd.toml

| Feature | Setting | Without it |
|---|---|---|
| [Signing up](/docs/getting-started#3-choose-how-to-join) in the app | `registration = "open"` | "… isn't taking sign-ups." |
| [Home and official channels](/docs/getting-started#4-youre-in) | `home = "#channel"` | New people join every public channel |
| Network name and icon in the rail | `name`, `icon` | A letter instead of the icon |
| [Voice, screen share, camera](/docs/voice) | `[livekit]` → `key`, `secret` (required) | chadd won't start |
| [Meetings](/docs/meetings) guests join from a link | `[meetings]` → `secret` | No **New meeting** |
| [Uploads](/docs/chat#files-images-and-links) and profile pictures | `[uploads]` → `dir` | "This network doesn't take uploads." |
| Upload size limit | `[uploads]` → `max_bytes` | 25 MB |
| [GIF search](/docs/chat#writing-messages) | `[gifs]` → `key` | "This network doesn't have GIFs." |
| [Custom emoji](/docs/chat#custom-emoji) | `[emoji]` → `dir` | Only the standard emoji |
| [Soundboard](/docs/voice#soundboard) | `[sounds]` → `dir` | "This server has no sounds yet." |
| [Game names in activity](/docs/chat#activity) | `discord_games = true` | Only Rich Presence games show |
| [Deleting your data with your account](/docs/getting-started#leaving-a-network-deleting-your-account) | `[delete]` | Only the account is removed |

Link previews and images from other sites need no setting. `[cache]` only makes them faster.

## ircd.yaml

| Feature | Setting | Without it |
|---|---|---|
| [Signing up](/docs/getting-started#3-choose-how-to-join) | `accounts.registration.enabled: true` and `accounts.registration.allow-before-connect: true` | Only admins can make accounts |
| Email code at sign-up | `accounts.registration.email-verification.enabled: true` | No email is asked for |
| Time to enter the email code | `server.idle-timeouts.registration: 5m` | The app reconnects while you wait |
| [Guests](/docs/getting-started#guest-or-account) | `accounts.require-sasl.enabled: false` | Everyone needs an account |
| Username is your name in chat | `accounts.nick-reservation.force-nick-equals-account: true` | Names and accounts can differ |
| [Staying in channels while offline](/docs/getting-started#4-youre-in), one identity across devices | `accounts.multiclient.enabled: true`, `accounts.multiclient.always-on: "opt-out"` | You leave when you close the app |
| Message history | `history.enabled: true` | No scrollback from the server |
| History that survives a server restart | `history.persistent.enabled: true`, plus a database such as `datastore.sqlite.enabled: true` | History is lost on restart |
| [Deleting messages](/docs/chat#replies-reactions-edits-deletes) | `history.retention.allow-individual-delete: true` | Delete fails |
| Reactions in history | `history.tagmsg-storage.whitelist` includes `+draft/react` | You see reactions only while online |
| [Read receipts](/docs/chat#read-receipts) while offline | `history.tagmsg-storage.whitelist` includes `+chad/read` | Receipts arrive only while online |
| Display names, pictures, activity, who's in voice, read receipts, [encrypted DMs](/docs/encrypted-dms) | `metadata.enabled: true`, `metadata.operator-only-modification: false` | None of these work |
| Long pasted messages | `limits.multiline.max-bytes`, `limits.multiline.max-lines` | Pastes are split into more messages |
| [Deleting your data with your account](/docs/getting-started#leaving-a-network-deleting-your-account) | `history.retention.enable-account-indexing: true` | Only the account is removed |
| [Push notifications](/docs/mobile#notifications) on Android | `webpush.enabled: true` | Use "Stay connected" instead |

## Set per channel

Channel operators set these on a channel from any IRC client. They aren't in a config file.

| Feature | Setting |
|---|---|
| [Turn voice off](/docs/voice#joining-and-leaving) in a channel | `/quote METADATA #channel SET chad/voice off` |
| [Official channels](/docs/getting-started#4-youre-in) everyone is added to | On the home channel: `/quote METADATA #home SET chad/channels :#one #two` |
| Who gets reports (the phone apps' **Report** button, shown only when this is set) | On the home channel: `/quote METADATA #home SET chad/report modname`. Reports arrive as a DM starting with `[Report]` |

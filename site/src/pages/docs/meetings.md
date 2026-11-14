---
layout: ../../layouts/Docs.astro
title: Meetings
description: "Meetings in IrChad: a call with video, screen share and chat that anyone can join from a link, no account needed."
order: 4.2
---

A meeting is a call you can share with anyone, including people who have never used IrChad. They open the link in a browser, type a name and they're in. They don't need an account, a password or an install.

## Starting a meeting

- **Desktop:** network menu (**⋯** next to the network's name) → **New meeting**.
- **Browser:** **New meeting** under the channel list.

The link is copied for you, and you join the meeting's call. Paste the link wherever you like.
The meeting shows in your channel list as **Meeting**. Its right-click menu (desktop) or link button (browser) copies the link again.

## Joining one

- **From the link, in a browser:** pick **Join in browser**, type your name, choose whether your mic and camera start on, then **Join now**.
  You see everyone's camera in a grid. A screen share takes the big spot. The bar at the bottom has mute, camera, screen share, chat, copy link and leave.
- **From the link, in the app:** pick **Open in IrChad**. If you're already on that network, you join the meeting right away.
- **People on the network** can also paste the link into **Join a network**, or join its channel (`#meet-…`) like any other.

Guests show with **(guest)** after their name, so nobody can pass as a member.

## When it ends

A meeting is its call. When you leave the call, you leave its chat too. Once everyone has left, the call and its chat are gone, and nothing said in it is kept.
The link keeps working for a day (the server can change how long). Opening it again after everyone has left starts a fresh, empty meeting. After that, the link says it has expired.

Anyone with the link can get in while it works, so share it only with people you'd let in. A channel operator can kick someone from its chat with a normal IRC client.

> **Server setting:** `chadd.toml` → `[meetings]` with a `secret` of at least 32 characters, plus optionally `ttl_hours` (how long a link works, default 24). Guests need `ircd.yaml` → `accounts.require-sasl.enabled: false`, which is ergo's default.
> The browser page is the web app at `<origin>/chat/`. Set `[endpoints]` → `web` if it lives elsewhere.

---
layout: ../../layouts/Docs.astro
title: Phones and tablets
description: "What's different about IrChad on Android and iOS: installing, notifications, and what phones can't do yet."
order: 6
---

The phone apps do chat, encrypted DMs, voice, and watching screen shares. Your phone and your PC are the same account, so you log in with **I have an account**.

## Android

**Install:** download the APK from the [home page](/#download), or add IrChad to [Obtainium](https://obtainium.imranr.dev) to get updates. Needs Android 10 or newer.

**Google Play beta:** join [irchad-testers](https://groups.google.com/g/irchad-testers) with your Google account, then [opt in on Google Play](https://play.google.com/apps/testing/chat.irchad.IrChad) and [install it from the Play Store](https://play.google.com/store/apps/details?id=chat.irchad.IrChad). Play can take a few minutes to notice you joined.

**Join:** tap **Click to Join** on an invite page, or paste the link in the app.

### Notifications

Android closes an app's connection in the background, so the app asks once how you want notifications. Change it later from the gear in the drawer.

| Choice | How it works |
|---|---|
| **Push** | The network sends notifications through a UnifiedPush app such as [ntfy](https://ntfy.sh), which you install separately. Best for battery. Only DMs and mentions are pushed. |
| **Stay connected** | IrChad keeps its own connection open, with a permanent notification. Uses more battery. Not in the Play Store build. |
| **Off** | No notifications while the app is closed. You catch up when you open it. |

A push for an encrypted DM says "🔒 Encrypted message, open IrChad to read". The text is decrypted when you open the app.

> **Server setting:** push needs `ircd.yaml` → `webpush.enabled: true`.

## iOS and iPadOS

Not released yet. The app exists and is being tested. Until push support is built, it will notify only during a call or just after you leave the app.

## What phones don't do

| Feature | On phones |
|---|---|
| Sharing your screen or camera | No. Watching works, with pinch to zoom and picture-in-picture. |
| Whisper | You hear whispers, but can't send them |
| Soundboard | Works. Others hear your clips at your voice volume, not their soundboard volume. |
| Activity (game, music) | No |
| Panes, quick switcher, text size | No. The system's font size applies. |
| Choosing a microphone | Android and iOS pick it with the audio route (speaker, earpiece, Bluetooth) |

Everything in [Chat](/docs/chat) otherwise applies. Where desktop says hover or right-click, long-press the message or person.

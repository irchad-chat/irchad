---
layout: ../../layouts/Docs.astro
title: Voice
description: "Voice channels in IrChad: joining, push to talk, whisper, the soundboard and audio settings."
order: 3
---

Every channel has a voice room. You need an [account](/docs/getting-started#guest-or-account) to use it. Guests can't join voice, except in a [meeting](/docs/meetings).

## Joining and leaving

- Click the **speaker** next to a channel, or right-click the channel → **Join voice**.
- People in a channel's voice room are listed under it, whether or not you've joined.
- You're in one call at a time, across all your networks.
- The voice panel at the bottom of the channel list has **Mute**, **Deafen** (mutes you and silences everyone else), **Disconnect**, and your connection quality.

> **Server setting:** voice works on every channel unless a channel operator turns it off for that channel by setting the channel's `chad/voice` metadata to `off`. From an IRC client: `/quote METADATA #channel SET chad/voice off`.
>
> The network itself needs `chadd.toml` → `[livekit]` (`key`, `secret`) matching its LiveKit server, and `ircd.yaml` → `metadata.enabled: true` so people can see who's in a call.

## Voice activity or push to talk

Choose in **Settings → Voice → Input**.

- **Voice activity** sends your mic when you speak. **Sensitivity** sets how loud counts as speaking.
- **Push to talk** sends only while you hold a key or mouse button. The default key is <kbd>`</kbd>. **Release delay** keeps the mic open a moment after you let go.

Push to talk works while IrChad is in the background on Windows, on X11, and on Wayland desktops that offer global shortcuts (KDE, GNOME 48 and newer, Hyprland). Elsewhere it works only while IrChad is focused, and the settings say so.

## Per-person volume

Right-click someone in the call for their **volume**. If they're sharing a screen, its sound has its own slider.

## Whisper

Hold the whisper key and only the people you've picked hear you. Everyone else in the call hears nothing.

1. Set a **Whisper key** in **Settings → Voice**.
2. Right-click people in the call and tick **Whisper to …** (up to 50 people).

It works in both input modes. Someone whispering to you is marked "whispering to you", and on desktop a short tone plays first. Whispers are sent from the desktop app, and heard on every platform.

## Soundboard

The **♫** button in the voice panel plays the network's sound clips into the call. Only people in the call hear them, and clips are dropped while you're muted. **Soundboard volume** sets how loud everyone's clips are for you, apart from voices.

> **Server setting:** `chadd.toml` → `[sounds]` with a `dir` holding mp3, ogg, wav or flac files. Each file is one clip, cut to 10 seconds. Without it the menu says "This server has no sounds yet."

## Audio settings

All in **Settings → Voice**.

| Setting | What it does |
|---|---|
| Microphone, Speakers | Which devices to use. "System default" follows your system when you plug something in. |
| Input volume | How loud others hear you |
| Test / Hear yourself | Plays your mic back, processed the way others hear it |
| Echo cancellation | Stops others hearing themselves through your speakers |
| Noise suppression | Removes steady background noise |
| Extra noise suppression (RNNoise) | Stronger, for keyboards and fans |
| Automatic gain control | Evens out your volume. It can "pump" between words. |
| Quality | Voice bitrate, 64 kbps by default. Applies the next time you join. |

## Sounds you'll hear

Short tones play when someone joins or leaves, when you mute or unmute, when a screen share starts or stops, and when someone starts watching your share.

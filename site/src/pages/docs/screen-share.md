---
layout: ../../layouts/Docs.astro
title: Screen share and camera
description: Share a screen or window with its sound in IrChad, turn on your camera, and watch other people's shares.
order: 4
---

Sharing happens inside a [voice call](/docs/voice), so join one first. Sharing a screen or camera is desktop only (Linux and Windows). Phones can watch.

> **Server setting:** none beyond what [voice](/docs/voice#joining-and-leaving) needs.

## Sharing your screen

Press **Share your screen** in the voice panel, then pick a screen or a window.

| Option | Choices |
|---|---|
| Quality | 720p at 30 fps, 1080p at 60 fps, or your screen's own resolution, with an optional bitrate cap |
| Optimize for | **Text** keeps code and documents sharp. **Motion** keeps games and video smooth. |
| Sound | One app's sound, or none. Your call's own audio is never sent, so nobody hears an echo. |

On Wayland your desktop shows its own picker. IrChad remembers what you chose, so it doesn't ask every time. **Choose again** brings the picker back.

While you share:

- the panel shows how many people are watching,
- **Preview** shows your share as others see it,
- **Stop sharing** ends it.

IrChad uses your graphics card to encode video when it can (Nvidia on Linux and Windows, AMD and Intel on Linux), which keeps games smooth while you share. **Settings → Screen Sharing** shows which encoder it found, and sets the default quality options.

| Encoder | When to pick it |
|---|---|
| Automatic | The default: the best your graphics card can do. Newer Nvidia cards (RTX 40 and later) send AV1, everything else sends H.264. |
| Best compatibility (H.264) | Viewers watching a [public link](/docs/voice#streaming-to-a-public-link) in a browser that won't play AV1, or viewers on slower machines. H.264 is lighter for them to decode. |
| Software | The share stutters or won't start with your graphics card. This uses a lot more CPU. |

## Camera

**Turn on camera** in the voice panel. Pick which camera in **Settings → Voice → Camera**. It sends 720p at 30 fps.

## Watching

Someone sharing has a **LIVE** badge next to their name in the call. Click it to watch. A camera has its own button. You watch one video at a time.

The bar over the video chooses where it goes, and IrChad remembers your choice:

| Layout | What it does |
|---|---|
| Fill the window | Takes over the chat area. Opening another channel stops watching. |
| Float over the chat | A small window you can move and resize |
| Tile beside the chat | A pane next to your channels |
| Pop out | A separate window |

There's also **Full screen** (<kbd>Esc</kbd> leaves it), a **volume** slider for the share's sound, and **Stop watching**.

IrChad only downloads a share while you're watching it.

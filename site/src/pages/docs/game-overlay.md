---
layout: ../../layouts/Docs.astro
title: Game overlay
description: "IrChad's game overlay: who's in voice and the latest messages, shown over the game you're playing, on Windows and Linux."
order: 4.5
---

The overlay is a small panel in a corner of your screen while you play. It shows who's in your voice call, who's speaking, and the latest messages from the channels you pick. It's on the desktop app only, for Windows and Linux.

It's only something to look at. Clicks go straight through it to the game, and nothing in it takes input. IrChad never puts anything inside the game, so it can't crash the game or trip anti-cheat.

## Turning it on

1. Open **Settings → Game Overlay** and tick **Show an overlay over games**.
2. Click **Preview** to see it for 10 seconds without a game.
3. Start a game. The overlay shows while the game has focus and hides when you switch away.

Under the checkbox, the settings tell you which game the overlay is showing over, or why it can't show.

## Which games count

The overlay shows over games on Discord's list of known games, and over any game you add yourself.

- To add a game, go to **Your games** → **Add a game** and type the name of its executable, like `MyGame.exe` or `mygame`. Use this for games that aren't on the list, and for anything else you want the overlay over.
- If the settings say "No game list from your server yet", only the games you add are recognized.

> **Server setting:** the game list needs `chadd.toml` → `discord_games = true`, the same setting that names games in your [activity](/docs/chat#activity). Your server sends the whole list to IrChad, so nobody learns what you play.

## Showing and hiding it

Press <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>O</kbd> to hide it, and again to bring it back. To change the combination, click it next to **Show or hide** and press a new one. When the overlay is hidden this way, the settings say so.

The combination works while you're in a game on Windows and on X11. On Wayland, read the next section.

### On Wayland

On Wayland, apps can't watch the keyboard. IrChad asks your desktop for a global shortcut instead, so your desktop decides which keys toggle the overlay, and that's where you change them.

How you set global shortcuts is different on every Wayland compositor, so look up your compositor's documentation for "global shortcuts". Some pointers:

- **KDE Plasma**: the first time, a dialog asks you to confirm the shortcut. Change it later in **System Settings → Keyboard → Shortcuts**, under IrChad.
- **Hyprland**: shortcuts that apps register do nothing until you bind them in `hyprland.conf` with the `global` dispatcher. `hyprctl globalshortcuts` lists IrChad's shortcuts and their exact names (the overlay's is `toggle-overlay`). See the Hyprland wiki's page on global keybinds.
- **Sway and other wlroots compositors** usually don't offer global shortcuts to apps. There, the combination works only while IrChad's window has focus.

The same goes for [push to talk](/docs/voice#voice-activity-or-push-to-talk) and whisper. Your desktop lists them next to the overlay's shortcut.

## Settings

All in **Settings → Game Overlay**.

| Setting | What it does |
|---|---|
| Corner | Which corner of the screen it sits in. Top left by default. |
| Shows | **Who's in voice, and who's speaking**, and **Messages from the channels below**. Either or both. |
| Messages | How many lines to show (5 by default) and how long each stays (10 seconds). Tick **Keep them on screen** to keep the newest messages up instead of fading them out. |
| Background | How see-through the panel is |
| Channels | Which channels' messages show. You can also right-click a channel in the sidebar and tick **Show in game overlay**. |

Only new messages show, each cut to one line, and your own are left out.

## Where it works

| System | Works |
|---|---|
| Windows | Yes |
| Linux, X11 | Yes |
| Linux, Wayland: KDE Plasma, Hyprland, Sway, COSMIC | Yes, with XWayland |
| Linux, Wayland: GNOME | No. GNOME doesn't let apps draw over other windows, and the settings say so. |

- **Fullscreen**: the overlay shows over windowed and borderless games, but not over exclusive fullscreen. If it doesn't show, set the game's display mode to **Borderless** or **Windowed fullscreen**.
- **Wayland**: it recognizes games running through XWayland, which covers Proton and Wine games and most native Linux games. On Hyprland it recognizes native Wayland games too. It sits in the corner of the screen, not the corner of a windowed game.

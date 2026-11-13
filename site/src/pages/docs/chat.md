---
layout: ../../layouts/Docs.astro
title: Chat
description: Channels, DMs, replies, reactions, uploads, GIFs, notifications and your profile in IrChad.
order: 2
---

This page describes the desktop app. Phones have the same features unless [Phones and tablets](/docs/mobile) says otherwise.

## Finding your way around

From left to right: the **network rail**, the **channel list** (channels, then direct messages), the **chat**, and the **member list**.

| To | Do |
|---|---|
| Join a channel | **+** next to CHANNELS, then type its name |
| Leave a channel | Right-click it → **Leave channel** |
| Message someone | **+** next to DIRECT MESSAGES, or click them in the member list |
| Close a DM | Right-click it → **Close conversation** |
| Jump anywhere | <kbd>Ctrl</kbd>+<kbd>K</kbd>, then type a channel or person |

Channels with unread messages are highlighted, and a badge counts messages that mention you. What you've read syncs between your devices.

The member list shows who's online and away, and each person's role in the channel (Owner, Admin, Operator, Half-op, Voice). On IRC those are the `~ & @ % +` prefixes.

## Writing messages

| To | Do |
|---|---|
| Send | <kbd>Enter</kbd> |
| New line | <kbd>Shift</kbd>+<kbd>Enter</kbd> |
| Mention someone | Type `@` and pick them, or type the start of a name and press <kbd>Tab</kbd> |
| Emoji | The emoji button, or a shortcode like `:smile:` |
| [Custom emoji](#custom-emoji) | The top of the emoji picker, or its name like `:chad:` |
| GIF | The **GIFs** tab of the emoji picker, or `/gif happy cat` |
| Bold, italic, code | `**bold**`, `*italic*`, `` `code` ``, and ```` ``` ```` around a code block |
| Edit your last message | <kbd>↑</kbd> in an empty message box |
| Cancel a reply or edit | <kbd>Esc</kbd> |

`/gif` is the only slash command. Other IRC commands such as `/join` are sent as text.

> **Server setting:** GIF search needs `chadd.toml` → `[gifs]` with a `key`. Without it the app says "This network doesn't have GIFs."
>
> Long pasted text is sent as one message up to `ircd.yaml` → `limits.multiline.max-bytes` (4096 by default) and `max-lines`.

### Custom emoji

A network can have its own emoji. They're listed first in the emoji picker, in messages and for reactions, and searching the picker finds them by name. You can also just type the name between colons.

People on ordinary IRC clients see the name, like `:chad:`, and so do you until the picture has loaded.

> **Server setting:** `chadd.toml` → `[emoji]` with a `dir` of images. A file named `chad.png` is `:chad:`. Names are 2 to 32 lowercase letters, digits and `_`, the file a png, jpg, gif or webp of at most 1 MiB. Add or remove files at any time; people get the change when they next connect or open the picker. Everything in that directory is public.

## Replies, reactions, edits, deletes

Hover a message for its buttons:

- **React** with an emoji. Click a reaction someone else left to add yours, and click yours again to take it back.
- **Reply**. The reply shows a quote of the message, and clicking the quote jumps to it.
- **Edit** your own message. It's marked "(edited)".
- **Delete** your own message. Everyone sees "This message was deleted."

People on ordinary IRC clients see an edit as a new line starting with `*`, the usual way to correct yourself on IRC.

> **Server setting:** deleting needs `ircd.yaml` → `history.retention.allow-individual-delete: true`.
>
> Reactions made while you were away show up only if `ircd.yaml` → `history.tagmsg-storage.whitelist` includes `+draft/react`.

## Files, images and links

- **Upload** with the paperclip, by pasting a file or image, or by dragging a file onto the chat. (Dragging doesn't work on Wayland yet.) The size limit is the network's.
- Images, GIFs and videos show in the chat. Other files show as a download card.
- The first link in a message gets a **preview**. The network fetches previews and images for you, so the site never sees your address.
- To load nothing until you click it, turn off **Settings → Chat → Images**. A second switch does the same for messages from guests only.

Anyone with an upload's link can open it.

> **Server setting:** uploads need `chadd.toml` → `[uploads]` with a `dir`. `max_bytes` is the size limit (25 MB by default). Without `[uploads]`, the app says "This network doesn't take uploads", and profile pictures are off too.
>
> Previews and images from other sites need no setting, but they do need an account. Guests see only files uploaded to the network itself.

## Notifications

**Settings → Chat → Notify me about**: every message, mentions and direct messages (the default), or nothing.

The tray icon shows a dot for unread messages and a red dot for mentions. On Windows, **Settings → Windows** decides whether closing the window keeps IrChad in the tray, and whether it starts with Windows.

## Your profile and status

Click your name at the bottom of the channel list.

- **Online / Away.** You also go away by yourself after 10 idle minutes. Change that in **Settings → General → Away**.
- **Edit profile…** sets your **display name** and **picture** for this network, turns on read receipts, and lists your devices that have [encrypted DMs](/docs/encrypted-dms).

> **Server setting:** display names, pictures, activity and read receipts are all stored as IRC metadata: `ircd.yaml` → `metadata.enabled: true` and `metadata.operator-only-modification: false`. Pictures also need uploads (above).

### Activity

Off by default. In **Settings → General → Activity**:

- **Share what I'm playing** shows a line like "Playing Celeste" under your name. It spots running games, and games that support Discord Rich Presence can add details. While Discord itself is running, those games report to Discord instead.
- **Share what I'm listening to** shows the song from the media players you tick. Nothing is shared from a player you haven't ticked, so a browser tab's title can't leak. Others see how far into the song you are and, from Spotify or a browser on Linux, can open it. Videos in a browser show as "Watching". **Say which player** adds "on Spotify".

> **Server setting:** naming games needs `chadd.toml` → `discord_games = true`. Without it, only games with Rich Presence show, using their own details line.

### Read receipts

Off by default, and only in DMs. Turn on **Read receipts in DMs** in your profile, and "Seen" appears under your newest message the other person has read, as long as they've turned it on too. The setting follows your account to every device.

> **Server setting:** receipts sent while you were offline arrive later only if `ircd.yaml` → `history.tagmsg-storage.whitelist` includes `+chad/read`. Without it they still work live.

## Blocking

Right-click someone's name → **Block**. You stop seeing anything they send: messages, DMs, reactions, typing. It only applies on this device, and they aren't told. Unblock in **Settings → Blocked Users**.

## Layout and looks

- **Panes:** opening a channel puts it beside the one you have open, and <kbd>Ctrl</kbd>+arrow keys move between them. Drag a pane's title onto another to swap them. Turn this off in **Settings → Appearance → Layout**.
- **Theme and text size:** **Settings → Appearance**. Dark, light, match the system, or one of the four Catppuccin flavors.

## Keyboard shortcuts

| Keys | Does |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>K</kbd> | Quick switcher |
| <kbd>Alt</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | Previous / next channel |
| <kbd>Ctrl</kbd>+arrows | Move between panes |
| <kbd>Tab</kbd> / <kbd>Shift</kbd>+<kbd>Tab</kbd> | Complete a name, forwards / backwards |
| <kbd>↑</kbd> in an empty box | Edit your last message |
| <kbd>Esc</kbd> | Cancel a reply or edit, leave full screen |

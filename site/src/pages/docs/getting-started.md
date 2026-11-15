---
layout: ../../layouts/Docs.astro
title: Getting started
description: Install IrChad, join a network from an invite link, and create an account.
order: 1
---

## 1. Install

### Linux (x86_64)

Paste this into a terminal:

```sh
curl -fsSL https://irchad.chat/install.sh | sh
```

It installs for your user only, without root: the app in `~/.local/bin`, its menu entry, and the handler for invite links.
To get a nightly build, add `-s -- --nightly` after `sh`. To remove IrChad, add `-s -- --uninstall`. Your settings and accounts are kept.

To do it by hand, download the tarball from the [home page](/#download), unpack it and run `irchad/install.sh`. `./install.sh --uninstall` removes it.

### Ubuntu and Debian

On Ubuntu 24.04 or newer, or Debian 13 or newer, add the apt repository and install IrChad:

```sh
sudo curl -fsSLo /etc/apt/keyrings/irchad.asc https://git.gentoo.party/api/packages/sam/debian/repository.key
echo "deb [signed-by=/etc/apt/keyrings/irchad.asc] https://git.gentoo.party/api/packages/sam/debian stable main" | sudo tee /etc/apt/sources.list.d/irchad.list
sudo apt update && sudo apt install irchad
```

New versions arrive with your normal system updates.


There's an overlay:

```sh
eselect repository add irchad git https://git.gentoo.party/sam/irchad-overlay.git
emaint sync -r irchad
emerge -av net-im/irchad
```

### Windows (x86_64)

Download the installer from the [home page](/#download) and run it. It installs for your user, with no admin prompt.

### Phones and tablets

On Android, install the APK from the [home page](/#download), or add it to Obtainium to get updates. See [Phones and tablets](/docs/mobile).
iOS isn't released yet.

The desktop app tells you when a new version is out. If you installed it from the Linux tarball or the Windows installer, it downloads the new version in the background, checks that we signed it, and then offers **Restart to update**. Nothing changes until you click that.
If your package manager installed IrChad, or the app can't replace its own file for some other reason, it only shows the notice, and you update it the usual way. You can turn either one off in **Settings → General → Updates**.

## 2. Get an invite link

A network's invite link looks like `https://chat.example.com/invite`. Whoever runs the network, or a friend who's on it, gives you one.

Either:

- open the link in a browser and follow its three steps: download the app, install it, then come back to the page and press **Open in IrChad**, or
- in the app, open **Join a network**, paste the link into **Server address** and press **Continue**. Just `chat.example.com` works too.

The app asks the server what it offers, then shows the network's name and its sign-in form. **Change** next to the address takes you back.

The first time you open IrChad, a few short cards explain what a network is and what happens next. Press **Next** to read them or **Skip** to go straight on. They don't come back.

## 3. Choose how to join

The form starts on **Log in**. The other choices sit under it, and a network only shows the ones it allows.

| Choice | You fill in | Use it when |
|---|---|---|
| **Log in** | Username, password | You signed up before, here or on another device |
| **Sign in with Discord** | Nothing: your browser opens Discord | The network offers it and you'd rather not have another password |
| **No account yet? Sign up** | Username, password, and an email if the network asks for one | You're new here |
| **Continue as guest** | A nickname | You just want to look around |

If the network checks email addresses, it sends you a code. Enter it under **Check your email**.

Your username is also the name people see. You can add a different display name later, in your [profile](/docs/chat#your-profile-and-status).

> **Server setting:** signing up needs `chadd.toml` → `registration = "open"`, and `ircd.yaml` → `accounts.registration.enabled: true` with `accounts.registration.allow-before-connect: true`. On a closed network there is no **Sign up**: the admin makes your account and you log in.
>
> The email code is `ircd.yaml` → `accounts.registration.email-verification.enabled: true`. With it on, `server.idle-timeouts.registration: 5m` gives you time to find the email.
>
> Guests need `chadd.toml` → `guests = true`, and `ircd.yaml` → `accounts.require-sasl.enabled: false`.

### Guest or account?

| | Guest | Account |
|---|---|---|
| Text chat | ✓ | ✓ |
| Keeps your name | No, anyone can take it | ✓ |
| Messages wait for you while you're away | No | ✓ |
| Voice, screen share, camera | No | ✓ |
| Uploads, GIF search, link previews | No | ✓ |
| Encrypted DMs | No | ✓ |
| Same identity on your phone and PC | No | ✓ |

A guest can sign up later without leaving: open the network's menu (next to its name, above the channel list) and choose **Create an account…**. You keep your channels.

## 4. You're in

On your first login IrChad puts you in the network's channels. From then on you stay in them, even with the app closed, and messages wait for you. Leaving a channel is always your choice, and it sticks.

> **Server setting:** `chadd.toml` → `home = "#general"` names the channel everyone lands in. Its operators list the network's other official channels, and you're added to those too. Without `home`, you join every public channel on first login.
>
> Staying in channels while offline is `ircd.yaml` → `accounts.multiclient.enabled: true` and `accounts.multiclient.always-on: "opt-out"`.
>
> History that waits for you is `ircd.yaml` → `history.enabled: true`. For it to survive a server restart it also needs `history.persistent.enabled: true` and a database, such as `datastore.sqlite.enabled: true`.

## More than one network

Use **Join a network** (the **+** in the rail on the far left) again with another invite link. Each network has its own account, and its own icon in the rail.

## Passwords

IrChad saves each network's password in your system's password store (the keyring on Linux, Credential Manager on Windows), so you log in once. If the store is locked or the password changed, the network shows **Sign in** with a password box.

## Leaving a network, deleting your account

Open the network's menu and choose **Leave network…**.

- **Leave** makes IrChad forget the network, your saved password, and this device's [encryption keys](/docs/encrypted-dms). Your account stays, so you can come back.
- Tick **Delete account** to also delete the account from the server, with your messages and uploads. The name stays reserved, so nobody can pretend to be you. See [what may remain](/delete-account).

> **Server setting:** deleting your messages and uploads along with the account needs `chadd.toml` → `[delete]` and `ircd.yaml` → `history.retention.enable-account-indexing: true`. Without them, only the account itself is removed.

# IrChad

A Discord-style client for IRC, with voice, screen share and encrypted DMs. Paste an invite link and you're in.

Requires a network running [ergo](https://github.com/ergochat/ergo) (≥ 2.19) + LiveKit + chadd. See `deploy/`.

Linux (Wayland and X11) is the primary platform, Windows is supported, and Android is in progress (voice and watching screen shares work).

## Install

### Linux (x86_64)

```sh
curl -fsSL https://irchad.chat/install.sh | sh
```

It installs for your user only, with no root: the app in `~/.local/bin`, its menu entry, and the handler for invite links. The app updates itself after that.
Add `-s -- --nightly` after `sh` for a build of `main`, or `-s -- --uninstall` to remove it.

Prefer to do it by hand? Download the [tarball](https://downloads.gentoo.party/irchad/latest/irchad-linux-x86_64.tar.gz), unpack it and run `irchad/install.sh`.

**Gentoo:** there's an overlay.

```sh
eselect repository add irchad git https://git.gentoo.party/sam/irchad-overlay.git
emaint sync -r irchad
emerge -av net-im/irchad
```

### Windows (x86_64)

Run the [installer](https://downloads.gentoo.party/irchad/latest/irchad-windows-x86_64-setup.exe). It installs for your user, with no admin prompt, and the app updates itself.

### Mobile

**Android:** install the [APK](https://downloads.gentoo.party/irchad/latest/irchad-android-arm64.apk), or add it to Obtainium to get updates:

<a href="https://apps.obtainium.imranr.dev/redirect?r=obtainium%3A%2F%2Fapp%2F%7B%22id%22%3A%22party.gentoo.irchad%22%2C%22url%22%3A%22https%3A%2F%2Fgit.gentoo.party%2Fsam%2Firchad-ng%22%2C%22author%22%3A%22sam%22%2C%22name%22%3A%22IrChad%22%2C%22overrideSource%22%3A%22Codeberg%22%2C%22additionalSettings%22%3A%22%7B%5C%22includePrereleases%5C%22%3Afalse%7D%22%7D"><img src="docs/assets/badge_obtainium.png" alt="Get it on Obtainium" height="48"></a>

**iOS:** not released yet.

### Nightly builds

Builds of `main`, rebuilt each night when it has changed:
[Linux tarball](https://downloads.gentoo.party/irchad/nightly/irchad-linux-x86_64.tar.gz) ·
[Windows installer](https://downloads.gentoo.party/irchad/nightly/irchad-windows-x86_64-setup.exe) ·
[release page](https://git.gentoo.party/sam/irchad-ng/releases/tag/nightly)

Status: planning. See [docs/PLAN.md](docs/PLAN.md).

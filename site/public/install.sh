#!/bin/sh
# Install or update IrChad for the current user (no root):
#   curl -fsSL https://irchad.chat/install.sh | sh
# Uninstall:  curl -fsSL https://irchad.chat/install.sh | sh -s -- --uninstall
# Nightly:    curl -fsSL https://irchad.chat/install.sh | sh -s -- --nightly
# It fetches the release tarball and runs its install.sh, which puts the app
# in ~/.local/bin with its menu entry and irchad:// link handler.
set -eu

# Everything is in main, so a download cut short runs nothing.
main() {
    channel=latest
    args=
    for a in "$@"; do
        case $a in
            --nightly) channel=nightly ;;
            --uninstall) args=--uninstall ;;
            *) echo "unknown option: $a" >&2; exit 2 ;;
        esac
    done
    [ "$(uname -s)" = Linux ] && [ "$(uname -m)" = x86_64 ] ||
        { echo "IrChad's Linux build is x86_64 only. See https://irchad.chat/#download" >&2; exit 1; }
    command -v curl >/dev/null || { echo "needs curl" >&2; exit 1; }

    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    echo "Downloading IrChad ($channel)..."
    curl -fSL --proto '=https' -o "$tmp/irchad.tar.gz" \
        "https://downloads.gentoo.party/irchad/$channel/irchad-linux-x86_64.tar.gz"
    tar xzf "$tmp/irchad.tar.gz" -C "$tmp"
    sh "$tmp/irchad/install.sh" $args
    [ -n "$args" ] && { echo "Uninstalled. Your settings and accounts are kept."; return; }
    case ":$PATH:" in
        *":$HOME/.local/bin:"*) ;;
        *) echo "Note: ~/.local/bin isn't on your PATH, so start it from the app menu." ;;
    esac
}

main "$@"

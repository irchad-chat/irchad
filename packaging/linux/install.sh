#!/bin/sh
# Installs IrChad for the current user from the release tarball: the binary
# in ~/.local/bin, plus the menu entry, icon and irchad:// link handler.
# Run it from the unpacked directory. Uninstall: ./install.sh --uninstall
set -eu
cd "$(dirname "$0")"
bin="${HOME}/.local/bin"
data="${XDG_DATA_HOME:-${HOME}/.local/share}"
id=party.gentoo.IrChad
if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$bin/irchad" "$data/applications/$id.desktop" \
        "$data/icons/hicolor/scalable/apps/$id.svg" "$data/metainfo/$id.metainfo.xml"
    exit 0
fi
install -Dm755 irchad "$bin/irchad"
install -Dm644 "$id.desktop" "$data/applications/$id.desktop"
install -Dm644 "$id.svg" "$data/icons/hicolor/scalable/apps/$id.svg"
install -Dm644 "$id.metainfo.xml" "$data/metainfo/$id.metainfo.xml"
# The packaged entry runs plain `irchad`, so use the full path in case
# ~/.local/bin isn't on the menu's PATH.
sed -i "s|^Exec=irchad |Exec=\"$bin/irchad\" |" "$data/applications/$id.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" || true
command -v xdg-mime >/dev/null && xdg-mime default "$id.desktop" x-scheme-handler/irchad || true
echo "Installed. Start IrChad from your app menu, or run $bin/irchad."

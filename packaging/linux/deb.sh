#!/bin/sh
# Pack dist/irchad (from tarball.sh) into irchad-linux-x86_64.deb for the apt
# repo on Gitea's Debian registry (release.yml uploads it). Built on the CI
# image (Ubuntu 24.04), so it needs glibc >= 2.39: Ubuntu 24.04+, Debian 13+.
set -eu
cd "$(dirname "$0")/../.."
src=dist/irchad
root=dist/deb
id=party.gentoo.IrChad
version=$(cat "$src/VERSION")
rm -rf "$root" && mkdir -p "$root/DEBIAN"
install -Dm755 "$src/irchad" "$root/usr/bin/irchad"
install -Dm644 "$src/$id.desktop" "$root/usr/share/applications/$id.desktop"
install -Dm644 "$src/$id.svg" "$root/usr/share/icons/hicolor/scalable/apps/$id.svg"
install -Dm644 "$src/$id.metainfo.xml" "$root/usr/share/metainfo/$id.metainfo.xml"
install -Dm644 -t "$root/usr/share/doc/irchad" "$src/LICENSE" "$src/THIRD_PARTY_LICENSES.txt" "$src/LIBWEBRTC_LICENSES.md"
# Linked: glib, pipewire, libc, libgcc. dlopened by winit/wgpu: xkbcommon, EGL.
cat > "$root/DEBIAN/control" <<EOF
Package: irchad
Version: $version
Architecture: amd64
Maintainer: Sam Hoffman <sam@weller.gentoo.party>
Section: net
Priority: optional
Homepage: https://git.gentoo.party/sam/irchad-ng
Depends: libc6 (>= 2.39), libgcc-s1, libglib2.0-0t64 | libglib2.0-0, libpipewire-0.3-0t64 | libpipewire-0.3-0, libxkbcommon0, libxkbcommon-x11-0, libegl1
Recommends: libvulkan1, xdg-desktop-portal
Suggests: libva2
Installed-Size: $(du -sk --apparent-size "$root/usr" | cut -f1)
Description: Discord-like client for IRC
 Chat, voice and screen share with your friends on an IrChad (ergo) network.
EOF
dpkg-deb --root-owner-group -Zxz --build "$root" irchad-linux-x86_64.deb

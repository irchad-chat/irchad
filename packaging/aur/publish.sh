#!/bin/sh
# Bump irchad-bin to a published release and push it to the AUR:
#   packaging/aur/publish.sh 0.1.37
# Needs an SSH key registered on the AUR account (AUR_SSH_KEY in CI).
# .SRCINFO is edited with sed rather than `makepkg --printsrcinfo` so this
# runs off Arch; only pkgver and the checksum ever change here.
# shortcut: pkgrel is reset to 1; bump it by hand for packaging-only fixes.
set -eu
v=${1#v}
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# Not piped into sha256sum: `set -e` wouldn't see a 404 (draft release).
curl -fsSLo "$tmp/t.tgz" "https://git.gentoo.party/sam/irchad-ng/releases/download/v$v/irchad-linux-x86_64.tar.gz"
sum=$(sha256sum "$tmp/t.tgz" | cut -d' ' -f1)
git clone -q ssh://aur@aur.archlinux.org/irchad-bin.git "$tmp/aur"
old=$(sed -n 's/^pkgver=//p' "$here/PKGBUILD")
sed -e "s/^pkgver=.*/pkgver=$v/" -e "s/^pkgrel=.*/pkgrel=1/" \
    -e "s/^sha256sums=.*/sha256sums=('$sum')/" "$here/PKGBUILD" > "$tmp/aur/PKGBUILD"
sed -e "s/$old/$v/g" -e "s/pkgrel = .*/pkgrel = 1/" \
    -e "s/sha256sums = .*/sha256sums = $sum/" "$here/.SRCINFO" > "$tmp/aur/.SRCINFO"
cd "$tmp/aur"
git add PKGBUILD .SRCINFO
git commit -qm "irchad-bin $v"
git push -q origin HEAD:master

#!/bin/sh
# Signs a release archive in place for the in-app updater (self_update's
# zipsign: ed25519, the signature stored inside the archive, so plain tar and
# unzip still work), then checks it against the committed packaging/update.pub.
# The signing context is the file name, which is what self_update verifies
# with, so sign the archive under its published asset name.
# Usage: UPDATE_SIGNING_KEY=<base64 private key> packaging/sign.sh <file.tar.gz|file.zip>
# packaging/windows/update-zip.ps1 is the Windows twin.
set -eu
f=$1
case $f in
    *.tar.gz) kind=tar ;;
    *.zip) kind=zip ;;
    *) echo "don't know how to sign $f" >&2; exit 1 ;;
esac
[ -n "${UPDATE_SIGNING_KEY:-}" ] || { echo "UPDATE_SIGNING_KEY secret missing" >&2; exit 1; }
pub="$(cd "$(dirname "$0")" && pwd)/update.pub"
command -v zipsign >/dev/null || cargo install -q --locked zipsign@0.2.2
key=$(mktemp)
trap 'rm -f "$key"' EXIT
printf '%s' "$UPDATE_SIGNING_KEY" | base64 -d > "$key"
zipsign sign "$kind" "$f" "$key"
zipsign verify "$kind" -q "$f" "$pub"
echo "signed $f"

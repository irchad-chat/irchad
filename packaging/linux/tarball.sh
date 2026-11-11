#!/bin/sh
# Pack target/release/irchad with install.sh and the desktop files into
# irchad-linux-x86_64.tar.gz (one `irchad/` dir), with the license notices. Used by release.yml and nightly.yml.
set -eu
cd "$(dirname "$0")/../.."
rm -rf dist/irchad && mkdir -p dist/irchad
cp target/release/irchad packaging/linux/install.sh packaging/linux/party.gentoo.IrChad.* dist/irchad/
packaging/licenses.sh dist/irchad chad-desktop x86_64-unknown-linux-gnu
# What this build is, inside the signed archive, so the updater can refuse an
# older release served as new (packaging/sign.sh signs the whole tarball).
if [ -n "${IRCHAD_NIGHTLY:-}" ]; then
    echo "nightly $IRCHAD_NIGHTLY"
else
    sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1
fi > dist/irchad/VERSION
tar czf irchad-linux-x86_64.tar.gz -C dist irchad

#!/bin/sh
# Pack target/release/irchad with install.sh and the desktop files into
# irchad-linux-x86_64.tar.gz (one `irchad/` dir), with the license notices. Used by release.yml and nightly.yml.
set -eu
cd "$(dirname "$0")/../.."
rm -rf dist/irchad && mkdir -p dist/irchad
cp target/release/irchad packaging/linux/install.sh packaging/linux/party.gentoo.IrChad.* dist/irchad/
packaging/licenses.sh dist/irchad chad-desktop x86_64-unknown-linux-gnu
tar czf irchad-linux-x86_64.tar.gz -C dist irchad

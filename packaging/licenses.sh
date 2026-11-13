#!/bin/sh
# Writes the license notices a binary distribution must carry into <dir>:
# LICENSE, THIRD_PARTY_LICENSES.txt (every crate <package> links for
# <target>, via cargo-about and about.toml/about.hbs), and libwebrtc's own
# notices when the build downloaded its prebuilt (webrtc-sys).
# Usage: packaging/licenses.sh <dir> <package> <target>
set -eu
dir=$1 pkg=$2 target=$3
cd "$(dirname "$0")/.."
mkdir -p "$dir"
cargo about --version >/dev/null 2>&1 || cargo install --locked cargo-about@0.9.3 --features cli
cp LICENSE "$dir/"
cargo about generate --locked --fail -m "crates/$pkg/Cargo.toml" --target "$target" \
    about.hbs -o "$dir/THIRD_PARTY_LICENSES.txt"
# The newest extracted prebuilt; it ships its third_party notices as LICENSE.md.
# Cargo ≥ 1.100 lays build dirs out as build/<pkg>/<hash>/, older as build/<pkg>-<hash>/.
t="${CARGO_TARGET_DIR:-target}"
webrtc=$(ls -t "$t"/*/build/scratch-*/out/livekit_webrtc/livekit/*/*/LICENSE.md \
    "$t"/*/build/scratch/*/out/livekit_webrtc/livekit/*/*/LICENSE.md 2>/dev/null | head -1 || true)
if [ -n "$webrtc" ]; then
    cp "$webrtc" "$dir/LIBWEBRTC_LICENSES.md"
elif [ "$pkg" = chad-desktop ]; then
    echo "libwebrtc's LICENSE.md not found; build $pkg first" >&2
    exit 1
fi

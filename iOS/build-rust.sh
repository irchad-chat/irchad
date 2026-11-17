#!/usr/bin/env bash
# Builds libchad_ffi.a for iOS devices into rust/aarch64-apple-ios/,
# generates the Swift bindings into Sources/IrChad/Generated and
# Sources/chad_ffiFFI, and copies the workspace version into Info.plist.
# Run it before `xtool dev` whenever the Rust side changed.
#
#   PROFILE=release            cargo profile (default: dev)
#   SDK=<path to iPhoneOS.sdk> (default: the one xtool installed)
#
# Rust std for iOS comes from rustup's targets when it has them; otherwise it
# is built from rust-src (`-Zbuild-std`, e.g. Gentoo's rustc).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
triple=aarch64-apple-ios
profile=${PROFILE:-dev}
dir=$([ "$profile" = dev ] && echo debug || echo "$profile")
# Keep in step with Package.swift's platforms.
min=18.0
platforms=$HOME/.swiftpm/swift-sdks/darwin.artifactbundle/Developer/Platforms
clang_target=arm64-apple-ios$min
sdk=${SDK:-$(ls -d "$platforms"/iPhoneOS.platform/Developer/SDKs/iPhoneOS*.sdk 2>/dev/null | sort -V | tail -1)}
[ -d "${sdk:-}" ] || { echo "no iOS SDK (run 'xtool setup' or set SDK)" >&2; exit 1; }
cc=$(command -v clang); cxx=$(command -v clang++); ar=$(command -v llvm-ar)

env_triple=${triple//-/_}
upper=${env_triple^^}
export SDKROOT=$sdk IPHONEOS_DEPLOYMENT_TARGET=$min
export "CC_$env_triple=$cc" "CXX_$env_triple=$cxx" "AR_$env_triple=$ar"
export "CFLAGS_$env_triple=--target=$clang_target -isysroot $sdk"
export "CXXFLAGS_$env_triple=--target=$clang_target -isysroot $sdk"
# Only the build scripts' own test links use this; the app links the .a.
export "CARGO_TARGET_${upper}_LINKER=$cc"
export "CARGO_TARGET_${upper}_RUSTFLAGS=-Clink-arg=--target=$clang_target -Clink-arg=-isysroot -Clink-arg=$sdk -Clink-arg=-fuse-ld=lld"
export CARGO_PROFILE_DEV_DEBUG=false
std=()
if ! rustc --print target-libdir --target "$triple" 2>/dev/null | xargs -I{} test -d {}; then
    export RUSTC_BOOTSTRAP=1
    std=(-Zbuild-std=std,panic_abort)
fi
(cd "$root" && cargo rustc -p chad-ffi --lib --crate-type staticlib --profile "$profile" --target "$triple" "${std[@]}")
lib="$here/rust/$triple/libchad_ffi.a"
mkdir -p "$(dirname "$lib")"
cp "$root/target/$triple/$dir/libchad_ffi.a" "$lib"

gen=$(mktemp -d)
trap 'rm -rf "$gen"' EXIT
(cd "$root" && cargo run -q -p chad-ffi --features cli --bin uniffi-bindgen -- \
    generate --library "$lib" --language swift --out-dir "$gen" --no-format)
mkdir -p "$here/Sources/IrChad/Generated" "$here/Sources/chad_ffiFFI/include"
cp "$gen/chad_ffi.swift" "$here/Sources/IrChad/Generated/"
cp "$gen/chad_ffiFFI.h" "$here/Sources/chad_ffiFFI/include/"

# The repo version goes in ChadVersion (shown in Settings, sent as the IRC version reply).
# CFBundleShortVersionString is bumped by hand, only for App Store submissions: each new one
# sends external TestFlight testers through beta review again.
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -1)
# Each upload needs a higher build number, even between releases: use the commit count.
build=$(git -C "$root" rev-list --count HEAD)
sed -i "/<key>ChadVersion<\/key>/{n;s|<string>.*</string>|<string>$version</string>|}" "$here/Info.plist"
sed -i "/<key>CFBundleVersion<\/key>/{n;s|<string>.*</string>|<string>$build</string>|}" "$here/Info.plist"

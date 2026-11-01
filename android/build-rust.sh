#!/usr/bin/env bash
# Builds libchad_ffi.so for each ABI into app/build/rust/jniLibs and generates
# the Kotlin bindings into app/build/rust/kotlin. Gradle runs this before
# every build.
#
#   ABIS="x86_64 arm64-v8a"  which ABIs (default: x86_64, for the emulator)
#   PROFILE=release          cargo profile (default: dev)
#
# Rust std for Android comes from rustup's targets when it has them;
# otherwise it is built from rust-src (`-Zbuild-std`, e.g. Gentoo's rustc).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
out="$here/app/build/rust"
sdk=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}
ndk=${ANDROID_NDK_HOME:-$(ls -d "$sdk"/ndk/* 2>/dev/null | sort -V | tail -1)}
[ -d "$ndk" ] || { echo "no Android NDK (set ANDROID_NDK_HOME)" >&2; exit 1; }
bin="$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin"
api=29
profile=${PROFILE:-dev}
dir=$([ "$profile" = dev ] && echo debug || echo "$profile")
# Debug info makes the APK ~150 MB; logcat backtraces still get symbol names.
export CARGO_PROFILE_DEV_DEBUG=false CARGO_PROFILE_DEV_STRIP=debuginfo

# Only the ABIs built now: a stale .so would not match the new bindings.
rm -rf "$out/jniLibs"
for abi in ${ABIS:-x86_64}; do
    case $abi in
        x86_64) triple=x86_64-linux-android ;;
        arm64-v8a) triple=aarch64-linux-android ;;
        *) echo "unknown ABI $abi" >&2; exit 1 ;;
    esac
    env_triple=${triple//-/_}
    upper=${env_triple^^}
    export "CARGO_TARGET_${upper}_LINKER=$bin/$triple$api-clang"
    export "CC_$env_triple=$bin/$triple$api-clang"
    export "CXX_$env_triple=$bin/$triple$api-clang++"
    export "AR_$env_triple=$bin/llvm-ar"
    std=()
    if ! rustc --print target-libdir --target "$triple" 2>/dev/null | xargs -I{} test -d {}; then
        export RUSTC_BOOTSTRAP=1
        std=(-Zbuild-std=std,panic_abort)
    fi
    (cd "$root" && cargo build -p chad-ffi --lib --profile "$profile" --target "$triple" "${std[@]}")
    mkdir -p "$out/jniLibs/$abi"
    cp "$root/target/$triple/$dir/libchad_ffi.so" "$out/jniLibs/$abi/"
    lib="$out/jniLibs/$abi/libchad_ffi.so"
done

rm -rf "$out/kotlin"
(cd "$root" && cargo run -q -p chad-ffi --features cli --bin uniffi-bindgen -- \
    generate --library "$lib" --language kotlin --out-dir "$out/kotlin" --no-format)

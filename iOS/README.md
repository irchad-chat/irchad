# IrChad for iOS and iPadOS

SwiftUI app over `chad-core`, through the same UniFFI bindings as Android (`crates/chad-ffi`).
It's built on Linux with [xtool](https://xtool.sh); no Mac or Xcode needed. iOS 18 and later.

Voice goes through the LiveKit Swift SDK. The app can watch screen shares (with picture-in-picture) but never publishes one.

## Build

Once: `xtool setup` (it installs the iOS SDK as the `darwin` Swift SDK), and sign in with your Apple ID when it asks.
You also need clang and `rust-src` (`-Zbuild-std` is used when rustup has no iOS target, as with Gentoo's rustc).

```sh
cd iOS
./build-rust.sh      # libchad_ffi.a, the Swift bindings, and the version in Info.plist
xtool dev            # build, sign, install and launch on the connected device
```

Run `build-rust.sh` again whenever the Rust side changes (`PROFILE=release` for a release build).
It writes `rust/`, `Sources/IrChad/Generated/` and `Sources/chad_ffiFFI/include/chad_ffiFFI.h`, which aren't committed.
`xtool dev build` builds without a device; `xtool devices` lists connected ones.

## Testing against a server

The phone can't reach `chat.localhost`, so the easiest test is a real deployment (`irchad.chat`).
For the dev stack, the phone needs a name for the host that discovery hands out (`deploy/.env`) and the host's ports 8443 and 6697.
It also has to trust Caddy's internal CA: AirDrop or mail `root.crt` to the phone, install the profile, then turn it on under
Settings › General › About › Certificate Trust Settings.
The core checks certificates with iOS's own verifier (D58), so the profile covers IRC, chadd and LiveKit alike.

## What's not here yet

See `docs/phases/8-ios.md`. Notably, there are no notifications once iOS suspends the app (D60), and no share extension.

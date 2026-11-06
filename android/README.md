# IrChad for Android

Compose app over `chad-core`, through UniFFI bindings in `crates/chad-ffi` (Phase 6).

## Build

Needs the Android SDK (platform 36) and NDK, and JDK 17 or 21 to run Gradle
(Gradle 8.14 can't run on JDK 25).

```sh
cd android
JAVA_HOME=/usr/lib/jvm/openjdk-bin-21 ./gradlew assembleFullDebug
adb install -r app/build/outputs/apk/full/debug/app-full-debug.apk
```

There are two flavors: `full` is the APK on the releases page (Obtainium), and `play` is the Play Store bundle (`bundlePlayRelease`).
`play` drops the "Stay connected" background mode, because Play reviews specialUse foreground services strictly (`app/src/play/AndroidManifest.xml`), so its background notifications only come through UnifiedPush.
Both have the same application ID and signing key, so either one installs over the other.

Gradle runs `build-rust.sh` first. It builds `libchad_ffi.so` and generates the Kotlin bindings into
`app/build/rust/`. It builds x86_64 for the emulator by default. Use `ABIS="x86_64 arm64-v8a"` for phones.
The APK only carries the ABIs in `ABIS`. When rustup has the Android targets they're used. Otherwise std is built from `rust-src` (`-Zbuild-std`), which is how it works with Gentoo's rustc.

## Dev stack from the emulator

The dev origin's discovery hands out `irchad.test` URLs (`deploy/.env`), so the emulator needs that name and the host's port 8443.
It also needs Caddy's internal CA.
This needs a rootable image (`google_apis`, not `google_apis_playstore`):

```sh
emulator -avd <avd> -writable-system &
adb root && adb remount && adb reboot   # first time only, then root + remount again
adb root && adb remount
adb shell "echo '127.0.0.1 irchad.test' >> /system/etc/hosts"
adb reverse tcp:8443 tcp:8443
# The app reads extra CAs from files/certs (see chad-ffi's trust_android_roots).
podman exec irchad-ng-caddy-1 cat /data/caddy/pki/authorities/local/root.crt > /tmp/caddy-root.crt
adb push /tmp/caddy-root.crt /data/local/tmp/
adb shell "run-as party.gentoo.irchad sh -c 'mkdir -p files/certs && cp /data/local/tmp/caddy-root.crt files/certs/'"
```

Then join `irchad.test:8443` as `bob` / `bobpass` (from `deploy/seed.py`).

Push (D47): install ntfy on the emulator and leave its server at ntfy.sh. ergo refuses push endpoints on private or loopback addresses, so a self-hosted ntfy on the LAN won't work.
Pushes only come for DMs and for channel messages that name the account (`bob`), and ergo has to reach ntfy.sh over the internet.

## Release signing

`release.yml` signs with a PKCS12 keystore from the `ANDROID_KEYSTORE` secret (base64), whose store and key (alias `irchad`) share `ANDROID_KEYSTORE_PASSWORD`.
Locally, `IRCHAD_KEYSTORE=<file> IRCHAD_KEYSTORE_PASSWORD=<pw> ./gradlew assembleFullRelease bundlePlayRelease` does the same; without them a release build gets the debug key.
Keep the keystore safe: an APK signed with another key won't install over the old one.

# Job container for the Linux CI/nightly/release builds: everything the old
# per-job apt + llvm.sh step installed, baked in once. Built by
# .gitea/workflows/ci-image.yml and pushed to Gitea's registry over the LAN
# (192.0.2.7:3000/sam/irchad-ci): through Cloudflare, layers over 100 MB
# get 413.
#
# Ubuntu 24.04 matches the runner's ubuntu-latest, so shipped binaries keep
# the same glibc floor (2.39).
FROM docker.io/library/ubuntu:24.04

# JS actions (checkout, rust-cache) run on the container's `node`. The
# official binary is glibc-linked against bookworm (2.36), so it runs here.
COPY --from=docker.io/library/node:22-bookworm-slim /usr/local/bin/node /usr/local/bin/node

# git/zstd/jq/curl for checkout, the cache action and release-asset uploads;
# build-essential for rustc's linker; lavapipe (software Vulkan) so the wgpu
# screenshot tests find an adapter. Headers found through pkg-config:
#   libpipewire-0.3-dev  pipewire-sys / libspa-sys
#   libfontconfig-dev    yeslogic-fontconfig-sys
#   libasound2-dev       alsa-sys (cpal always has it on Linux)
#   libglib2.0-dev       webrtc-sys (glib/gobject/gio headers)
#   libva-dev            webrtc-sys VAAPI encoders; without it they're
#                        silently left out (libva is dlopened at runtime)
# Wayland, X11, xkbcommon and EGL load at runtime; SQLite is bundled.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates curl wget git jq zstd xz-utils unzip build-essential \
        pkg-config libpipewire-0.3-dev libfontconfig-dev libasound2-dev \
        libglib2.0-dev libva-dev \
        mesa-vulkan-drivers libvulkan1 \
    && rm -rf /var/lib/apt/lists/*

# clang 21 from apt.llvm.org for webrtc-sys (>= 21, hermetic libc++) and
# bindgen: just the compiler and libclang, not llvm.sh's lldb/lld/clangd.
RUN wget -qO /etc/apt/trusted.gpg.d/apt.llvm.org.asc https://apt.llvm.org/llvm-snapshot.gpg.key \
    && echo "deb http://apt.llvm.org/noble/ llvm-toolchain-noble-21 main" > /etc/apt/sources.list.d/llvm.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends clang-21 libclang-21-dev \
    && rm -rf /var/lib/apt/lists/* \
    && clang++-21 --version

ENV CC=clang-21 \
    CXX=clang++-21 \
    LIBCLANG_PATH=/usr/lib/llvm-21/lib \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

# Workflows still run dtolnay/rust-toolchain@stable, which only downloads
# when a newer stable is out, so a stale image costs one toolchain update.
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal \
        --default-toolchain stable --component rustfmt,clippy \
    && rustc --version \
    && cargo install --locked cargo-about@0.9.3 --features cli \
    && rm -rf $CARGO_HOME/registry

# Android (Phase 6): JDK 21 for Gradle (8.14 can't run on 25), and the SDK
# pieces android/app needs: platform 36, AGP 8.13's default build-tools, and
# the NDK whose clang android/build-rust.sh links with. Gradle itself and
# its dependencies download per run. Rust's Android targets come from the
# workflow's rust-toolchain step, so a stable update brings them along.
RUN apt-get update \
    && apt-get install -y --no-install-recommends openjdk-21-jdk-headless \
    && rm -rf /var/lib/apt/lists/*
ENV ANDROID_HOME=/opt/android-sdk \
    JAVA_HOME=/usr/lib/jvm/java-21-openjdk-amd64
RUN mkdir -p $ANDROID_HOME/cmdline-tools \
    && curl -sSfo /tmp/clt.zip https://dl.google.com/android/repository/commandlinetools-linux-13114758_latest.zip \
    && unzip -q /tmp/clt.zip -d $ANDROID_HOME/cmdline-tools \
    && mv $ANDROID_HOME/cmdline-tools/cmdline-tools $ANDROID_HOME/cmdline-tools/latest \
    && rm /tmp/clt.zip \
    && yes | $ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager --licenses > /dev/null \
    && $ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager --install \
        "platforms;android-36" "build-tools;35.0.0" "ndk;29.0.14206865" > /dev/null \
    && ls $ANDROID_HOME/ndk

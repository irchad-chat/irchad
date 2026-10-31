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
# build-essential for rustc's linker; PipeWire headers for pipewire-sys;
# lavapipe (software Vulkan) so the wgpu screenshot tests find an adapter.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates curl wget git jq zstd xz-utils unzip build-essential \
        libpipewire-0.3-dev libfontconfig-dev pkg-config \
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
    && rustc --version

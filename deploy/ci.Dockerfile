# Job container for the Linux CI/nightly/release builds: everything the old
# per-job apt + llvm.sh step installed, baked in once. Built and pushed to
# git.gentoo.party/sam/irchad-ci by .gitea/workflows/ci-image.yml.
#
# Ubuntu 24.04 matches the runner's ubuntu-latest, so shipped binaries keep
# the same glibc floor (2.39).
#
# Cloudflare in front of the registry rejects requests over 100 MB, and
# `docker push` uploads each layer in one request, so the packages go in
# several smaller layers (ci-image.yml prints their sizes). Rust isn't in
# here for the same reason: dtolnay/rust-toolchain installs it in seconds.
FROM docker.io/library/ubuntu:24.04

# JS actions (checkout, rust-cache) run on the container's `node`. The
# official binary is glibc-linked against bookworm (2.36), so it runs here.
COPY --from=docker.io/library/node:22-bookworm-slim /usr/local/bin/node /usr/local/bin/node

# Checkout, the cache action, release-asset uploads, and the C toolchain
# that rustc links with.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates curl wget git jq zstd xz-utils unzip build-essential \
    && rm -rf /var/lib/apt/lists/*

# PipeWire headers for pipewire-sys.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        libpipewire-0.3-dev libfontconfig-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*

# lavapipe (software Vulkan) so the wgpu screenshot tests find an adapter.
RUN apt-get update \
    && apt-get install -y --no-install-recommends mesa-vulkan-drivers libvulkan1 \
    && rm -rf /var/lib/apt/lists/*

# clang 21 from apt.llvm.org for webrtc-sys (>= 21, hermetic libc++) and
# bindgen: just the compiler and libclang, not llvm.sh's lldb/lld/clangd.
# libllvm21 goes first, in its own layer, to keep each layer small.
RUN wget -qO /etc/apt/trusted.gpg.d/apt.llvm.org.asc https://apt.llvm.org/llvm-snapshot.gpg.key \
    && echo "deb http://apt.llvm.org/noble/ llvm-toolchain-noble-21 main" > /etc/apt/sources.list.d/llvm.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends libllvm21 \
    && rm -rf /var/lib/apt/lists/*
RUN apt-get update \
    && apt-get install -y --no-install-recommends clang-21 \
    && rm -rf /var/lib/apt/lists/*
RUN apt-get update \
    && apt-get install -y --no-install-recommends libclang-21-dev \
    && rm -rf /var/lib/apt/lists/* \
    && clang++-21 --version

ENV CC=clang-21 \
    CXX=clang++-21 \
    LIBCLANG_PATH=/usr/lib/llvm-21/lib

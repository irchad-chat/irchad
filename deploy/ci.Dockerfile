# Job container for the Linux CI/nightly/release builds: everything the old
# per-job apt + llvm.sh step installed, baked in once. Built and pushed to
# git.gentoo.party/sam/irchad-ci by .gitea/workflows/ci-image.yml.
#
# Ubuntu 24.04 matches the runner's ubuntu-latest, so shipped binaries keep
# the same glibc floor (2.39).
FROM docker.io/library/ubuntu:24.04

# JS actions (checkout, rust-cache) run on the container's `node`. The
# official binary is glibc-linked against bookworm (2.36), so it runs here.
COPY --from=docker.io/library/node:22-bookworm-slim /usr/local/bin/node /usr/local/bin/node

# PipeWire headers for pipewire-sys; lavapipe (software Vulkan) so the wgpu
# screenshot tests find an adapter; clang 21 from apt.llvm.org for
# webrtc-sys (>= 21, hermetic libc++) and bindgen. git/zstd/jq/curl for
# checkout, the cache action, and the release-asset uploads.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates curl wget git jq zstd xz-utils unzip build-essential \
        gnupg lsb-release software-properties-common \
        libpipewire-0.3-dev libfontconfig-dev pkg-config \
        mesa-vulkan-drivers libvulkan1 \
    && wget -qO /tmp/llvm.sh https://apt.llvm.org/llvm.sh \
    && bash /tmp/llvm.sh 21 \
    && apt-get install -y --no-install-recommends libclang-21-dev \
    && rm -rf /tmp/llvm.sh /var/lib/apt/lists/* \
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

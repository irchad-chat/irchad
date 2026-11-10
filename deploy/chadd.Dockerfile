# Built from the workspace root (compose sets context: ..).
FROM docker.io/library/rust:1-slim-trixie AS build
WORKDIR /src
# Before COPY, so the layer stays cached.
RUN cargo install --locked cargo-about@0.9.3 --features cli
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p chadd && cp target/release/chadd /chadd \
    && packaging/licenses.sh /licenses chadd x86_64-unknown-linux-gnu

FROM docker.io/library/debian:trixie-slim
# Link previews fetch arbitrary https sites.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /chadd /usr/local/bin/chadd
COPY --from=build /licenses /usr/share/doc/chadd
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/chadd", "/etc/chadd/chadd.toml"]

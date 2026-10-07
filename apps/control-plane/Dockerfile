FROM rust:1.90.0-bookworm AS builder
WORKDIR /src
COPY proto/ proto/
COPY schemas/ schemas/
COPY apps/control-plane/ apps/control-plane/
RUN cargo build --locked --release --manifest-path apps/control-plane/Cargo.toml

FROM debian:bookworm-slim
RUN useradd -r -u 10001 aether \
    && mkdir -p /data \
    && chown -R 10001:10001 /data
COPY --from=builder /src/apps/control-plane/target/release/aether-control-plane /usr/local/bin/aether-control-plane
USER 10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/aether-control-plane"]

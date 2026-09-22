FROM rust:1-bookworm AS builder
WORKDIR /src
COPY proto/ proto/
COPY apps/control-plane/ apps/control-plane/
RUN cargo build --release --manifest-path apps/control-plane/Cargo.toml
FROM debian:bookworm-slim
RUN useradd -r -u 10001 aether
COPY --from=builder /src/apps/control-plane/target/release/aether-control-plane /usr/local/bin/aether-control-plane
USER 10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/aether-control-plane"]

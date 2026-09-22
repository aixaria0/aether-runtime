FROM debian:bookworm AS builder
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends cmake g++ make libgrpc++-dev protobuf-compiler-grpc libprotobuf-dev libssl-dev pkg-config && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY proto/ proto/
COPY apps/execution-engine/ apps/execution-engine/
RUN cmake -S apps/execution-engine -B /build -DCMAKE_BUILD_TYPE=Release -DBUILD_TESTING=ON && cmake --build /build -j2 && ctest --test-dir /build --output-on-failure
FROM debian:bookworm-slim
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends libgrpc++1.51 libprotobuf32 libssl3 && rm -rf /var/lib/apt/lists/* && useradd -r -u 10001 aether
COPY --from=builder /build/aether-execution-engine /usr/local/bin/aether-execution-engine
USER 10001
EXPOSE 50051
ENTRYPOINT ["/usr/local/bin/aether-execution-engine"]

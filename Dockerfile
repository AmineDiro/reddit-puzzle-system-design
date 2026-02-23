FROM rust:latest as builder

# Install build dependencies
RUN apt-get update && apt-get install -y cmake clang pkg-config && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/bare-metal-canvas
# Copy workspace config
COPY Cargo.toml ./

# Copy packages
COPY server ./server
COPY client ./client

# Build server
RUN cargo build --release -p server

FROM debian:bookworm-slim
# Runtime dependencies
RUN apt-get update && apt-get install -y libzstd1 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/bare-metal-canvas/target/release/server /usr/local/bin/server

EXPOSE 4433/udp

# NOTE: Since io_uring requires kernel interaction, run this container with --privileged
CMD ["server"]

FROM rust:1-slim-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/*
ENV TZ=Asia/Shanghai
COPY --from=builder /build/target/release/yulink-linux /usr/local/bin/yulink-linux
ENTRYPOINT ["yulink-linux"]

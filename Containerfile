FROM alpine:3.24 AS build
RUN apk add --no-cache rust cargo
WORKDIR /build
COPY Cargo.toml .
COPY src/ src/
RUN RUSTFLAGS="-C target-feature=+crt-static" cargo build --release

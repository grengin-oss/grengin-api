FROM rust:1.89-alpine AS builder

# sys deps
RUN apk add --no-cache \
  build-base curl pkgconfig perl clang lld musl-dev \
  openssl-dev openssl-libs-static ca-certificates cargo

# install rust (adds cargo)
ENV CARGO_HOME=/root/.cargo \
    RUSTUP_HOME=/root/.rustup \
    PATH=/root/.cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
 && rustup target add x86_64-unknown-linux-musl

WORKDIR /usr/src/grengin-api

# cache deps
COPY Cargo.* .

# build
COPY src ./src
RUN cargo build --release --target x86_64-unknown-linux-musl

EXPOSE 8080

# runtime
FROM alpine:latest
RUN apk add --no-cache libssl3 ca-certificates
COPY --from=builder /usr/src/grengin-api/target/x86_64-unknown-linux-musl/release/grengin-api /usr/local/bin/app
CMD ["app"]
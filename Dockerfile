FROM rust:1.91-alpine AS builder
ARG TARGETARCH
ARG TARGETPLATFORM

# sys deps (no openssl needed now)
RUN apk add --no-cache build-base curl pkgconfig perl clang lld musl-dev ca-certificates

# install rustup + musl target
ENV CARGO_HOME=/usr/local/cargo RUSTUP_HOME=/root/.rustup PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
 && ARCH="${TARGETARCH}" \
 && if [ -z "$ARCH" ]; then ARCH="$(uname -m)"; fi \
 && if [ "$ARCH" = "arm64" ] || [ "$ARCH" = "aarch64" ]; then \
      rustup target add aarch64-unknown-linux-musl; \
    else \
      rustup target add x86_64-unknown-linux-musl; \
    fi

WORKDIR /usr/src/grengin-api

# cache deps
# copy the minimal set that makes dependency resolution stable
COPY Cargo.* ./
COPY migration/Cargo.toml migration/Cargo.toml
# create empty src trees so cargo can resolve features without invalidating cache
RUN mkdir -p src migration/src && echo "fn main(){}" > src/main.rs && echo "" > migration/src/lib.rs
RUN cargo fetch
# install sqlx-mcp from lihongjie0209 repo (supports --database-url args)
ENV RUSTFLAGS="-C target-feature=+crt-static"
RUN ARCH="${TARGETARCH}" \
 && if [ -z "$ARCH" ]; then ARCH="$(uname -m)"; fi \
 && if [ "$ARCH" = "arm64" ] || [ "$ARCH" = "aarch64" ]; then \
      RUST_TARGET=aarch64-unknown-linux-musl; \
    else \
      RUST_TARGET=x86_64-unknown-linux-musl; \
    fi \
 && cargo install --git https://github.com/rbatis/rbdc-mcp.git --target "$RUST_TARGET" --root /usr/local/cargo

# now copy real sources
COPY src ./src
COPY migration ./migration
COPY swagger-overrides .
ENV SWAGGER_UI_OVERWRITE_FOLDER=/swagger-overrides

# build (fully static by default on musl)
RUN ARCH="${TARGETARCH}" \
 && if [ -z "$ARCH" ]; then ARCH="$(uname -m)"; fi \
 && if [ "$ARCH" = "arm64" ] || [ "$ARCH" = "aarch64" ]; then \
      RUST_TARGET=aarch64-unknown-linux-musl; \
    else \
      RUST_TARGET=x86_64-unknown-linux-musl; \
    fi \
 && cargo build --release --target "$RUST_TARGET" \
 && cp "/usr/src/grengin-api/target/$RUST_TARGET/release/grengin-api" /usr/local/bin/grengin-api

# runtime: static binary; only certs if your app makes HTTPS requests
FROM scratch
# for HTTPS/TLS trust store:
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /usr/local/bin/grengin-api /usr/local/bin/app
COPY --from=builder /usr/local/cargo/bin/rbdc-mcp /usr/local/cargo/bin/rbdc-mcp
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/app"]

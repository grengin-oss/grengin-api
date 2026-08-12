# SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
# SPDX-License-Identifier: Apache-2.0

FROM rust:1.91-alpine AS builder
ARG TARGETARCH

# sys deps (no openssl needed now)
RUN apk add --no-cache build-base curl pkgconfig perl clang lld musl-dev ca-certificates

# install rustup + musl target
ENV CARGO_HOME=/usr/local/cargo RUSTUP_HOME=/root/.rustup PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
 && case "$TARGETARCH" in \
      amd64) rustup target add x86_64-unknown-linux-musl ;; \
      arm64) rustup target add aarch64-unknown-linux-musl ;; \
      *) echo "Unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac

WORKDIR /usr/src/grengin-api

# cache deps
# copy the minimal set that makes dependency resolution stable
COPY Cargo.* ./
COPY llm-plugin/Cargo.toml llm-plugin/Cargo.toml
COPY migration/Cargo.toml migration/Cargo.toml
COPY sqlx-mcp/Cargo.toml sqlx-mcp/Cargo.toml
# create empty src trees so cargo can resolve features without invalidating cache
RUN mkdir -p src llm-plugin/src migration/src sqlx-mcp/src \
 && echo "fn main(){}" > src/main.rs \
 && echo "" > llm-plugin/src/lib.rs \
 && echo "" > migration/src/lib.rs \
 && echo "fn main(){}" > sqlx-mcp/src/main.rs
RUN cargo fetch
ENV RUSTFLAGS="-C target-feature=+crt-static"
RUN case "$TARGETARCH" in \
      amd64) RUST_TARGET="x86_64-unknown-linux-musl" ;; \
      arm64) RUST_TARGET="aarch64-unknown-linux-musl" ;; \
      *) echo "Unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac \
 && cargo build --release --target "$RUST_TARGET" -p grengin-api -p sqlx-mcp -j 2

# now copy real sources
COPY src ./src
COPY llm-plugin ./llm-plugin
COPY migration ./migration
COPY sqlx-mcp ./sqlx-mcp
COPY swagger-overrides .
RUN touch src/main.rs llm-plugin/src/lib.rs migration/src/lib.rs sqlx-mcp/src/main.rs
ENV SWAGGER_UI_OVERWRITE_FOLDER=/swagger-overrides

# build (fully static by default on musl)
RUN case "$TARGETARCH" in \
      amd64) RUST_TARGET="x86_64-unknown-linux-musl" ;; \
      arm64) RUST_TARGET="aarch64-unknown-linux-musl" ;; \
      *) echo "Unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac \
 && cargo build --release --target "$RUST_TARGET" -p grengin-api -p sqlx-mcp -j 2 \
 && cp "/usr/src/grengin-api/target/$RUST_TARGET/release/grengin-api" /usr/local/bin/grengin-api \
 && cp "/usr/src/grengin-api/target/$RUST_TARGET/release/sqlx-mcp" /usr/local/bin/sqlx-mcp

# runtime: static binary; only certs if your app makes HTTPS requests
FROM scratch
LABEL org.opencontainers.image.licenses="Apache-2.0"
# for HTTPS/TLS trust store:
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /usr/local/bin/grengin-api /usr/local/bin/app
COPY --from=builder /usr/local/bin/sqlx-mcp /usr/local/bin/sqlx-mcp
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/app"]

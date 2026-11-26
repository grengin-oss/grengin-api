# SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
# SPDX-License-Identifier: Apache-2.0

# glibc (Debian 12) instead of static musl: the runtime image below ships the same glibc 2.36
# and receives Debian security updates when the image is rebuilt, instead of a libc frozen
# into the binary.
FROM rust:1.91-bookworm AS builder
ARG TARGETARCH
ARG CARGO_BUILD_JOBS=2

# utoipa-swagger-ui downloads its pinned Swagger UI archive with curl unless
# the vendored feature is enabled. Keep the builder self-contained and retain
# TLS certificates for the download. OpenSSL is vendored by the openssl crate.
RUN apt-get update \
 && apt-get install -y --no-install-recommends build-essential pkg-config perl clang lld curl ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# Legacy builders leave TARGETARCH unset; BuildKit's explicit target takes precedence.
RUN arch="${TARGETARCH:-$(uname -m)}" \
 && case "$arch" in \
      amd64|x86_64) RUST_TARGET="x86_64-unknown-linux-gnu" ;; \
      arm64|aarch64) RUST_TARGET="aarch64-unknown-linux-gnu" ;; \
      *) echo "Unsupported target architecture: $arch" >&2; exit 1 ;; \
    esac \
 && printf '%s\n' "$RUST_TARGET" > /tmp/grengin-rust-target \
 && rustup target add "$RUST_TARGET"

WORKDIR /usr/src/grengin-api

# cache deps
# copy the minimal set that makes dependency resolution stable
COPY Cargo.* ./
COPY llm-plugin/Cargo.toml llm-plugin/Cargo.toml
COPY connector-plugin/Cargo.toml connector-plugin/Cargo.toml
COPY migration/Cargo.toml migration/Cargo.toml
# create empty src trees so cargo can resolve features without invalidating cache
RUN mkdir -p src llm-plugin/src connector-plugin/src migration/src \
 && echo "fn main(){}" > src/main.rs \
 && echo "" > llm-plugin/src/lib.rs \
 && echo "" > connector-plugin/src/lib.rs \
 && echo "" > migration/src/lib.rs
COPY swagger-overrides /swagger-overrides
ENV SWAGGER_UI_OVERWRITE_FOLDER=/swagger-overrides
RUN cargo fetch --locked
RUN RUST_TARGET="$(cat /tmp/grengin-rust-target)" \
 && cargo build --release --locked --target "$RUST_TARGET" -p grengin-api -j "$CARGO_BUILD_JOBS"

# now copy real sources
COPY src ./src
COPY llm-plugin ./llm-plugin
COPY connector-plugin ./connector-plugin
COPY migration ./migration
RUN touch src/main.rs llm-plugin/src/lib.rs connector-plugin/src/lib.rs migration/src/lib.rs

# build (dynamically linked against glibc; everything else, including OpenSSL, is static)
RUN RUST_TARGET="$(cat /tmp/grengin-rust-target)" \
 && cargo build --release --locked --target "$RUST_TARGET" -p grengin-api -p migration -j "$CARGO_BUILD_JOBS" \
 && cp "/usr/src/grengin-api/target/$RUST_TARGET/release/grengin-api" /usr/local/bin/grengin-api \
 && cp "/usr/src/grengin-api/target/$RUST_TARGET/release/migration" /usr/local/bin/grengin-migrate

# runtime: glibc, libgcc and the CA trust store (/etc/ssl/certs/ca-certificates.crt), no shell.
# Pinned by digest (multi-arch index); bump it to pick up Debian security updates.
FROM gcr.io/distroless/cc-debian12@sha256:e5d81ddde149641e2a9ba55be4545bc125c67de07508b03ba4c22e6eb0ded5aa
ARG IMAGE_VERSION=dev
ARG IMAGE_REVISION=unknown
ARG MIGRATION_HEAD=unknown
LABEL org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.source="https://github.com/grengin-oss/grengin-api" \
      org.opencontainers.image.version="${IMAGE_VERSION}" \
      org.opencontainers.image.revision="${IMAGE_REVISION}" \
      io.grengin.migration-head="${MIGRATION_HEAD}"
COPY --from=builder /usr/local/bin/grengin-api /usr/local/bin/app
COPY --from=builder /usr/local/bin/grengin-migrate /usr/local/bin/grengin-migrate
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/app"]

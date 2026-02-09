FROM rust:1.91-alpine AS builder

# sys deps (no openssl needed now)
RUN apk add --no-cache build-base curl pkgconfig perl clang lld musl-dev ca-certificates

# install rustup + musl target
ENV CARGO_HOME=/root/.cargo RUSTUP_HOME=/root/.rustup PATH=/root/.cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
 && rustup target add x86_64-unknown-linux-musl

WORKDIR /usr/src/grengin-api

# cache deps
# copy the minimal set that makes dependency resolution stable
COPY Cargo.* ./
COPY migration/Cargo.toml migration/Cargo.toml
# create empty src trees so cargo can resolve features without invalidating cache
RUN mkdir -p src migration/src && echo "fn main(){}" > src/main.rs && echo "" > migration/src/lib.rs
RUN cargo fetch

# now copy real sources
COPY src ./src
COPY migration ./migration
COPY swagger-overrides .
ENV SWAGGER_UI_OVERWRITE_FOLDER=/swagger-overrides

# build (fully static by default on musl)
ENV RUSTFLAGS="-C target-feature=+crt-static"
RUN cargo build --release --target x86_64-unknown-linux-musl

# runtime: static binary; only certs if your app makes HTTPS requests
FROM scratch
# for HTTPS/TLS trust store:
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /usr/src/grengin-api/target/x86_64-unknown-linux-musl/release/grengin-api /usr/local/bin/app
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/app"]
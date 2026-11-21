# SPDX-License-Identifier: AGPL-3.0-or-later
# Build context: repository root.  docker build -f server/Dockerfile -t alwywatch-server .
FROM rust:1-alpine AS tools
RUN apk add --no-cache musl-dev
# Must match the wasm-bindgen version pinned in web/Cargo.toml.
RUN cargo install --locked wasm-bindgen-cli --version 0.2.130

FROM rust:1-alpine AS build
# clang and llvm (llvm-ar): ring's C parts for the browser build of the web app.
RUN apk add --no-cache musl-dev ca-certificates gzip clang llvm \
 && rustup target add wasm32-unknown-unknown
COPY --from=tools /usr/local/cargo/bin/wasm-bindgen /usr/local/bin/wasm-bindgen
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p alwywatch-server \
 && cp target/release/alwywatch-server /alwywatch-server \
 && web/build.sh

FROM scratch
COPY --from=build /alwywatch-server /alwywatch-server
# The web app, served at / (ALWYWATCH_WEB).
COPY --from=build /src/web/dist /web
# TLS to plex.tv: the system trust store is read from here.
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt \
    ALWYWATCH_LISTEN=0.0.0.0:8080 \
    ALWYWATCH_DATA=/data \
    ALWYWATCH_CONFIG=/data/config.toml
VOLUME ["/data"]
EXPOSE 8080
USER 65534:65534
ENTRYPOINT ["/alwywatch-server"]

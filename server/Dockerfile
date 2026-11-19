# SPDX-License-Identifier: AGPL-3.0-or-later
# Build context: repository root.  docker build -f server/Dockerfile -t alwywatch-server .
FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p alwywatch-server \
 && cp target/release/alwywatch-server /alwywatch-server

FROM scratch
COPY --from=build /alwywatch-server /alwywatch-server
# SQLite database and config live in /data (mount a volume).
VOLUME ["/data"]
ENV ALWYWATCH_LISTEN=0.0.0.0:8080 \
    ALWYWATCH_DATA=/data
EXPOSE 8080
USER 65534:65534
ENTRYPOINT ["/alwywatch-server"]

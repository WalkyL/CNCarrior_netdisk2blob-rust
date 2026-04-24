FROM rust:1.94-bookworm AS builder

WORKDIR /app
COPY . .
RUN cargo build --release -p gatewayd

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --shell /usr/sbin/nologin gateway

WORKDIR /srv/gateway
COPY --from=builder /app/target/release/gatewayd /usr/local/bin/gatewayd
COPY config/example.env /srv/gateway/example.env

USER gateway
EXPOSE 61080
ENTRYPOINT ["/usr/local/bin/gatewayd"]

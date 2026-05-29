# ESP32-S3 Relay-Lite Feasibility

## Decision

Current decision: do not make ESP32-S3 a near-term gateway host.

Relay-lite remains a conditional future experiment. The product path should stay:

1. Linux/OpenWRT host runs `gatewayd`.
2. ESP32-S3 runs client-only S3 calls.
3. Relay-lite is reconsidered only after provider code can be expressed as chunked callbacks without SQLite, OneDrive, browser auth, or background replication.

## PoC

The boundary PoC lives in [examples/esp32-s3-relay-lite-poc](../examples/esp32-s3-relay-lite-poc/README.md).

It validates:

- one provider interface
- one request in flight
- `1024` byte chunk buffer
- `64 KiB` max object budget
- no dynamic provider registry
- no SQLite
- no OneDrive
- no replication worker

Run:

```bash
scripts/check-esp32-s3-relay-lite-poc.sh
```

## Acceptance For Future Real PoC

A future board-level relay-lite PoC must prove:

- stable heap under repeated `PutObject` / `GetObject`
- no object-sized heap allocation
- no provider credentials outside a board-secure storage area
- clear failure behavior on network timeout
- no retry loop that can block the main system indefinitely
- recovery after power loss without SQLite

## Go / No-Go

- client-only: `go`
- relay-lite with fake provider boundary: `go for investigation`
- relay-lite with one real carrier provider: `conditional go`
- full daemon on ESP32-S3: `no-go`

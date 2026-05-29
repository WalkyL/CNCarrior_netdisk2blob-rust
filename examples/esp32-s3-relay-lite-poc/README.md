# ESP32-S3 Relay-Lite PoC

This is a feasibility boundary PoC, not a product feature.

It models the smallest relay shape that could plausibly fit on an ESP32-S3:

- one provider interface
- one request in flight
- fixed `1024` byte chunk buffer
- max object `64 KiB`
- no SQLite
- no OneDrive
- no replication worker
- no Admin Web
- no provider credential browser flow

The PoC does not implement a real carrier provider. It uses an in-memory provider in `example_main.c` so the resource and API boundary can be compiled and inspected without network dependencies.

Current go/no-go:

- `go` for continued client-only work
- `conditional go` for a future relay experiment only after a real provider can be expressed as chunked read/write callbacks
- `no-go` for making ESP32-S3 a near-term host platform for the current daemon

Host-side check:

```bash
scripts/check-esp32-s3-relay-lite-poc.sh
```

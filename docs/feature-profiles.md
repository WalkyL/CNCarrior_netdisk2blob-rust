# Feature Profiles

## Profiles

The project uses four explicit cargo profile names:

- `full-host`: complete Linux host, including `gatewayd`, Admin Web, SQLite metadata, provider adapters, replication, and optional OneDrive.
- `lite-host`: OpenWRT-class host, still Linux-based, with `gatewayd` and local metadata but conservative runtime defaults.
- `esp-client`: client-only profile for MCU examples. It must not compile the host daemon or pull host dependencies.
- `esp-relay`: feasibility profile for a future single-provider relay. It must not compile the host daemon or pull host dependencies.

`gatewayd` currently supports only host profiles:

```bash
cargo check -p gatewayd --no-default-features --features full-host
cargo check -p gatewayd --no-default-features --features lite-host
```

ESP profiles are represented by `ccbg-platform-profiles` until MCU-specific client crates exist. This is intentional: ESP targets should not inherit `rusqlite`, `reqwest`, `tower-http`, `axum`, provider adapters, or the replication daemon by accident.

## Verification

Run:

```bash
scripts/check-feature-profiles.sh
```

The script verifies:

- all four profile names compile through `ccbg-platform-profiles`
- `gatewayd` compiles under `full-host`
- `gatewayd` compiles under `lite-host`
- `esp-client` and `esp-relay` do not pull `rusqlite`, `reqwest`, `tower-http`, or `axum`

This keeps the product boundary explicit: host profiles produce daemon builds; ESP profiles stay client/relay contracts until dedicated MCU code is added.

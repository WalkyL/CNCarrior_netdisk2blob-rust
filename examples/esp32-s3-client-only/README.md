# ESP32-S3 Client-Only Example

This ESP-IDF example adapts the portable STM32 client-only code to ESP32-S3. The board remains a client of a nearby CCBG host; it does not run the gateway daemon.

## What It Does

- Uses mbedTLS for SHA-256 and HMAC-SHA256
- Uses `esp_http_client` for HTTP
- Reuses `examples/stm32-client-only/ccbg_stm32_client.c`
- Calls `PutObject`, `HeadObject`, and `GetObject`
- Uses `UNSIGNED-PAYLOAD` streaming upload
- Keeps one request in flight
- Keeps a `1024` byte IO chunk
- Keeps the default object budget at `32 KiB`

## What It Does Not Do

- No `gatewayd`
- No SQLite
- No local replication engine
- No OneDrive
- No provider credentials
- No Admin Web
- No MCP stdio

## Build

From this directory, with ESP-IDF installed:

```bash
idf.py set-target esp32s3
idf.py menuconfig
idf.py build
```

Configure the gateway host and S3 credentials under:

```text
CCBG ESP32-S3 Client-Only Demo
```

The demo assumes Wi-Fi or Ethernet is already connected and system time is valid before `app_main` runs. In a real firmware project, call the same client functions after your network and SNTP setup completes.

## Gateway Configuration

Use a dedicated S3 key:

```dotenv
CCBG_BIND_ADDR=0.0.0.0:61080
CCBG_S3_ACCESS_KEY_ID=ccbg-esp32
CCBG_S3_SECRET_ACCESS_KEY=<board-specific-secret>
CCBG_PRIMARY_PROVIDER=stub
CCBG_ONEDRIVE_ENABLED=false
CCBG_ONEDRIVE_REPLICATION_ENABLED=false
```

## Acceptance

Board-level pass criteria:

- `PutObject` succeeds for `esp32-s3/demo.txt`
- `HeadObject` returns success for the uploaded object
- `GetObject` reads the object through the chunk sink
- heap telemetry shows no object-sized buffer allocation
- retry count remains bounded
- no provider credentials or control API key are compiled into firmware

Host-side structural check:

```bash
scripts/check-esp32-s3-client-example.py
```

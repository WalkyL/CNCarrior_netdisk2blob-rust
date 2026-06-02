# OPS-007: .47 Release Build Host

## Status

Accepted

## Date

2026-06-01

## Decision

`192.168.1.47` is the default CCBG build and release host. GitHub is only the source, branch, tag, issue, optional Release page, and release-record system. GitHub Actions must not compile Linux, OpenWrt, Windows, container tar/image, or Cloudflare release artifacts. The current local/LAN environment has no macOS builder, so macOS release assets use a GitHub Actions macOS-only exception build entry until a local/LAN macOS builder is documented.

A tag or GitHub Release page is not a completed release. A release is complete only after the local/LAN build artifacts have been generated, synchronized to the public delivery path, and `/downloads/latest/*` resolves to the new assets.

The `.47` workspace is:

```text
C:\Users\walky\workspaces\carrier-cloud-blob-gateway
```

`.46` must not keep a CCBG checkout and must not run CCBG build tasks.

## Build Host Mapping

| Target / architecture | Build host | Release status | Build entry |
| --- | --- | --- | --- |
| PVE LXC `x86/x64` | `.47` | Official host | `scripts/release-local.sh <tag>` |
| Linux native `x86_64-unknown-linux-gnu` | `.47` | Official package input | `CCBG_RELEASE_LINUX_TARGET=x86_64-unknown-linux-gnu scripts/release-local.sh <tag>` |
| Docker `x86/x64` | `.47` | Official host | `docker build -f deploy/Dockerfile .` |
| Podman `x86/x64` | `.47` | Official host | `podman build -f deploy/Containerfile .` |
| Windows `x86_64` | `.47` | Official host | `CCBG_RELEASE_BUILD_WINDOWS=true scripts/release-local.sh <tag>` |
| OpenWrt `arm64` | `.47` | Experimental host | `CCBG_RELEASE_BUILD_OPENWRT=true scripts/release-local.sh <tag>` |
| macOS `x86_64` | GitHub Actions macOS-only exception, merged on `.47` | Community / experimental package | `CCBG_RELEASE_MACOS_ASSET_DIR=<downloaded-artifacts> scripts/release-local.sh <tag>` |
| macOS `arm64` | GitHub Actions macOS-only exception, merged on `.47` | Community / experimental package | `CCBG_RELEASE_MACOS_ASSET_DIR=<downloaded-artifacts> scripts/release-local.sh <tag>` |
| STM32 client-only example | `.47` | Embedded client example | `CC='zig cc' scripts/check-stm32-client-example.sh` |
| ESP32-S3 client-only example | `.47` | Embedded client example | `$(bash scripts/resolve-python.sh) scripts/check-esp32-s3-client-example.py` |
| ESP32-S3 relay-lite PoC | `.47` | Feasibility PoC | `CC='zig cc' scripts/check-esp32-s3-relay-lite-poc.sh` |

## macOS Boundary

macOS packages are community / experimental packages. They are unsigned, unnotarized, and not macOS smoke-tested by the official release gate.

Current state: this project has no local/LAN macOS builder. For releases, build macOS assets with the GitHub Actions macOS-only workflow, download those artifacts, and merge them on `.47` with `CCBG_RELEASE_MACOS_ASSET_DIR`. This exception does not restore GitHub Actions as a general CI/release system. The resulting assets must still enter the same checksum, R2/GitHub fallback, and `/downloads/latest/*` smoke path.

## Embedded Boundary

STM32 and ESP32-S3 are not gateway hosts. They are client-only examples that call a nearby Linux/OpenWrt/Windows gateway through the local S3 data plane.

If future firmware builds are added, they inherit the same rule: build from `.47`, keep memory use bounded, and do not introduce provider credentials, SQLite, Admin Web, MCP stdio, or full replication logic into embedded firmware.

## Verification

Before publishing a release from `.47`, run:

```bash
bash -n scripts/build-lxc-package.sh scripts/release-local.sh
scripts/check-release-ready.sh
CCBG_RELEASE_BUILD_WINDOWS=true CCBG_RELEASE_BUILD_OPENWRT=true scripts/release-local.sh <tag>
```

If release assets must be uploaded to GitHub, `scripts/release-local.sh` resolves GitHub CLI
through `scripts/resolve-gh.sh`; `.47` can use `C:\Program Files\GitHub CLI\gh.exe` even when
Git Bash does not expose `gh` in `PATH`.

Merge downloaded macOS artifacts on `.47`:

```bash
CCBG_RELEASE_MACOS_ASSET_DIR=<downloaded-artifacts> scripts/release-local.sh <tag>
```

For embedded examples:

```bash
CC='zig cc' scripts/check-stm32-client-example.sh
$(bash scripts/resolve-python.sh) scripts/check-esp32-s3-client-example.py
CC='zig cc' scripts/check-esp32-s3-relay-lite-poc.sh
```

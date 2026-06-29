# OPS-007: .52 Release Build Host

## Status

Accepted

## Date

2026-06-01

## Decision

`192.168.1.52` is the default CCBG build and release host. GitHub is only the source, branch, tag, issue, optional Release page, and release-record system. GitHub Actions must not become a generic or automatic build/deploy system for Linux, OpenWrt, Windows, container tar/image, or Cloudflare release artifacts.

There is one controlled exception: when `.52` cannot build Linux locally, a manually triggered self-hosted build-runner workflow `release-assets-build-runner.yml` may produce the Linux LXC package inside the local Podman image `localhost/product-build-runner:latest`. macOS release assets use the same self-hosted build-runner container path. Those artifacts still merge back on `.52`; GitHub Actions does not publish them directly.

A tag or GitHub Release page is not a completed release. A release is complete only after the local/LAN build artifacts have been generated, synchronized to the public delivery path, and `/downloads/latest/*` resolves to the new assets.

The `.52` workspace is:

```text
C:\Users\walky\workspaces\carrier-cloud-blob-gateway
```

`.46` must not keep a CCBG checkout and must not run CCBG build tasks.

## Build Host Mapping

| Target / architecture | Build host | Release status | Build entry |
| --- | --- | --- | --- |
| PVE LXC `x86/x64` | `.52`; fallback: self-hosted build-runner container, merged on `.52` | Official host | `scripts/release-local.sh <tag>`; fallback: `CCBG_RELEASE_LXC_ASSET_DIR=<downloaded-artifacts> scripts/release-local.sh <tag>` |
| Linux native `x86_64-unknown-linux-gnu` | `.52` | Official package input | `CCBG_RELEASE_LINUX_TARGET=x86_64-unknown-linux-gnu scripts/release-local.sh <tag>` |
| Docker `x86/x64` | `.52` | Official host | `docker build -f deploy/Dockerfile .` |
| Podman `x86/x64` | `.52` | Official host | `podman build -f deploy/Containerfile .` |
| Windows `x86_64` | `.52` | Official host | `CCBG_RELEASE_BUILD_WINDOWS=true scripts/release-local.sh <tag>` |
| OpenWrt `arm64` | `.52` | Experimental host | `CCBG_RELEASE_BUILD_OPENWRT=true scripts/release-local.sh <tag>` |
| macOS `x86_64` | self-hosted build-runner container on the configured LAN runner, merged on `.52` | Community / experimental package | `CCBG_RELEASE_MACOS_ASSET_DIR=<downloaded-artifacts> scripts/release-local.sh <tag>` |
| macOS `arm64` | self-hosted build-runner container on the configured LAN runner, merged on `.52` | Community / experimental package | `CCBG_RELEASE_MACOS_ASSET_DIR=<downloaded-artifacts> scripts/release-local.sh <tag>` |
| STM32 client-only example | `.52` | Embedded client example | `CC='zig cc' scripts/check-stm32-client-example.sh` |
| ESP32-S3 client-only example | `.52` | Embedded client example | `$(bash scripts/resolve-python.sh) scripts/check-esp32-s3-client-example.py` |
| ESP32-S3 relay-lite PoC | `.52` | Feasibility PoC | `CC='zig cc' scripts/check-esp32-s3-relay-lite-poc.sh` |

## macOS Boundary

macOS packages are community / experimental packages. They are unsigned, unnotarized, and not macOS smoke-tested by the official release gate.

Current state: macOS assets are produced by the GitHub Actions workflow `release-assets-build-runner.yml` running inside the configured self-hosted build-runner container. When `.52` local Linux build is unavailable, the same workflow may also produce the Linux LXC package. Download those artifacts and merge them on `.52` with `CCBG_RELEASE_MACOS_ASSET_DIR` and, if needed, `CCBG_RELEASE_LXC_ASSET_DIR`. This does not restore GitHub Actions as a general CI/release system. The resulting assets must still enter the same checksum, R2/GitHub fallback, and `/downloads/latest/*` smoke path.

## Embedded Boundary

STM32 and ESP32-S3 are not gateway hosts. They are client-only examples that call a nearby Linux/OpenWrt/Windows gateway through the local S3 data plane.

If future firmware builds are added, they inherit the same rule: build from `.52`, keep memory use bounded, and do not introduce provider credentials, SQLite, Admin Web, MCP stdio, or full replication logic into embedded firmware.

## Verification

Before publishing a release from `.52`, run:

```bash
bash -n scripts/build-lxc-package.sh scripts/release-local.sh
scripts/check-release-ready.sh
CCBG_RELEASE_BUILD_WINDOWS=true CCBG_RELEASE_BUILD_OPENWRT=true scripts/release-local.sh <tag>
```

If release assets must be uploaded to GitHub, `scripts/release-local.sh` resolves GitHub CLI
through `scripts/resolve-gh.sh`; `.52` can use `C:\Program Files\GitHub CLI\gh.exe` even when
Git Bash does not expose `gh` in `PATH`.

Merge downloaded build-runner artifacts on `.52`:

```bash
scripts/download-build-runner-release-assets.sh --run-id <github-run-id>
source target/build-runner-assets/release-inputs/release-local.env.sh
scripts/release-local.sh <tag>
```

```powershell
bash scripts/download-build-runner-release-assets.sh --run-id <github-run-id>
. .\target\build-runner-assets\release-inputs\release-local.env.ps1
bash scripts/release-local.sh <tag>
```

For embedded examples:

```bash
CC='zig cc' scripts/check-stm32-client-example.sh
$(bash scripts/resolve-python.sh) scripts/check-esp32-s3-client-example.py
CC='zig cc' scripts/check-esp32-s3-relay-lite-poc.sh
```

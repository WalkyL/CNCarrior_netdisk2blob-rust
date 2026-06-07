# `.49` S3 rclone mount listing fix

Date: `2026-06-07`

Source issue report:

- `/D:/Users/walky/stock-rag-bridge-rust/docs/CCBG49_S3_MOUNT_ISSUE_REPORT_2026-06-07.md`

## Summary

The `.49` S3-compatible endpoint could serve exact-key `HeadObject` and `GetObject`, but `rclone mount` was not usable because directory traversal depended on S3 `ListObjectsV2` with `delimiter=/`.

The live symptoms were:

- parent prefix listings such as `root/`, `root/stock-rag-bridge-rust/`, and `root/stock-rag-bridge-rust/graph/` were empty or too slow;
- `rclone cat` for a known key did not return within the test window;
- mounted `ls/stat/head` calls had long delays;
- historical mount writes had `InvalidPart` risk and needed explicit re-validation.

## Root Cause

Two S3 compatibility gaps affected rclone:

1. Delimited listing called upstream provider listing/head paths too eagerly. For provider-backed buckets this made parent directory discovery depend on slow or incomplete carrier-cloud directory listings.
2. `ListObjectsV2` XML returned provider-native `LastModified` strings such as `2026-06-05 01:18:08.354+08:00`. rclone expects S3-compatible RFC3339-style timestamps and retried the listing repeatedly when it could not parse object entries.

## Fix

Implemented in `crates/gatewayd/src/main.rs`:

- `ListObjectsV2` with a non-empty delimiter now uses local object placements as the directory index for provider-backed buckets.
- Direct children in delimited listings are projected from local placement/logical-object metadata instead of requiring upstream `head_object` calls.
- The `stub` provider keeps backend listing behavior so local S3 unit tests can still seed objects directly.
- `LastModified` values in S3 listing XML are normalized to UTC RFC3339 milliseconds, for example `2026-06-07T06:58:01.000Z`.

## Local Verification

Passed:

```bash
cargo test -p gatewayd list_objects_v2_with_delimiter -- --nocapture
cargo test -p gatewayd multipart_complete -- --nocapture
cargo check -p gatewayd
```

`cargo check` still reports existing non-blocking warnings unrelated to this fix.

## `.49` Deployment Verification

Built and deployed a dirty validation package:

```text
target/release-local/v0.1.7-s3mount-fix2/ccbg-lxc-package.tar.gz
```

After deployment:

- `/opt/ccbg/bin/gatewayd --version`: `gatewayd 0.1.7`
- `systemctl is-active ccbg.service`: `active`
- `GET http://127.0.0.1:61080/healthz`: `unicom-cloud-drive healthy`

## `.49` rclone Acceptance

Using a temporary rclone remote pointed at `http://127.0.0.1:61080`, these commands returned successfully:

```bash
rclone lsf ccbg49:root
rclone lsf ccbg49:root/stock-rag-bridge-rust
rclone lsf ccbg49:root/stock-rag-bridge-rust/graph
rclone lsf ccbg49:root/stock-rag-bridge-rust/graph/cache
rclone cat ccbg49:root/stock-rag-bridge-rust/graph/cache/graph_rebuild_plan.json
```

Observed listing latency was about `0.26s` to `0.35s` per prefix. `rclone cat` returned the JSON body in about `4.1s`.

## `.49` mount Acceptance

Mounted:

```bash
rclone mount ccbg49:root/stock-rag-bridge-rust/graph /mnt/ccbg-s3mount-accept \
  --config /tmp/ccbg-rclone-s3mount-test.conf \
  --vfs-cache-mode writes \
  --dir-cache-time 1m \
  --poll-interval 0 \
  --daemon
```

Passed:

- `ls -la /mnt/ccbg-s3mount-accept`
- `ls -la /mnt/ccbg-s3mount-accept/cache`
- `stat /mnt/ccbg-s3mount-accept/cache/graph_rebuild_plan.json`
- `head -c 160 /mnt/ccbg-s3mount-accept/cache/graph_rebuild_plan.json`

Mounted write validation passed for:

- `small.txt`
- `medium-8m.bin`
- `large-32m.bin`

The test directory was removed after verification. A follow-up check found no `s3mount-accept-*` test directory under `graph/cache`.

Gateway logs for the verification window did not show `InvalidPart`, `panic`, or gateway errors.

## Remaining Boundary

This validates mount traversal and mounted writes up to `32 MiB` on `.49`. It does not raise the current provider upload-size policy. For `.49` unicom the health endpoint still reports:

```text
max_single_upload_bytes=3650722202
multipart_upload=false
```

Large graph rebuild use should continue to respect that provider boundary until provider-native large upload behavior is separately validated.

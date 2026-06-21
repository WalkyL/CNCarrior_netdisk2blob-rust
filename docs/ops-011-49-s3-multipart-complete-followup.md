# `.49` S3 multipart complete follow-up

Date: `2026-06-21`

## Scope

This follow-up closes the `.49` S3 large-upload regression that still affected `rclone` after the earlier listing fix.

## Problems

Three separate compatibility gaps were confirmed on `.49`:

1. `CompleteMultipartUpload` could return `InvalidPart` when the client sent entity-encoded ETags such as `&#34;etag&#34;` or `&quot;etag&quot;`.
2. Even after part validation succeeded, `rclone` could still fail with `timeout awaiting response headers` because `.49` waited for the full backend finalize before sending any HTTP `200` response headers.
3. Even after the header-timeout fix, forced multipart uploads could still fail client-side with `multipart upload corrupted: Etag differ` because `.49` returned or exposed SHA256-style object ETags instead of S3 multipart ETags.

## Changes

The `gatewayd` multipart-complete path now does the following:

1. XML-unescapes complete-request ETags before comparing them with stored part metadata.
2. Returns HTTP `200` headers immediately for `CompleteMultipartUpload`, then keeps the response body stream alive with whitespace chunks while the actual finalize continues in a background task.
3. Computes the final multipart object ETag using the S3-compatible `md5(part_md5_1 || ... || part_md5_n)-n` rule, and persists that ETag into logical object metadata so later `HEAD` / `LIST` / object status surfaces stay aligned with the complete response.
4. Exposes multipart-complete runtime status through `runtime.multipart_complete` in the control-plane `/api/status` payload:
   - `active_count`
   - `recent_limit`
   - `active_jobs[]`
   - `recent_jobs[]`
   - per-job `stage`, `stage_history`, `last_error`, timestamps, requested part count, object identity, and application id

The current runtime stages are:

- `queued`
- `opening_parts`
- `writing_home_object`
- `cleanup`
- `completed`
- `failed`

## Validation

### Local tests

The following targeted tests passed on `2026-06-21`:

- `cargo fmt --all --check`
- `cargo test -p metadata-store logical_object_round_trip_and_delete`
- `cargo test -p metadata-store backup_export_and_replace_round_trip`
- `cargo test -p gatewayd s3_multipart_complete_etag_uses_md5_of_part_md5s`
- `cargo test -p gatewayd public_object_info_uses_logical_etag_when_backend_omits_one`
- `cargo test -p gatewayd multipart_complete_status_exposes_active_and_recent_jobs`
- `cargo test -p gatewayd multipart_complete_returns_response_before_slow_backend_finishes`
- `cargo test -p gatewayd multipart_complete_accepts_entity_encoded_etags`
- `cargo test -p gatewayd multipart_initiate_upload_complete_and_get_succeeds`
- `cargo test -p gatewayd multipart_complete_counts_as_spooled_write_path`

### Live `.49` deployment

- Deployed binary SHA256:
  - `0a303778d66c420523c4f857be003660ba7df48de59574d2473e87c605bfbcd0`
- Deployment date:
  - `2026-06-21`
- Service status after restart:
  - `systemctl is-active ccbg.service` => `active`
- Runtime status field present after deploy:
  - `/api/status` returned `runtime.multipart_complete.active_count=0`
  - `/api/status` returned `runtime.multipart_complete.recent_limit=16`

### Live multipart smoke after logical-etag fix

Using a temporary `rclone` config against `http://192.168.1.49:61080`, a forced multipart upload of a `12 MiB` test file completed successfully on `2026-06-22`:

- object key:
  - `root/ccbg-smoke/multipart-complete-obsv-20260622-001625.bin`
- `rclone copyto` exit code:
  - `0`
- runtime status evidence:
  - `recent_stage=completed`
  - `requested_part_count=3`
  - `stage_history=[queued, opening_parts, writing_home_object, cleanup, completed]`

This is the live proof that `.49` now satisfies all three multipart expectations at once:

1. accepts the client-complete request,
2. does not stall at the response-header phase,
3. exposes a stable S3-compatible multipart ETag that `rclone` accepts.

### Original large-file repro

The previous live repro with `rclone --timeout 30s` and the `5.475 GiB` archive now succeeds end-to-end after the header-streaming fix.

Key evidence from:

- `D:\tmp\ccbg-invalidpart-retest\rclone-complete-stream-20260621-223012.log`

Observed milestones:

- last multipart chunk uploaded at `2026-06-21 22:39:53`
- multipart upload finished at `2026-06-21 22:56:15`
- `Copied (new)` at `2026-06-21 22:56:16`

## Remaining limitation

The request no longer times out at the header stage, but the overall complete path is still slow.

Current behavior is still:

1. read all staged multipart files from local spool
2. re-stream the combined object into the selected home backend
3. only then delete multipart session metadata and staged parts

So the remaining problem is not S3 compatibility at the response-header layer anymore. The remaining problem is finalize duration while `.49` rewrites the full object into the home backend.

## Counterpart note

You can paste the following to the counterpart:

> `.49` 这边现在已经把 multipart 相关的三个兼容性问题都补上了：一是 `CompleteMultipartUpload` 会正确接受 XML entity-encoded ETag，不会再因为 `&#34;etag&#34;` / `&quot;etag&quot;` 误判成 `InvalidPart`；二是 complete 路径会先回 HTTP 200 header，再用 keepalive body 持续占住连接，避免 `rclone timeout awaiting response headers`；三是 complete 结果和后续对象元数据现在都会统一使用 S3 兼容的 multipart ETag，不会再出现 `multipart upload corrupted: Etag differ`。现网已经用强制 multipart 的 12 MiB `rclone copyto` 复测通过，exit code=0，同时 `/api/status` 的 `runtime.multipart_complete` 能看到 completed job 和阶段时间线。当前剩余瓶颈不是协议兼容性，而是 `.49` 在 complete 阶段仍需把所有 multipart 临时分片重新串流写入 home backend，所以总 complete 耗时依然偏长。`

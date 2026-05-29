# S3 Smoke Tests

`scripts/s3-smoke.py` 提供本地和 CI 复用的 S3 smoke harness，固定面向 `stub` primary provider，不依赖真实云盘账号。

## 运行方式

```bash
python3 scripts/s3-smoke.py
```

可选参数:

- `--skip-build`: 跳过 `cargo build -p gatewayd`，直接复用 `target/debug/gatewayd`。
- `--report-path <path>`: 默认 `target/s3-smoke/report.json`。
- `--require-clients aws-cli,boto3,rclone,aws-sdk-rust`: 强制要求指定客户端可用；缺失或 skip 会失败。
- `--keep-temp`: 保留临时目录与 `gatewayd` 日志用于排查。
- `--allow-protocol-skips`: 允许尚未完成的 range/multipart 协议检查记为 skipped；默认严格要求已完成的 S3-001~S3-006 行为全绿。

## 测试覆盖

内置 `internal-sigv4` 客户端（始终执行）覆盖:

- `ListBuckets` / `ListObjectsV2`
- `PutObject` / `GetObject` / `HeadObject` / `DeleteObject`
- `CopyObject`
- multipart `initiate -> upload part -> complete` 与 `abort`
- `Range GET`（有效区间）
- 稳定错误断言:
  - missing key => `NoSuchKey` / `404`
  - invalid range => `InvalidRange` / `416`
  - malformed range => `InvalidRequest` / `400`
- 显式 path-style 访问 (`/<bucket>/<key>`)
- 显式 virtual-hosted-style 访问 (`Host: root.localhost:<port>`, path 为 `/<key>`)

可选客户端矩阵:

- `aws-cli`
- `boto3` (AWS SDK Python)
- `rclone`
- `aws-sdk-rust`（通过 `CCBG_SMOKE_RUST_SDK_COMMAND` 接入外部 aws-sdk-s3 smoke binary）

默认模式下可选客户端缺失会记为 `skipped`，不会导致失败；`--require-clients` 可切换成严格模式。
内置 `internal-sigv4` 默认严格校验已完成的 range/multipart/virtual-hosted-style 行为；只有显式传入 `--allow-protocol-skips` 时，未完成协议项才会被记录为 `skipped`。

### 可选客户端 expected step names

- `aws-cli`:
  - `put_object`
  - `get_object`
  - `copy_object`
  - `range_get`
  - `error_invalid_range`
  - `multipart_create`
  - `multipart_upload_part`
  - `multipart_complete`
  - `multipart_abort`
  - `delete_object`
- `boto3`:
  - `put_object`
  - `get_object`
  - `copy_object`
  - `range_get`
  - `error_no_such_key`
  - `multipart_create`
  - `multipart_upload_part`
  - `multipart_complete`
  - `multipart_abort`
  - `delete_object`
  - `addressing_style_path`
- `rclone`:
  - `put_object`
  - `get_object`
  - `range_get`（优先 `rclone cat --offset --count`；命令/版本不支持时可 `skipped` 并记录 detail）
  - `copy_object`
  - `multipart_via_rclone`（通过 5MiB `--s3-upload-cutoff` / `--s3-chunk-size` 阈值触发 provider multipart 路径，不是手写 multipart API）
  - `list_objects`
  - `delete_object`

### `aws-sdk-rust` 外部命令契约

脚本读取 `CCBG_SMOKE_RUST_SDK_COMMAND`，注入:

- `CCBG_SMOKE_ENDPOINT`
- `CCBG_SMOKE_ACCESS_KEY_ID`
- `CCBG_SMOKE_SECRET_ACCESS_KEY`
- `CCBG_SMOKE_REGION`

外部命令必须输出 JSON 到 stdout，支持两种形态:

1. 顶层数组：`[{ "name": "...", "ok": true, "skipped": false, "detail": "..." }]`
2. 顶层对象：`{ "steps": [ ...同上... ] }`

每个 step 至少要有 `name`；脚本会合并这些 steps，并额外校验 required names:

- `put_object`
- `get_object`
- `delete_object`
- `multipart_create`
- `multipart_upload_part`
- `multipart_complete`
- `multipart_abort`
- `range_get`

如果外部命令仅返回 `0` 但没有合法 JSON steps，或缺少 required names，`aws-sdk-rust` 客户端状态为 `failed`（不可验收）。
未设置 `CCBG_SMOKE_RUST_SDK_COMMAND` 时该客户端为 `skipped`。

## CI 约束

脚本启动本地 `gatewayd` 时会设置:

- `CCBG_PRIMARY_PROVIDER=stub`
- 临时 `CCBG_METADATA_DB_PATH` / `CCBG_CONTROL_PLANE_FILE` / `CCBG_CREDENTIALS_DIR` / `CCBG_BODY_SPOOL_DIR`
- 本地高端口 `CCBG_BIND_ADDR` / `CCBG_ADMIN_BIND_ADDR` / `CCBG_METRICS_BIND_ADDR`（范围 `60000..65534`）

脚本退出时会清理后台 `gatewayd` 进程和临时目录（除非显式 `--keep-temp`）。

## 报告格式

`target/s3-smoke/report.json` 示例字段:

- `started_at` / `ended_at`
- `endpoint`
- `required_clients`
- `overall_status`
- `failures`
- `clients[]`:
  - `client`
  - `status` (`passed` / `failed` / `skipped`)
  - `steps[]`:
    - `name`
    - `ok`
    - `skipped`
    - `detail`

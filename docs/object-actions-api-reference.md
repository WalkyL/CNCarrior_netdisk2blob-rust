# 对象动作 API 参考

这份文档只描述 `gatewayd` 对象动作相关 API 契约。

如果你要看怎么在 Admin Web 里实际操作、怎么导出共享历史、联通有哪些边界，去看:

- [docs/object-actions-and-history.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-and-history.md:1)

## 1. 概览

当前对象动作控制面相关接口:

- `POST /api/object-actions`
- `POST /api/object-actions/history/clear`
- `POST /api/replication/jobs/{job_id}/retry`
- `POST /api/replication/targets/{target}/retry-failed`
- `GET /api/status`

当前支持的动作:

- `rename`
- `copy`
- `move`

## 2. POST /api/object-actions

用途:

- 对当前 primary provider 执行对象级 `rename/copy/move`

成功返回:

- `204 No Content`

失败场景包括:

- 请求字段不完整
- bucket/key 非法或为空
- provider 上游错误
- 对象不存在
- 当前 primary provider 不支持该动作
- 联通 `rename` 不满足当前边界

当前已接入对象动作实现的 provider 包括:

- `unicom`
- `onedrive`

运行策略说明:

- 这个 API 始终对“当前 primary provider”执行动作
- 本项目当前只允许运营商 provider 作为 primary
- 因此 OneDrive 虽然已经实现对象动作，但运行时仍定位为异步备份 / 可选 fallback 目标，而不是 primary provider

所有 `rename/copy/move` 请求都支持可选审计字段:

- `operator`
- `ticket`
- `notes`

这三个字段会在服务端做 trim 后写入共享历史，方便后续筛选和 CSV 导出。

### 2.1 rename

请求体:

```json
{
  "action": "rename",
  "bucket": "family",
  "key": "shared/note.txt",
  "new_key": "shared/renamed.txt"
}
```

语义:

- primary provider 执行 rename
- 网关复制元数据按 `rename = put(new) + delete(old)` 更新

### 2.2 copy

请求体:

```json
{
  "action": "copy",
  "source_bucket": "family",
  "source_key": "shared/renamed.txt",
  "destination_bucket": "root",
  "destination_key": "docs/copied.txt"
}
```

语义:

- primary provider 执行 copy
- 网关复制元数据按 `copy = put(dest)` 更新

### 2.3 move

请求体:

```json
{
  "action": "move",
  "source_bucket": "root",
  "source_key": "docs/copied.txt",
  "destination_bucket": "family",
  "destination_key": "shared/moved.txt"
}
```

语义:

- primary provider 执行 move
- 网关复制元数据按 `move = put(dest) + delete(src)` 更新

## 3. POST /api/object-actions/history/clear

用途:

- 清空服务端 control-plane 文件里的对象动作共享历史

成功返回:

- `204 No Content`

注意:

- 这是共享状态，不是单浏览器状态
- 清空会影响所有访问同一网关控制面的操作者

## 4. POST /api/replication/jobs/{job_id}/retry

用途:

- 对复制队列中的最新 failed job 执行人工重试

成功返回:

```json
{
  "job_id": 42,
  "status": "pending",
  "target": "onedrive",
  "bucket": "root",
  "key": "docs/report.txt"
}
```

行为说明:

- 只允许重试 `failed` 状态的 job
- 只允许重试该 `target + bucket + key` 上当前最新的一条 failed job
- 重试后该 job 会被重新置成 `pending` 并重新入内存队列
- 会清空 `last_error` 和 `next_attempt_at_unix_ms`

`replication_state` 与 `monitoring.replication` 中的失败/完成/重试计数，当前都按“每个对象在每个 target 上的最新状态”统计。

失败场景包括:

- `job_id` 不存在
- 该 job 当前不是 `failed`
- 该 job 已经不是这个对象在该 target 上的最新状态

这条接口的目标是“值守时人工恢复”，不是批量补偿系统。

## 5. POST /api/replication/targets/{target}/retry-failed

用途:

- 对某个复制 target 上“当前最新状态仍为 failed”的对象批量执行人工重试

成功返回:

```json
{
  "target": "onedrive",
  "retried_jobs": 2,
  "jobs": [
    {
      "job_id": 42,
      "status": "pending",
      "bucket": "root",
      "key": "docs/report.txt"
    },
    {
      "job_id": 43,
      "status": "pending",
      "bucket": "root",
      "key": "docs/archive.txt"
    }
  ]
}
```

行为说明:

- `target` 需要是已知 provider 名称，例如 `onedrive` 或 `telecom`
- 只会扫描这个 target 上每个 `bucket + key` 的最新 job
- 只有“最新 job 仍是 `failed`”的对象会被重试
- 已被后续 `completed` / `pending` / `retry_scheduled` 覆盖的旧失败不会被重新入队
- 重试后这些 job 会被重新置成 `pending` 并重新入内存队列
- 会清空 `last_error` 和 `next_attempt_at_unix_ms`

失败场景包括:

- `target` 不是合法 provider
- 元数据存储更新失败

如果当前没有可重试的 latest failed jobs，接口仍返回 `200`，但 `retried_jobs` 会是 `0`。

## 6. GET /api/status

用途:

- 获取 Admin Web 运行状态
- 提供对象动作共享历史、历史上限和其他控制面状态

与对象动作直接相关的字段包括:

- `monitoring`
- `operations_overview`
- `notify`
- `runtime_topology`
- `object_action_history`
- `object_action_history_limit`
- `provider_health`
- `replication_state`

如果配置了 `CCBG_CONTROL_API_KEY`，这个接口以及其他 Admin Web / 控制面接口都需要以下任一认证方式:

- `x-api-key: <key>`
- `Authorization: Bearer <key>`
- 浏览器 Admin Web 现在应走本地用户名密码登录；脚本和机器间调用继续使用上面的 header 凭据

### 5.1 应用 API Key 管理

数据面 S3 key 可以按应用管理，控制面接口:

```http
GET /api/applications
POST /api/applications
```

`POST` 使用全量替换语义:

```json
{
  "applications": [
    {
      "id": "video-ingest",
      "label": "Video ingest",
      "access_key_id": "video-access-key",
      "secret_access_key": "video-secret-key",
      "enabled": true,
      "content_policy_id": "large-video"
    }
  ]
}
```

响应不会返回明文 secret，只返回 `secret_access_key_present`。如果控制面还没有 `applications` 配置，网关会自动回退到旧的 `CCBG_S3_ACCESS_KEY_ID` / `CCBG_S3_SECRET_ACCESS_KEY`，保持现有客户端兼容。

### 5.2 内容策略

内容策略是对象级写入策略，控制面接口:

```http
GET /api/content-policies
POST /api/content-policies
```

`POST` 使用全量替换语义:

```json
{
  "policies": [
    {
      "id": "large-video",
      "label": "Large video",
      "enabled": true,
      "application_ids": ["video-ingest"],
      "buckets": ["root"],
      "prefixes": ["videos"],
      "content_types": ["video/*"],
      "sync_targets": ["telecom", "mobile"],
      "fallback_read_order": ["mobile"]
    }
  ]
}
```

匹配维度包括应用、bucket、prefix、content-type。当前策略已用于覆盖新写入对象的复制目标；`fallback_read_order` 会随策略保存并参与策略拓扑校验，读路径按应用选择 fallback 的执行会在双写/强一致 fallback 切片中接入。如果没有匹配策略，则继续使用全局 topology。多目标策略下，后续双写/强一致 fallback 应以这些目标的最小文件体积和最小分片限制作为有效上限。

重要语义:

- 内容策略变化默认只影响后续新写入和覆盖写。
- 历史对象不会因为策略变化而自动补副本、删旧副本、迁移 home provider、加密重写或解密重写。
- 历史对象的这些变化必须通过 Admin 里的显式工具先预览、再执行，避免操作者在不知情的情况下丢数据或制造大规模搬迁。
- 为了让历史对象能够安全预览和后续显式迁移，网关会从新写入开始把 `application_id` 持久化进逻辑对象元数据；更早的旧对象如果缺少这个上下文，Admin 只会提示“暂不能安全预览”，不会猜测策略结果。
- 这类显式收敛在设计上必须额外检查目标 provider 容量、同 provider 两阶段重写峰值，以及本地 spool 预算；只要其中任一项不足或未知，就必须阻断执行。
- 这组规则的完整说明见 [docs/historical-object-reconcile-and-buffer-budget.md](./historical-object-reconcile-and-buffer-budget.md)。

`replication_state` 当前除了 `persisted.recent_jobs` 和 `target_statuses`，还会额外返回:

- `latest_failed_jobs`: 只包含“每个 target + bucket + key 当前最新状态仍为 failed”的对象级失败视图

这个字段的用途是给 Admin Web 的 `Latest Failed Objects` 表格、对象/时间过滤以及 JSON / CSV 导出使用，避免运维在 `recent_jobs` 历史里手工排除已经被后续成功覆盖的旧失败。

### 6.1 runtime

类型:

```json
{
  "started_at_unix_ms": 1710000000000,
  "uptime_ms": 60000,
  "bind_addr": "127.0.0.1:61080",
  "admin_mode": "web",
  "admin_bind_addr": "127.0.0.1:61081",
  "auth_callback_bind_addr": "127.0.0.1:61082",
  "metrics_bind_addr": "127.0.0.1:61083",
  "control_plane_file": "./data/control-plane.json",
  "metadata_db_path": "./data/metadata.db",
  "credentials_dir": "./data/provider-credentials",
  "browser_flow_catalog_dir": "./config/browser-flows",
  "provider_capability_catalog_dir": "./config/provider-capabilities",
  "replication_workers": 2,
  "data_plane_max_in_flight": 8,
  "data_plane_max_requests_per_second": 0,
  "object_action_history_limit": 12
}
```

字段说明:

- `started_at_unix_ms`: 服务启动时间
- `uptime_ms`: 已运行时长
- `bind_addr`: 数据面监听地址
- `admin_mode`: Admin Web 运行模式
- `admin_bind_addr`: Admin Web 监听地址
- `auth_callback_bind_addr`: OAuth / auth callback 监听地址
- `metrics_bind_addr`: Metrics / extended health 监听地址
- `control_plane_file`: control-plane 状态文件
- `metadata_db_path`: 元数据数据库路径
- `credentials_dir`: 凭证存储目录
- `browser_flow_catalog_dir`: browser flow catalog 目录
- `provider_capability_catalog_dir`: provider capability catalog 目录
- `replication_workers`: 复制 worker 数量
- `data_plane_max_in_flight`: 数据面最大并发处理数；超限时 S3 路径直接返回 `503 ServiceUnavailable`
- `data_plane_max_requests_per_second`: 数据面每秒请求上限；`0` 表示关闭，启用后超限同样直接返回 `503 ServiceUnavailable`
- `object_action_history_limit`: 当前共享历史窗口大小

### 6.2 object_action_history

类型:

```json
[
  {
    "executed_at_unix_ms": 1710000000000,
    "primary_provider": "unicom",
    "action": "rename",
    "operator": "alice",
    "ticket": "CHG-2026-0514",
    "notes": "rename for maintenance",
    "description": "family/shared/note.txt -> family/shared/renamed.txt",
    "outcome": "success",
    "message": "Completed family/shared/note.txt -> family/shared/renamed.txt",
    "warnings": [
      "Current Unicom rename only supports staying in the same parent directory. Use move for cross-directory changes."
    ],
    "references": [
      {
        "label": "source before / old key after",
        "bucket": "family",
        "key": "shared/note.txt",
        "changes": [
          "exists: yes -> no"
        ]
      },
      {
        "label": "renamed target",
        "bucket": "family",
        "key": "shared/renamed.txt",
        "changes": [
          "exists: no -> yes"
        ]
      }
    ]
  }
]
```

字段说明:

- `executed_at_unix_ms`: 执行时间
- `primary_provider`: 当时的 primary provider
- `action`: `rename | copy | move`
- `operator`: 操作者标识，可为空
- `ticket`: 工单号 / 变更号，可为空
- `notes`: 备注，可为空
- `description`: 人类可读动作摘要
- `outcome`: `success | failed`
- `message`: 成功/失败消息
- `warnings`: 预警信息
- `references`: 被影响对象及变化摘要

### 6.3 monitoring

类型:

```json
{
  "open_alert_count": 1,
  "provider_summary": {
    "total": 2,
    "healthy": 1,
    "degraded": 1,
    "unavailable": 0
  },
  "replication": {
    "pending_jobs": 0,
    "retry_scheduled_jobs": 0,
    "failed_jobs": 1,
    "completed_jobs": 12
  },
  "object_actions": {
    "total_entries": 4,
    "successful_entries": 3,
    "failed_entries": 1,
    "unique_operators": 2,
    "last_action_at_unix_ms": 1710000000000
  },
  "recent_failures": [
    {
      "kind": "replication_job",
      "provider": "unicom",
      "action": "put",
      "target": "onedrive",
      "object": "root/alerts/failure.txt",
      "occurred_at_unix_ms": 1710000000000,
      "message": "upstream failed"
    }
  ]
}
```

用途:

- 给 Admin Web 的 `Monitoring Summary` 卡片提供聚合摘要
- 让运维先看到告警、provider 健康、复制失败和最近失败事件，再决定是否下钻 `provider_health`、`replication_state` 或 `object_action_history`

字段说明:

- `open_alert_count`: 当前告警数量
- `provider_summary`: provider 健康计数汇总
- `replication`: 复制任务状态汇总
- `object_actions`: 对象动作历史汇总
- `latest_failed_objects`: 当前 latest-only 失败对象摘要，适合 webhook / 外部监控直接消费
- `recent_failures`: 最近失败事件列表，来源包括失败的对象动作和失败的复制任务

### 6.4 operations_overview

类型:

```json
{
  "primary_provider": "unicom",
  "sync_targets": ["onedrive"],
  "fallback_read_order": ["onedrive"],
  "replication_mode": "async_backup",
  "onedrive_async_backup_enabled": true,
  "replication_workers": 2,
  "data_plane_max_in_flight": 8,
  "data_plane_permits_available": 7,
  "data_plane_max_requests_per_second": 8,
  "data_plane_requests_current_second": 3,
  "pending_jobs": 3,
  "retry_scheduled_jobs": 1,
  "latest_failed_objects": 2,
  "oldest_pending_job_age_ms": 45000,
  "oldest_retry_scheduled_job_age_ms": 180000,
  "oldest_latest_failed_object_age_ms": 240000,
  "latest_object_action_age_ms": 120000,
  "notify_webhook_enabled": true,
  "notify_last_success_age_ms": 15000,
  "notify_last_error": null,
  "replication_failed_alert_threshold": 2,
  "replication_failed_alert_min_age_ms": 60000,
  "data_plane_loopback_only": false,
  "admin_loopback_only": true,
  "auth_callback_loopback_only": true,
  "metrics_loopback_only": true,
  "s3_secret_uses_default": false
}
```

用途:

- 给 Admin Web 的 `Operations Overview` 总览卡提供轻量运维视图
- 让软路由场景下的值守方先看到主写 / 异步备份拓扑、复制积压年龄、失败对象年龄和 webhook 新鲜度
- 这个字段只是复用现有状态做聚合，不代表服务端额外引入了 Prometheus server 或其他常驻监控组件

字段说明:

- `primary_provider`: 当前主写 provider
- `sync_targets`: 当前异步复制目标列表
- `fallback_read_order`: 当前 fallback 读顺序
- `replication_mode`: 当前复制模式，现阶段固定为 `async_backup`
- `onedrive_async_backup_enabled`: OneDrive 是否作为异步备份启用
- `replication_workers`: 当前复制 worker 数量
- `data_plane_max_in_flight`: 当前数据面并发上限
- `data_plane_permits_available`: 当前剩余可用 permit 数；如果经常贴近 `0`，说明上限可能过低或宿主负载过高
- `data_plane_max_requests_per_second`: 当前数据面每秒请求阀门；`0` 表示关闭
- `data_plane_requests_current_second`: 当前 1 秒窗口内已计入阀门的请求数
- `pending_jobs`: 当前持久化 pending job 数量
- `retry_scheduled_jobs`: 当前持久化 retry_scheduled job 数量
- `latest_failed_objects`: 当前 latest-only failed object 数量
- `oldest_pending_job_age_ms`: 当前最老 pending job 的年龄
- `oldest_retry_scheduled_job_age_ms`: 当前最老 retry_scheduled job 的年龄
- `oldest_latest_failed_object_age_ms`: 当前最老 latest failed object 的年龄
- `latest_object_action_age_ms`: 距离最近一次对象动作过去多久
- `notify_webhook_enabled`: 当前是否启用了 notify webhook
- `notify_last_success_age_ms`: 距离最近一次 webhook 成功投递过去多久
- `notify_last_error`: 最近一次 webhook 错误
- `replication_failed_alert_threshold`: latest failed object 正式触发告警的数量阈值
- `replication_failed_alert_min_age_ms`: latest failed object 参与正式告警的最小失败年龄
- `data_plane_loopback_only`: 数据面是否仍绑定在回环地址
- `admin_loopback_only`: Admin Web 是否仍绑定在回环地址
- `auth_callback_loopback_only`: OAuth callback 是否仍绑定在回环地址
- `metrics_loopback_only`: Metrics / Health 是否仍绑定在回环地址
- `s3_secret_uses_default`: 当前 S3 secret 是否还在使用示例默认值 `change-me`

### 6.5 notify

类型:

```json
{
  "webhook_enabled": true,
  "webhook_url_present": true,
  "signature_enabled": true,
  "poll_interval_seconds": 15,
  "last_alert_hash": "6f9d4d2d...",
  "last_attempt_at_unix_ms": 1710000000000,
  "last_success_at_unix_ms": 1710000000500,
  "last_error": null
}
```

用途:

- 给 Admin Web 的 `Notify` 卡片提供 webhook 投递状态
- 让运维确认告警是否已经对外发送，而不是只停留在本地控制面

字段说明:

- `webhook_enabled`: 当前是否启用了 webhook 外发
- `webhook_url_present`: 当前是否配置了 webhook URL
- `signature_enabled`: 当前是否启用了 webhook HMAC 签名
- `poll_interval_seconds`: 告警轮询间隔
- `last_alert_hash`: 最近一次成功发送的 alerts 指纹
- `last_attempt_at_unix_ms`: 最近一次尝试发送时间
- `last_success_at_unix_ms`: 最近一次发送成功时间
- `last_error`: 最近一次发送错误

当前 webhook 总会包含:

- `x-ccbg-notify-event-id`
- `x-ccbg-notify-timestamp`

当前若启用了签名，请求头还会额外包含:

- `x-ccbg-notify-signature-version`
- `x-ccbg-notify-signature`

签名算法:

- `hex(HMAC_SHA256(secret, "<timestamp>.<sha256(body)>"))`

接收端建议:

- 校验 `x-ccbg-notify-signature-version` 当前应为 `v1`
- 校验 `x-ccbg-notify-timestamp` 是否落在可接受时间窗内
- 按 `x-ccbg-notify-event-id` 做幂等去重
- 可直接参考 [docs/notify-webhook-reference.md](/home/walky/carrier-cloud-blob-gateway/docs/notify-webhook-reference.md:1) 与 [scripts/notify-webhook-receiver-example.py](/home/walky/carrier-cloud-blob-gateway/scripts/notify-webhook-receiver-example.py:1)

复制失败告警阈值说明:

- `monitoring.latest_failed_objects` 会完整返回当前 latest-only 失败对象摘要
- `alerts` 里的复制失败告警则会受 `CCBG_REPLICATION_FAILED_ALERT_THRESHOLD` 和 `CCBG_REPLICATION_FAILED_ALERT_MIN_AGE_MS` 控制
- 也就是“摘要可见”和“正式告警触发”现在是分开的，方便值守时既看全量现状，又减少瞬时噪声

### 6.6 object_action_history_limit

类型:

- `number`

用途:

- 表示当前服务端保留多少条共享历史
- Admin Web 用它来展示当前历史窗口大小

Admin Web 会基于 `object_action_history` 做本地筛选和 JSON/CSV 导出，所以这里不单独提供导出接口。

对应环境变量:

```dotenv
CCBG_OBJECT_ACTION_HISTORY_LIMIT=12
```

## 7. 配置项

### 7.1 CCBG_OBJECT_ACTION_HISTORY_LIMIT

用途:

- 控制服务端共享历史保留条数

示例:

```dotenv
CCBG_OBJECT_ACTION_HISTORY_LIMIT=12
```

### 7.2 CCBG_CONTROL_PLANE_FILE

用途:

- 指定 control-plane 状态文件路径
- 对象动作共享历史会持久化在这里

### 7.3 CCBG_REPLICATION_FAILED_ALERT_THRESHOLD

用途:

- 控制 latest failed objects 达到多少个时才触发复制失败告警

默认值:

- `1`

### 7.4 CCBG_REPLICATION_FAILED_ALERT_MIN_AGE_MS

用途:

- 控制失败对象至少持续多久后才参与复制失败告警统计

默认值:

- `0`

## 8. 行为约束

### 8.1 历史来源

- 历史以服务端状态为准
- 不以浏览器 `localStorage` 为准

### 8.2 截断规则

- 新记录插入头部
- 超过 `object_action_history_limit` 后，裁剪最旧记录

### 8.3 一致性边界

- primary provider 动作成功，不代表 backup 已同步完成
- before/after 检查展示的是当前观察状态
- fallback 仍受最新复制状态约束

### 8.4 复制人工重试边界

- `Retry` 不是强制执行通道，只是把 job 重新交给后台 worker
- 如果根因没修复，job 仍然会再次失败
- 当前支持单 job 重试，以及按 target 对 latest failed jobs 的批量重试

### 8.5 Provider 文件大小探测

控制面提供受控的大文件读写探测接口:

```http
POST /api/providers/{provider}/limit-probe
```

请求示例:

```json
{
  "bucket": "root",
  "key_prefix": "ccbg-limit-probes",
  "sizes": ["512MiB", "1GiB", "2GiB"],
  "read_back": true,
  "delete_after": true,
  "chunk_size": "1MiB"
}
```

实现约束:

- 上传体是零字节流，不会把完整探测对象放进内存。
- `read_back=true` 会读回并丢弃响应流，只校验字节数。
- 如果 provider 不支持 `delete_object`，响应会标记 `cleanup_required=true`，需要人工清理探测对象。
- 探测成功后的最大值应写入对应 `CCBG_*_MAX_SINGLE_UPLOAD_BYTES` / `CCBG_*_MAX_SINGLE_DOWNLOAD_BYTES`，作为内容策略计算最小共同限制的 provider 事实输入。

## 9. Provider 特殊说明

### 9.1 联通

当前联通 `rename` 仍有边界:

- 只支持同父目录 rename

如果需要跨目录或跨容器调整，优先用:

- `move`

### 9.2 OneDrive

当前 OneDrive 对象动作语义:

- `rename` 和 `move` 通过 Graph `PATCH` 更新 `name` / `parentReference`
- `copy` 通过 Graph async copy 请求和 monitor URL 轮询完成

这意味着:

- `rename` 可同时覆盖“改名”和“跨目录移动”
- `copy` 成功前可能会多一次短暂轮询

## 10. 相关文档

- [docs/object-actions-and-history.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-and-history.md:1)
- [docs/auth-step-by-step.md](/home/walky/carrier-cloud-blob-gateway/docs/auth-step-by-step.md:1)
- [docs/provider-completion-standard.md](/home/walky/carrier-cloud-blob-gateway/docs/provider-completion-standard.md:1)

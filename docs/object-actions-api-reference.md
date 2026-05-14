# 对象动作 API 参考

这份文档只描述 `gatewayd` 对象动作相关 API 契约。

如果你要看怎么在 Admin Web 里实际操作、怎么导出共享历史、联通有哪些边界，去看:

- [docs/object-actions-and-history.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-and-history.md:1)

## 1. 概览

当前对象动作控制面相关接口:

- `POST /api/object-actions`
- `POST /api/object-actions/history/clear`
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
- 本项目当前只允许运营商 provider 或 `stub` 作为 primary
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

## 4. GET /api/status

用途:

- 获取 Admin Web 运行状态
- 提供对象动作共享历史、历史上限和其他控制面状态

与对象动作直接相关的字段包括:

- `monitoring`
- `notify`
- `runtime_topology`
- `object_action_history`
- `object_action_history_limit`
- `provider_health`
- `replication_state`

### 4.1 runtime

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
- `object_action_history_limit`: 当前共享历史窗口大小

### 4.2 object_action_history

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

### 4.3 monitoring

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
      "provider": "stub",
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
- `recent_failures`: 最近失败事件列表，来源包括失败的对象动作和失败的复制任务

### 4.4 notify

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

当前若启用了签名，请求头会额外包含:

- `x-ccbg-notify-event-id`
- `x-ccbg-notify-signature-version`
- `x-ccbg-notify-timestamp`
- `x-ccbg-notify-signature`

签名算法:

- `hex(HMAC_SHA256(secret, "<timestamp>.<sha256(body)>"))`

接收端建议:

- 校验 `x-ccbg-notify-signature-version` 当前应为 `v1`
- 校验 `x-ccbg-notify-timestamp` 是否落在可接受时间窗内
- 按 `x-ccbg-notify-event-id` 做幂等去重

### 4.5 object_action_history_limit

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

## 5. 配置项

### 5.1 CCBG_OBJECT_ACTION_HISTORY_LIMIT

用途:

- 控制服务端共享历史保留条数

示例:

```dotenv
CCBG_OBJECT_ACTION_HISTORY_LIMIT=12
```

### 5.2 CCBG_CONTROL_PLANE_FILE

用途:

- 指定 control-plane 状态文件路径
- 对象动作共享历史会持久化在这里

## 6. 行为约束

### 6.1 历史来源

- 历史以服务端状态为准
- 不以浏览器 `localStorage` 为准

### 6.2 截断规则

- 新记录插入头部
- 超过 `object_action_history_limit` 后，裁剪最旧记录

### 6.3 一致性边界

- primary provider 动作成功，不代表 backup 已同步完成
- before/after 检查展示的是当前观察状态
- fallback 仍受最新复制状态约束

## 7. Provider 特殊说明

### 7.1 联通

当前联通 `rename` 仍有边界:

- 只支持同父目录 rename

如果需要跨目录或跨容器调整，优先用:

- `move`

### 7.2 OneDrive

当前 OneDrive 对象动作语义:

- `rename` 和 `move` 通过 Graph `PATCH` 更新 `name` / `parentReference`
- `copy` 通过 Graph async copy 请求和 monitor URL 轮询完成

这意味着:

- `rename` 可同时覆盖“改名”和“跨目录移动”
- `copy` 成功前可能会多一次短暂轮询

## 8. 相关文档

- [docs/object-actions-and-history.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-and-history.md:1)
- [docs/auth-step-by-step.md](/home/walky/carrier-cloud-blob-gateway/docs/auth-step-by-step.md:1)
- [docs/provider-completion-standard.md](/home/walky/carrier-cloud-blob-gateway/docs/provider-completion-standard.md:1)

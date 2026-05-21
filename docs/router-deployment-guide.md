# 软路由部署指南

这份文档针对 `x86/arm64` 软路由、OpenWRT 衍生系统和其他“小宿主 + 长期驻留”的部署场景。

目标不是把所有控制面都公开出去，而是用尽量小的暴露面把 `gatewayd` 跑稳。

## 1. 默认原则

推荐基线:

- 只在需要时对外开放 `S3 API`
- `Admin Web`、`OAuth Callback`、`Metrics / Health` 默认都保持 `127.0.0.1` 监听
- 不在路由器上本机跑重监控栈；值守优先用 `Admin Web + notify webhook`
- 如果 `S3 API` 要给局域网客户端访问，必须先旋转 `CCBG_S3_SECRET_ACCESS_KEY`

## 2. 推荐监听配置

最保守的软路由配置:

```dotenv
CCBG_BIND_ADDR=127.0.0.1:61080
CCBG_ADMIN_BIND_ADDR=127.0.0.1:61081
CCBG_AUTH_CALLBACK_BIND_ADDR=127.0.0.1:61082
CCBG_METRICS_BIND_ADDR=127.0.0.1:61083
```

如果需要让局域网设备访问 S3 数据面，再只放开数据面:

```dotenv
CCBG_BIND_ADDR=0.0.0.0:61080
CCBG_ADMIN_BIND_ADDR=127.0.0.1:61081
CCBG_AUTH_CALLBACK_BIND_ADDR=127.0.0.1:61082
CCBG_METRICS_BIND_ADDR=127.0.0.1:61083
```

不建议的默认做法:

- `CCBG_ADMIN_BIND_ADDR=0.0.0.0:61081`
- `CCBG_AUTH_CALLBACK_BIND_ADDR=0.0.0.0:61082`
- `CCBG_METRICS_BIND_ADDR=0.0.0.0:61083`

这三类端口更适合通过以下方式访问:

- SSH 本地端口转发
- 仅管理网可达的反向代理
- 单独的受控运维 VLAN

## 3. 必做安全项

至少完成以下项目后，再考虑把数据面对外开放:

1. 替换 `CCBG_S3_SECRET_ACCESS_KEY=change-me`
2. 把 `Admin Web`、`OAuth Callback`、`Metrics` 维持在回环地址；如必须暴露控制面，至少配置 `CCBG_CONTROL_API_KEY`
3. 控制对象内存上限，例如 `CCBG_MAX_IN_MEMORY_OBJECT_BYTES=4194304`
4. 限制数据面并发，例如 `CCBG_DATA_PLANE_MAX_IN_FLIGHT=2`
5. 如有突发请求，再加每秒请求阀门，例如 `CCBG_DATA_PLANE_MAX_REQUESTS_PER_SECOND=8`
6. 限制复制 worker，例如 `CCBG_REPLICATION_WORKERS=1`
7. 把日志降到 `warn`

推荐起步值:

```dotenv
CCBG_MAX_IN_MEMORY_OBJECT_BYTES=4194304
CCBG_DATA_PLANE_MAX_IN_FLIGHT=2
CCBG_DATA_PLANE_MAX_REQUESTS_PER_SECOND=8
CCBG_CONTROL_API_KEY=replace-with-a-long-random-value
CCBG_REPLICATION_WORKERS=1
CCBG_REPLICATION_RECENT_LIMIT=16
CCBG_METADATA_SNAPSHOT_RECENT_LIMIT=16
CCBG_METADATA_COMPLETED_HISTORY_LIMIT=64
CCBG_METADATA_FAILED_HISTORY_LIMIT=64
RUST_LOG=warn
```

`CCBG_DATA_PLANE_MAX_IN_FLIGHT` 的语义是“同时允许多少个 S3 数据面请求进入实际处理”。超过上限时网关直接返回 `503 ServiceUnavailable`，不会在内存里继续排队。对软路由来说，这比无限并发或应用内排队更稳。

`CCBG_DATA_PLANE_MAX_REQUESTS_PER_SECOND` 是一个更保守的外层阀门。它默认关闭；启用后按固定 1 秒窗口计数，超限同样直接返回 `503 ServiceUnavailable`。如果你的局域网客户端会在很短时间里打出突发请求，这个值通常比继续压低并发上限更好调。

`CCBG_CONTROL_API_KEY` 只保护控制面，不影响 S3 数据面。脚本可以用 `x-api-key` 或 `Authorization: Bearer`；浏览器首次访问 Admin Web 时，也可以在 URL 后面带 `?api_key=...` 让服务端落一个 `HttpOnly` cookie。

## 4. OneDrive 建议

软路由场景下，OneDrive 仍建议保持“异步备份 / 可选 fallback”定位，而不是主写。

建议:

- `primary provider` 用联通/电信/移动其一
- `onedrive` 只放在 `CCBG_SYNC_TARGETS`
- `fallback` 只在明确需要时启用
- 如果宿主很小，先关闭 OneDrive，再按资源逐步打开

## 5. 运维方式

轻量值守建议顺序:

1. 先看 `Admin Web` 的 `Operations Overview`
2. 再看 `Monitoring Summary`、`Latest Failed Objects` 和 `Notify`
3. 需要对外集成时，再配置 `CCBG_NOTIFY_WEBHOOK_URL`

`Operations Overview` 现在还会显示 `Data plane concurrency: max=<n> | available=<n>` 和 `Data plane rate cap: max=<n> req/s | current=<n>`，可以直接看到当前并发保护和每秒请求阀门。

本机 `/metrics` 只是兼容出口，不建议把“在路由器上再跑 Prometheus server”当默认方案。

## 6. 软路由场景下的浏览器 / CDP 放置方式

软路由通常不应该承担浏览器进程。

推荐模型:

- 软路由只跑 `carrier-cloud-blob-gateway`
- 管理员电脑或另一台 LAN 浏览器主机负责 Chrome / Edge + CDP
- Admin 页面中的 CDP 地址填写浏览器主机的局域网地址，例如 `http://192.168.1.36:9222`
- 不要在这个场景里填写 `http://127.0.0.1:9222`

推荐顺序:

1. 先在浏览器主机上运行 [scripts/setup-cdp-browser-host.sh](/home/walky/workspaces/carrier-cloud-blob-gateway/scripts/setup-cdp-browser-host.sh:1) 或 [scripts/setup-cdp-browser-host.ps1](/home/walky/workspaces/carrier-cloud-blob-gateway/scripts/setup-cdp-browser-host.ps1:1)
2. 先验证 `http://127.0.0.1:9222/json/version` 和 `http://<浏览器主机局域网IP>:9222/json/version` 都可达
3. 再去软路由上的 Admin 页面填写 `Browser / CDP`
4. 用 `Probe Preferred` 或 `Probe All` 验证

更完整的逐步操作见 [docs/auth-step-by-step.md](/home/walky/workspaces/carrier-cloud-blob-gateway/docs/auth-step-by-step.md:146) 里的“软路由 + 管理员电脑的 CDP 认证模型”章节。

## 7. 当前运行时提醒

当前控制面已经会在以下情况主动报警:

- `Admin Web` 监听到非回环地址
- `OAuth Callback` 监听到非回环地址
- `Metrics` 监听到非回环地址
- `CCBG_S3_SECRET_ACCESS_KEY` 仍然是默认示例值 `change-me`
- `Admin Web` 或 `Metrics` 暴露到非回环地址，但未配置 `CCBG_CONTROL_API_KEY`

这些提醒的目的，是避免软路由宿主在默认配置下把管理面直接暴露出去。

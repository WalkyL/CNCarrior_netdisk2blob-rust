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

`CCBG_CONTROL_API_KEY` 只保护控制面脚本调用、机器间调用和指标接口，不影响 S3 数据面。脚本可以用 `x-api-key` 或 `Authorization: Bearer`；浏览器 Admin Web 应改用本地用户名密码登录，并由服务端发 `HttpOnly` session cookie。

## 4. OneDrive Parking

软路由默认流程不启用 OneDrive，也不把 OneDrive 放进 `CCBG_SYNC_TARGETS` 或 `CCBG_FALLBACK_READ_ORDER`。当前阶段的近线主线是运营商 provider、本地 S3、MCP、Skill 和本地控制面；OneDrive 只保留为未来真实需求触发后的恢复项。

建议:

- `primary provider` 用联通/电信/移动其一；未注入凭据前会显示 unavailable
- `CCBG_SYNC_TARGETS` 默认留空；如需异步备份，优先选择已完成探测和回归的运营商 provider
- `CCBG_FALLBACK_READ_ORDER` 默认留空；只有明确需要读侧 fallback 时才填写
- 保持 `CCBG_ONEDRIVE_ENABLED=false` 与 `CCBG_ONEDRIVE_REPLICATION_ENABLED=false`
- 如果后续确有真实 OneDrive 需求，先按恢复清单完成产品、OAuth、Graph 行为、复制、告警和回滚验证，再进入默认示例

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

这两个脚本都是幂等的，可在浏览器主机重启后直接重复执行。它们只在本机 CDP 不可达时才启动浏览器实例，并会重建防火墙和端口代理状态。

更完整的逐步操作见 [docs/auth-step-by-step.md](/home/walky/workspaces/carrier-cloud-blob-gateway/docs/auth-step-by-step.md:146) 里的“软路由 + 管理员电脑的 CDP 认证模型”章节。

## 7. 当前运行时提醒

当前控制面已经会在以下情况主动报警:

- `Admin Web` 监听到非回环地址
- `OAuth Callback` 监听到非回环地址
- `Metrics` 监听到非回环地址
- `CCBG_S3_SECRET_ACCESS_KEY` 仍然是默认示例值 `change-me`
- `Admin Web` 或 `Metrics` 暴露到非回环地址，但未配置 `CCBG_CONTROL_API_KEY`

这些提醒的目的，是避免软路由宿主在默认配置下把管理面直接暴露出去。

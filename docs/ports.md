# 端口策略

## 约束

- 所有本项目使用的服务端口统一位于 `60000-65534`
- 默认不监听 `1024-59999`
- 默认仅绑定本机回环地址或管理网口

## 默认端口分配

| 用途 | 默认地址 | 说明 |
| --- | --- | --- |
| S3 API | `127.0.0.1:61080` | Agent 读写对象的主入口 |
| Admin Web UI | `127.0.0.1:61081` | 浏览器访问的本地管理界面 |
| OAuth Callback | `127.0.0.1:61082` | OneDrive 授权回调 |
| Metrics / Extended Health | `127.0.0.1:61083` | 指标和增强健康检查 |
| MCP Streamable HTTP | `127.0.0.1:61084` | 可选的 MCP HTTP 接入端口 |

`61083` 当前实际暴露:

- `GET /healthz`: 扩展健康摘要，返回运行态、监控聚合和 alerts
- `GET /readyz`: 只给探针用；当前 primary provider 为 `unavailable` 时返回 `503`
- `GET /metrics`: Prometheus 文本格式指标

## 端口选择原则

1. `61080-61084` 作为默认核心端口段。
2. `61085-61099` 预留给调试、事件流、回放、诊断。
3. `61100-61149` 预留给未来 sidecar 或多实例部署。
4. 若设备上有多实例，实例间按固定偏移量分配。

## 多实例建议

示例:

- 实例 A:
  - S3 API `61080`
  - Admin UI `61081`
  - MCP HTTP `61084`
- 实例 B:
  - S3 API `61100`
  - Admin UI `61101`
  - MCP HTTP `61104`

## PVE / 容器建议

- 容器内部也继续使用 `61080+`
- 需要对外暴露时，通过反向代理或端口映射转发
- 不建议直接把管理界面暴露到公网
- 若启用 MCP Streamable HTTP，建议只开放到受控内网并校验来源

## 软路由建议

- `S3 API` 是否对外开放按你的 Agent/客户端接入方式决定
- `Admin Web UI`、`OAuth Callback`、`Metrics / Health` 默认都应保持 `127.0.0.1`
- 软路由长期值守建议结合 [router-deployment-guide.md](/home/walky/carrier-cloud-blob-gateway/docs/router-deployment-guide.md:1)

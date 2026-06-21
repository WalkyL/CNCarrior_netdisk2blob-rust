# Agent-nats-redmine-hub Integration

本说明固定 `ccbg` 项目侧接入 `.51` 协调 Hub 的当前约定，避免把外部 Hub MCP 和本项目自带的本地 MCP 混用。

如果你是从历史 `hermes-redmine-hub` 身份迁移过来，先看上游迁移说明：

- [AGENT-NATS-REDMINE-HUB-MIGRATION.md](D:/workspaces/hermes-redmine-hub/docs/AGENT-NATS-REDMINE-HUB-MIGRATION.md)
- [AGENT-SERVICE-INTEGRATION.md](D:/workspaces/hermes-redmine-hub/docs/AGENT-SERVICE-INTEGRATION.md)

## 当前 live Hub

- 服务身份：`agent-nats-redmine-hub`
- HTTP base URL：`http://192.168.1.51:8787`
- MCP endpoint：`http://192.168.1.51:8787/mcp`
- 鉴权方式：`Authorization: Bearer <adapter-token>`

## 必须改的点

如果 `ccbg` 侧脚本、Agent runtime、接入文档或人工排障步骤里显式检查以下内容，应统一改成新值：

- `/healthz` 返回中的 `service` 应为 `agent-nats-redmine-hub`
- `/info` 返回中的 `service` 应为 `agent-nats-redmine-hub`
- 未鉴权响应头 `WWW-Authenticate` 应接受：
  - `Bearer realm="agent-nats-redmine-hub"`
- 如果需要引用该项目的 canonical project key，应使用：
  - `agent-nats-redmine-hub`

## 不要改错的点

这次不是整套基础设施更名。以下值当前仍保持旧名或旧路径，不要自行猜测改名：

- GitHub repo：`WalkyL/hermes-redmine-hub`
- Redmine project identifier：`hermes-redmine-hub`
- systemd unit：`hermes-redmine-hub.service`
- 部署目录：`/srv/hermes-redmine-hub`

## 对 ccbg 最重要的边界

`ccbg` 自带的 MCP 和 `.51` 外部 Hub MCP 不是同一个服务：

- `ccbg` 本地 MCP：
  - 默认 `stdio`
  - 可选 HTTP `http://127.0.0.1:61084/mcp`
- `.51` 外部协调 Hub MCP：
  - `http://192.168.1.51:8787/mcp`

不要把 `.51` Hub 当成 `ccbg` 的 `61084/mcp`。

尤其不要使用：

- `http://192.168.1.51:61084/mcp`

这不是 live `agent-nats-redmine-hub` 的 MCP 地址。

## 推荐接入方式

### HTTP

环境变量可继续保持：

```bash
REDMINE_HUB_URL=http://192.168.1.51:8787
REDMINE_HUB_TOKEN=<adapter-token>
```

健康检查：

```bash
curl -fsS "$REDMINE_HUB_URL/healthz"
curl -fsS "$REDMINE_HUB_URL/info"
```

业务调用：

```bash
curl -fsS \
  -H "Authorization: Bearer $REDMINE_HUB_TOKEN" \
  "$REDMINE_HUB_URL/v1/projects/<project-id>/tickets?status=open"
```

### Remote MCP HTTP

```json
{
  "mcpServers": {
    "redmine_hub": {
      "type": "http",
      "url": "http://192.168.1.51:8787/mcp",
      "headers": {
        "Authorization": "Bearer <adapter-token>"
      }
    }
  }
}
```

说明：

- initialize 前就带上 bearer token
- 首次 `GET /mcp` 返回 `405 Allow: POST` 不算故障
- 不要再对服务名做 `hermes-redmine-hub` 断言

## 最小验证

无鉴权：

```bash
curl -fsS http://192.168.1.51:8787/healthz
curl -fsS http://192.168.1.51:8787/info
curl -sS -D - -o NUL http://192.168.1.51:8787/v1/progress
```

有鉴权：

```bash
curl -fsS \
  -H "Authorization: Bearer $REDMINE_HUB_TOKEN" \
  "http://192.168.1.51:8787/v1/projects/<project-id>/tickets?status=open"
```

MCP：

- endpoint 为 `/mcp`
- bearer token 在 initialize 前已带上
- 如需显式检查未鉴权响应头，应接受：
  - `WWW-Authenticate: Bearer realm="agent-nats-redmine-hub"`

## CCBG Inbox Ops

如果当前 Windows 宿主没有本地 `nats` CLI，也没有把 `.51` NATS 凭据直接落到本机 env，
可以使用仓库内的 `.51` 远程运维脚本：

- [ops-012-51-agent-bus-read-reply.md](D:\workspaces\ccbg\docs\ops-012-51-agent-bus-read-reply.md)

这套脚本会通过 SSH 到 `.51`，复用 `/etc/nats/agent-bus.env` 里的 live
`codex-ccbg` 身份做 JetStream 历史读取和 bounded 回执发布，不会把密码写入仓库。

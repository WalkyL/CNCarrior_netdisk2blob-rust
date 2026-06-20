# OpenWRT Lite 部署与验收

## 目标

`OpenWRT lite` 包面向 `arm64` 软路由和小内存 Linux 宿主。默认配置优先保证驻留稳定性:

- `gatewayd` 作为本地 S3 host 常驻
- `Admin Web` 不作为默认入口，控制面仅以 `terminal` 模式绑定 `127.0.0.1`
- `Metrics`、`OAuth Callback` 只保留 loopback 配置
- `mcp-server` 以 stdio 二进制随包交付，由 Agent 按需拉起
- OneDrive 默认关闭，不作为近期默认备份或 fallback 目标

## 打包

本机 host 架构验证:

```bash
scripts/build-openwrt-lite-package.sh
```

交叉编译时显式传入 Rust target:

```bash
scripts/build-openwrt-lite-package.sh --target aarch64-unknown-linux-musl
```

产物:

- `target/openwrt-lite/ccbg-openwrt-lite.tar.gz`
- `target/openwrt-lite/ccbg-openwrt-lite.tar.gz.sha256`
- 包内 `MANIFEST.sha256`

## 安装路径

包内 `scripts/install.sh` 会写入:

- `/usr/bin/gatewayd`
- `/usr/bin/mcp-server`
- `/usr/lib/ccbg/assets/admin/index.html`
- `/etc/init.d/ccbg`
- `/etc/ccbg/openwrt-lite.env`
- `/etc/ccbg/config/`
- `/etc/ccbg/scripts/smoke.sh`
- `/overlay/ccbg/`
- `/overlay/ccbg/provider-credentials/`
- `/tmp/ccbg-spool/`

如果 `/etc/ccbg/openwrt-lite.env` 已存在，安装脚本会保留现有文件，并把新样例写成 `openwrt-lite.env.package-<timestamp>`。

正式 release 约定:

- `gatewayd` 与 Admin HTML 必须随同一个安装包发布
- 运行时会优先读取二进制同前缀下的 `assets/admin/index.html`
- 不应依赖宿主机额外手工散落的 HTML 覆盖来修正正式版本页面

## 默认配置

[openwrt-lite.env](../config/openwrt-lite.env) 的默认值用于离线 smoke:

- `CCBG_PRIMARY_PROVIDER=unicom`
- `CCBG_ADMIN_MODE=terminal`
- `CCBG_DATA_PLANE_MAX_IN_FLIGHT=2`
- `CCBG_DATA_PLANE_MAX_REQUESTS_PER_SECOND=8`
- `CCBG_MAX_IN_MEMORY_OBJECT_BYTES=4194304`
- `CCBG_REPLICATION_WORKERS=1`
- `CCBG_ONEDRIVE_ENABLED=false`
- `CCBG_ONEDRIVE_REPLICATION_ENABLED=false`
- `CCBG_CONTROL_API_KEY=change-me-control-api-key`
- `MCP_SERVER_HTTP_ENABLED=false`

接入真实运营商 provider 前，先把 `CCBG_PRIMARY_PROVIDER` 改成 `unicom`、`telecom` 或 `mobile`，再写入对应凭证。

如果要在非本机网络暴露任何控制面入口，必须先替换 `CCBG_CONTROL_API_KEY`；OpenWRT lite 的默认包只假设本机 loopback control API 供 stdio MCP 使用。

## OpenWRT 命令

```sh
/etc/init.d/ccbg enable
/etc/init.d/ccbg start
/etc/init.d/ccbg restart
/etc/init.d/ccbg stop
logread -e ccbg
```

## MCP 使用

OpenWRT lite 不默认常驻 MCP HTTP。客户端不需要 Rust 环境，直接调用已安装的二进制:

```json
{
  "command": "/usr/bin/mcp-server",
  "env": {
    "MCP_CONTROL_BASE_URL": "http://127.0.0.1:61081",
    "MCP_CONTROL_API_KEY": "change-me-control-api-key",
    "MCP_CONTROL_TIMEOUT_MS": "2000",
    "MCP_CONTROL_MAX_RETRIES": "1"
  }
}
```

当前 MCP 协议版本由 `mcp-server` 初始化响应声明为 `2025-03-26`。

如果要启用可选的 MCP HTTP:

- 旧环境变量名继续可用: `MCP_SERVER_HTTP_ENABLED`、`MCP_SERVER_HTTP_BIND`、`MCP_SERVER_HTTP_PATH`、`MCP_SERVER_HTTP_BEARER_TOKEN`
- 也接受别名: `CCBG_MCP_HTTP_ENABLED`、`CCBG_MCP_HTTP_BIND_ADDR`、`CCBG_MCP_HTTP_ENDPOINT`、`CCBG_MCP_HTTP_BEARER_TOKEN`
- 未鉴权调用可做公开能力发现；真正的运维 tool call 仍要求 bearer token

注意：

- 这里的 `mcp-server` 是本机 `ccbg` 的 stdio MCP，用来访问本机 `gatewayd` 控制面。
- 如果 Agent 还要接入 `.51` 的外部 Agent-nats-redmine-hub，当前 live MCP endpoint 是 `http://192.168.1.51:8787/mcp`。
- 不要把外部 Hub 错配成 `192.168.1.51:61084/mcp`。

## Smoke

在 OpenWRT 上:

```sh
/etc/ccbg/scripts/smoke.sh
```

如果手动解包测试:

```sh
./scripts/install.sh --no-start
/etc/init.d/ccbg start
./scripts/smoke.sh
```

`smoke.sh` 会检查:

- `gatewayd --version` 可输出 provenance/fingerprint
- `GET /healthz` 可访问
- 配置了 `CCBG_CONTROL_API_KEY` 时，`/readyz` 可访问
- `mcp-server` stdio `initialize` 返回 `protocolVersion=2025-03-26`

## 24 小时稳定性验收

测试前记录:

```sh
date
cat /proc/meminfo
df -h /overlay /tmp
gatewayd --version
grep -E 'CCBG_(ADMIN_MODE|PRIMARY_PROVIDER|ONEDRIVE|DATA_PLANE|MAX_IN_MEMORY|REPLICATION_WORKERS)' /etc/ccbg/openwrt-lite.env
```

测试中每 30 分钟记录:

```sh
date
pidof gatewayd
grep -E 'VmRSS|VmHWM' /proc/$(pidof gatewayd)/status
du -h /overlay/ccbg/ccbg.db 2>/dev/null || true
du -sh /tmp/ccbg-spool 2>/dev/null || true
logread -e ccbg | tail -80
```

通过标准:

- 24 小时内无频繁 OOM、无服务反复重启
- `VmHWM` 与对象上限、并发上限匹配，没有持续爬升
- `/overlay/ccbg/ccbg.db` 可控增长
- `/tmp/ccbg-spool` 没有残留堆积
- `GET /healthz` 持续成功
- 基础 S3 `ListBuckets`、小对象 `PUT/GET/DELETE` 成功
- `CCBG_ONEDRIVE_ENABLED=false` 与 `CCBG_ONEDRIVE_REPLICATION_ENABLED=false` 未被安装脚本改写

# Cargo Proxy For Windows

当本机直接访问 `crates.io` 不稳定时，Windows PowerShell 下推荐用仓库自带脚本：

- [scripts/cargo-with-proxy.ps1](D:\workspaces\ccbg\scripts\cargo-with-proxy.ps1)

## 默认行为

- 默认代理：`socks5h://127.0.0.1:10808`
- 默认 `NO_PROXY`：`127.0.0.1,localhost,::1`
- 默认 `CARGO_HTTP_TIMEOUT`：`120`

这样做的目的有两个：

1. 让 Cargo 下载 `crates.io` 依赖时走本地代理
2. 避免测试进程访问本机 `127.0.0.1` 时也被 SOCKS 代理拦走

如果不设置 `NO_PROXY`，像 `mcp-server` 这类会在单元测试里访问本机 HTTP listener 的 crate，可能报：

- `unsupported scheme socks5h`
- `control API unavailable`

## 常用命令

运行 `mcp-server` 测试：

```powershell
.\scripts\cargo-with-proxy.ps1 test -p mcp-server
```

运行 `gatewayd` 检查：

```powershell
.\scripts\cargo-with-proxy.ps1 check -p gatewayd
```

执行工作区格式检查：

```powershell
.\scripts\cargo-with-proxy.ps1 fmt --all --check
```

## 自定义代理地址

通过环境变量覆盖：

```powershell
$env:CCBG_CARGO_PROXY_URL = "socks5h://127.0.0.1:7897"
$env:CCBG_CARGO_NO_PROXY = "127.0.0.1,localhost,::1"
$env:CCBG_CARGO_HTTP_TIMEOUT = "180"
.\scripts\cargo-with-proxy.ps1 test -p mcp-server
```

## 当前已验证

在本机 `127.0.0.1:10808` 代理可用的前提下，以下命令已通过：

```powershell
.\scripts\cargo-with-proxy.ps1 test -p mcp-server
```

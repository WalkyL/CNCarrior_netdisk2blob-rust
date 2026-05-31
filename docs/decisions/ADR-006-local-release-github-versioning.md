# ADR-006: GitHub 只做版本管理，本地执行发布与部署

## Status

Accepted

## Date

2026-06-01

## Context

CCBG 的发布流程同时涉及内网测试机、Cloudflare 公开站、Linux/LXC/OpenWrt/Windows
包和 macOS 原生包。之前的方案把 CI、Cloudflare 部署、局域网 runner 构建、macOS
构建和 GitHub Release 汇总放在同一个 GitHub Actions release workflow 中。

这个模式有几个问题：

- CCBG 与 `llm-router` 的局域网 runner 语境容易混淆。
- `.43` 是验收测试机，不应该被隐式当成构建或发布 runner。
- Cloudflare 部署依赖本机已有凭据和可访问网络，从 GitHub-hosted runner 发起 smoke
  容易被边缘安全策略干扰。
- Linux/Windows/OpenWrt 包更适合由本地或明确指定的内网机器生成并留存证据。
- macOS 包仍需要 Darwin 工具链和 macOS runner。

## Decision

GitHub 对 CCBG 默认只承担版本管理职责：源码、分支、tag、issue 模板和 release
记录。通用 CI、Linux/Windows/OpenWrt 组包、Cloudflare 公开站部署不再由 GitHub
Actions 自动触发。

保留一个例外：macOS `x86_64` / `arm64` 包继续通过 GitHub `release-macos` workflow
构建，并可上传到同名 GitHub Release。

本地流程改为：

- `scripts/check-release-ready.sh` 执行 release gate。
- `scripts/release-local.sh <tag>` 生成本地发布物、checksums 和 provenance。
- `scripts/deploy-cloudflare-public.sh test|production` 部署 Cloudflare Worker + Assets。

## Alternatives Considered

### 继续使用统一 GitHub Actions release workflow

优点是入口集中、产物能自动汇总。缺点是把内网构建、GitHub-hosted macOS、GHCR、
Cloudflare 部署和公网 smoke 绑定在一起，任何一环的环境差异都会影响整条发布链。

### 完全去掉 GitHub Actions

优点是边界最清晰。缺点是 macOS 包需要额外维护本地 macOS 构建机；现阶段 GitHub
macOS runner 更简单，也更容易覆盖 `x86_64` 与 `arm64`。

## Consequences

- 推送 `main` 或 `test` 不会自动部署。
- release 前必须显式运行本地质量门并留存结果。
- Cloudflare 部署使用本机 Cloudflare 凭据，发布人需要确认当前 shell 环境。
- macOS GitHub workflow 只负责 macOS 包，不负责其他平台。
- 如果公网安装 catalog 继续指向 GitHub release download URL，发布人必须手动保证
  对应资产已上传到 GitHub Release。
- 新增任何内网构建机前，必须先在项目文档里记录机器、目录、凭据边界和清理方案。

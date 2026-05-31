# ADR-006: GitHub 只做版本管理，本地执行发布与部署

## Status

Accepted

## Date

2026-06-01

## Context

CCBG 的发布流程同时涉及内网测试机、Cloudflare 公开站、Linux/LXC/OpenWrt/Windows
包和 macOS 社区/实验包。之前的方案把 CI、Cloudflare 部署、局域网 runner 构建、
macOS 构建和 GitHub Release 汇总放在同一个 GitHub Actions release workflow 中。

这个模式有几个问题：

- CCBG 与 `llm-router` 的局域网 runner 语境容易混淆。
- `.43` 是验收测试机，不应该被隐式当成构建或发布 runner。
- Cloudflare 部署依赖本机已有凭据和可访问网络，从 GitHub-hosted runner 发起 smoke
  容易被边缘安全策略干扰。
- Linux/Windows/OpenWrt 包更适合由明确指定的内网机器生成并留存证据。
- GitHub hosted macOS runner 有 credit 和节奏限制，不再适合作为 AI 高频发布路径。

## Decision

GitHub 对 CCBG 只承担版本管理职责：源码、分支、tag、issue 模板和 release 记录。
通用 CI、Linux/Windows/OpenWrt/macOS 组包、Cloudflare 公开站部署不再由 GitHub
Actions 自动触发。

`192.168.1.47` 是 CCBG 唯一发布构建主机。Linux LXC、OpenWrt、Windows、macOS、
STM32 示例、ESP32-S3 示例，以及后续新增的嵌入式或固件构建，都必须从 `.47` 的
项目工作区执行。`.46` 不再保留项目代码，也不再运行 CCBG 编译任务。

macOS `x86_64` / `arm64` 暂时降级为社区/实验交叉编译包，由 `.47` Windows 主机
产出；这些包未签名、未公证、未经过 macOS 真机 smoke，不按官方宿主承诺。

本地流程改为：

- `scripts/check-release-ready.sh` 执行 release gate。
- `scripts/release-local.sh <tag>` 在 `.47` 生成本地发布物、checksums 和 provenance。
- `scripts/deploy-cloudflare-public.sh test|production` 部署 Cloudflare Worker + Assets。

## Alternatives Considered

### 继续使用统一 GitHub Actions release workflow

优点是入口集中、产物能自动汇总。缺点是把内网构建、GitHub-hosted macOS、GHCR、
Cloudflare 部署和公网 smoke 绑定在一起，任何一环的环境差异都会影响整条发布链。

### 继续保留 GitHub macOS workflow

优点是能使用真实 macOS runner。缺点是 GitHub hosted macOS runner 有 credit 限制，
并且会重新把构建职责放回 GitHub Actions；与“GitHub 只做版本管理”的边界冲突。

## Consequences

- 推送 `main` 或 `test` 不会自动部署。
- release 前必须显式运行本地质量门并留存结果。
- Cloudflare 部署使用 `.47` Cloudflare 凭据，发布人需要确认当前 shell 环境。
- macOS 包是社区/实验交叉编译包；需要 macOS SDK 和 Darwin 目标工具链，失败时作为
  `.47` 工具链缺口处理，不切回 GitHub Actions。
- 如果公网安装 catalog 继续指向 GitHub release download URL，发布人必须手动保证
  对应资产已上传到 GitHub Release。
- 新增任何构建主机前，必须先在项目文档里记录机器、目录、凭据边界和清理方案。

# GitHub 公开仓库发布规划

## 目标

源码一期按公开仓库交付，但仓库不是 MIT 开源仓库。它采用商业核心、公开材料、以及可申请的个人非商业源码审查模式。

## 一期最小资产

- `LICENSE`
- `COMMERCIAL-LICENSE.md`
- `PUBLIC-MATERIALS-LICENSE.md`
- 清晰的 `README.md`
- 基础 CI
- Docker / Podman 构建入口
- Windows / macOS 原生包脚本
- 公开的规划文档
- `public/cloudflare/`

## 当前发布资产

- 商业核心边界: [LICENSE](../LICENSE)
- 商业许可文本: [COMMERCIAL-LICENSE.md](../COMMERCIAL-LICENSE.md)
- 公开材料许可文本: [PUBLIC-MATERIALS-LICENSE.md](../PUBLIC-MATERIALS-LICENSE.md)
- GitHub Actions CI: [.github/workflows/ci.yml](../.github/workflows/ci.yml)
- Docker 构建入口: [Dockerfile](../deploy/Dockerfile)
- Podman 构建入口: [Containerfile](../deploy/Containerfile)
- 原生宿主打包入口: [build-native-package.sh](../scripts/build-native-package.sh)
- macOS `launchd` 安装脚本: [deploy/macos](../deploy/macos)
- Windows 后台常驻安装脚本: [deploy/windows](../deploy/windows)
- Homebrew formula 模板: [packaging/homebrew](../packaging/homebrew)
- winget manifest 模板: [packaging/winget](../packaging/winget)
- Cloudflare 公共前端: [public/cloudflare](../public/cloudflare)
- 个人非商业源码审查流程: [personal-source-review.md](personal-source-review.md)

## 仓库结构要求

公开仓库需要让第一次打开的人快速理解:

1. 这是一个面向 Agent 的对象网关。
2. 当前哪些能力已经实现，哪些还在规划中。
3. 哪些平台是一期目标，哪些只是客户端兼容。
4. MCP 和 Skill 是正式交付物，而不是未来可选集成。
5. 商业核心、公开材料和个人非商业源码审查边界是分开的。

## CI 最低要求

一期 CI 只做最小闭环:

- `cargo fmt --check`
- `cargo test --workspace`
- Cloudflare public fingerprint check
- 原生发布包结构 smoke

后续可扩展:

- `clippy`
- 交叉编译检查
- Docker / Podman 构建验证
- Homebrew formula / winget manifest 发布校验

## 建议的 GitHub 发布节奏

### `v0.1.0-planning`

- 完成架构和路线图
- 明确平台矩阵
- 明确 Agent 交付策略

### `v0.2.0-core-skeleton`

- 完成 OneDrive、策略层、元数据层骨架
- 完成 primary provider / sync targets 模型

### `v0.3.0-mcp-alpha`

- 交付 stdio MCP
- 交付首版 Skill

### `v0.4.0-single-provider-mvp`

- 打通首个运营商 provider
- 完成主写 + 异步备份 + fallback

## 公开仓库注意事项

1. 不要把真实 token、cookie、refresh token 提交到仓库。
2. 样例配置只能保留空值和占位符。
3. 文档要明确区分“已实现”和“规划中”。
4. 若后续提供 Docker 镜像，也不要在镜像层内固化 secrets。

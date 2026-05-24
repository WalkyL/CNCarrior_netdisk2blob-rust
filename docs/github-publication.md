# GitHub 公开仓库发布规划

## 目标

源码一期就按公开 GitHub 仓库交付，因此仓库必须从一开始具备最基本的开源发布条件。

## 一期最小资产

- `LICENSE`
- 清晰的 `README.md`
- 基础 CI
- Docker / Podman 构建入口
- 公开的规划文档

## 当前发布资产

- MIT 许可证: [LICENSE](../LICENSE)
- GitHub Actions CI: [.github/workflows/ci.yml](../.github/workflows/ci.yml)
- Docker 构建入口: [Dockerfile](../deploy/Dockerfile)
- Podman 构建入口: [Containerfile](../deploy/Containerfile)

## 仓库结构要求

公开仓库需要让第一次打开的人快速理解:

1. 这是一个面向 Agent 的对象网关。
2. 当前哪些能力已经实现，哪些还在规划中。
3. 哪些平台是一期目标，哪些只是客户端兼容。
4. MCP 和 Skill 是正式交付物，而不是未来可选集成。

## CI 最低要求

一期 CI 只做最小闭环:

- `cargo fmt --check`
- `cargo test --workspace`

后续可扩展:

- `clippy`
- 交叉编译检查
- Docker / Podman 构建验证

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

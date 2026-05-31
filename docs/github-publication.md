# GitHub 版本管理与发布流程

## 目标

GitHub 在 CCBG 项目里主要承担版本管理职责：保存源码、分支、tag、issue 模板和
macOS 专用构建 workflow。Linux、Windows、OpenWrt、Cloudflare 公开站点部署不再
依赖 GitHub Actions 自动执行，改为由本机或明确指定的内网构建机运行脚本。

源码一期按公开仓库交付，但仓库不是 MIT 开源仓库。它采用商业核心、公开材料、
以及可申请的个人非商业源码审查模式。

## 当前发布资产

- 商业核心边界: [LICENSE](../LICENSE)
- 商业许可文本: [COMMERCIAL-LICENSE.md](../COMMERCIAL-LICENSE.md)
- 公开材料许可文本: [PUBLIC-MATERIALS-LICENSE.md](../PUBLIC-MATERIALS-LICENSE.md)
- Docker 构建入口: [Dockerfile](../deploy/Dockerfile)
- Podman 构建入口: [Containerfile](../deploy/Containerfile)
- LXC 打包入口: [build-lxc-package.sh](../scripts/build-lxc-package.sh)
- 原生宿主打包入口: [build-native-package.sh](../scripts/build-native-package.sh)
- OpenWrt lite 打包入口: [build-openwrt-lite-package.sh](../scripts/build-openwrt-lite-package.sh)
- 本地 release gate: [check-release-ready.sh](../scripts/check-release-ready.sh)
- 本地 release 组包: [release-local.sh](../scripts/release-local.sh)
- 本地 Cloudflare 部署: [deploy-cloudflare-public.sh](../scripts/deploy-cloudflare-public.sh)
- macOS GitHub 构建 workflow: [release-macos.yml](../.github/workflows/release-macos.yml)
- macOS `launchd` 安装脚本: [deploy/macos](../deploy/macos)
- Windows 后台常驻安装脚本: [deploy/windows](../deploy/windows)
- Homebrew formula 模板: [packaging/homebrew](../packaging/homebrew)
- winget manifest 模板: [packaging/winget](../packaging/winget)
- Cloudflare 公共前端: [public/cloudflare](../public/cloudflare)
- 个人非商业源码审查流程: [personal-source-review.md](personal-source-review.md)

## 分支与 tag

- `main`: 当前可发布主线。
- `test`: 测试部署分支，用于把同一提交推到 `.43` 和测试站点验收。
- release tag: 以 `vX.Y.Z` 形式标记已验收提交。

推送到 GitHub 本身不会触发 Linux/Windows/OpenWrt 构建，也不会触发 Cloudflare
部署。需要发布时，先在本地跑 release gate，再手动生成发布物和部署公开站。

## 本地 release gate

```bash
scripts/check-release-ready.sh
```

默认检查：

- `cargo fmt --all --check`
- `python3 scripts/license-check.py`
- `cargo test --workspace`
- catalog lint
- Cloudflare public fingerprint
- OneDrive parking / restore checklist
- backup restore drill
- S3 smoke
- `git diff --check`

如需同时跑原生 Linux 包结构 smoke：

```bash
CCBG_CHECK_NATIVE_PACKAGE_SMOKE=true scripts/check-release-ready.sh
```

## 本地发布物

```bash
scripts/release-local.sh v0.1.0
```

默认生成 LXC 包、`ccbg-checksums.txt`、`release-provenance.json` 和
`release-provenance.md`，输出到：

```text
target/release-local/<tag>/
```

Windows 和 OpenWrt 需要对应交叉编译工具链，按需打开：

```bash
CCBG_RELEASE_BUILD_WINDOWS=true \
CCBG_RELEASE_BUILD_OPENWRT=true \
scripts/release-local.sh v0.1.0
```

macOS 包仍由 GitHub macOS runner 构建。拿到 macOS artifacts 后，可以把它们合并
进本地 release 目录：

```bash
CCBG_RELEASE_MACOS_ASSET_DIR=/path/to/macos-assets \
scripts/release-local.sh v0.1.0
```

如果确实需要把本地生成的资产上传到同一个 GitHub Release，显式打开：

```bash
CCBG_RELEASE_UPLOAD_GITHUB=true scripts/release-local.sh v0.1.0
```

这个开关不是默认流程；只有公网安装 catalog 仍指向 GitHub release download URL 时
才需要使用。

## macOS GitHub 流程

macOS `x86_64` 与 `arm64` 包继续走 GitHub Actions，因为 Darwin 工具链和
`launchd` 路径需要 macOS runner 验证：

1. 打开 `release-macos` workflow。
2. 输入 release tag，例如 `v0.1.0`。
3. 保持 `publish_release=true` 时，workflow 会创建或更新同名 GitHub Release。
4. 产物包括：
   - `ccbg-macos-x86_64.tar.gz`
   - `ccbg-macos-x86_64.tar.gz.sha256`
   - `ccbg-macos-arm64.tar.gz`
   - `ccbg-macos-arm64.tar.gz.sha256`
   - `ccbg-macos-checksums.txt`

该 workflow 不负责 Linux、Windows、OpenWrt、容器镜像或 Cloudflare 部署。

## Cloudflare 公开站部署

公开站部署由本地命令执行：

```bash
scripts/deploy-cloudflare-public.sh test
scripts/deploy-cloudflare-public.sh production
```

脚本读取本机环境变量：

- `CLOUDFLARE_API_TOKEN` 或 `CF_API_TOKEN`
- `CLOUDFLARE_ACCOUNT_ID` 或 `CF_ACCOUNT_ID`

默认 Worker 名称：

- test: `ccbg-public-test`
- production: `ccbg-public`

默认生产域名：

- `carrier-disk-gateway.agi2030.online`

日常部署只更新 Worker + Assets。只有首次绑定或需要重新绑定 custom domain 时才设置：

```bash
CCBG_CF_BIND_DOMAIN_ON_DEPLOY=true scripts/deploy-cloudflare-public.sh production
```

如果当前网络能稳定访问生产域名，可以同时打开发布后 fingerprint smoke：

```bash
CCBG_CF_SMOKE_DOMAIN_ON_DEPLOY=true scripts/deploy-cloudflare-public.sh production
```

## 机器边界

- `192.168.1.43` 是 CCBG 验收测试机，用来跑发布候选版本和人工全量验收。
- `192.168.1.46` / `192.168.1.47` 是 `llm-router` 历史本地 runner 语境里的机器，
  不是 CCBG 默认构建 runner。
- CCBG 需要内网构建机时，必须先在文档里明确机器、目录、凭据边界和清理方案。

## 发布注意事项

1. 不要把真实 token、cookie、refresh token 提交到仓库。
2. 样例配置只能保留空值和占位符。
3. 每个宿主包都必须包含 Rust `gatewayd` 和 `assets/admin/index.html`。
4. OpenWrt lite 包还必须包含 `mcp-server`。
5. 文档要明确区分“已实现”和“规划中”。
6. 若公网安装页继续引用 GitHub release URL，必须确保对应 release 资产已经手动上传
   或由 macOS workflow 发布。

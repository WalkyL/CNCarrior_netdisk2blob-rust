# GitHub 版本管理与发布流程

## 目标

GitHub 在 CCBG 项目里只承担版本管理职责：保存源码、分支、tag、issue 模板和
release 记录。Linux、Windows、OpenWrt、macOS、Cloudflare 公开站点部署不再依赖
GitHub Actions 自动执行，统一由 `.47` 发布构建主机运行脚本。

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

推送到 GitHub 本身不会触发 Linux/Windows/OpenWrt/macOS 构建，也不会触发 Cloudflare
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

在 `.47` 上默认生成 LXC 包、`ccbg-checksums.txt`、`release-provenance.json` 和
`release-provenance.md`，输出到：

```text
target/release-local/<tag>/
```

Windows、OpenWrt 和 macOS 需要对应交叉编译工具链，按需打开：

```bash
CCBG_RELEASE_BUILD_WINDOWS=true \
CCBG_RELEASE_BUILD_OPENWRT=true \
CCBG_RELEASE_BUILD_MACOS=true \
scripts/release-local.sh v0.1.0
```

macOS `x86_64` / `arm64` 现在由 `.47` Windows 主机交叉编译产出，并按社区/实验包
发布。它们未签名、未公证、未经过 macOS 真机 smoke。若 `.47` 缺 Darwin SDK 或目标
工具链，发布记录必须把它列为工具链缺口，而不是切回 GitHub Actions。

保留 `CCBG_RELEASE_MACOS_ASSET_DIR` 只用于手工合并已有 macOS 资产，不作为默认构建
路径：

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

## `.47` 构建主机

所有构建都收敛到 `192.168.1.47`：

| 目标 | 构建主机 | 入口 |
| --- | --- | --- |
| PVE LXC `x86/x64` | `.47` | `scripts/release-local.sh <tag>` |
| Docker `x86/x64` | `.47` | `docker build -f deploy/Dockerfile .` |
| Podman `x86/x64` | `.47` | `podman build -f deploy/Containerfile .` |
| Windows `x86_64` | `.47` | `CCBG_RELEASE_BUILD_WINDOWS=true scripts/release-local.sh <tag>` |
| OpenWrt `arm64` | `.47` | `CCBG_RELEASE_BUILD_OPENWRT=true scripts/release-local.sh <tag>` |
| macOS `x86_64/arm64` | `.47` | `CCBG_RELEASE_BUILD_MACOS=true scripts/release-local.sh <tag>` |
| STM32 client-only 示例 | `.47` | `scripts/check-stm32-client-example.sh` |
| ESP32-S3 client-only 示例 | `.47` | `scripts/check-esp32-s3-client-example.py`；如需要 ESP-IDF 真编译，也只在 `.47` 上执行 |

更完整的主机边界见 [ops-007-47-release-build-host.md](ops-007-47-release-build-host.md)。

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
- `192.168.1.43` 是 CCBG 验收测试机，用来跑发布候选版本和人工全量验收。
- `192.168.1.47` 是 CCBG 唯一发布构建主机，工作区为
  `C:\Users\walky\workspaces\carrier-cloud-blob-gateway`。
- `192.168.1.46` 不再保留 CCBG 项目代码，也不再运行 CCBG 编译任务。

## 发布注意事项

1. 不要把真实 token、cookie、refresh token 提交到仓库。
2. 样例配置只能保留空值和占位符。
3. 每个宿主包都必须包含 Rust `gatewayd` 和 `assets/admin/index.html`。
4. OpenWrt lite 包还必须包含 `mcp-server`。
5. 文档要明确区分“已实现”和“规划中”。
6. 若公网安装页继续引用 GitHub release URL，必须确保对应 release 资产已经从 `.47`
   手动上传。

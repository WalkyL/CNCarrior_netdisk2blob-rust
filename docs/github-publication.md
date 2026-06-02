# GitHub 版本管理与发布流程

## 目标

GitHub 在 CCBG 项目里默认只承担版本管理职责：保存源码、分支、tag、issue 模板和
可选 GitHub Release 页面记录。Linux、OpenWrt、Windows、容器 tar/镜像二进制和
Cloudflare 公开站点部署不依赖 GitHub Actions 自动执行，统一由 `.47` 发布构建主机
或受控局域网 runner 运行脚本。当前本机和局域网没有 macOS 构建器，所以发布时 macOS
资产固定走 GitHub Actions 的 macOS-only 例外构建入口；该例外只覆盖 macOS，不恢复
通用 GitHub CI 或通用发版。macOS 资产仍必须进入同一 release 校验和
`/downloads/latest/*` 发布链路。

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
- 公网发版 SOP: [public-release-sop.md](public-release-sop.md)
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

推送到 GitHub 本身不会触发 Linux/OpenWrt/Windows/容器构建，也不会触发
Cloudflare 部署。只打 tag 或只创建 GitHub Release 页面不算发版完成；必须先在本机
或受控局域网 runner 跑 release gate、生成发布物、同步发布资产，并确认公网
`/downloads/latest/*` 指向新资产后，才算完成发版。macOS 资产由 macOS-only GitHub
Actions 构建后，也必须下载回发布流程并参与同一校验、上传和下载 smoke。

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
scripts/release-local.sh v0.1.1
```

在 `.47` 上默认生成 LXC 包、`ccbg-checksums.txt`、`release-provenance.json` 和
`release-provenance.md`，输出到：

```text
target/release-local/<tag>/
```

Windows 和 OpenWrt 需要对应交叉编译工具链，按需打开：

```bash
CCBG_RELEASE_BUILD_WINDOWS=true \
CCBG_RELEASE_BUILD_OPENWRT=true \
scripts/release-local.sh v0.1.1
```

macOS `x86_64` / `arm64` 当前由 GitHub Actions macOS-only workflow 产出，并按社区/
实验包发布。它们未签名、未公证、未经过本项目控制的 macOS 真机 smoke。发布人必须
下载 GitHub Actions 产物并合并回本地 release 目录：

```bash
CCBG_RELEASE_MACOS_ASSET_DIR=/path/to/macos-assets \
scripts/release-local.sh v0.1.1
```

`CCBG_RELEASE_BUILD_MACOS=true` 现在不会默认在 `.47` 构建 macOS 包；除非已有文档记录的
本机/局域网 macOS 构建器或 Darwin 交叉工具链，并显式设置
`CCBG_RELEASE_ALLOW_LOCAL_MACOS_BUILD=true`。

如果确实需要把本地生成的资产上传到同一个 GitHub Release，显式打开：

```bash
CCBG_RELEASE_UPLOAD_GITHUB=true scripts/release-local.sh v0.1.1
```

脚本会通过 `scripts/resolve-gh.sh` 查找 GitHub CLI；在 `.47` 上即使 Git Bash 默认
`PATH` 没有裸 `gh`，也会使用 `C:\Program Files\GitHub CLI\gh.exe`。

这个开关不是默认流程；只有公网安装 catalog 仍指向 GitHub release download URL 时
才需要使用。

## `.47` 构建主机

Linux / OpenWrt / Windows / 容器 tar 或镜像二进制默认走本机 `.47` 或受控局域网
runner，不走 GitHub Actions。当前构建收敛到 `192.168.1.47`：

| 目标 | 构建主机 | 入口 |
| --- | --- | --- |
| PVE LXC `x86/x64` | `.47` | `scripts/release-local.sh <tag>` |
| Docker `x86/x64` | `.47` | `docker build -f deploy/Dockerfile .` |
| Podman `x86/x64` | `.47` | `podman build -f deploy/Containerfile .` |
| Windows `x86_64` | `.47` | `CCBG_RELEASE_BUILD_WINDOWS=true scripts/release-local.sh <tag>` |
| OpenWrt `arm64` | `.47` | `CCBG_RELEASE_BUILD_OPENWRT=true scripts/release-local.sh <tag>` |
| macOS `x86_64/arm64` | GitHub Actions macOS-only exception, then merged on `.47` | `CCBG_RELEASE_MACOS_ASSET_DIR=<downloaded-artifacts> scripts/release-local.sh <tag>` |
| STM32 client-only 示例 | `.47` | `scripts/check-stm32-client-example.sh` |
| ESP32-S3 client-only 示例 | `.47` | `scripts/check-esp32-s3-client-example.py`；如需要 ESP-IDF 真编译，也只在 `.47` 上执行 |

更完整的主机边界见 [ops-007-47-release-build-host.md](ops-007-47-release-build-host.md)。

## Cloudflare 公开站部署

公开站部署由本地命令执行：

```bash
scripts/deploy-cloudflare-public.sh test
scripts/deploy-cloudflare-public.sh production
```

如果要让公网安装页优先从 Cloudflare R2 命中 release 缓存，而不是每次都回源 GitHub，
在部署前设置：

```bash
CCBG_CF_PROD_RELEASE_R2_BUCKET=ccbg-release-assets scripts/deploy-cloudflare-public.sh production
CCBG_CF_TEST_RELEASE_R2_BUCKET=ccbg-release-assets-test scripts/deploy-cloudflare-public.sh test
```

脚本会先把当前本地 release 包上传到对应 bucket 的 `latest/<asset-name>`，再生成一个
临时 Wrangler 配置，把 `RELEASE_ASSETS` 绑定到 Worker。

截至 2026-06-02，当前实际使用的 bucket 为：

- test: `ccbg-release-assets-test`
- production: `ccbg-release-assets`

同日线上验收结果：

- `ccbg-public-test` 已带 `RELEASE_ASSETS` 绑定重新部署
- `ccbg-public` 已带 `RELEASE_ASSETS` 绑定重新部署
- `HEAD https://carrier-disk-gateway.agi2030.online/downloads/latest/ccbg-lxc-package.tar.gz`
  返回 `200`，响应头 `x-ccbg-release-source=r2`
- `HEAD https://carrier-disk-gateway.agi2030.online/downloads/latest/ccbg-windows-x86_64.zip`
  返回 `200`，响应头 `x-ccbg-release-source=r2`

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
- `192.168.1.47` 是 CCBG 默认发布构建主机，工作区为
  `C:\Users\walky\workspaces\carrier-cloud-blob-gateway`。
- 当前本机和局域网没有 macOS 构建器；macOS 发布资产走 GitHub Actions macOS-only
  例外构建入口，产物下载回 `.47` 后再进入统一发布链路。
- `192.168.1.46` 不再保留 CCBG 项目代码，也不再运行 CCBG 编译任务。

## 发布注意事项

1. 不要把真实 token、cookie、refresh token 提交到仓库。
2. 样例配置只能保留空值和占位符。
3. 每个宿主包都必须包含 Rust `gatewayd` 和 `assets/admin/index.html`。
3.1 官方宿主包（LXC / Windows / macOS）默认把 Admin Web 打开到 `0.0.0.0:61081`；OpenWrt lite 不跟随这个默认值。
4. OpenWrt lite 包还必须包含 `mcp-server`。
5. 文档要明确区分“已实现”和“规划中”。
6. 若公网安装页走 Cloudflare `/downloads/latest/...`，也必须保证以下两者至少其一完整可用：
   Cloudflare R2 `latest/<asset-name>` 缓存，或 GitHub latest release 对应资产。
   当前正式流程优先同步 R2，并保留 GitHub release 资产作为回源兜底。
7. 只打 tag、只推分支、只创建 GitHub Release 页面、或只上传 GitHub 资产都不算发版完成。
   完成条件是公网安装页和 `/downloads/latest/*` 已经指向本轮新资产，并通过下载 smoke。

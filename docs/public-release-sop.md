# CCBG Public Release SOP

这份 SOP 只覆盖 `.47` 发版主机上的公网交付链路：

- 生成本地 release 包
- 补传 GitHub release 资产作为回源兜底
- 同步 Cloudflare R2 release cache
- 发布 Cloudflare test / production Worker
- 做最小线上 smoke

发版完成定义：

- GitHub 默认只保存分支、tag、issue、可选 Release 页面和发布记录。
- Linux / OpenWrt / Windows / 容器 tar 或镜像二进制默认走 `.47` 或受控局域网 runner。
- 当前本机和局域网没有 macOS 构建器；发布时 macOS 资产走 GitHub Actions macOS-only 例外构建入口。
- 只打 tag 或只创建 GitHub Release 页面不算发版完成。
- 必须确认 `/downloads/latest/*` 已指向本轮新资产，并通过下载 smoke，才算发版完成。
- provider 能力文案也属于 release 的一部分。没有新的 limit-probe 或隔离实测证据时，不得把中国移动的 `04010319 / Insufficient Rights` 已知限制写成“超大文件已验证通过”。

更完整的背景、边界和历史记录见：

- [github-publication.md](github-publication.md)
- [release-checklist.md](release-checklist.md)
- [ops-007-47-release-build-host.md](ops-007-47-release-build-host.md)

## 1. 前提

在 `.47` 上确认这些环境变量已经存在：

```bash
CLOUDFLARE_API_TOKEN
CLOUDFLARE_ACCOUNT_ID
GH_TOKEN
```

当前约定的 Cloudflare R2 bucket：

- test: `ccbg-release-assets-test`
- production: `ccbg-release-assets`

当前公网域名：

- production: `carrier-disk-gateway.agi2030.online`

## 2. 本地质量门

```bash
scripts/check-release-ready.sh
```

如果这一步不过，不继续发公网版本。

同时检查本轮对外文案：

- Admin
- `docs/provider-matrix.md`
- `docs/auth-step-by-step.md`
- 如有单独发布说明，也要同步

如果中国移动没有新的大文件放行证据，继续沿用当前已验证结论：

- `.49` 于 2026-06-05 的 16 GiB 隔离实测仍返回 `code=04010319`
- `message=Insufficient Rights`

## 3. 生成 release 包

标准正式包：

```bash
CCBG_RELEASE_BUILD_WINDOWS=true \
CCBG_RELEASE_BUILD_OPENWRT=true \
scripts/release-local.sh v0.1.7
```

Linux / OpenWrt / Windows / 容器 tar 或镜像二进制默认都在 `.47` 或受控局域网 runner 上
生成，不把 GitHub Actions 当默认构建入口。macOS 因当前没有本机/局域网构建器，发布时
走 GitHub Actions macOS-only 例外构建；下载产物后用下面的方式合并回本 SOP 的校验、
上传和 `/downloads/latest/*` smoke：

```bash
CCBG_RELEASE_MACOS_ASSET_DIR=/path/to/macos-assets \
scripts/release-local.sh v0.1.7
```

至少确认以下文件已经生成：

- `target/lxc-package/ccbg-lxc-package.tar.gz`
- `target/native-packages/ccbg-windows-x86_64.zip`
- `target/native-packages/ccbg-macos-x86_64.tar.gz`
- `target/native-packages/ccbg-macos-arm64.tar.gz`
- `target/openwrt-lite/ccbg-openwrt-lite.tar.gz`

同时确认官方宿主包默认行为：

- LXC / Windows / macOS 包默认把 Admin Web 打开到 `0.0.0.0:61081`
- OpenWrt lite 继续保持原有 profile，不跟随宿主包默认值

## 4. 补传 GitHub release 资产

Cloudflare 现在优先读 R2，但仍保留 GitHub latest release 作为回源兜底，所以正式发版时仍要补传。

如果 `scripts/release-local.sh` 这轮没有带 `CCBG_RELEASE_UPLOAD_GITHUB=true`，手工补传：

```bash
gh release upload v0.1.7 \
  target/lxc-package/ccbg-lxc-package.tar.gz \
  target/native-packages/ccbg-windows-x86_64.zip \
  target/native-packages/ccbg-macos-x86_64.tar.gz \
  target/native-packages/ccbg-macos-arm64.tar.gz \
  target/openwrt-lite/ccbg-openwrt-lite.tar.gz \
  --repo WalkyL/CNCarrior_netdisk2blob-rust
```

快速确认：

```bash
gh release view v0.1.7 --repo WalkyL/CNCarrior_netdisk2blob-rust --json assets
```

## 5. 部署 test

```bash
CCBG_CF_TEST_RELEASE_R2_BUCKET=ccbg-release-assets-test \
CCBG_RELEASE_LOCAL_TAG=v0.1.7 \
scripts/deploy-cloudflare-public.sh test
```

这一步会自动做三件事：

1. staging public assets
2. 把当前 release 包上传到 `ccbg-release-assets-test/latest/<asset-name>`
3. 生成带 `RELEASE_ASSETS` 绑定的临时 Wrangler 配置并部署 test Worker

## 6. 部署 production

```bash
CCBG_CF_PROD_RELEASE_R2_BUCKET=ccbg-release-assets \
CCBG_RELEASE_LOCAL_TAG=v0.1.7 \
scripts/deploy-cloudflare-public.sh production
```

Release note:

- For a formal release, set `CCBG_RELEASE_LOCAL_TAG=vX.Y.Z` during Cloudflare deploy.
- This makes `scripts/sync-cloudflare-release-cache.sh` prefer
  `target/release-local/<tag>/` assets instead of stale files that may still exist under
  `target/native-packages/`.

如果是首次绑定 production custom domain，额外带：

```bash
CCBG_CF_BIND_DOMAIN_ON_DEPLOY=true
```

## 7. 最小线上 smoke

首页和 catalog：

```bash
curl -I https://carrier-disk-gateway.agi2030.online/
curl -I https://carrier-disk-gateway.agi2030.online/install/
curl -I https://carrier-disk-gateway.agi2030.online/data/install-catalog.json
```

下载代理：

```bash
curl -I https://carrier-disk-gateway.agi2030.online/downloads/latest/ccbg-lxc-package.tar.gz
curl -I https://carrier-disk-gateway.agi2030.online/downloads/latest/ccbg-windows-x86_64.zip
```

LXC 安装命令使用：

```bash
curl -fsSLO https://carrier-disk-gateway.agi2030.online/downloads/latest/ccbg-lxc-package.tar.gz
tar --no-same-owner -xzf ccbg-lxc-package.tar.gz
sudo ./ccbg-lxc-package/scripts/install.sh --s3-only
sudo ./ccbg-lxc-package/scripts/install.sh --enable-smb-sidecar
```

预期：

- HTTP `200`
- `cache-control: public, max-age=300`
- `x-ccbg-release-source: r2`
- 安装页命令已指向 `/downloads/latest/...`
- 安装页同时展示 LXC `--s3-only` 与 `--enable-smb-sidecar` 两个 profile
- 首页和 `/install/` 的命令块带复制按钮

如果下载代理不是 `r2`，先检查：

1. 本轮 deploy 是否带了 `CCBG_CF_*_RELEASE_R2_BUCKET`
2. 对应 bucket 下是否存在 `latest/<asset-name>`
3. Worker deploy 输出里是否出现 `env.RELEASE_ASSETS (...)`

## 8. 回滚

如果 public 站点 HTML/FAQ 有问题，但 release 包没问题：

- 重新部署上一版 Worker + assets

如果 release 下载有问题：

- 先检查 R2 对象是否齐全
- 必要时重新运行：

```bash
scripts/sync-cloudflare-release-cache.sh ccbg-release-assets
```

- 如果 R2 绑定临时失效，GitHub release 资产仍可作为回源兜底

## 9. 发布后留档

至少记录这些内容：

- Git commit / tag
- GitHub release 资产列表
- test Worker version ID
- production Worker version ID
- test / production R2 bucket 名
- 下载代理 smoke 结果

推荐把结果补到：

- [ops-006-43-acceptance.md](ops-006-43-acceptance.md)
- [release-checklist.md](release-checklist.md)

如果本轮对中国移动大文件能力有任何新的正向表述，留档里必须附带对应的探测命令、绝对日期和原始结果，不接受“代码已修”“登录已通”“AList 可以”这类间接推断。

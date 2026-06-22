# CCBG Release Checklist

这份清单用于把当前版本从 `.43` 测试机推进到正式 release。

目标很简单：

- release 产物自洽
- `.43` 作为 release candidate 做一次全量验证
- 所有阻塞项清零后再开始正式 release

建议每次正式发布都复制一份，按实际结果勾选并留档。

当前最短执行路径见 [public-release-sop.md](public-release-sop.md)。

最近完成的正式发版：

- `v0.1.9` 于 2026-06-09 完成公网发布，release tag 指向
  `325946c50c5067108192c5653e4114f48eba932c`。
- `/downloads/latest/*` 已切到 Cloudflare R2 的 v0.1.9 资产，生产 Worker version ID 为
  `b48c3096-49a9-413b-9327-af5c36af9772`。
- GitHub Release 已包含 LXC / Windows / macOS x86_64 / macOS arm64 / OpenWrt lite
  五类宿主包、SHA256 sidecar、`ccbg-checksums.txt` 和 provenance 文件。
- 完整验收记录见 [ops-006-43-acceptance.md](ops-006-43-acceptance.md) 的
  `2026-06-09 v0.1.9 formal public release` 小节。

## 1. Release 产物

- [x] Git 工作区干净
- [x] 已有明确 release 候选提交：本地 `main` HEAD 或明确 release tag
- [x] `gatewayd` 与 Admin HTML 作为同一发布物交付
- [x] LXC 包包含 `assets/admin/index.html`
- [x] LXC 包安装脚本公开 `--s3-only` 与 `--enable-smb-sidecar` 两个 profile
- [x] `--enable-smb-sidecar` profile 会安装 SMB 依赖、启用 sidecar systemd units，并打开 Admin/control-plane SMB 开关
- [x] OpenWRT lite 包包含 `assets/admin/index.html`
- [x] 容器镜像构建文件已显式复制 Admin HTML
- [x] 已有本地 release 流程：`scripts/check-release-ready.sh` 与 `scripts/release-local.sh`
- [x] 所有 release 编译入口收敛到 `.47`
- [x] GitHub 默认只负责分支、tag、可选 Release 页面和发布记录；当前 macOS 发布资产由 GitHub Actions 在 self-hosted build-runner 容器中构建
- [x] Linux / OpenWrt / Windows / 容器 tar 或镜像二进制默认走 `.47` 或受控局域网 runner
- [x] 只打 tag 或只创建 GitHub Release 页面不算发版完成，必须让 `/downloads/latest/*` 指到新资产并通过 smoke
- [ ] 生成本次正式 release 的 provenance 文件
- [ ] 生成本次正式 release 的对外交付包与 SHA256
- [ ] Windows `x86_64` 原生包包含 `gatewayd.exe` 与 `assets/admin/index.html`
- [ ] macOS `x86_64` 原生包包含 `gatewayd` 与 `assets/admin/index.html`
- [ ] macOS `arm64` 原生包包含 `gatewayd` 与 `assets/admin/index.html`
- [x] LXC / Windows / macOS 官方宿主包默认把 Admin Web 打开到 `0.0.0.0:61081`
- [x] LXC/OpenWrt 默认 primary provider 不再使用 `stub`；未注入真实凭据前显示 unavailable
- [x] LXC 包构建会拒绝非 Linux ELF `gatewayd`，避免 Windows `gatewayd.exe` 被误打进 LXC 包
- [ ] Homebrew formula 模板已用正式 tag/SHA256 渲染
- [ ] winget manifest 模板已用正式 tag/SHA256 渲染

正式对外 release 必须至少包含下面这些公网安装页会引用的资产。所有资产由 `.47`
本地 release 脚本生成后按当前下载托管策略上传；macOS 为社区/实验交叉编译包。

- `ccbg-lxc-package.tar.gz`
- `ccbg-windows-x86_64.zip`
- `ccbg-macos-x86_64.tar.gz`
- `ccbg-macos-arm64.tar.gz`
- `ccbg-openwrt-lite.tar.gz`
- `ccbg-checksums.txt`
- `release-provenance.json`
- `release-provenance.md`

LXC 包验收必须确认:

- 包内 `etc/ccbg.env` 为 `CCBG_PRIMARY_PROVIDER=unicom`
- 包内 `bin/gatewayd` 是 Linux ELF，不是 Windows PE/`gatewayd.exe`
- `--enable-smb-sidecar` 安装后 `smbd` 监听 `0.0.0.0:445`
- Admin 默认密码登录返回 `401`

## 2. 本地质量门

- [ ] `scripts/check-release-ready.sh`
- [x] `cargo check -p gatewayd --quiet`
- [x] `cargo test -p gatewayd --quiet`
- [x] `cargo test -p browser-cdp --quiet`
- [x] `cargo test -p blob-core --quiet`
- [x] `git diff --check`
- [ ] 需要时补跑完整 workspace 级测试
- [x] `scripts/build-native-package.sh --skip-build --target x86_64-unknown-linux-gnu` 包结构 smoke 通过

## 3. `.43` 自动 smoke

测试机：`192.168.1.43`

- [x] `GET http://192.168.1.43:61080/healthz` 返回 `200 OK`
- [x] `GET http://192.168.1.43:61081/` 返回 `302 /login`
- [x] 已确认 `.43` Admin API 登录态可取到 `api/status`
- [x] `GET /api/showcase?limit=3` 返回 `200`
- [x] `POST /api/browser-cdp/open-endpoint-info` 返回 `200`
- [x] `POST /api/browser-cdp/probe` 返回 `200`

## 4. 公共站点 smoke

- [x] `scripts/deploy-cloudflare-public.sh test` 已部署测试站点
- [x] `scripts/deploy-cloudflare-public.sh production` 已部署生产站点
- [x] `https://carrier-disk-gateway.agi2030.online/` 首屏为深色安装优先入口
- [x] `https://carrier-disk-gateway.agi2030.online/faq/` 返回 `200`
- [x] `https://carrier-disk-gateway.agi2030.online/install/` 返回 `200`
- [x] `https://carrier-disk-gateway.agi2030.online/data/install-catalog.json` 返回 `200`
- [x] `https://carrier-disk-gateway.agi2030.online/api/faq/catalog` 返回 `200`
- [x] `https://carrier-disk-gateway.agi2030.online/api/faq/match` 返回 `200`
- [x] `https://carrier-disk-gateway.agi2030.online/downloads/latest/ccbg-lxc-package.tar.gz` 返回 `200`
- [x] `https://carrier-disk-gateway.agi2030.online/downloads/latest/ccbg-windows-x86_64.zip` 返回 `200`

截至 2026-06-02：

- production Worker 已绑定 `RELEASE_ASSETS=ccbg-release-assets`
- test Worker 已绑定 `RELEASE_ASSETS=ccbg-release-assets-test`
- 生产下载代理响应头已显示 `x-ccbg-release-source=r2`

### 4.1 安装页人工验收

- [ ] 首页顶部配色与 `llm-router.agi2030.online` 的深色运维风格一致
- [ ] 首页首屏可见 LXC/Linux 推荐安装命令
- [ ] 首页和 `/install/` 的安装命令旁有复制按钮
- [ ] `/install/` 可见 Windows zip + `install.ps1`、macOS 实验 tar.gz + `install.sh`、PVE LXC、Docker、Podman 命令
- [ ] `/install/` 可见 PVE LXC `S3 only` 与 `SMB sidecar` 两个安装选项
- [ ] OpenWrt / fnOS 明确标为实验宿主
- [ ] STM32 / ESP32-S3 明确标为嵌入式客户端示例，不进入可安装宿主列表
- [ ] 页面没有要求输入任何运营商、网关或 LLM 凭据

## 5. `.43` 全量人工验收

这一节是当前真正的 release gate。CLI 不替代浏览器人工确认。

### 5.1 顶部与基础导航

- [ ] 顶部第一行布局正确：站点标题、语言选择、生态项目展示栏同一行
- [ ] showcase 展示位内容可见且可点击
- [ ] 中文语言下关键页面不再残留明显英文文案
- [ ] Dashboard / Providers / 联通 / 电信 / 移动 / Browser-CDP / Logs 可正常切换

### 5.2 Browser / CDP

- [ ] CDP 端点列表可加载
- [ ] “打开识别页”会在目标浏览器打开端点识别页
- [ ] 识别页能明确显示 endpoint 信息和 port，避免用户连错浏览器
- [ ] Probe 结果与当前真实浏览器标签页一致

### 5.3 联通助手

- [ ] 切到 `联通` tab 时不再自动弹出引导浮窗
- [ ] 点击 `Open Guided Window` 可正常打开联通助手窗口
- [ ] 点击 `Start SMS Login + Validate` 后，CDP 浏览器会新开页并跳到联通登录页
- [ ] 已登录联通页面时可直接抓当前会话
- [ ] 联通助手主要文案为中文
- [ ] 发送短信验证码、继续提交验证码、抓取输出三条主链都可用

### 5.4 电信助手

- [ ] 电信助手窗口主要文案为中文
- [ ] 可进入账号登录 / 短信登录正确路径
- [ ] `Start SMS Login + Validate` 可驱动真实电信登录页
- [ ] 发送验证码、继续提交验证码、抓取当前会话可用

### 5.5 移动助手

- [ ] 移动助手窗口主要文案为中文
- [ ] `Start SMS Login` / 引导流程可驱动真实移动登录页
- [ ] 已登录页面时可直接抓当前会话
- [ ] 上传探针 / 捕获相关面板可正常使用
- [ ] 如果本轮 release 仍没有新的中国移动大文件放行证据，Admin / 文档 / 发布说明必须明确写出“.49 于 2026-06-05 的 16 GiB 隔离实测仍返回 `04010319 / Insufficient Rights`”
- [ ] 如果本轮 release 宣称中国移动大文件能力有提升，必须附带新的 limit-probe 或隔离实测证据，不能只凭登录成功、小文件成功或代码改动来表述

### 5.6 Provider / Health / Logs

- [ ] Provider Health 正常显示 personal / family 等 scope 信息
- [ ] Dashboard 能显示关键运行状态、告警和对象动作摘要
- [ ] Logs 页面可加载
- [ ] Logs 的级别 / 来源 / 搜索过滤可用
- [ ] AI 解释入口可打开统一 modal，FAQ 命中和复制 Prompt 可用

### 5.7 对象与控制面

- [ ] 对象动作入口可见
- [ ] shared history / object action history 可加载与导出
- [ ] 复制失败与 retry 路径可见
- [ ] 控制面中文化没有明显断裂

## 6. Release 决策门

只有在下面条件同时满足时才开始正式 release：

- [ ] `.43` 全量人工验收全部通过
- [ ] release 产物与 SHA256 已生成
- [ ] provenance 已生成
- [ ] macOS `x86_64` 与 `arm64` 社区/实验包已由 GitHub Actions self-hosted build-runner workflow 生成，并已下载合并回 `.47` release 目录
- [ ] 回滚包 / 上一个稳定版本可用
- [ ] 发布记录已落文档
- [ ] Windows/macOS 后台常驻路径至少经过对应真机 smoke
- [ ] 中国移动能力文案与本轮最新实测一致；没有新证据时，不把 `04010319 / Insufficient Rights` 的已知限制写成“已解决”

## 7. 回滚准备

- [ ] 保留上一版 `gatewayd` 二进制或完整安装包
- [ ] 保留上一版 Admin HTML
- [ ] 保留上一版配置与 control-plane 快照
- [ ] 明确正式发布失败时的回滚执行人和步骤

## 8. 当前结论

截至 2026-05-31：

- 自动化 smoke 已基本通过
- release 产物“一体化打包 Rust 程序 + Admin HTML”已落地
- 当前唯一明确阻塞项是 `.43` 上的全量人工验收尚未完成

截至 2026-06-01：

- GitHub Actions 不再承担通用 CI、Linux/Windows/OpenWrt 发版或 Cloudflare 部署
- macOS 发布资产走 GitHub Actions self-hosted build-runner 容器入口，不依赖 GitHub-hosted macOS runner
- Linux/Windows/OpenWrt release gate 与 Cloudflare 部署改为 `.47` 本地脚本；macOS 产物下载回 `.47` 后进入同一校验和 `/downloads/latest/*` 发布链路
- macOS 暂时降级为社区/实验包

# Windows / macOS 与公网安装页 rollout TODO

说明：本 TODO 用于把 `carrier-disk-gateway.agi2030.online` 调整为安装优先的公开入口，并把 Windows 纳入正式宿主支持；macOS 仍作为社区/实验包展示，但实际构建由 GitHub Actions 在 self-hosted build-runner 容器中完成。实现时继续遵守“组件可插拔、内存使用最小化”的项目原则：平台 catalog、安装命令、打包脚本和服务管理脚本都应可替换，不把供应商、平台或发布源写死到 Rust 核心里。

## 角色分工

| 角色 | 模型 | 产出 |
| --- | --- | --- |
| 规划 | `gpt-5.5` | 本 TODO 的可实施任务、验收标准和 release gate |
| 构建 | `gpt-5.3-codex` | 站点、脚本、CI、文档和包结构实现 |
| 验收 | `gpt-5.4` | 独立验收记录、阻塞项和 release 决策建议 |

当前执行约定：不用多代理工具；在现有供应商模型配置不可用或不需要时，由 Codex 直接完成实现与本地验收。

## WS-001: 公网站点安装优先改版

**优先级:** P0
**状态:** in-progress
**目标:** 首页首屏直接展示安装入口，页面配色与 `llm-router.agi2030.online` 的深色运维风格一致。
**Coding 指导:** 使用静态 HTML/CSS/JS；平台数据从 `public/cloudflare/data/install-catalog.json` 读取；JS 只负责渲染小型 catalog，不引入前端框架。
**验收方法:** 本地静态服务器打开 `/` 与 `/install/`；运行 `python3 scripts/check-cloudflare-public-fingerprint.py`。
**验收标准:** 首屏能看到 PVE LXC、Docker、Podman、Windows、macOS 安装命令；无一色紫蓝/浅色旧主题残留；移动端不溢出。
**依赖:** PKG-001
**不做事项:** 不在公开站点收集任何运营商、网关或 LLM 凭据。
**风险:** 发布资产还没实际上传时，命令必须明确指向 GitHub release latest，不伪造已发布版本号。

## WS-002: 安装 catalog 数据化

**优先级:** P0
**状态:** in-progress
**目标:** 新增 `install-catalog.json`，统一声明官方宿主、实验宿主和嵌入式客户端示例。
**Coding 指导:** catalog 包含平台 id、状态、架构、安装命令、包名、服务模式和验收命令；页面只消费 catalog，不复制平台矩阵。
**验收方法:** JSON 可由浏览器 `fetch` 读取；无敏感字段名；Cloudflare public boundary check 通过。
**验收标准:** 官方宿主包含 PVE LXC `x86/x64`、Docker `x86/x64`、Podman `x86/x64`、Windows `x86_64`；实验包含 fnOS、OpenWrt `arm64`、macOS `x86_64`、macOS `arm64`；STM32、ESP32-S3 只作为嵌入式客户端示例展示。
**依赖:** 无
**不做事项:** 不把 OneDrive 或运营商凭据样例写进 catalog。
**风险:** catalog 与 release 脚本漂移，需要 CI 或文档 checklist 兜底。

## PKG-001: 原生平台包结构

**优先级:** P0
**状态:** in-progress
**目标:** 新增通用原生打包脚本，生成包含 `gatewayd` 与 Admin HTML 的 Windows/macOS 发布包。
**Coding 指导:** 包内结构固定为 `bin/`、`assets/admin/`、`config/`、`deploy/`、`docs/`；支持 `--target` 与 `--skip-build`；不把 secrets 打进包。
**验收方法:** 使用本机 release binary 或 fake binary 执行 `scripts/build-native-package.sh --skip-build --target <triple>`。
**验收标准:** 产物包含 Rust 二进制、`assets/admin/index.html`、安装脚本、manifest 和 SHA256；Windows 目标支持 `.exe`。
**依赖:** 已完成的 Admin HTML 打包能力。
**不做事项:** 不在 Linux CI 中强行完成 Windows/macOS 交叉编译链安装。
**风险:** 目标平台运行时差异需要在真机验收中确认。

## PKG-002: macOS 后台常驻

**优先级:** P1
**状态:** in-progress
**目标:** 提供 macOS `launchd` 安装/卸载路径。
**Coding 指导:** 使用 `~/Library/LaunchAgents/online.agi2030.ccbg.gatewayd.plist` 作为用户级默认路径；日志写入 `~/Library/Logs/ccbg/`；配置位于 `~/Library/Application Support/ccbg/`。
**验收方法:** macOS 上执行安装脚本后 `launchctl print gui/$UID/online.agi2030.ccbg.gatewayd`。
**验收标准:** gatewayd 能后台启动；Admin HTML 从包内路径读取；卸载不会删除用户数据。
**依赖:** PKG-001
**不做事项:** 不默认申请系统级 root daemon。
**风险:** Gatekeeper/签名/公证后续需要 release 阶段补齐。

## PKG-003: Windows 后台常驻

**优先级:** P1
**状态:** in-progress
**目标:** 提供 Windows 原生后台常驻安装路径。
**Coding 指导:** 默认用 Scheduled Task 作为无额外依赖的 native resident path；保留服务名、安装目录和环境文件的显式参数。
**验收方法:** Windows PowerShell 执行安装脚本后 `Get-ScheduledTask CCBG-GatewayD` 与 `Invoke-WebRequest http://127.0.0.1:61080/healthz`。
**验收标准:** 登录后自动启动；卸载任务不删除用户数据；脚本不把密码/token 写到命令行。
**依赖:** PKG-001
**不做事项:** 不依赖 NSSM 或第三方服务 wrapper。
**风险:** 如果后续需要真正 Windows Service，需要在 Rust 进程中增加 service control handler。

## PKG-004: 包管理器模板

**优先级:** P1
**状态:** in-progress
**目标:** 为 Homebrew 和 winget 提供 repo-managed 模板，release 阶段用真实 tag 和 SHA256 渲染。
**Coding 指导:** 模板只放占位符，不提交真实 token；Homebrew 指向 macOS x86_64/arm64 tarball，winget 指向 Windows x86_64 zip。
**验收方法:** release checklist 中用本次 artifact SHA256 替换占位符并 dry-run lint。
**验收标准:** 模板包含包名、版本、URL、SHA256、license/provenance 边界；不声称已完成上架。
**依赖:** PKG-001
**不做事项:** 不在没有正式 artifact 前提交固定 SHA256。
**风险:** 包管理器规范升级时模板需要同步更新。

## CI-001: 发布和验收门

**优先级:** P1
**状态:** pending
**目标:** CI 至少覆盖 catalog、Cloudflare public fingerprint、原生打包脚本 smoke。
**Coding 指导:** 先做不依赖交叉编译工具链的 smoke；Windows/macOS 真机运行作为 release checklist gate。
**验收方法:** `.47` 本地 release gate 能跑通；macOS `x86_64` 与 `arm64` 社区/实验包由 GitHub Actions self-hosted build-runner workflow 生成，下载后通过 `CCBG_RELEASE_MACOS_ASSET_DIR` 合并回 `.47` release 目录。
**验收标准:** `git diff --check`、public fingerprint、license check、native packaging smoke 全部通过。
**依赖:** WS-002, PKG-001
**不做事项:** 不在 CI 保存真实签名证书或 GitHub release token。
**风险:** 没有真机 runner 时只能验证包结构，不能证明 launchd/Scheduled Task 运行。

## DOC-001: 平台矩阵和发布文档同步

**优先级:** P0
**状态:** in-progress
**目标:** 更新 compatibility matrix、GitHub publication 和 release checklist。
**Coding 指导:** 明确“官方宿主 / 实验宿主 / 嵌入式客户端示例”三类，不把 STM32/ESP32-S3 描述为完整宿主或可安装平台。
**验收方法:** 人工检查文档；release checklist 能直接指导 `.43` 与公网验收。
**验收标准:** Windows 被列为官方宿主；macOS、fnOS、OpenWrt 是实验；release 二进制必须包含 Rust 程序和 Admin HTML。
**依赖:** WS-001, PKG-001
**不做事项:** 不承诺未实现的包管理器上架已经完成。
**风险:** 公开文案过度承诺会影响 release 可信度。

## 验收命令

```bash
python3 scripts/check-cloudflare-public-fingerprint.py
python3 scripts/license-check.py --skip-cargo-metadata
git diff --check

mkdir -p target/release
printf '#!/bin/sh\nexit 0\n' > target/release/gatewayd
chmod +x target/release/gatewayd
scripts/build-native-package.sh --skip-build --target x86_64-unknown-linux-gnu
```

## Release Gate

- [ ] 首页和 `/install/` 安装入口通过人工浏览器验收
- [ ] Windows/macOS 包结构包含 `gatewayd` 与 Admin HTML
- [ ] LXC/OpenWrt/container 既有包结构未回归
- [ ] `.43` release candidate smoke 通过
- [ ] Windows 真机后台常驻路径通过
- [ ] macOS 真机 `launchd` 路径通过
- [ ] 生成 release provenance、SHA256 和回滚记录

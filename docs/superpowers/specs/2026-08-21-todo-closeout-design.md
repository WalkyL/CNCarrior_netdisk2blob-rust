# CCBG 仓库内待办收口设计

## 背景

当前 CCBG 的主功能 TODO 清单仅剩 `.43` 测试机发布与人工验收项，但平台 rollout 和 release checklist 仍混合记录了四类不同性质的工作：仓库内可以自动完成的检查、需要生成发布材料的本地步骤、需要真实设备或浏览器的人工验收，以及需要外部凭据或发布权限的运维动作。部分 README、历史 CDP 记录和 release 说明也已落后于当前代码。

本设计的目标是把仓库内可验证的事项一次性收口，并把不能由本地仓库证明的事项明确保留为外部阻塞，不用静态检查或模拟结果替代真实验收。

## 范围与边界

纳入范围：

- 对照当前源码、配置、脚本和历史提交，修正文档中的能力状态和历史描述。
- 修复在本地可复现的 release/check 脚本缺口；新增测试时遵循 RED -> GREEN -> REFACTOR。
- 执行 workspace Rust 测试、格式检查、文档/目录/license/public-boundary 检查、native package smoke 和 release asset merge 检查。
- 在 CCBG 专用临时目录生成需要验证的 package、provenance、checksum 和 manifest，并在验证后删除临时产物。
- 只在本地创建分组提交；保留 `.codegraph`，不推送 `origin/main`。

不纳入范围：

- 不读取、猜测或写入运营商凭据，不替 `.49` 执行重新登录或改变线上 topology。
- 不把 `.43` 的浏览器人工验收、Windows/macOS 真机后台运行、macOS 签名/公证视为已完成。
- 不执行公网发布、GitHub Release 上传、Cloudflare 生产发布或包管理器上架。
- 不触碰其他项目、共享 Cargo 缓存或与 CCBG 无关的工作区。

## 设计方案

### 1. 状态事实层

以源码和当前脚本为准，逐项检查以下记录：

- `README.md` 中关于电信对象动作、S3 multipart/presigned 能力和 provider 真实回归的表述。
- `docs/todo-20260711.md` 中遗留的 CDP keepalive 24 小时记录；当前实现已经使用 credential lease probe，历史记录应明确标记为 superseded，而不是继续作为活动待办。
- `docs/lessons-learned.md` 中关于 lease probing、primary fencing 和 CDP keepalive 的历史结论，区分当时观察和当前实现。
- `docs/release-checklist.md` 与 `docs/windows-macos-public-site-rollout-todo.md` 的自动化、本地材料、人工验收和外部发布权限四类条目。

文档只会把有新证据的本地事项标为完成；真实运营商会话、真机和浏览器条目继续保持未完成，并在阻塞说明中写清所需证据。

### 2. 本地质量门与发布材料层

优先复用仓库已有脚本，不新建第二套 release 流程：

- 运行 `cargo fmt --all -- --check`、`cargo check --locked` 和 `cargo test --workspace`，低磁盘场景把 target 定向到 CCBG 专用临时目录。
- 运行 catalog、license、Cloudflare public fingerprint、native package、release asset merge 和相关脚本测试。
- 使用 fake binary 或现有 release binary 在 CCBG 临时目录构建 Windows/macOS package structure smoke；不宣称这些包通过真机运行。
- 使用现有 provenance/checksum 脚本生成并校验临时 release metadata；验证完成后删除临时目录，不把未发布资产伪装成正式 release。
- 如果某个检查失败，先定位是环境、脚本还是源码问题；只有源码/脚本缺陷才进入 TDD 修复，外部依赖失败记录为阻塞。

### 3. 工作区与提交层

- 修改前后检查 `git status`、当前分支和 `origin/main` 差异，保留已有 `ce08d4b` 与 `.codegraph`。
- 文档同步、脚本/测试修复、清单状态更新按逻辑分组提交，提交前运行 `git diff --check` 和对应验证命令。
- 不执行 push；最终报告本地 commit、未提交变更、清理的临时路径和仍需外部动作的清单。

## 验收标准

仓库内收口满足以下条件：

1. 当前源码能力与 README、TODO、release checklist 的描述一致；历史 CDP 记录不会再被误读为当前实现。
2. 所有可在本机完成的自动检查均有新鲜命令输出；失败项有明确根因，不能用“代码看起来正确”替代。
3. workspace 测试和新增/修复脚本测试通过，且 CCBG 临时 build output 已清理。
4. `.codegraph` 保留、其他项目未改动、工作区状态可审计。
5. `.49` 凭据刷新、`.43` 浏览器人工验收、Windows/macOS 真机、签名/公证和正式发布仍按真实证据单独列出。

## 风险与处理

- 真实 provider session 可能继续过期；本地验证只证明代码路径和当前观测，不能替代重新认证。
- native package structure smoke 不证明 Scheduled Task 或 launchd 在真实系统启动；发布清单必须保留真机 gate。
- release 脚本可能产生较大的 Rust target 或压缩包；统一使用 CCBG 专用临时路径，验证后确认路径不存在。
- 文档历史记录可能与多个时期的实现交错；更新时保留时间上下文，不覆盖历史事实。

# 架构说明

## 设计原则

1. 三家运营商的网页接口都可能变化，因此每个 provider 都必须独立成 crate。
2. 任意时刻只允许一个运营商 provider 作为唯一写入主云盘。
3. OneDrive 是默认异步备份同步目标与可选 fallback 目标，不作为主写 provider。
4. 写入语义采用“主写成功即返回，后台异步复制到同步目标集合”，系统接受最终一致性。
5. fallback 只能在对象已复制到对应同步目标时触发，不能假定备份侧永远有数据。
6. 认证信息只允许通过受控输入进入服务；如后续启用自动抓取，也必须经由独立 `auth-broker` / sidecar，而不是直接塞进 data plane。
7. 一期完整宿主优先覆盖 PVE LXC `x86/x64`、Docker `x86/x64`、Podman `x86/x64` 和 OpenWRT `arm64`。
8. STM32 在一期按客户端兼容处理，而不是网关宿主。
9. 源码一期按公开仓库交付，但商业核心、公开材料和个人非商业源码审查边界必须分开，仓库必须具备基础许可证、CI 和容器构建入口。
10. 所有正式 provider 都必须最终达到流式读与流式写；若上游要求预哈希或预分片，只允许使用有界磁盘 spool，不允许整对象内存缓冲。
11. provider 页面改版时，优先改 catalog / probe / flow / action executor 等可插拔层，不能因为页面细节变化就重写整个 Rust 数据面。
12. 对象动作执行器、客户端 bucket 派生策略、桶级加密写入策略都必须保持可插拔，不能写死为某一家云盘或某一种动作流程。

## 系统分层

### Data Plane

- 统一对 Agent 暴露本地 S3 兼容 HTTP API
- 处理读写、下载、元数据查询
- 负责 primary provider 选择、sync target 选择、fallback 判定和结果返回

### Control Plane

- 管理 provider 配置、连接状态、令牌状态、复制策略
- 提供管理接口、授权回调、告警输出
- 维护对象复制状态与后端健康状态
- 为未来 `auth-broker` 提供 LLM endpoint / model 配置，以及交互式认证输入队列

### Auth Capture Broker

- 可与 `gatewayd` 同机部署，也可独立部署到另一台 `x86/x64` 主机
- 负责无头浏览器、网页会话抓取、短信验证码等待、手机号输入和必要的 LLM 辅助分析
- 通过受控 API 把“需要手机号 / 短信码 / 验证码”的提示推送到 Admin Web，而不是在后台静默卡住
- 应优先消费 `config/browser-flows/*.json` 这类 provider-specific 流程配置，而不是把 selector 和页面动作写死在执行器代码里
- `gatewayd` / Admin Web / auth session 与 provider-specific flow id、surface、runtime 映射之间的绑定，应优先消费 `config/provider-bridges/*.json`，而不是在 Rust 或内联 JS 里再复制一份条件分支
- CDP / 浏览器流程的职责仅限于取证、抓取登录态、验证页面事实、采集真实请求形状与必要运行时参数；它不是正式数据面的长期依赖
- 任何进入正式交付范围的读写、分片、提交、删除、复制、重命名能力，都必须回落到 `gatewayd` / provider crate 内独立执行，不能要求某个 CDP 页面一直开着
- 页面改版时，首选修 catalog / flow / capability / probe，不应把整个 provider crate 或 gateway 数据面当作页面脚本重写
- 如果某条能力只有“浏览器页面保持在线时才能完成”，它仍然只算 probe / discovery，不算 provider 已完成
- 对小内存设备建议只跑 `gateway-lite`，把这层放到另一台设备

### Admin UX

- `Web UI`: 适合 OneDrive 授权与日常运维
- `TUI`: 适合 SSH、软路由、容器内诊断与应急配置

### Agent Integration

- `MCP Server`: 给 Hermes / Open Claw / 其他 Agent 提供标准化工具接口
- `Skill`: 给 Agent 提供任务选择、调用约束、fallback 语义和最佳实践

### Deployment Profiles

- `full-host`: PVE LXC `x86/x64`、Docker `x86/x64`、Podman `x86/x64`
- `lite-host`: OpenWRT `arm64`
- `split-host`: `gateway-lite` 跑在 OpenWRT / 软路由，`auth-broker` 跑在另一台 `x86/x64`
- 嵌入式客户端示例: STM32 / ESP32-S3

## 当前模块与规划模块

### 当前已存在

### `blob-core`

- 定义容器、对象、健康状态、错误模型
- 定义 `BlobBackend` trait
- 为 HTTP 层与上游适配层提供稳定边界
- 定义浏览器流程执行的通用抽象，例如 `BrowserFlowSession`，让真实浏览器 transport 可以独立实现

### `browser-cdp`

- 基于 Chrome DevTools Protocol 提供标准浏览器 transport 适配层
- 负责可配置的 CDP endpoint、target 选择、websocket 会话和基础页面动作
- 当前已经支持把 `blob-core::BrowserFlowSessionExecutor` 接到真实 page target，会按浏览器流程配置执行 `navigate/click/set_input/set_files/wait_for_request/wait_for_page`
- 目标是兼容任何支持 CDP 的浏览器，而不是绑定 Edge/Chrome 的某个私有集成

### `provider-unicom` / `provider-telecom` / `provider-mobile`

- 分别封装联通、电信、移动云盘网页接口访问逻辑
- 管理 token 来源、请求头拼装、错误归类
- 联通当前已支持目录遍历、文件下载、上传、对象删除，以及对象级 rename/copy/move，并把 personal/family scope 映射成 `root` / `family` 容器；电信当前已支持 personal 目录遍历、流式下载、受控根目录 multipart 上传、回收站软删除，并可在配置 Family ID + Access Token 后把家庭云映射成只读写删中的读/删 `family` 容器；移动当前已支持 native `file/list`、`file/create`、`file/complete`、`file/getDownloadUrl` 驱动下的对象列举、真实上传与真实下载，且对象动作已覆盖 native `delete/rename/move` 与受能力开关约束的 `copy`；其中上传链路已按上游约束改成“`file/create` 首批最多 100 个 `partInfos`，剩余分片再通过 `file/getUploadUrl` 补齐”，并兼容同名旧对象导致的 `exist=true` / `uploadId=null` 返回，会先删除受控根目录下的同名旧对象并短暂重试 `file/create`；但 `.49` 在 2026-06-05 的 16 GiB 隔离实测仍返回 `04010319 / Insufficient Rights`，因此超大文件能力仍须按最新实测保守表述
- 后续继续扩展写入、分片上传、断点续传
- provider 正式写路径必须使用程序内稳定实现，不得把“CDP 页面里点击上传”当成最终交付能力本身
- provider 正式读写路径最终都必须流式化；若上游需要显式内容哈希或预分片，可使用有界 spool，但不能把整对象先收进内存
- 对于任何支持真实写入的 provider，网关写入的真实对象都必须收口到 provider 内一个受控根目录下，再在其下展开 bucket/key 语义；不能把几千个散碎对象直接撒到云盘根目录
- 对于来自真实浏览器/CDP 的页面元素、流程和请求形状，优先沉淀到浏览器流程配置层，而不是把页面细节直接硬编码到 provider crate
- 对于 provider-specific 的 surface 名称、flow alias、logged-in probe、runtime→credential 映射，优先沉淀到 `config/provider-bridges/*.json`，让控制面和 auth session 逻辑复用同一套绑定
- 对于已经稳定下来的 native dispatcher 动作，优先把操作名、默认字段和值约束沉淀到 `config/provider-capabilities/*.json`，让 provider crate 只保留执行器、鉴权、加密和错误归类
- 对象动作、客户端 bucket 派生、桶级加密写入等策略也应遵循同样的可插拔边界，优先通过 catalog / executor / policy 扩展，而不是把某种流程写死在 `gatewayd`
- 对于账号发现、作用域发现、下载/上传/写路径确认这类“后续要自动探测”的目标，优先沉淀到 `config/provider-probes/*.json`，让控制面和 sidecar 后续直接消费同一份探测清单
- provider 真实 I/O 表现必须进入网关可观测面：至少沉淀首次响应时间、最近读/写延迟、滚动吞吐与最近一次大流量传输均速，供后续读写路由策略消费

### `gatewayd`

- 对外暴露本地 S3 兼容数据面服务
- 根据配置选择具体 provider，并将 REST 请求转换为 `BlobBackend` 调用
- 通过 `policy-engine` 校验唯一主写与 sync target 拓扑
- 在对象写入和删除成功后向 `replication-engine` 写入复制任务
- 负责汇总 provider 级 I/O 观测结果，并通过 `GET /api/status` / `GET /metrics` 暴露给控制面与后续路由策略
- 已内嵌最小控制面，提供 Admin HTML 首页、`GET /api/status`、`GET /api/auth/onedrive/status`、`GET /api/auth/onedrive/web/start`、`POST /api/auth/onedrive/device/start`、`GET /api/auth/onedrive/device/{flow_id}` 和 `GET /auth/onedrive/callback`
- 已支持 `POST /api/object-actions`，并已在 Admin Web 暴露对象动作面板，可把对象级 `rename/copy/move` 动作直接下发给当前主 provider，并在成功后补齐对应的异步复制元数据：`rename=put(new)+delete(old)`、`copy=put(dest)`、`move=put(dest)+delete(src)`
- Admin Web 对象动作面板现在会展示 before/after 对象检查结果，并把最近共享执行历史持久化到 control-plane 文件；`POST /api/object-actions/history/clear` 用于清空这份服务端共享历史
- Admin Web 现在也直接展示运行态 `runtime` 摘要，包括 uptime、监听地址、复制 worker 数量、control-plane / metadata 路径等，并支持自动刷新控制，已具备基础运行监控面板能力
- `GET /api/status` 现已额外返回 `monitoring` 聚合摘要，并在 Admin Web 里展示 open alerts、provider 健康计数、复制失败数、对象动作失败数、最近对象动作时间和最近失败事件列表，便于日常值守时先看摘要再下钻原始表格
- `GET /api/status` 现已额外返回 `notify` 状态摘要；若配置 `CCBG_NOTIFY_WEBHOOK_URL`，后台会按固定间隔评估 alerts，并仅在告警状态变化时向外发送 webhook
- webhook 接收端的验签 / 时间窗 / 去重参考实现单独放在 [docs/notify-webhook-reference.md](/home/walky/carrier-cloud-blob-gateway/docs/notify-webhook-reference.md:1) 和 [scripts/notify-webhook-receiver-example.py](/home/walky/carrier-cloud-blob-gateway/scripts/notify-webhook-receiver-example.py:1)
- 对象动作共享历史现在支持 `operator` / `ticket` / `notes` 审计字段，以及 operator / object / time-window 过滤与 CSV 导出
- 这部分控制面的详细运维说明单独放在 [docs/object-actions-and-history.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-and-history.md:1)，避免把操作细节挤进架构文档
- 这部分 API 契约单独放在 [docs/object-actions-api-reference.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-api-reference.md:1)，避免把字段细节塞进架构说明
- 如果要把联通作为正式 primary provider 上线，最后再走 [docs/unicom-go-live-checklist.md](/home/walky/carrier-cloud-blob-gateway/docs/unicom-go-live-checklist.md:1)
- 已支持把运行中的主写 provider / sync targets / fallback 顺序保存到本地 control-plane 文件，并热更新到数据面
- 已支持在网页中直接修改 OneDrive 的 async backup / fallback 开关，以及 `memory_only` 作用域
- 已支持在网页中直接修改 `auth-capture` sidecar 地址、LLM endpoint / model，以及 provider 独立凭证
- 已支持交互式“验证输入队列”占位能力，后续可供手机号 / 短信码 / 验证码输入
- 已支持 `POST /api/browser-flows/session-run`，可用控制面默认 CDP 配置或请求级覆盖配置直接执行一条浏览器 flow
- 当 flow 缺少必填人工输入时，`session-run` 现在会先落一条 `auth_session_id` 会话、自动创建 prompts，并返回 `awaiting_input`，而不是立即失败
- 已支持 `GET /api/browser-flows/session/{session_id}` 轮询这条人工认证会话的状态、prompts、last_error 和最近 execution report
- 已支持在 flow 成功执行后把 `script_value` / `dom_text` / `url` 类型的 browser-flow outputs 回填到该 auth session 的 `runtime`，供后续复用同一 `auth_session_id` 的 flow 直接消费登录态
- `POST /api/browser-flows/session-run` 的返回值当前也会直接带回这次会话最新的 `runtime`，方便控制面或 auth-broker 读取页面事实，而不用再写 provider-specific API
- 已支持按 `prerequisite_flow_id` 递归执行浏览器 flow 前置链，便于把“当前会话抓取 -> uploader 准备 -> 实际上传”这类页面内依赖收口到同一条 auth session
- Admin Web 已从 Rust 内联模板抽出到 `crates/gatewayd/assets/admin/index.html`；后续 FAQ 匹配、AI 解释弹窗、运营商引导文案、多语言渲染和大部分交互逻辑都应继续留在前端资产层，而不是再回灌到 Rust
- Admin 动态面板的本地化需要在每次重渲染后重新执行；优先使用 `tr(...)` 和 exact-text 映射，不要在 Rust 里复制 UI 文案分支
- 输出健康检查、日志、后续指标和审计信息

### `provider-onedrive`

- 封装 OneDrive 授权后访问能力的 provider
- 保留为 Parking optional provider；默认不作为异步备份目标或 fallback 目标
- 当前已落 Graph 映射，支持 `root_prefix/<bucket>/<key>` 路径映射、健康检查、列容器、列对象、对象读写删，以及对象级 rename/copy/move
- 当前已支持显式 access token、token file、OAuth session file，以及 access token 过期后使用 refresh token 自动续期并回写 session
- 对象动作执行层已做成可替换结构；当前默认执行器走 Graph `PATCH` 更新 `name` / `parentReference` 完成 rename/move，并走 Graph async copy + monitor URL 轮询完成 copy
- 可直接复用 `gatewayd` 内建的 Web PKCE / Device Code 授权链路
- 后续再补分片上传、更稳健的 delta/sync 与更完整的运维面

### `policy-engine`

- 统一表达 `primary provider`、`sync targets`、`fallback read order`
- 校验主写唯一、fallback 子集、parking provider 不进入默认拓扑等约束
- 为 `gatewayd` 和后续控制面提供稳定的拓扑判定边界

### `replication-engine`

- 提供异步复制任务的本地队列骨架
- 当前实现内存队列、最近任务记录、对象级 `put/delete` 入队、启动时 pending job 恢复，以及 `retry_scheduled` 延迟重试出队
- 复制 job 需要绑定创建当时的 `source provider`，避免主写热切换后旧 job 串读到新的主写后端
- `gatewayd` 已启动后台 worker 消费任务，并把结果回写 SQLite
- 当前已支持基础重试 / 退避、按 latest object state 统计的 target 级状态汇总，以及 latest failed job 的控制面人工重试；后续再补更完整的死信和批量人工重试入口

### `metadata-store`

- 基于 SQLite 持久化复制任务
- 提供 pending job 恢复、状态更新、`next_attempt_at` 记录和 per-target 复制摘要查询
- 当前已用于 `gatewayd` 内部状态接口、worker 状态落盘、对象级复制视图，以及关键应用远端 WAL 的本地高水位状态

## 关键应用写前日志

- 对被显式标记为关键的应用，`gatewayd` 会在主写前把一条很小的远端 WAL 记录写到第三个 provider 的专用目录
- 控制面备份继续承担 checkpoint 角色；恢复时先恢复 checkpoint，再回放 `checkpoint_lsn` 之后的远端 WAL
- 如果 home provider 上真实对象存在，则恢复器重建 placement / logical object / protection plan，并补回缺失复制任务
- 如果 home provider 上真实对象不存在，则恢复器按 WAL 中记录的旧状态回滚本地残留元数据
- 这套能力的目标是降低关键应用控制面恢复 `RPO`，而不是替代对象数据迁移或对象字节归档
- 详细设计见 [docs/gateway-write-ahead-log.md](/home/walky/workspaces/carrier-cloud-blob-gateway/docs/gateway-write-ahead-log.md:1) 和 [docs/decisions/ADR-003-remote-write-ahead-log-for-critical-apps.md](/home/walky/workspaces/carrier-cloud-blob-gateway/docs/decisions/ADR-003-remote-write-ahead-log-for-critical-apps.md:1)

### `gatewayd` 内嵌 `auth-broker`

- 提供 OneDrive 授权编排
- 统一维护 PKCE state、Device Code flow 状态和共享 HTTP client
- Web 模式下监听独立回调端口，完成 code exchange 后把 session 落盘
- Terminal 模式下启动 Device Code flow 并后台轮询，成功后把 session 落盘
- 当前实现为内嵌最小控制面，后续如复杂度继续上升再拆分独立 crate
- 对运营商网页登录，不直接放进 data plane；后续统一走独立 `auth-broker` / sidecar

### `config/browser-flows`

- 存放 provider 网页流程的声明式配置，例如 selector、步骤、JS 入口点和关键请求
- 当前联通桌面站样例已覆盖短信登录和个人空间上传
- 这层是为了隔离网页改版风险，供 `auth-capture` sidecar / CDP 执行层消费
- schema 当前由 `blob-core::BrowserFlowCatalog` 定义

### `config/provider-bridges`

- 存放 provider 与控制面 / auth session 之间的桥接元数据，例如 `surface`、flow alias、runtime→credential 映射、browser profile runtime key、logged-in probe 绑定
- 这层是为了让页面 flow id 或控制面字段改动优先落在 JSON，而不是重写 `gatewayd` 的 Rust / 内联 JS
- schema 当前由 `blob-core::ProviderBridgeCatalog` 定义

### 后续扩展模块

#### `admin-ui-web` / `admin-ui-tui`

- 当前 `gatewayd` 已提供最小 HTML 页面和 JSON auth/status 接口
- 当前 Web 页已能查看 provider 健康、复制队列、失败 job 告警和热切换后的运行拓扑
- 后续补更细粒度的对象状态、provider 测试和专用 TUI
- 两种界面共用同一套控制平面逻辑

#### `mcp-server`

- 将核心网关能力封装为 MCP tools / resources / prompts
- 默认优先支持 stdio transport
- 可选支持 Streamable HTTP transport 供远程或多客户端接入

#### `skills/carrier-cloud-blob-gateway`

- 作为面向 Codex 类 Agent 的 Skill 封装
- 指导 Agent 何时使用 MCP、何时直连本地 S3 API
- 向 Agent 明确异步复制与 fallback 的语义限制

## 请求流

```text
Agent -> gatewayd -> policy-engine -> primary provider -> carrier cloud
                               \-> metadata-store
                               \-> replication-engine -> sync targets (carrier mirrors + onedrive)
Agent -> mcp-server -> gatewayd
Agent -> Skill -> mcp-server / gatewayd
```

## S3 兼容策略

- 数据面目标兼容 S3 API，而不是自定义对象接口
- 控制面、管理界面、健康检查与授权回调继续走独立端口与路径
- 一期优先支持 path-style bucket addressing
- 一期优先支持 Agent 常用的最小 S3 子集，而不是完整 AWS S3 全量行为

## 认证策略

当前已支持以下方式:

- 运营商云盘:
  - 环境变量直接注入 token
  - 本地文件注入 token
  - 明确配置 Cookie 头
  - provider 级 `auto|ipv4|ipv6` 出站策略
- OneDrive:
  - Web UI 走 Authorization Code + PKCE
  - TUI 走 Device Code Flow
  - 环境变量直接注入 access token
  - 本地文件注入 access token
  - 本地 session file 注入 OAuth 会话并自动 refresh
- Auth Capture:
- 当前先保存 broker URL、LLM endpoint、LLM model
- 当前也保存 CDP endpoint、target selector 和 target timeout 这类 transport 配置，用来把 browser flow 执行器收口到标准 CDP，而不是绑定某个具体浏览器
- 当前 `gatewayd` 已能直接消费这组 CDP 配置执行最小 flow；后续独立 `auth-broker` 主要补的是短信码/验证码/人工确认/失败恢复编排，而不是另起一套浏览器 transport
- 当前已预留验证输入队列，用于手机号 / 短信码 / 验证码回显
- 后续再把无头浏览器和页面分析独立接入

不纳入自动化范围的方式:

- 从浏览器 profile 直接读取登录态
- 抓取浏览器进程流量
- 自动代理网页登录凭据

## 网络出口策略

- provider 需要支持独立的 `auto | ipv4 | ipv6` 出站策略，不能假设所有网页会话都能跨 IPv4/IPv6 复用。
- 当前已确认天翼云盘网页会话可能绑定出口 IP；若浏览器抓取发生在 IPv4，会话复用时应优先强制 `ipv4`。
- 这一策略必须保持在 provider 配置层，而不是写死在全局网络层，避免影响其他 provider。

## 存储域与容量视图

- 当前 `blob-core::ServiceHealth` 已把各 provider 的“个人空间 / 家庭空间 / 共享空间”抽象成统一 `storage-scope` 视图。
- 当前 Admin Web 已按 provider 渲染 scope 卡片，而不是只输出 JSON。
- 每个 scope 至少需要暴露:
  - scope 类型: `personal | family | shared | unknown`
  - 可写性
  - 总容量、已用容量、剩余容量
  - 关联的根目录 / 容器映射
- 对联通、电信、移动三家 provider，容量接口和 personal/family 入口不同，因此这一层必须放在 provider 适配层内完成。
- 当前已落地的 provider-specific 探测:
  - 联通: `QueryAllFiles` personal root，`AppQueryUser` 容量解析，`QueryFamilyGroups` 自动发现 familyId，family root 走 `spaceType=1`
  - 电信: `listFiles.action` personal root，`getUserInfoForPortal.action` 容量解析；配置 Family ID + Access Token 后通过 `/open/family/file/listFiles.action` 探测 family root
  - 移动: 仍为骨架，scope 先报告为 `personal` + `capacity unknown`

## 历史对象收敛与本地缓冲

- 内容策略变化默认只影响新写入和后续覆盖写；历史对象不会自动补副本、删旧副本、迁移 home provider、加密重写或解密重写。
- 历史对象相关变化必须走“先预览、再显式执行”的路径，不能在后台静默搬迁。
- 任何显式收敛都必须同时检查三类空间:
  - 目标 provider 的最终所需空间
  - 目标 provider 的峰值所需空间
  - 本地 spool 卷的峰值所需空间
- 如果目标 provider 容量未知，默认阻断；如果本地 spool 预算未知或不足，也默认阻断。
- 同 provider、同 key 的加密 / 解密 / profile 切换必须走临时对象两阶段路径，不能原地覆盖。
- 对这组规则的完整说明单独放在 [docs/historical-object-reconcile-and-buffer-budget.md](./historical-object-reconcile-and-buffer-budget.md)。

## 数据语义

### 写入

1. 客户端写入唯一主 provider
2. 主写成功后立即向客户端返回成功
3. 生成针对每个 sync target 的复制任务写入本地队列，并把当时的 `source provider` 一起写入 job
4. 后台异步复制到目标集合
5. 更新每个 target 的复制状态
6. provider 侧真实对象必须落在该 provider 的单一受控根目录下，再在其下映射 bucket/key；不能直接散落到用户云盘根目录

### 读取

1. 优先从主 provider 读取
2. 当主 provider 熔断或健康异常时按 fallback 读取顺序检查目标集合
3. 仅当对象已复制到目标 provider 时才允许切换读取
4. OneDrive 可作为最终 fallback 目标，但是否启用由用户控制
5. 当前 S3 数据面会用 `x-ccbg-source-provider` / `x-ccbg-fallback-from` 显式标记实际读取来源
6. 如果对象对所有目标都仍处于 `pending` 或 `failed`，返回明确错误并发出提醒

### 删除

1. 主后端删除成功
2. 对所有 sync targets 生成异步删除传播任务
3. 全部目标删除完成后再把对象状态更新为已清理

## 同步拓扑

- `primary provider`: 唯一写入主云盘
- `sync targets`: 零个或多个异步同步目标
- `parking provider`: OneDrive 当前保留实现但默认不进入 sync/fallback

约束:

1. `primary provider` 只能有一个。
2. `sync targets` 不应包含当前 primary provider。
3. 如果用户配置了多个运营商账号，其他账号只能作为异步镜像目标参与同步，不能并发主写。
4. `fallback read order` 由用户显式控制；留空表示只做异步同步，不做 fallback 读取。

主写切换策略:

1. 用户可在 Web 控制面直接切换新的 `primary provider`
2. 新写入请求应立即使用新的主写 provider
3. 已经入队的旧复制 job 必须继续读取它们各自记录的旧 `source provider`
4. 已经入队的旧 job 不应因为热切主而丢失、重定向或串读到新的主写后端

## 端口策略

- 所有内部服务端口统一位于 `60000-65534`
- 默认 S3 API 为 `61080`
- 默认 Admin Web UI 为 `61081`
- 默认 OneDrive OAuth 本地回调为 `61082`
- 默认 Metrics / 扩展健康检查为 `61083`，当前已提供 `/healthz`、`/readyz`、`/metrics`
- 可选 MCP Streamable HTTP 为 `61084`

## Agent 交付策略

### MCP

- 本地 Agent 首选 stdio transport
- 远程或多客户端场景可选 Streamable HTTP
- MCP 层只做受控能力暴露，不绕过策略层直接访问 provider
- MCP 工具必须支持读取和修改 primary provider / sync targets 配置

### Skill

- Skill 只负责“什么时候、怎么用”这个网关
- Skill 需要显式告知:
  - fallback 不是强保证
  - 异步复制意味着存在备份延迟
  - 系统同一时刻只允许一个写入主云盘
  - 对高价值对象应先确认复制状态再依赖 OneDrive 读取

## 建议的后续实现顺序

1. 先做 `provider-onedrive`、`metadata-store`、`policy-engine` 的最小骨架
2. 在已落地的 OneDrive / metadata / policy 基础上，选定一家具备最好验证条件的运营商先打通读写
3. 完成主写 + 异步复制 + 受控 fallback 的闭环
4. 在已落地的内建 admin/auth 控制面基础上，补更完整的 dashboard 与 TUI
5. 增加 `mcp-server` 的 stdio 版本
6. 再补 Skill 封装与更多 provider / 更完整的 Blob 语义

## 部署边界

- Ubuntu 本机阶段: 先用 systemd 用户服务或前台运行
- PVE/LXC 阶段: 用单进程容器部署，挂载只读配置、数据库目录和独立 secrets 文件
- provider 认证覆盖建议挂载独立目录，并通过 `CCBG_CREDENTIALS_DIR` 指向，例如 `unicom.json`、`telecom.json`、`mobile.json`、`onedrive.json`
- Docker / Podman 阶段: 使用仓库内标准构建文件
- 软路由阶段: 优先保证低内存占用、可观测性和断线恢复
- 不建议在容器镜像中固化 token 或 refresh token

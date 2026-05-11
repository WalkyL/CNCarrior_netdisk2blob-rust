# 架构说明

## 设计原则

1. 三家运营商的网页接口都可能变化，因此每个 provider 都必须独立成 crate。
2. 任意时刻只允许一个运营商云盘作为唯一写入主云盘。
3. OneDrive 是默认备份同步目标，其他运营商账号可按配置加入异步同步目标集合。
4. 写入语义采用“主写成功即返回，后台异步复制到同步目标集合”，系统接受最终一致性。
5. fallback 只能在对象已复制到对应同步目标时触发，不能假定备份侧永远有数据。
6. 认证信息只允许通过受控输入进入服务；如后续启用自动抓取，也必须经由独立 `auth-broker` / sidecar，而不是直接塞进 data plane。
7. 一期完整宿主优先覆盖 PVE LXC `x86/x64`、Docker `x86/x64`、Podman `x86/x64` 和 OpenWRT `arm64`。
8. STM32 在一期按客户端兼容处理，而不是网关宿主。
9. 源码一期按公开 GitHub 仓库交付，仓库必须具备基础许可证、CI 和容器构建入口。

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
- `client-only`: STM32

## 当前模块与规划模块

### 当前已存在

### `blob-core`

- 定义容器、对象、健康状态、错误模型
- 定义 `BlobBackend` trait
- 为 HTTP 层与上游适配层提供稳定边界

### `provider-unicom` / `provider-telecom` / `provider-mobile`

- 分别封装联通、电信、移动云盘网页接口访问逻辑
- 管理 token 来源、请求头拼装、错误归类
- 联通 / 电信当前已支持目录遍历与文件下载
- 后续继续扩展写入、分片上传、断点续传

### `gatewayd`

- 对外暴露本地 S3 兼容数据面服务
- 根据配置选择具体 provider，并将 REST 请求转换为 `BlobBackend` 调用
- 通过 `policy-engine` 校验唯一主写与 sync target 拓扑
- 在对象写入和删除成功后向 `replication-engine` 写入复制任务
- 已内嵌最小控制面，提供 Admin HTML 首页、`GET /api/status`、`GET /api/auth/onedrive/status`、`GET /api/auth/onedrive/web/start`、`POST /api/auth/onedrive/device/start`、`GET /api/auth/onedrive/device/{flow_id}` 和 `GET /auth/onedrive/callback`
- 已支持把运行中的主写 provider / sync targets / fallback 顺序保存到本地 control-plane 文件，并热更新到数据面
- 已支持在网页中直接修改 OneDrive 的 async backup / fallback 开关，以及 `memory_only` 作用域
- 已支持在网页中直接修改 `auth-capture` sidecar 地址、LLM endpoint / model，以及 provider 独立凭证
- 已支持交互式“验证输入队列”占位能力，后续可供手机号 / 短信码 / 验证码输入
- 输出健康检查、日志、后续指标和审计信息

### `provider-onedrive`

- 封装 OneDrive 授权后访问能力的 provider
- 作为默认异步备份目标和最终 fallback 目标的入口
- 当前已落最小 Graph 映射，支持 `root_prefix/<bucket>/<key>` 路径映射、健康检查、列容器、列对象、对象读写删
- 当前已支持显式 access token、token file、OAuth session file，以及 access token 过期后使用 refresh token 自动续期并回写 session
- 可直接复用 `gatewayd` 内建的 Web PKCE / Device Code 授权链路
- 后续再补分片上传、更稳健的 delta/sync 与更完整的运维面

### `policy-engine`

- 统一表达 `primary provider`、`sync targets`、`fallback read order`
- 校验主写唯一、fallback 子集、OneDrive 默认备份等约束
- 为 `gatewayd` 和后续控制面提供稳定的拓扑判定边界

### `replication-engine`

- 提供异步复制任务的本地队列骨架
- 当前实现内存队列、最近任务记录、对象级 `put/delete` 入队、启动时 pending job 恢复，以及 `retry_scheduled` 延迟重试出队
- 复制 job 需要绑定创建当时的 `source provider`，避免主写热切换后旧 job 串读到新的主写后端
- `gatewayd` 已启动后台 worker 消费任务，并把结果回写 SQLite
- 当前已支持基础重试 / 退避与 target 级状态汇总；后续补死信和人工重试入口

### `metadata-store`

- 基于 SQLite 持久化复制任务
- 提供 pending job 恢复、状态更新、`next_attempt_at` 记录和 per-target 复制摘要查询
- 当前已用于 `gatewayd` 内部状态接口、worker 状态落盘和对象级复制视图

### `gatewayd` 内嵌 `auth-broker`

- 提供 OneDrive 授权编排
- 统一维护 PKCE state、Device Code flow 状态和共享 HTTP client
- Web 模式下监听独立回调端口，完成 code exchange 后把 session 落盘
- Terminal 模式下启动 Device Code flow 并后台轮询，成功后把 session 落盘
- 当前实现为内嵌最小控制面，后续如复杂度继续上升再拆分独立 crate
- 对运营商网页登录，不直接放进 data plane；后续统一走独立 `auth-broker` / sidecar

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
  - 电信: `listFiles.action` personal root，`getUserInfoForPortal.action` 容量解析
  - 移动: 仍为骨架，scope 先报告为 `personal` + `capacity unknown`

## 数据语义

### 写入

1. 客户端写入唯一主 provider
2. 主写成功后立即向客户端返回成功
3. 生成针对每个 sync target 的复制任务写入本地队列，并把当时的 `source provider` 一起写入 job
4. 后台异步复制到目标集合
5. 更新每个 target 的复制状态

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
- `default backup target`: `onedrive`

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
- 默认 Metrics / 扩展健康检查为 `61083`
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

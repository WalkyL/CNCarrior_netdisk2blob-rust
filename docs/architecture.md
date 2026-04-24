# 架构说明

## 设计原则

1. 三家运营商的网页接口都可能变化，因此每个 provider 都必须独立成 crate。
2. 任意时刻只允许一个运营商云盘作为唯一写入主云盘。
3. OneDrive 是默认备份同步目标，其他运营商账号可按配置加入异步同步目标集合。
4. 写入语义采用“主写成功即返回，后台异步复制到同步目标集合”，系统接受最终一致性。
5. fallback 只能在对象已复制到对应同步目标时触发，不能假定备份侧永远有数据。
6. 认证信息只允许通过受控输入进入服务，不从浏览器会话中自动提取。
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

### Admin UX

- `Web UI`: 适合 OneDrive 授权与日常运维
- `TUI`: 适合 SSH、软路由、容器内诊断与应急配置

### Agent Integration

- `MCP Server`: 给 Hermes / Open Claw / 其他 Agent 提供标准化工具接口
- `Skill`: 给 Agent 提供任务选择、调用约束、fallback 语义和最佳实践

### Deployment Profiles

- `full-host`: PVE LXC `x86/x64`、Docker `x86/x64`、Podman `x86/x64`
- `lite-host`: OpenWRT `arm64`
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
- 后续扩展目录遍历、文件下载、分片上传、断点续传

### `gatewayd`

- 对外暴露本地 S3 兼容数据面服务
- 根据配置选择具体 provider，并将 REST 请求转换为 `BlobBackend` 调用
- 通过 `policy-engine` 校验唯一主写与 sync target 拓扑
- 在对象写入和删除成功后向 `replication-engine` 写入复制任务
- 输出健康检查、日志、后续指标和审计信息

### `provider-onedrive`

- 封装 OneDrive 授权后访问能力的 provider
- 作为默认异步备份目标和最终 fallback 目标的入口
- 当前已落显式 token 模式下的最小 Graph 映射:
- `root_prefix/<bucket>/<key>` 路径映射
- 健康检查、列容器、列对象、对象读写删
- 后续再补 OAuth broker、分片上传、delta 同步和更细粒度复制状态

### `policy-engine`

- 统一表达 `primary provider`、`sync targets`、`fallback read order`
- 校验主写唯一、fallback 子集、OneDrive 默认备份等约束
- 为 `gatewayd` 和后续控制面提供稳定的拓扑判定边界

### `replication-engine`

- 提供异步复制任务的本地队列骨架
- 当前实现内存队列、最近任务记录、对象级 `put/delete` 入队和启动时 pending job 恢复
- `gatewayd` 已启动后台 worker 消费任务，并把结果回写 SQLite
- 后续补重试策略、退避和更细粒度对象状态

### `metadata-store`

- 基于 SQLite 持久化复制任务
- 提供 pending job 恢复、状态更新和复制摘要查询
- 当前已用于 `gatewayd` 内部状态接口和 worker 状态落盘

### 规划中的模块

#### `auth-broker`

- 提供 OneDrive 授权编排
- 统一保存运营商 token / cookie 引用、过期时间、上次校验结果
- 对 Web UI 与 TUI 暴露一致的认证控制接口

#### `admin-api` / `admin-ui-web` / `admin-ui-tui`

- 提供配置、告警、授权、健康状态、复制状态查看能力
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

当前仅规划以下方式:

- 运营商云盘:
  - 环境变量直接注入 token
  - 本地文件注入 token
  - 明确配置 Cookie 头
- OneDrive:
  - Web UI 走 Authorization Code + PKCE
  - TUI 走 Device Code Flow

不纳入自动化范围的方式:

- 从浏览器 profile 直接读取登录态
- 抓取浏览器进程流量
- 自动代理网页登录凭据

## 数据语义

### 写入

1. 客户端写入唯一主 provider
2. 主写成功后立即向客户端返回成功
3. 生成针对每个 sync target 的复制任务写入本地队列
4. 后台异步复制到目标集合
5. 更新每个 target 的复制状态

### 读取

1. 优先从主 provider 读取
2. 当主 provider 熔断或健康异常时按 fallback 读取顺序检查目标集合
3. 仅当对象已复制到目标 provider 时才允许切换读取
4. 默认把 OneDrive 作为最终 fallback 目标
5. 当前 S3 数据面会用 `x-ccbg-source-provider` / `x-ccbg-fallback-from` 显式标记实际读取来源
5. 如果对象对所有目标都仍处于 `pending` 或 `failed`，返回明确错误并发出提醒

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
4. 默认建议始终包含 `onedrive`，作为最终 fallback 备份层。

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
4. 增加 `mcp-server` 的 stdio 版本
5. 再补 Skill 封装与更丰富的管理界面
6. 最后再扩到更多 provider 与更完整的 Blob 语义

## 部署边界

- Ubuntu 本机阶段: 先用 systemd 用户服务或前台运行
- PVE/LXC 阶段: 用单进程容器部署，挂载只读配置、数据库目录和独立 secrets 文件
- Docker / Podman 阶段: 使用仓库内标准构建文件
- 软路由阶段: 优先保证低内存占用、可观测性和断线恢复
- 不建议在容器镜像中固化 token 或 refresh token

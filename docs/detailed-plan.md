# 详细规划

## 产品定位

`carrier-cloud-blob-gateway` 是一个面向边缘 Agent 的本地 S3 兼容对象网关。

目标用户:

- Hermes Agent 用户
- Open Claw Agent 用户
- 在 Ubuntu、PVE/LXC、软路由、ARM Linux 设备上部署本地 Agent 的用户
- 需要从 STM32 等从设备调用本地对象接口的用户

核心定位:

- 以运营商云盘作为主存储集合
- 任意时刻只允许一个运营商或 `stub` provider 作为唯一写入主云盘
- 其他运营商云盘可按用户指定作为异步同步目标
- 以 OneDrive 作为默认异步备份同步对象
- 在运营商入口收紧、限流、认证失败或接口变化时，按规则 fallback 到已同步完成的目标云盘
- 对用户显式告警，不做静默切换
- 面向 Agent 的最终成品必须同时交付为 daemon、MCP 和 Skill

## 运行边界

### 推荐宿主

- Ubuntu 主机
- PVE LXC `x86/x64`
- Docker `x86/x64`
- Podman `x86/x64`
- OpenWRT `arm64`
- x86 软路由
- ARM Linux 设备

### 不推荐宿主

- STM32 直接运行完整网关

STM32 更适合作为客户端消费本地 S3 API，而不是承载完整控制平面、复制队列和 OneDrive 授权流程。

## 一期兼容目标

### 完整宿主

- PVE LXC `x86/x64`
- Docker `x86/x64`
- Podman `x86/x64`

### 轻量宿主

- OpenWRT `arm64`

### 客户端兼容

- STM32

### 源码发布方式

- GitHub 公开仓库
- 商业核心边界 + 公开材料许可 + 个人非商业源码审查申请
- `.47` 本地 release gate；macOS 包为社区/实验交叉编译包

## 系统目标

1. 给 Agent 提供稳定、简单、长期可维护的本地 S3 兼容接口。
2. 隔离运营商网页接口的不稳定性。
3. 将写入主路径和备份复制路径解耦。
4. 让用户能在本地界面中完成授权、配置、查看告警和排障。
5. 所有服务端口统一放在 `60000-65534` 范围。
6. 成品必须优先服务 Hermes Agent、Open Claw Agent 等编程 Agent 的接入方式。
7. 一期必须以公开仓库形态交付，但不宣称整体 MIT 或 OSI 开源。

## 关键设计决策

### 主存储与备份

- 唯一写入主云盘: `unicom | telecom | mobile`
- 异步同步目标: `telecom | mobile | unicom | onedrive`
- 默认备份同步目标: `onedrive`
- 复制模式: `async_backup`

约束:

1. 同一时刻只能有一个 primary provider。
2. sync targets 不应包含当前 primary provider。
3. 用户若配置多个运营商账号，其他账号只能作为异步同步目标。

### 一致性模型

- 主写成功即返回
- 后台异步复制到 OneDrive
- fallback 只在备份已完成时生效

这意味着系统语义是“最终一致性”，不是“双写强一致”。

### 管理界面策略

- 控制平面逻辑统一
- UI 提供两种接入形式:
  - `Web UI`
  - `TUI`

推荐交付顺序:

1. 先做 `admin-api`
2. 先交付 `Web UI`
3. 再补 `TUI`

原因:

- OneDrive 授权流程更适合网页回调
- 边缘用户通常能从局域网浏览器访问设备
- TUI 更适合作为诊断和应急入口

### 平台兼容策略

- `PVE LXC x86/x64` 作为首要完整宿主
- `Docker / Podman x86/x64` 作为开发、CI 和标准部署入口
- `OpenWRT arm64` 作为轻量宿主，需要裁剪 Web UI 和并发配置
- `STM32` 作为客户端目标，不承担控制平面和复制引擎

### Agent 封装策略

- 成品必须同时具备:
  - 后台服务
  - MCP Server
  - Skill
- MCP 是主要执行入口
- Skill 是主要使用约束与流程指导层

### 数据面协议策略

- 数据面正式目标兼容 S3 API
- 一期优先 path-style bucket addressing
- 一期只做 Agent 常用的最小 S3 子集
- 控制面和管理接口不混入 S3 命名空间

## 顶层架构

```text
Agent
  |
  v
gatewayd (S3 API :61080)
  |
  +--> policy-engine
  |      |
  |      +--> primary carrier provider
  |      |
  |      +--> sync target providers
  |
  +--> metadata-store (SQLite)
  |
  +--> replication-engine
  |
  +--> notify
  |
  +--> admin-api
          |
          +--> admin-ui-web (:61081)
          +--> admin-ui-tui
          +--> auth-broker
  |
  +--> mcp-server
          |
          +--> stdio
          +--> Streamable HTTP (:61084, optional)
  |
  +--> skill package
```

## 模块清单与当前实现状态

### `gatewayd`

- 暴露本地 S3 兼容 API
- 将请求转给 `policy-engine`
- 对响应附加 fallback、provider、复制状态元信息

### `policy-engine`

- 判定当前唯一主 provider
- 判定 sync targets 与 fallback 顺序
- 控制熔断、回切和手动切主

### `provider-unicom`

- 联通云盘接入
- 可作为主云盘或异步同步目标

### `provider-telecom`

- 天翼云盘接入
- 可作为主云盘或异步同步目标

### `provider-mobile`

- 中国移动云盘接入
- 可作为主云盘或异步同步目标

### `provider-onedrive`

- 默认备份写入
- 最终 fallback 读取
- 删除传播
- 当前最小实现使用 `root_prefix/<bucket>/<key>` 映射到单个 OneDrive drive
- 当前认证入口已支持显式 access token、token file、OAuth session file、Web `Authorization Code + PKCE`、Terminal `Device Code Flow`
- access token 过期后使用 refresh token 自动续期并回写 session file

### `metadata-store`

- 推荐 SQLite
- 保存 provider 状态、对象的 per-target 复制状态、队列状态、最近错误

### `replication-engine`

- 维护 outbox / job queue
- 异步把数据推送到 sync targets
- 每个 job 记录创建时的 `source provider`
- 管理失败重试和死信任务

### `auth-broker`

- 当前已以内嵌模式落在 `gatewayd`
- 统一 OneDrive 授权和运营商连接状态管理
- 当前已管理 PKCE state、Device Code flow 状态和 OneDrive session 落盘
- 给 Web UI、TUI 和后端 provider 提供统一认证视图

### `admin-api`

- 管理配置
- 查询健康状态
- 查询复制状态
- 管理告警和事件

### `admin-ui-web`

- 默认管理入口
- 当前最小版本已可通过浏览器启动 OneDrive Web 授权并查看基础状态
- 当前应支持在浏览器中热切换主写 / sync target / fallback 拓扑
- 当前已可在浏览器中直接修改 OneDrive async backup / fallback 开关，以及 `memory_only` bucket/prefix 作用域
- 当前已可在浏览器中按 provider 查看、修改并热注入独立认证材料
- provider 凭证按 `CCBG_CREDENTIALS_DIR/{provider}.json` 独立落盘
- 后续补完整告警、复制队列和对象状态界面

### `admin-ui-tui`

- SSH/串口/控制台环境下的备用管理入口
- 当前可先通过 Device Code + 配置文件完成终端场景接入
- 后续补专用 TUI，用于诊断、手工注入 token、查看队列和手动切主

### `deploy/Dockerfile` / `deploy/Containerfile`

- 分别提供 Docker 和 Podman 的标准构建入口
- 作为一期公开仓库的正式发布资产

### `notify`

- 记录本地事件
- 发送 webhook
- 为 Agent 或 Web UI 推送提醒

### `scripts/check-release-ready.sh`

- 为发布前提供本地质量门禁
- 一期至少覆盖格式检查、workspace 测试、catalog lint 和公开站 fingerprint

### `.47` 发布构建主机

- 负责 Linux LXC、Windows、OpenWrt、macOS 社区/实验包、STM32 示例和 ESP32-S3 示例的构建入口
- GitHub 只保留源码、分支、tag、issue 模板和 release 记录

### `mcp-server`

- 封装 tools / resources / prompts
- 将内部状态转成 Agent 友好的工具调用面
- 默认优先支持 stdio，后续补 Streamable HTTP

### `skills/carrier-cloud-blob-gateway`

- 定义触发条件、推荐调用顺序、语义边界和风险提醒
- 面向 Codex 类 Agent 交付

## 认证规划

### OneDrive

#### Web UI 模式

- 使用 `Authorization Code + PKCE`
- 浏览器访问设备的 Admin Web UI
- 本地回调地址走 `61082`
- 授权成功后将 token 安全存入本地状态库或 secrets 文件

#### TUI 模式

- 使用 `Device Code Flow`
- 适合没有浏览器回调的 SSH 环境

### 运营商云盘

统一采用“受控接入”策略:

- 手工注入 token
- 手工注入 cookie header
- 通过 Admin Web 写入各 provider 独立凭证文件并立即热生效
- 连通性测试与状态校验
- 记录过期时间、上次成功时间、最近错误

当前落盘规划:

- `unicom.json`
- `telecom.json`
- `mobile.json`
- `onedrive.json`

字段边界:

- 联通 / 电信 / 移动: `token`、`cookie_header`
- OneDrive: `client_id`、`tenant`、`drive_id`、`redirect_url`、可选 `token`

明确不做:

- 自动抓浏览器会话
- 读取浏览器 profile
- 代理网页登录过程以窃取会话

## Agent 交付封装

### daemon

- 持续运行的核心服务
- 管理状态、复制、fallback 和认证

一期宿主范围:

- PVE LXC `x86/x64`
- Docker `x86/x64`
- Podman `x86/x64`
- OpenWRT `arm64` 轻量模式

### MCP

- 给 Agent 暴露标准工具接口
- 默认优先 `stdio`
- 可选 `Streamable HTTP`

### Skill

- 给 Agent 定义操作规范
- 告诉 Agent 如何安全使用 MCP 和本地 S3 能力

### STM32 客户端

- 通过 S3 API 或 MCP 间接接入
- 重点支持小对象传输和状态查询
- 不承诺承载完整 Rust 服务

## S3 兼容目标

### 一期最小 S3 子集

- `ListBuckets`
- `HeadBucket`
- `ListObjectsV2`
- `HeadObject`
- `GetObject`
- `PutObject`
- `DeleteObject`

### 一期明确不做

- IAM
- ACL
- STS
- Bucket Policy
- 完整 AWS 控制面

### 兼容方式

- 自定义 endpoint
- 本地 SigV4 鉴权
- path-style bucket 访问
- 控制面与 S3 数据面分离

## 异步复制模型

### 对象状态

- `pending`
- `replicating`
- `replicated`
- `failed`
- `stale`
- `delete_pending`

### 写入流程

1. 客户端发起写入
2. `policy-engine` 选择当前主 provider
3. 主 provider 写入成功
4. 立即向客户端返回成功
5. 为每个 sync target 写入复制任务到 `metadata-store`
6. `replication-engine` 异步复制到目标集合
7. 更新每个 target 的状态

### 读取流程

1. 默认从主 provider 读取
2. 如果主 provider 不健康或被熔断，按 fallback 顺序检查 sync targets
3. 若对象已复制到对应 target，则读取该 target
4. OneDrive 可作为最后的 fallback 备份目标，但是否启用由用户控制
5. 若对象未复制完成，返回明确错误，并附带告警信息

### 删除流程

1. 主 provider 删除成功
2. 对所有 sync targets 生成删除传播任务
3. 全部目标删除成功后清理状态
4. 若任一目标删除失败，保留 `delete_pending`

## 多账号同步拓扑

### provider 角色

- `primary provider`
- `sync target`
- `default backup target`

### 默认规则

1. `primary provider` 只能是 `unicom | telecom | mobile` 之一。
2. `sync targets` 可包含其他运营商和 `onedrive`。
3. `onedrive` 默认应加入 `sync targets`。
4. fallback 读取顺序应由用户显式配置；留空表示仅做异步同步，不启用 fallback 读取。

## fallback 策略

### 触发条件

- 认证失败
- 连续 `403`
- 连续 `429`
- 健康检查失败
- 延迟超过阈值
- 接口结构变化导致解析失败
- 手动禁用某 provider

### fallback 成立条件

- 目标 provider 在 fallback 顺序中
- 对象已复制到该 target
- 目标 provider 自身健康状态正常

### 恢复条件

- 主 provider 健康检查连续通过
- 最近一段时间内错误率恢复到阈值以下
- 或操作者手工确认回切

### 告警要求

发生下列事件必须提醒用户:

- fallback 被触发
- 复制失败累计超阈值
- 运营商 provider 持续不可用
- OneDrive 备份不可用
- 某个异步同步目标持续失败
- 复制任务积压

## MCP 封装规划

### 首批工具

- `s3_list_buckets`
- `s3_list_objects_v2`
- `s3_get_object`
- `s3_put_object`
- `s3_delete_object`
- `replication_get_status`
- `replication_get_target_status`
- `provider_health`
- `provider_test_connection`
- `provider_get_primary`
- `provider_set_primary`
- `provider_get_sync_targets`
- `provider_set_sync_targets`
- `auth_begin_onedrive_login`

### 首批资源

- 当前 provider 健康摘要
- 最近 fallback 事件
- 复制失败摘要

### transport 策略

1. `stdio` 作为默认模式
2. `Streamable HTTP` 作为后续扩展模式
3. HTTP 模式默认监听 `61084`

## Skill 封装规划

Skill 必须明确告诉 Agent:

1. 主写成功不代表已备份到 OneDrive
2. fallback 只有在对象 `replicated` 时才能依赖
3. 同一时刻只能有一个写入主云盘
4. 其他运营商账号只作为异步同步目标参与复制
5. provider 不健康时应先查状态而不是盲目重试
6. 删除与覆盖前应优先核对复制状态
7. 本地数据面兼容 S3 API，但不等于完整 AWS S3 控制面

推荐目录:

```text
skills/
└── carrier-cloud-blob-gateway/
    ├── SKILL.md
    └── agents/openai.yaml
```

## 管理界面规划

### Web UI

页面建议:

- 仪表盘
- Provider 管理
- OneDrive 连接
- 复制队列
- 对象状态查询
- 告警中心
- 系统设置

关键操作:

- 连接 OneDrive
- 添加/更新运营商 token
- 测试 provider 可用性
- 查看 fallback 事件
- 手动切主/禁用 provider

### TUI

页面建议:

- 总览页
- Provider 状态页
- 复制任务页
- 告警页
- 系统设置页

关键操作:

- 手工注入 token
- 启动 OneDrive Device Code 授权
- 查看复制失败任务
- 强制重试或切主

## 端口规划

端口统一策略:

- 所有服务端口必须在 `60000-65534`
- 默认不占用常见系统端口
- 所有对外能力默认仅绑定 `127.0.0.1` 或管理网口

默认分配:

- S3 API: `61080`
- Admin Web UI: `61081`
- OneDrive OAuth Callback: `61082`
- Metrics / Extended Health: `61083`
- MCP Streamable HTTP: `61084`

端口选择原则:

- `61080-61089`: 数据面与控制面核心端口
- `61090-61149`: 预留给未来事件流、调试端口、管理扩展
- `61150+`: 预留给容器并行部署或多实例场景

## GitHub 公开发布要求

一期仓库必须包含:

- `LICENSE`
- `scripts/check-release-ready.sh`
- `docs/ops-007-47-release-build-host.md`
- Docker / Podman 构建入口
- 明确区分“已实现”和“规划中”的文档

## 里程碑规划

### Phase 1: 控制平面基础

- 明确统一配置模型
- 落地 SQLite 元数据层
- 接入 OneDrive provider 与最小 auth broker
- 明确 primary provider / sync targets 模型
- 完成端口策略和部署模型
- 明确一期平台兼容矩阵和 GitHub 发布要求
- 明确 S3 兼容边界和最小子集

验收标准:

- 服务按 `61080+` 端口约定启动
- 本地状态库可初始化
- provider 健康状态有统一模型
- primary provider 与 sync targets 有统一配置模型
- 兼容矩阵和公开发布资产已落文档
- S3 兼容文档已落地

### Phase 2: OneDrive 备份层

- 实现 `provider-onedrive`
- 完成异步复制队列
- 完成对象状态记录

当前进展:

- `provider-onedrive` 已建立最小 Graph 读写实现，支持健康检查、列举、读写删
- `metadata-store` 已基于 SQLite 落盘复制任务
- `gatewayd` 已在写入/删除后创建复制任务，并由后台 worker 消费
- `gatewayd` 已补最小内建控制面，包含 Admin HTML 首页、OneDrive Web `Authorization Code + PKCE`、OneDrive Terminal `Device Code Flow`、OAuth callback listener、session 文件落盘和 provider 侧自动 refresh token 续期

验收标准:

- 主写后能产生复制任务
- 可查询对象是否已复制
- 可查看复制失败原因

### Phase 3: S3 单一运营商 MVP

- 先选联通作为首家接入
- 打通 `ListObjectsV2`、`GetObject`、`PutObject` 最小闭环
- 实现 fallback 到 OneDrive

验收标准:

- 一个对象完成主写 + 异步备份 + fallback 读取
- fallback 事件可被记录和查看
- S3 客户端可完成最小对象读写

### Phase 4: 多目标同步

- 为同一对象增加 per-target 复制状态
- 支持把其他运营商账号加入 sync targets
- 支持按配置切换 fallback 顺序

验收标准:

- 主写后可向多个目标并发异步同步
- 一个对象可分别查询各 target 的复制状态

### Phase 5: Agent 封装

- 实现 `mcp-server`
- 优先交付 stdio transport
- 设计稳定的 tools / resources / prompts 面
- 落地 Skill 首版

验收标准:

- 本地 Agent 能通过 MCP 调用核心对象操作
- Skill 能正确指导 Agent 使用复制状态和 fallback 语义

### Phase 6: Admin UX 扩展

- 落地 `admin-api`
- 先交付 Web UI
- 支持 OneDrive 授权、provider 测试、队列查看、告警查看

当前进展:

- `gatewayd` 已提供最小 HTML 管理页
- 已提供 `/api/status` 与 OneDrive auth status / start / poll 接口
- 浏览器内完成 OneDrive 连接已可用
- 主写 / sync / fallback 拓扑已支持在控制面中立即热切换
- 已提供 OneDrive 作用域策略入口，可限制只把 Hermes / OpenClaw memory bucket 或 prefix 复制 / fallback 到 OneDrive
- 已提供 provider 健康卡片、按 provider 的即时测试动作、复制队列概览、最近 job 历史和基础告警摘要
- 已提供对象级状态视图，可按 bucket/key 查看主写对象、各 sync target 的存在性、fallback gate、最新复制 job 和当前 gateway 读路径
- 更细粒度的 provider 探针、对象级 diff/批量视图和专用 TUI 仍待补齐

验收标准:

- 用户可在浏览器中完成 OneDrive 连接
- 用户可在页面中查看 provider 健康状态与复制队列
- TUI 与更完整 Web dashboard 可覆盖无浏览器或低带宽运维场景

### Phase 7: 多 provider 扩展

- 接入电信
- 接入移动
- 抽公共响应模型和错误模型

验收标准:

- 三家 provider 共用统一对象模型
- 至少两家 provider 可完成只读闭环

### Phase 8: TUI 与边缘优化

- 补 TUI
- 优化软路由内存占用
- 增强缓存、限流、并发控制
- 验证 OpenWRT `arm64` 轻量部署参数
- 明确 STM32 客户端接入样式

验收标准:

- SSH 环境可完成基本管理
- 软路由场景可稳定运行
- STM32 客户端兼容边界文档清晰

## 风险与约束

1. 运营商网页接口不稳定，是系统最大脆弱点。
2. fallback 的可靠性依赖复制完成率，而不是单纯依赖 OneDrive 可用性。
3. OneDrive 授权需要控制平面和本地状态存储配合，不能只靠静态 env。
4. 软路由和 ARM 设备的 CPU、IO、内存都要求复制队列与缓存实现足够克制。
5. 如果运营商存在 IP、UA、Cookie 强绑定，容器迁移时要尽量保持外部环境一致。

## 推荐的后续执行顺序

1. 先把主写热切换与 `source provider` 绑定 job 的闭环做稳
2. 补齐 Admin Web 的 provider 测试、复制队列和告警视图
3. 开始 `mcp-server` 的 stdio 版本，并固定 tools/resources/prompts 面
4. 基于 MCP 交付首版 Skill
5. 再扩到电信、移动和更完整的多目标同步

# 实施规划

## Phase 0: 已完成

- Rust workspace 建立
- 网关入口建立
- 三个 provider 适配层目录预留
- 上游适配层与核心抽象拆分
- 配置样例与容器构建文件落盘

## Phase 1: 控制平面底座

- 固化 `61080+` 端口策略
- 引入统一配置模型和本地状态库存储
- 设计 `auth-broker`、`metadata-store`、`policy-engine` 接口
- 明确 OneDrive 当前阶段延后集成、默认禁用和隐藏的系统语义
- 明确唯一主云盘与 sync targets 配置模型
- 明确 daemon、MCP、Skill 三种交付形态
- 明确一期兼容矩阵: `PVE LXC x86/x64`、`Docker x86/x64`、`Podman x86/x64`、`OpenWRT arm64`、`STM32 client`
- 明确公开仓库交付要求，以及商业核心 / 公开材料 / 个人源码审查边界
- 明确 S3 兼容边界和最小数据面子集
- 明确可选入口能力的原则：幂等、低内存、可插拔，浏览器/CDP 只做 capture / probe，不做 core 长期依赖

当前进展:

- `policy-engine` 已落 crate，并已接入 `gatewayd` 的拓扑校验
- `gatewayd` 已支持把 `primary/sync/fallback` 作为统一配置模型加载

交付物:

- 本地状态库结构设计
- provider 健康状态模型
- 复制任务模型
- 端口与部署文档
- Agent 交付策略文档
- primary provider / sync targets 配置模型
- 兼容矩阵文档
- GitHub 发布规划文档
- S3 兼容文档

## Phase 2: 三大运营商复制底座

- 建立异步复制队列
- 完成对象复制状态记录
- 先围绕联通 / 电信 / 移动完成同步目标与 fallback 骨架
- OneDrive 保留为 Parking 集成项，不作为当前阶段默认备份层

当前进展:

- OneDrive 代码已存在，但当前产品口径为默认禁用 / 隐藏，后续有真实需求再恢复
- `metadata-store` 已落 SQLite 持久化层，可保存并恢复 pending replication jobs
- `replication-engine` 已接入后台 worker，`PutObject` / `DeleteObject` 后会入队并消费
- 对象级状态已支持按 target 汇总与按对象查询；更完整的死信、人工重试入口和更细粒度策略仍待实现

交付物:

- 可写入复制任务
- 可查询对象复制状态

## Phase 3: S3 单一运营商 MVP

- 先接入联通或最先确认接口的一家运营商
- 打通 `ListObjectsV2`、`GetObject`、`PutObject` 的最小 S3 闭环
- 实现主写成功后异步复制到已配置的运营商同步目标
- 实现读取时按规则 fallback

交付物:

- 一个对象完成主写 + 异步同步 + fallback 读取
- 本地 S3 客户端可完成最小对象读写
- fallback 事件可查询
- 错误日志能区分认证失败、风控失败、接口变更

## Phase 4: 多目标同步

- 扩展 per-target 复制状态到更完整的人工重试 / 死信运维闭环
- 支持把其他运营商账号作为异步同步目标
- 明确 fallback 读取顺序

交付物:

- 一个对象可同步到多个目标
- 可分别查询每个目标的复制状态

## Phase 5: Agent 封装

- 实现 `mcp-server`
- 优先支持 stdio transport
- 设计稳定的 tools / resources / prompts 面
- HTTP transport 补齐“未鉴权公开发现 + 已鉴权运维”分层
- 生成首版 Skill 包

交付物:

- 本地 Agent 可通过 MCP 调用核心能力
- Skill 明确复制与 fallback 语义

## Phase 6: 管理界面与授权

- 实现 `admin-api`
- 先交付 Web UI
- 支持运营商 provider 测试、复制队列与告警展示
- 支持设置 primary provider 与 sync targets
- 支持 auth-broker / LLM endpoint 配置
- 支持把手机号 / 短信验证码 / 图形验证码 等交互式认证输入回显到网页
- 预留 TUI 的共用控制面接口

交付物:

- 浏览器内查看 provider 健康状态和告警
- 浏览器内查看对象复制状态
- 浏览器内能响应运营商登录时的交互式输入步骤

## Phase 7: 多运营商扩展

- 接入第二家运营商
- 接入第三家运营商
- 抽公共错误模型、分页与重试逻辑
- 优化 provider 切换和优先级策略

交付物:

- 至少两家运营商可完成只读闭环
- 三家 provider 共用统一对象模型

## Phase 8: TUI 与边缘部署优化

- 构建最小运行镜像
- 对接 PVE/LXC 的卷挂载与网络
- 补 TUI 入口
- 增加 systemd 或容器健康探针
- 补充备份与日志保留策略
- 优化软路由场景的内存与并发配置
- 验证 Docker / Podman 构建流程
- 验证 OpenWRT `arm64` 轻量模式
- 明确 STM32 客户端接入限制

交付物:

- 可在 PVE 容器中稳定运行的服务包
- 标准化启动参数与 secrets 管理
- SSH 环境下可完成基本管理
- 容器构建路径清晰可复现

## PVE/LXC 迁移注意事项

1. 优先使用 Debian/Ubuntu LXC，便于证书、DNS、系统日志排障。
2. token 文件用单独挂载点管理，避免进入镜像层。
3. 如果任一 provider 有 IP 或 UA 风控，容器网络出口、DNS 和主机环境要保持一致。
4. 上传下载大文件前，先确认 LXC 的磁盘缓存与临时目录容量。
5. 如果使用 Web UI 和 OAuth 回调，确保 `61081-61082` 在容器内部保留。
6. 如果启用 MCP Streamable HTTP，确保 `61084` 仅暴露给受控网络。
7. 如果同时接入 `.51` 的外部 Agent-nats-redmine-hub，使用 `http://192.168.1.51:8787/mcp`，不要把外部 Hub 配到 `61084`。

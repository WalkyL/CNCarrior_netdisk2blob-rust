# Agent 交付封装

## 目标

这个项目的目标用户不是传统的人类文件管理用户，而是:

- Hermes Agent
- Open Claw Agent
- 其他编程 Agent / 自动化 Agent

因此成品必须至少提供三种交付形态:

1. 后台网关进程
2. MCP Server
3. Skill

## 交付形态

### 1. 后台网关进程

职责:

- 暴露本地 S3 兼容 API
- 管理唯一写入主 provider / 异步同步目标集合
- 执行异步复制和 fallback

这是系统的事实来源，MCP 和 Skill 都不直接替代它。

### 2. MCP Server

职责:

- 将网关能力封装成 Agent 能直接调用的标准工具接口
- 把底层 S3 API、复制状态、provider 健康状态包装成适合 Agent 的工具粒度
- 将危险操作显式化，例如删除、强制切主、修改同步目标、重试复制

### 3. Skill

职责:

- 教会 Agent 在什么场景下使用 MCP 工具
- 指导 Agent 如何理解复制状态、fallback 语义和错误分类
- 限制 Agent 不要错误假设“主写成功等于所有同步目标已完成”
- 限制 Agent 不要错误假设“多个运营商账号可以同时作为主写入端”

## MCP 设计原则

## Transport 策略

优先级:

1. `stdio`
2. `Streamable HTTP`

原因:

- 本地 Agent 集成最简单的是 stdio
- MCP 官方当前标准 transport 是 `stdio` 和 `Streamable HTTP`
- 旧的 HTTP+SSE 只作为兼容层考虑，不作为新实现主路径

### `stdio`

适用场景:

- Hermes / Open Claw 本地拉起子进程
- 单机、单用户、本地自动化

优点:

- 不需要额外开放端口
- 更容易跟本地环境变量和 secrets 集成
- 更适合作为默认模式

### `Streamable HTTP`

适用场景:

- 多个 Agent 共用同一实例
- 远程或容器编排场景
- 需要会话管理和 HTTP 接入控制

安全要求:

- 默认只绑定 localhost 或管理网口
- 校验 `Origin`
- 加认证层
- 不直接裸露到公网

规划中的默认地址:

- `127.0.0.1:61084/mcp`

注意：

- 这里的 `127.0.0.1:61084/mcp` 是 `ccbg` 自己的可选 MCP HTTP 入口。
- 如果 Agent 还要接入 `.51` 的 ticket / coordination Hub，那个外部服务当前应使用：
  - `http://192.168.1.51:8787/mcp`
  - 服务身份：`agent-nats-redmine-hub`
- 不要把 `.51` 外部 Hub 错配成 `192.168.1.51:61084/mcp`。

## MCP 能力边界

MCP 不应把所有内部细节暴露成工具。建议分成四类:

### 当前实现状态

- 未鉴权公开发现:
  - `tools/list`
  - `resources/list`
  - `prompts/list`
  - tool: `mcp_feature_access_summary`
  - resource: `ccbg://public/feature-access-summary`
  - prompt: `discover_feature_access_model`
- 已鉴权运维工具:
  - 只读: `provider_list`、`provider_health`、`replication_get_status`、`replication_list_failed_jobs`、`alerts_list_recent`、`admin_status_get`、`applications_get`、`content_policies_get`、`provider_credentials_get`、`auth_capture_policy_get`、`replication_dlq_list`
  - 修改: `applications_update`、`content_policies_update`、`topology_update`、`provider_credentials_update`、`auth_capture_policy_update`、`replication_retry_job`、`replication_dlq_replay_job`、`replication_dlq_replay_target`
- MCP HTTP 若启用 bearer token，未鉴权调用仍可看能力面，但运维调用会返回 `401` 并引导先读公开发现摘要。

### S3 Tools

- `s3_list_buckets`
- `s3_list_objects_v2`
- `s3_get_object`
- `s3_put_object`
- `s3_delete_object`
- `s3_head_object`

### Replication Tools

- `replication_get_status`
- `replication_retry_object`
- `replication_list_failed_jobs`

### Provider Tools

- `provider_list`
- `provider_health`
- `provider_test_connection`
- `provider_get_primary`
- `provider_switch_primary`
- `provider_get_sync_targets`
- `provider_set_sync_targets`

### Admin / Auth Tools

- `auth_get_status`
- `auth_begin_carrier_login`
- `auth_set_carrier_token`
- `alerts_list_recent`

## MCP Resource 与 Prompt 规划

### Resources

- 当前 provider 状态
- 最近 fallback 事件
- 复制失败队列摘要
- 端口与部署配置摘要

### Prompts

- “安全读取对象”
- “先确认备份再读取”
- “排查 provider 连接失败”
- “为某个对象重试复制”
- “先发现能力与鉴权边界”

## Skill 设计原则

Skill 不是 SDK 文档，也不是产品 README。

Skill 的职责是给 Agent 提供:

- 何时调用 MCP
- 优先调用哪些工具
- 哪些状态不能被错误推断
- 高风险操作前应该做哪些检查

### Skill 必须强调的语义

1. 主写成功不代表异步同步目标已完成。
2. 同一时刻只能有一个运营商云盘作为主写入端。
3. fallback 只有在对象状态为 `replicated` 时才能依赖。
4. 删除和覆盖应优先确认对象状态，避免破坏尚未复制的数据。
5. 当 provider 健康异常时，应优先调用状态与告警工具，而不是盲目重试写入。
6. 本地数据面虽然兼容 S3 API，但不等于完整 AWS S3 控制面。

### Skill 建议结构

```text
skills/
└── carrier-cloud-blob-gateway/
    ├── SKILL.md
    └── agents/
        └── openai.yaml
```

建议内容:

- `SKILL.md`
  - 触发条件
  - 推荐调用顺序
  - fallback 与复制语义
  - 何时升级到人工确认
- `agents/openai.yaml`
  - Skill 的展示名
  - 简短描述
  - 默认提示词

## 推荐的 MCP / Skill 配合方式

1. Skill 决定调用顺序和策略
2. MCP 提供可执行能力
3. 网关核心服务提供最终状态和持久化语义

关系如下:

```text
Skill -> MCP tools/resources/prompts -> gatewayd -> policy-engine -> providers
```

## 代码结构规划

建议增加:

- `crates/mcp-server`
- `skills/carrier-cloud-blob-gateway/SKILL.md`
- `skills/carrier-cloud-blob-gateway/agents/openai.yaml`

模块职责:

- `mcp-server`
  - 注册工具
  - 适配 stdio / Streamable HTTP
  - 统一把请求映射到 `gatewayd` 或控制平面
- `skills/carrier-cloud-blob-gateway`
  - 给 Agent 提供操作规范和调用策略

## 多运营商账号策略

当用户同时拥有多个运营商云盘账号时:

1. 只允许一个账号所在 provider 被指定为唯一写入主云盘。
2. 其他运营商账号如被选中，只参与异步同步，不参与并发主写。
3. OneDrive 当前属于延后集成 provider，默认禁用 / 隐藏，不默认加入同步目标集合。
4. Agent 在修改 primary provider 前，应先检查当前复制积压和目标健康状态。

## 路线建议

1. 先完成核心服务与运营商异步复制闭环
2. 再完成 `mcp-server` 的 stdio 版
3. 基于 MCP 工具稳定面再生成 Skill
4. 最后才补 Streamable HTTP transport

## 不建议的做法

- 只交付 HTTP API，不交付 MCP
- 把 Skill 写成通用产品介绍，而没有工具调用规则
- 在 MCP 中暴露过细、不可控、绕过策略层的 provider 内部接口
- 在 Skill 中假定 fallback 永远可用

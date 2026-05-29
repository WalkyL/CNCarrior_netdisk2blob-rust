# carrier-cloud-blob-gateway TODO（可执行版）

说明：近线主线为“运营商 providers + 本地 S3 + MCP + Skill + 本地控制面”。OneDrive 仅作为 Parking 未来项，默认禁用/隐藏，不作为近期默认备份或默认 fallback 目标。

实现原则：

1. 前端归前端，后端归后端。能在 Admin HTML / Cloudflare 侧完成的 FAQ、匹配、AI解释、页面交互与展示逻辑，不回灌到 Rust。
2. Rust 只承担必须的 API、状态、鉴权和运行时控制职责；新增后端代码时，默认把内存上限、缓冲区上限、日志容量上限和低配宿主预算当成硬约束。
3. 对低配宿主，优先选择流式、截断、有界 ring buffer 和显式 limit/cursor；避免整包读取、无限缓存和重复保留大对象副本。
4. 所有新增组件尽量可插拔。优先显式接口、可替换配置源、可选启停和模块化边界，避免把 provider 特定逻辑、FAQ 数据源、日志来源、AI解释链路或安装入口焊死在单一路径。

## MCP-001: stdio MCP server

**优先级:** P0
**状态:** completed
**目标:** 新增 `crates/mcp-server`，实现 stdio transport 的 MCP 服务主入口。
**Coding 指导:** 复用现有 control/gateway 能力，不直连 provider 内部细节；实现工具注册、请求分发、统一 tracing/request-id。
**验收方法:** `cargo test -p mcp-server`；本地用 MCP Inspector 走 stdio 调 `provider_list`、`replication_get_status`。
**验收标准:** stdio 可稳定握手；至少 6 个核心工具可调用；错误返回结构一致。
**依赖:** ADMIN-001, MCP-002
**不做事项:** 先不做 Streamable HTTP。
**风险:** 工具命名和参数面后续变更导致 Skill 失效。

## MCP-002: MCP tools schema 与错误模型

**优先级:** P0
**状态:** completed
**目标:** 固化 MCP tools 输入/输出 schema 与错误码映射。
**Coding 指导:** 建 `schema` 模块，按 S3/replication/provider/admin 四类分组；错误统一映射为可机读 code + message + retryable。
**验收方法:** schema 快照测试 + 破坏性参数测试。
**验收标准:** 所有工具均有 JSON schema；4xx/5xx 与 retryable 语义稳定。
**依赖:** 无
**不做事项:** 不暴露 provider 私有调试接口。
**风险:** schema 频繁漂移影响 Agent 提示词稳定性。

## MCP-003: MCP 到 gatewayd/control API 客户端层

**优先级:** P0
**状态:** completed
**目标:** 实现 MCP 到 `gatewayd`/control API 的 typed client 层。
**Coding 指导:** 新建 client abstraction，统一超时、重试、鉴权头与错误转换；避免工具内散落 HTTP 细节。
**验收方法:** mock server 集成测试；异常注入（超时/401/503）。
**验收标准:** MCP 工具全部经 client 层访问；超时与错误行为一致。
**验收备注:** `HttpControlPlaneClient` 已统一请求构造与状态映射，复用 `admin-api` 路由/错误契约并补齐 from_env、401/503/timeout/retry/脱敏测试。
**依赖:** ADMIN-001
**不做事项:** 不把 provider 凭证直接暴露给 MCP。
**风险:** control API contract 未冻结导致返工。

## MCP-004: MCP resources/prompts

**优先级:** P1
**状态:** completed
**目标:** 补齐 resources 与 prompts（provider 状态、失败复制摘要、fallback 事件）。
**Coding 指导:** resources 仅输出摘要与可追踪 ID；prompts 强调“主写成功不等于已复制”。
**验收方法:** MCP 客户端读取 resources + 执行 prompts 模板回放。
**验收标准:** 至少 4 个 resources、4 个 prompts；字段稳定且无敏感信息泄露。
**依赖:** MCP-001, MCP-002
**不做事项:** 不在 prompt 中写死某个 provider。
**风险:** 摘要过长导致 Agent 上下文污染。

## MCP-005: Streamable HTTP transport

**优先级:** P2
**状态:** completed
**目标:** 为 MCP 增加 Streamable HTTP 传输（非默认）。
**Coding 指导:** 默认仅监听 localhost；增加 origin 检查与 token 鉴权；与 stdio 共用工具层。
**验收方法:** HTTP 会话并发测试、鉴权失败测试。
**验收标准:** `61084` 可选启用；未鉴权请求被拒绝；stdio 不受回归影响。
**依赖:** MCP-001, MCP-002
**不做事项:** 不对公网裸露默认配置。
**风险:** 长连接资源占用与 DoS 面扩大。

## SKILL-001: Skill 包结构

**优先级:** P1
**状态:** completed
**目标:** 落地 `skills/carrier-cloud-blob-gateway/` 包结构与基础清单。
**Coding 指导:** 包含 `SKILL.md` 与 `agents/openai.yaml`；内容与 MCP tool 名称一一对应。
**验收方法:** 本地 agent 加载 skill，检查触发和展示。
**验收标准:** skill 可被识别；调用路径与 MCP 工具匹配。
**依赖:** MCP-001, MCP-002
**不做事项:** 不把 README 复制成 Skill。
**风险:** 工具重命名造成 Skill 失配。

## SKILL-002: Agent 调用规则与风险约束

**优先级:** P1
**状态:** completed
**目标:** 定义 Agent 调用顺序与高风险操作闸门。
**Coding 指导:** 先查状态再执行写操作；删除/切主/批量重试前强制确认复制状态与告警。
**验收方法:** 设计 6 组场景回放（正常、失败、降级）。
**验收标准:** 规则覆盖主写、fallback、复制重试、provider 健康异常四大场景。
**依赖:** SKILL-001, MCP-004
**不做事项:** 不允许“盲删+盲切主”。
**风险:** 约束过松导致误操作；过严影响自动化效率。

## S3-001: Presigned URL

**优先级:** P1
**状态:** completed
**目标:** 支持 Presigned GET/PUT URL。
**Coding 指导:** 复用 SigV4 验签与过期策略；限制签名头集合。
**验收方法:** aws-cli/sdk 用 presign URL 上传下载。
**验收标准:** 在有效期内成功，过期/篡改签名明确失败。
**依赖:** S3 现有鉴权路径
**不做事项:** 不支持匿名长期 URL。
**风险:** 签名规范偏差造成 SDK 兼容问题。

## S3-002: Multipart Upload 状态模型

**优先级:** P0
**状态:** completed
**目标:** 定义 multipart upload/session/part 元数据模型。
**Coding 指导:** 在 metadata-store 增 `upload_id`、part etag、offset、ttl；崩溃可恢复。
**验收方法:** 单测覆盖创建、列 part、并发 part、恢复。
**验收标准:** 状态可持久化；重复上传 part 幂等。
**依赖:** metadata-store
**不做事项:** 不先做跨节点共享 upload session。
**风险:** 模型不完整导致 complete 阶段不可恢复。

## S3-003: Multipart bounded spool 与 complete/abort

**优先级:** P0
**状态:** completed
**目标:** 完成 bounded spool 的 multipart 写入与 complete/abort。
**Coding 指导:** 受 `CCBG_MAX_IN_MEMORY_OBJECT_BYTES` 与 spool 上限约束；complete 前校验 part 列表；abort 清理临时文件。
**验收方法:** 1GB+ 文件分片上传、异常中断、abort 清理检查。
**验收标准:** 不出现整对象内存缓存；complete 后对象可读；abort 后无脏文件。
**依赖:** S3-002
**不做事项:** 不绕过 spool 上限。
**风险:** 大文件并发触发磁盘耗尽。

## S3-004: CopyObject

**优先级:** P1
**状态:** completed
**目标:** 支持 `CopyObject` 并接入复制语义。
**Coding 指导:** 按网关 object-actions 语义映射；更新 replication 元数据。
**验收方法:** aws-cli `cp s3://a/x s3://b/y`；检查新对象与状态。
**验收标准:** copy 成功后 `HeadObject` 可见，复制队列状态正确。
**依赖:** 对象动作执行器
**不做事项:** 不先承诺服务端同 provider 零拷贝。
**风险:** 源对象读取失败时出现半成品状态。

## S3-005: Range GET

**优先级:** P1
**状态:** completed
**目标:** 完整支持单区间 `Range GET`。
**Coding 指导:** 处理 `bytes=start-end`、`start-`、`-suffix`；返回 `206` 与正确 `Content-Range`。
**验收方法:** curl + aws-sdk range 读取测试。
**验收标准:** RFC 行为正确；越界返回可预期错误。
**依赖:** GetObject 路径
**不做事项:** 不先做多区间 multipart/byteranges。
**风险:** provider 原生 range 能力不一致。

## S3-006: virtual-hosted-style bucket

**优先级:** P2
**状态:** completed
**目标:** 增加 virtual-hosted-style bucket 访问。
**Coding 指导:** 基于 Host 头解析 bucket；保留 path-style 默认。
**验收方法:** SDK 设置 addressing style 为 virtual 测试。
**验收标准:** path-style 与 virtual-style 并存；路由无歧义。
**依赖:** S3 路由层
**不做事项:** 不要求外部 DNS 自动化。
**风险:** 反向代理下 Host 重写导致签名不匹配。

## S3-007: S3 SDK/rclone/aws-cli smoke tests

**优先级:** P0
**状态:** completed
**目标:** 建立 S3 兼容 smoke 测试矩阵。
**Coding 指导:** 覆盖 aws-cli、AWS SDK Rust/Python、rclone 基本读写删、multipart、range。
**验收方法:** CI/本地脚本批量执行并产出报告。
**验收标准:** 主干场景全绿；失败场景有稳定错误码。
**依赖:** S3-001~S3-005
**不做事项:** 不把 provider 实网账号放进 CI。
**风险:** 不同客户端默认行为差异引入误报。

## PROVIDER-001: provider full 标准测试夹具

**优先级:** P0
**状态:** completed
**目标:** 抽取 provider `full` 标准测试夹具。
**Coding 指导:** 按 provider-completion-standard 维度建立通用测试 trait 与测试数据。
**验收方法:** 在 unicom/telecom/mobile 复用同一夹具运行。
**验收标准:** 三个 provider 输出统一完成度报告。
**依赖:** docs/provider-completion-standard.md
**不做事项:** 不把页面选择器硬编码进夹具。
**风险:** 夹具与真实 API 漂移。

## PROVIDER-002: Telecom family upload

**优先级:** P0
**状态:** completed
**目标:** 补齐 telecom family 容器上传。
**Coding 指导:** 复用 telecom multipart 能力；接入 family_id 映射和目录策略。
**验收方法:** family bucket 上传/下载 roundtrip。
**验收标准:** family 上传可用，元数据与 personal 一致。
**依赖:** provider-telecom 现有上传链路
**不做事项:** 不改 personal 已稳定路径。
**风险:** family token 失效或风控导致偶发失败。

## PROVIDER-003: Telecom rename/copy/move/delete 补齐

**优先级:** P0
**状态:** completed
**目标:** 完成 telecom 对象动作与删除能力对齐。
**Coding 指导:** 对齐 object-actions API；保证动作后 replication metadata 同步。
**验收方法:** 对同对象执行 rename/copy/move/delete 流程测试。
**验收标准:** 四类动作全部可用且可审计。
**依赖:** PROVIDER-001
**不做事项:** 不新增 telecom 专有动作 API。
**风险:** 上游动作接口幂等性差。

## PROVIDER-004: Mobile family view

**优先级:** P1
**状态:** completed
**目标:** 增加 mobile family scope 发现与容器映射。
**Coding 指导:** 健康检查返回 family 可用性与容量事实；仅在真实 `family_root_folder_id` 已配置/捕获且可列举时映射为稳定容器。
**验收方法:** `provider_health` 与对象列举验证。
**验收标准:** family 在 UI/API 可见且状态准确。
**依赖:** provider-mobile
**不做事项:** 不伪造容量数据。
**风险:** family 接口变更导致探测失真。

## PROVIDER-005: Mobile object actions

**优先级:** P1
**状态:** completed
**目标:** 为 mobile 补齐 rename/copy/move/delete。
**Coding 指导:** 落地 provider-native capability；覆盖 root_prefix 托管目录。
**验收方法:** 对象动作 API 集成测试。
**验收标准:** 成功/失败均有稳定错误映射；历史审计可回读。
**依赖:** PROVIDER-004
**不做事项:** 不绕过统一对象动作执行器。
**风险:** 移动云盘动作接口频繁改版。

## PROVIDER-006: provider probes/capabilities/catalog 对齐

**优先级:** P0
**状态:** completed
**目标:** 对齐 `provider-probes` / `provider-capabilities` / `browser-flows` 三层事实目录。
**Coding 指导:** 每个 provider 至少有 confirmed probe、稳定 capability、可替换 browser flow；文档同步更新。
**验收方法:** catalog lint + 启动自检。
**验收标准:** 缺失项会在 CI 失败；运行态能报告 catalog 版本。
**依赖:** PROVIDER-001
**不做事项:** 不把临时抓包结果直接写死代码。
**风险:** 目录版本不一致引发运行时歧义。

## AUTH-001: 独立 auth-broker crate

**优先级:** P0
**状态:** completed
**目标:** 把认证流程拆分为 `auth-broker` crate。
**Coding 指导:** 统一会话抽象、token 刷新、人工输入回调接口；与 gatewayd 解耦。
**验收方法:** crate 单测 + gatewayd 集成测试。
**验收标准:** 至少支持 telecom/unicom/mobile 的人工输入式认证编排。
**验收备注:** 已新增 `crates/auth-broker`，gatewayd 的 prompt/session 存储已切换到 broker 类型并保持现有流程测试通过。
**依赖:** 现有 auth-step-by-step 流程
**不做事项:** 不在 broker 内实现会话窃取。
**风险:** 认证状态机复杂导致边界漏测。

## AUTH-002: 短信/验证码/人工输入队列

**优先级:** P0
**状态:** completed
**目标:** 建立统一人工输入队列（手机号、短信码、图形码等）。
**Coding 指导:** 增加 queue item TTL、状态迁移、重试次数、审计字段。
**验收方法:** API + Web 交互联测，模拟超时与重复提交。
**验收标准:** 队列可回读、可过期、可取消；并发输入不串单。
**验收备注:** `auth-broker` prompt 队列已支持 `pending -> answered/expired/canceled`、默认 TTL（10 分钟）、attempt/retry 与审计字段；`gatewayd` 已在 list/get/reply 前触发过期迁移，新增 `/api/auth-capture/prompts/{prompt_id}/cancel`，并覆盖重复提交拒绝与跨 session 同名 input 不串单测试。
**依赖:** AUTH-001, ADMIN-001
**不做事项:** 不把验证码明文写入日志。
**风险:** 长时间挂起造成运营商会话失效。

## AUTH-003: session handoff 与 credential store 接入

**优先级:** P0
**状态:** completed
**目标:** 完成认证会话向 credential store 的交接与热更新。
**Coding 指导:** 统一 session 序列化格式；更新后触发 provider runtime refresh。
**验收方法:** 完成一次认证后重启服务验证会话可恢复。
**验收标准:** 凭证落盘、加载、轮换全流程可用。
**验收备注:** 已新增服务端 `POST /api/browser-flow/sessions/{session_id}/handoff`，仅允许 `unicom/telecom/mobile` 的 completed session，通过 provider-bridge `runtime_credential_mappings` 与 browser_profile 绑定写入 provider credential 文件；同时落盘 `credentials_dir/auth-session-handoffs/<safe-session-file>.json`（0600，且不含 token/cookie 明文，非安全文件名会改用 hash），并触发 lease reset/probe + backend rebuild。新增测试覆盖成功 handoff、重启后加载、token 轮换覆盖、pending/onedrive/无映射拒绝，以及 handoff audit 文件名防路径穿越。
**依赖:** AUTH-001
**不做事项:** 不把 session 直接放在前端 localStorage。
**风险:** handoff 失败导致“认证成功但运行态不可用”。

## ADMIN-001: admin-api contract 拆分

**优先级:** P0
**状态:** completed
**目标:** 将 admin contract 从 gatewayd 路由内抽象为独立 `admin-api` crate。
**Coding 指导:** 固化 DTO、版本与错误码；区分 operator API 与 internal API。
**验收方法:** contract 测试与向后兼容测试。
**验收标准:** Web/TUI/MCP 均可复用同一 contract。
**验收备注:** 已新增 `crates/admin-api` 并接入 workspace，提供 `ADMIN_API_VERSION`、`AdminApiSurface`、`AdminRouteContract`、`AdminDtoKind`、稳定 operator 端点常量（`/api/status`、`/api/control-plane/topology`、`/api/policy/auth-capture`、provider credentials、browser-flow session handoff）与 internal 认证端点常量（login/logout/change-password）；GET/POST 双向资源在合同中分别登记 typed request/response DTO kind。已迁移 Admin 登录/改密 DTO、拓扑 DTO、auth-capture policy DTO、provider credential DTO、browser-flow handoff DTO，以及通用 Admin status envelope 到 `admin-api`，并提供结构化错误合同 `AdminApiErrorResponse{ error, code?, api_version? }`；`gatewayd` 已引用新 crate 的路由常量、DTO 与错误响应模型，保持原有 `error` 字段不变，仅追加可机读 `code/api_version`。Provider credential 响应继续隐藏顶层 token/cookie/browser_id，并对 `browser_profile.headers` 做公开响应清洗（authorization/cookie/browser-id/token/session/password/sms/captcha 等不回显，`source_url` 去 query/fragment）。新增 `admin-api` 合同测试覆盖 operator/internal surface、双向路由、DTO kind、operator DTO JSON shape、error JSON shape 与 owned-client 反序列化、operator provider contract 不含 `onedrive`；新增 `gatewayd` 兼容测试验证 Admin 状态顶层字段、错误响应 JSON 兼容、provider credential browser profile 不泄露敏感 header。
**依赖:** 现有控制面接口
**不做事项:** 不修改 S3 数据面行为。
**风险:** contract 变更影响前端与 MCP。

## ADMIN-002: Admin Web 改为 API-only client

**优先级:** P1
**状态:** completed
**目标:** Admin Web 只通过 admin-api 访问，不直接依赖内部模块。
**Coding 指导:** 清理前端直连内部状态的路径；统一鉴权与错误展示。
**验收方法:** E2E 覆盖登录、provider 管理、复制重试、告警查看。
**验收标准:** Web 断开内部耦合；API 变更可通过版本控制。
**验收备注:** `gatewayd` Admin Web 已增加由 Rust 注入的 `adminApiRoutes/adminApi` 客户端层，统一注入 `ADMIN_API_VERSION` 与核心 contract route（status/login/logout/change-password/topology/auth-capture/provider credentials/browser-flow handoff template）；`fetchJson` 统一附带 `x-admin-api-version` 且在带 body 时自动补 `content-type`。ADMIN-001 范围内的关键调用已迁移为 `adminApi` helper（状态、改密、拓扑更新、auth-capture policy、provider credentials），保留其他未纳入合同的 legacy/internal endpoints，不新增隐藏接口；OneDrive 仍保持现有隐藏/parking 策略，未扩展到 visible provider 主线。新增 `admin_web_e2e_login_provider_retry_and_alert_workflow_uses_contract_api` HTTP workflow 测试，覆盖登录、受保护 Admin 页面、provider 凭据保存、复制失败重试和告警关闭。
**依赖:** ADMIN-001
**不做事项:** 不新增一次性“隐藏接口”。
**风险:** 迁移期可能出现字段不一致。

## TUI-001: SSH/低带宽 TUI

**优先级:** P2
**状态:** completed
**目标:** 交付低带宽可用的 TUI 管理界面。
**Coding 指导:** 复用 admin-api；提供 provider 状态、复制失败重试、告警查看三类核心操作。
**验收方法:** 纯 SSH 环境手工回归。
**验收标准:** 在高延迟终端可完成基本运维闭环。
**验收备注:** 已新增 `crates/admin-tui`（low-bandwidth CLI/TUI first slice，非全屏 curses），默认 `summary` 走 `GET /api/status` 输出 providers/replication/open alerts/failed jobs sample；支持 `providers`（仅 operator providers，隐藏 OneDrive）、`failed-jobs --limit N`、`retry-job <job_id>`（`POST /api/replication/jobs/{job_id}/retry`）、`alerts --limit N`、`suppress-alert <alert_id> [--title TITLE]`（`POST /api/alerts/suppressions`）。HTTP 头统一附带 `x-admin-api-version`，API key 使用 `x-api-key`，并支持 `--base-url`/`--api-key`/`--timeout-ms` 与环境变量回退（`CCBG_TUI_BASE_URL`/`CCBG_ADMIN_BASE_URL`、`CCBG_TUI_API_KEY`/`CCBG_CONTROL_API_KEY`）。`--base-url` 会拒绝空值、非 HTTP(S)、URL 内凭据、query 与 fragment；子命令会拒绝多余参数。TUI 输出不打印 token/cookie/password/sms/captcha/browser_id 等敏感字段，错误解析 `AdminApiErrorResponse` 并做敏感信息脱敏。
**实际验证命令:** `cargo fmt --all --check`；`git diff --check`；`cargo test -p admin-api`；`cargo test -p admin-tui`；`cargo test -p gatewayd admin_web`；`cargo test -p gatewayd`。
**依赖:** ADMIN-001
**不做事项:** 不在 TUI 内实现复杂图表。
**风险:** 终端兼容性与键位冲突。

## OPS-001: DLQ 死信体系

**优先级:** P0
**状态:** completed
**目标:** 为复制任务建立 DLQ（死信）与人工恢复流程。
**Coding 指导:** 定义入 DLQ 条件、回放策略、目标粒度重试；与 alerts 联动。
**验收方法:** 故障注入触发重复失败直到入 DLQ。
**验收标准:** DLQ 可查询、可筛选、可回放，且不丢审计。
**验收备注:** `metadata-store` 新增 `replication_dead_letters` 审计表与 `mark_job_dead_letter`/`list_dead_letter_jobs`/`count_dead_letter_jobs`/`open_dead_letter_target_counts`/`replay_dead_letter_job`/`replay_dead_letter_jobs_for_target`，保留原失败 job 审计并通过新 job_id 回放；DLQ original job 会被 retention prune 保护，避免因 replay 后原 failed job 不再是 latest 而丢审计；单条/批量回放都会拒绝覆盖已有 job id，target 批量回放在 metadata-store 单事务内完成。`gatewayd` 复制 worker 最终失败改为入 DLQ，新增 `GET /api/replication/dlq`、`POST /api/replication/dlq/jobs/{job_id}/replay`、`POST /api/replication/dlq/targets/{target}/replay`（单次最多回放 500 条，超出后可重复调用），并在 admin alerts 增加 `replication_dead_letter_queue_open`；`admin-api` 补齐 DLQ route contract 与 DTO。
**实际验证命令:** `cargo fmt --all --check`；`git diff --check`；`cargo test -p metadata-store`；`cargo test -p admin-api`；`cargo test -p gatewayd replication_dead_letter`；`cargo test -p gatewayd admin_alerts`；`cargo test -p gatewayd replication_jobs_stop_retrying_after_max_attempts`；`cargo test -p gatewayd`。
**依赖:** replication-engine, metadata-store
**不做事项:** 不无限自动重试。
**风险:** DLQ 膨胀导致存储与 UI 压力。

## OPS-002: historical reconcile executor

**优先级:** P1
**状态:** completed
**目标:** 实现历史对象显式收敛执行器。
**Coding 指导:** 按“先预检、再执行、删除后置”规则；支持 dry-run 与批次执行。
**验收方法:** 用小样本对象集做策略变更演练。
**验收标准:** 容量不足或 spool 不足时阻断；成功后状态一致。
**验收备注:** `gatewayd` 已落地 `GET /api/object-reconcile/preview`（只读预检）与 `POST /api/object-reconcile/execute`（支持 `dry_run` 与批次执行），执行器按预检状态对 `skipped/blocked/no_change/needs_changes` 分流；容量/本地缓冲预算复用 `reconcile_capacity_check`，在容量未知、spool 上限不足、spool 可用空间未知或不足时会阻断；`admin-api` 已公开 object reconcile preview/execute route contract 与 DTO；Admin 页面可对当前页可收敛项先 dry-run 再显式执行。`needs_changes` 的副本计划更新路径会先更新保护计划并排入新增/删旧 replication job，涉及 home/encryption 重写时先完成新 home 写入，再切换 placement/logical 元数据，最后删除旧 home 对象并记录审计历史。
**实际验证命令:** `cargo fmt --all --check`；`git diff --check`；`cargo test -p admin-api`；`cargo test -p gatewayd object_reconcile`；`cargo test -p gatewayd admin_web`；`cargo test -p admin-tui`。
**依赖:** docs/historical-object-reconcile-and-buffer-budget.md
**不做事项:** 不做自动全量迁移。
**风险:** 大批次任务时间长，失败恢复复杂。

## OPS-003: External KMS

**优先级:** P2
**状态:** completed
**目标:** 接入外部 KMS 管理密钥材料。
**Coding 指导:** 设计 KMS trait + provider；支持本地回退策略与轮换。
**验收方法:** mock KMS + 真机最小集成测试。
**验收标准:** 密钥可拉取、缓存、轮换；失败时服务可降级告警。
**验收备注:** `gatewayd` 已提供 `ExternalKmsProvider` 抽象与 `mock-env://`、`mock-file://` 本地 provider；`external_kms` 配置已接入 `load_profile_encryption_key_material`，支持按 profile/key/source_ref 维度缓存和 TTL 刷新（`CCBG_EXTERNAL_KMS_CACHE_TTL_MS`），并在刷新失败时优先回退到已缓存密钥、同时记录可见错误状态。Admin alerts 已新增 `external_kms_degraded`，runtime status 只暴露脱敏 locator、缓存时间和 key fingerprint，避免把密钥材料、env 名或本机路径写入日志/API/测试输出。补充了 `external_kms_file_source_supports_cache_refresh_and_rotation`、`external_kms_uses_stale_cache_on_refresh_failure_without_exposing_secrets`、`external_kms_status_redacts_env_and_file_locators`、`admin_status_surfaces_external_kms_degraded_alert` 覆盖缓存、轮换、降级、脱敏与可见性。当前真机验收按 local mock provider 冒烟，不声明已接真实云 KMS SDK。
**实际验证命令:** `cargo fmt --all --check`；`git diff --check`；`cargo check -p gatewayd`；`cargo test -p gatewayd external_kms`；`cargo test -p gatewayd encryption`；`cargo test -p admin-api`。
**依赖:** object-crypto
**不做事项:** 不把主密钥明文落盘。
**风险:** 外部 KMS 不可用导致写路径阻断。

## OPS-004: remote WAL 扩展到 delete/object-actions/reconcile

**优先级:** P1
**状态:** completed
**目标:** 将 remote WAL 从 PUT 扩展到 delete、对象动作、reconcile。
**Coding 指导:** 扩展 tx phase 与 replay 规则，保证崩溃恢复可重建最终元数据。
**验收方法:** 崩溃注入 + 重启 replay 回归。
**验收标准:** 三类事务可恢复；`checkpoint_lsn` 清理规则不变。
**验收备注:** 已完成：`gatewayd` 的 remote WAL record 新增 `operation`（`put/delete/metadata_sync`）并允许 `logical_record` 可选；S3 `DELETE Object`、admin object action 的 delete/rename/copy/move、object reconcile 的 `replica_plan_only` 与 `rewrite_home` 均接入 prepare→committed WAL。prepare 写失败会阻断对应事务，避免业务成功但缺少恢复起点；commit 写失败时保留 prepare 记录，重启 replay 可按远端最终状态补偿 metadata 与 replication 意图。replay 已覆盖 delete 去重、metadata sync 补元数据、prepare 丢 commit 的对象动作/reconcile 恢复；`PUT` WAL 与 checkpoint 清理规则保持原样（仍只清理 checkpoint 以内 committed 记录，且不记录对象字节）。
**实际验证命令:** `cargo fmt --all --check`；`git diff --check`；`cargo test -p gatewayd gateway_write_ahead_log -- --nocapture`；`cargo test -p gatewayd object_actions -- --nocapture`；`cargo test -p gatewayd object_reconcile -- --nocapture`；`cargo test -p gatewayd replayable_wal_metadata -- --nocapture`；`cargo test -p gatewayd critical_delete -- --nocapture`；`cargo test -p admin-api`。
**依赖:** docs/gateway-write-ahead-log.md, OPS-002
**不做事项:** 不记录对象字节到 WAL。
**风险:** replay 规则错误导致状态回滚异常。

## OPS-005: backup/restore 演练任务

**优先级:** P1
**状态:** completed
**目标:** 固化备份恢复演练脚本与操作手册。
**Coding 指导:** 覆盖 checkpoint、credential、WAL、metadata 恢复；输出演练报告模板。
**验收方法:** 月度演练一次，记录 RTO/RPO。
**验收标准:** 可在离线演练目录完成 checkpoint/credential/WAL/metadata 恢复证据校验并生成结构化报告；真实新宿主 LXC 安装与服务启动 smoke 由操作者在测试机手动执行。
**验收备注:** 已新增 `scripts/check-backup-restore-drill.py` 离线演练校验脚本，固定校验输入目录中的 checkpoint/credential/WAL/metadata/report 五类证据，并对 `checkpoint_lsn` 与 `replay_from_lsn`、checkpoint 后 committed WAL 记录、凭据清单去敏、metadata 关键计数字段做一致性检查；脚本支持 `--write-sample` 生成最小离线样例，便于 CI 与桌面 smoke，且缺文件/字段错误也会输出 `drill-check-result.json` 作为失败证据。`docs/gateway-backup-and-restore.md` 已补 `OPS-005` 演练输入结构、步骤与验收口径；新增 `docs/backup-restore-drill-report-template.md` 作为月度演练报告模板（含 RTO/RPO、异常与改进项）。本任务按“离线校验优先”执行，不包含 LXC/实机安装步骤。
**实际验证命令:** `python3 -m py_compile scripts/check-backup-restore-drill.py`；构造 `target/backup-restore-drill/` 最小样例并执行 `python3 scripts/check-backup-restore-drill.py --drill-root target/backup-restore-drill`；`python3 scripts/check-backup-restore-drill.py --drill-root target/backup-restore-drill-smoke --write-sample`；`git diff --check`。
**依赖:** gateway-backup-and-restore, OPS-004
**不做事项:** 不只做文档不做演练。
**风险:** 演练环境与生产差异导致假阳性。

## OBS-001: metrics/alerts/notify 生产化

**优先级:** P0
**状态:** completed
**目标:** 完成指标、告警阈值、通知签名链路生产化。
**Coding 指导:** 固化核心 SLI（可用性、复制延迟、DLQ 数、fallback 命中）；补齐告警抑制和去重。
**验收方法:** Prometheus 抓取 + webhook 联测。
**验收标准:** 告警不风暴；关键故障 1 分钟内可见。
**实现备注:** `/metrics` 已暴露 provider health、复制最老年龄、DLQ 总数/目标分组、data-plane fallback 读命中；Admin operations overview 显示 Open DLQ 与 Fallback Reads；notify 告警指纹按稳定字段排序后去重。
**实际验证命令:** `cargo fmt --all --check`；`cargo test -p gatewayd metrics_ -- --nocapture`；`cargo test -p gatewayd notify_webhook -- --nocapture`；`cargo test -p gatewayd object_reads_fallback_to_sync_target_and_mark_response_headers -- --nocapture`；`cargo test -p gatewayd alert_fingerprint -- --nocapture`；`cargo test -p gatewayd prometheus_label_value -- --nocapture`；`cargo test -p gatewayd metrics_include_nonzero_replication_delay_sli_values -- --nocapture`；`cargo test -p gatewayd notify_webhook_retries_unchanged_alerts_after_failed_delivery -- --nocapture`；`cargo test -p gatewayd admin_web_contract_routes_are_injected_and_key_calls_use_helpers -- --nocapture`；`cargo test -p gatewayd operations_overview_surfaces_queue_age_and_notify_freshness -- --nocapture`；`cargo test -p gatewayd admin_alerts_include_open_dead_letter_queue_summary -- --nocapture`；`cargo test -p gatewayd admin_status_surfaces_data_plane_transfer_totals -- --nocapture`。
**依赖:** 现有 `/metrics` `/healthz` notify
**不做事项:** 不把 debug 指标直接当生产告警。
**风险:** 阈值不当导致漏报或噪声。

## PLATFORM-001: Docker/Podman CI smoke

**优先级:** P1
**状态:** completed
**目标:** 在 CI 执行 Docker/Podman 构建与启动 smoke。
**Coding 指导:** 复用 `deploy/Dockerfile` 与 `deploy/Containerfile`；最小健康检查。
**验收方法:** CI job 拉起容器并跑 S3 基础请求。
**验收标准:** 双镜像构建成功，启动后健康检查通过。
**实现备注:** CI 新增 `container-smoke` matrix，分别用 Dockerfile/Docker 与 Containerfile/Podman 构建镜像；`scripts/container-smoke.py` 会用 stub provider、空 sync/fallback、`CCBG_ONEDRIVE_ENABLED=false` 启动容器，检查 `/healthz` 并执行 SigV4 `ListBuckets`。容器镜像现在复制 `config/` catalog，并为非 root `gateway` 用户准备可写 `data` 与 `body-spool` 目录。工作流支持 `workflow_dispatch`，可复用现有 GitHub PAT/Actions 手动触发模式。
**实际验证命令:** `python3 -m py_compile scripts/container-smoke.py scripts/license-check.py scripts/catalog-lint.py`；`python3 scripts/container-smoke.py --help`；workflow YAML 解析检查；`git diff --check`。本机有 Podman 但无 Docker；Podman 真机构建已验证进入 `rust:1.94-bookworm` 拉取阶段，因首次拉取 builder 镜像超过本轮验证窗口中止，完整双运行时验收交由 GitHub Actions `container-smoke` 执行。
**依赖:** CI workflow
**不做事项:** 不在 CI 使用真实 provider 凭证。
**风险:** runner 环境差异导致 flaky。

## PLATFORM-002: PVE/LXC 部署包验收

**优先级:** P1
**状态:** completed
**目标:** 形成 PVE/LXC 标准部署包与验收清单。
**Coding 指导:** 固化目录挂载、端口、日志、备份点与升级流程。
**验收方法:** 新建 LXC 一键部署并跑 smoke。
**验收标准:** 可复现部署；回滚流程可执行。
**实现备注:** 新增 `deploy/lxc/` 标准包模板，包含默认安全 `ccbg.env`、systemd unit、install/rollback/smoke 脚本；新增 `scripts/build-lxc-package.sh` 生成 `target/lxc-package/ccbg-lxc-package.tar.gz`、tarball SHA256 与包内 `MANIFEST.sha256`；新增 `docs/pve-lxc-deployment.md` 固化 LXC 目录挂载、端口、日志、备份点、升级、回滚和验收流程。默认包使用 `stub`、空 sync/fallback、`CCBG_ONEDRIVE_ENABLED=false`，不包含真实 provider 凭证。
**实际验证命令:** `bash -n scripts/build-lxc-package.sh deploy/lxc/install.sh deploy/lxc/rollback.sh deploy/lxc/smoke.sh`；`scripts/build-lxc-package.sh --skip-build`；`sha256sum -c target/lxc-package/ccbg-lxc-package.tar.gz.sha256`；检查包内 `MANIFEST.sha256`、`etc/ccbg.env`、`systemd/ccbg.service`、`scripts/install.sh`、`scripts/rollback.sh`、`scripts/smoke.sh`；`python3 scripts/license-check.py --skip-cargo-metadata`。
**依赖:** docs/router-deployment-guide.md
**不做事项:** 不把手工步骤留成口头流程。
**风险:** 容器网络与风控 IP 绑定问题。

## PLATFORM-003: OpenWRT arm64 lite-host

**优先级:** P1
**状态:** completed
**目标:** 交付 OpenWRT arm64 轻量宿主配置。
**Coding 指导:** 默认关闭重型组件，限制并发和缓存，启用 stdio MCP 优先。
**验收方法:** OpenWRT 实机运行 24h 稳定性测试。
**验收标准:** 内存受控、无频繁 OOM、基础 S3 可用。
**实现备注:** 新增 `deploy/openwrt/` OpenWRT lite 包模板，包含 procd init、安装脚本与 smoke 脚本；新增 `scripts/build-openwrt-lite-package.sh`，生成 `target/openwrt-lite/ccbg-openwrt-lite.tar.gz`、tarball SHA256 与包内 `MANIFEST.sha256`。默认配置更新为 `stub` 起步、`CCBG_ADMIN_MODE=terminal` loopback-only control API、`CCBG_DATA_PLANE_MAX_IN_FLIGHT=2`、`CCBG_DATA_PLANE_MAX_REQUESTS_PER_SECOND=8`、`CCBG_MAX_IN_MEMORY_OBJECT_BYTES=4194304`、`MCP_SERVER_HTTP_ENABLED=false`；`mcp-server` 作为 stdio 二进制随包交付，客户端不需要 Rust 环境。OneDrive 保持 `CCBG_ONEDRIVE_ENABLED=false` 与 `CCBG_ONEDRIVE_REPLICATION_ENABLED=false`。新增 `docs/openwrt-lite-deployment.md` 固化安装路径、MCP 配置、smoke 和 24 小时稳定性验收清单；同步更新 `docs/openwrt-host-profile.md` 的资源边界描述。
**实际验证命令:** `bash -n scripts/build-openwrt-lite-package.sh deploy/openwrt/install.sh deploy/openwrt/smoke.sh deploy/openwrt/ccbg.init`；`scripts/build-openwrt-lite-package.sh`；`scripts/build-openwrt-lite-package.sh --skip-build`；`sha256sum -c target/openwrt-lite/ccbg-openwrt-lite.tar.gz.sha256`；包内 `sha256sum -c MANIFEST.sha256`；检查包内 `etc/openwrt-lite.env` 含 `CCBG_PRIMARY_PROVIDER=stub`、`CCBG_ONEDRIVE_ENABLED=false`、`CCBG_ONEDRIVE_REPLICATION_ENABLED=false`、`MCP_SERVER_HTTP_ENABLED=false`；`printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}\n' | target/release/mcp-server`；`cargo test -p mcp-server`；使用 OpenWRT lite env 约束在本地端口启动 `gatewayd` 并执行 `deploy/openwrt/smoke.sh`；`python3 scripts/license-check.py --skip-cargo-metadata`；`git diff --check`。
**依赖:** docs/openwrt-host-profile.md
**不做事项:** 不默认启用 Admin Web 全功能。
**风险:** flash 写放大与 SQLite 寿命。

## PLATFORM-004: full/lite/esp feature profiles

**优先级:** P0
**状态:** completed
**目标:** 增加 `full/lite/esp-client/esp-relay` feature profiles。
**Coding 指导:** cargo feature 分层隔离 `rusqlite/reqwest/tower-http` 等重依赖。
**验收方法:** 分 profile 编译矩阵。
**验收标准:** 四档均可编译；`esp-*` 不引入重型依赖。
**实现备注:** 新增 `crates/platform-profiles` 作为 profile contract crate，定义互斥的 `full-host`、`lite-host`、`esp-client`、`esp-relay` 四档，并暴露 `ACTIVE_PROFILE` 能力边界；`gatewayd` 新增 `full-host` 与 `lite-host` host feature，默认仍为 `full-host`。新增 `scripts/check-feature-profiles.sh`，对四档 profile 执行编译/测试矩阵，对 `gatewayd` 执行 `full-host` 与 `lite-host` 编译，并用 `cargo tree` 验证 `esp-client` / `esp-relay` 不拉入 `rusqlite`、`reqwest`、`tower-http`、`axum`。新增 `docs/feature-profiles.md` 明确 host profile 产出 daemon，ESP profile 只作为 MCU client/relay 合同，不继承完整 daemon。
**实际验证命令:** `scripts/check-feature-profiles.sh`；`cargo fmt --all --check`；`python3 scripts/license-check.py --skip-cargo-metadata`；`git diff --check`。`gatewayd` check 仍有既存 dead_code warning：`ObjectActionInput` 若干字段、browser flow helper、backup archive header、`list_objects_v2` 未使用。
**依赖:** docs/esp32-s3-profile.md
**不做事项:** 不维持“单二进制覆盖全部设备”的假设。
**风险:** feature 交叉组合过多导致维护成本升高。

## PLATFORM-005: STM32 client-only 示例

**优先级:** P2
**状态:** completed
**目标:** 提供 STM32 作为 S3/MCP 调用端的最小示例。
**Coding 指导:** 给出请求签名、重试、超时、分块上传示例代码与限制。
**验收方法:** 板级 demo 上传/下载小对象。
**验收标准:** 文档+示例可复现；明确非宿主定位。
**实现备注:** 新增 `examples/stm32-client-only/`，提供无动态内存的 C client-only 示例：固定 SigV4 header 签名头集合、`UNSIGNED-PAYLOAD` streaming `PutObject`、`HeadObject`、`GetObject`、有界 retry/timeout、固定 chunk/object 上限；板级工程通过 callback 注入 SHA256/HMAC/UTC/HTTP transport。新增 `docs/stm32-client-only.md` 与兼容矩阵链接，明确 STM32 不运行 `gatewayd`、不保存 provider 凭证、不承载 SQLite/OneDrive/复制/Admin/MCP stdio。新增 `scripts/check-stm32-client-example.sh` 用 host C 编译器执行 `-Wall -Wextra -Werror -pedantic` 编译和 fake transport 运行检查；`scripts/license-check.py` 已把 `.c/.h` 纳入 SPDX 检查。
**实际验证命令:** `scripts/check-stm32-client-example.sh`；`python3 scripts/license-check.py --skip-cargo-metadata`；`git diff --check`。
**依赖:** compatibility-matrix
**不做事项:** 不在 STM32 上承载 daemon。
**风险:** MCU TLS/网络栈兼容问题。

## PLATFORM-006: ESP32-S3 client-only 示例

**优先级:** P2
**状态:** completed
**目标:** 提供 ESP32-S3 client-only 参考实现。
**Coding 指导:** 限制单并发与小缓冲；实现最小 `Head/Get/Put`。
**验收方法:** `esp-idf` 示例联调本地网关。
**验收标准:** 在预算内稳定跑通三类请求。
**实现备注:** 新增 `examples/esp32-s3-client-only/` ESP-IDF 示例，复用 `examples/stm32-client-only/ccbg_stm32_client.c` portable client 核心，用 mbedTLS 实现 SHA256/HMAC-SHA256，用 `esp_http_client` 实现 HTTP transport；默认 `CCBG_ESP32S3_IO_CHUNK_BYTES=1024`、`CCBG_ESP32S3_MAX_OBJECT_BYTES=32 KiB`、单请求在途、有界 timeout/retry，并执行 `PutObject`、`HeadObject`、`GetObject`。新增 `docs/esp32-s3-client-only.md` 与 `docs/esp32-s3-profile.md` 链接，明确 ESP32-S3 client-only 不包含 `gatewayd`、SQLite、replication、OneDrive、provider credentials、Admin Web、MCP stdio。新增 `scripts/check-esp32-s3-client-example.py` 做结构验收，防止示例误引入 host 依赖/能力。当前环境无 `idf.py`，未执行 ESP-IDF build 或板级真机三请求联调。
**实际验证命令:** `python3 -m py_compile scripts/check-esp32-s3-client-example.py`；`scripts/check-esp32-s3-client-example.py`；`scripts/check-stm32-client-example.sh`；`python3 scripts/license-check.py --skip-cargo-metadata`；`git diff --check`。
**依赖:** PLATFORM-004
**不做事项:** 不接入本地复制引擎。
**风险:** 内存峰值超预算。

## PLATFORM-007: ESP32-S3 relay-lite 可行性任务

**优先级:** Parking
**状态:** completed
**目标:** 评估 ESP32-S3 relay-lite 的工程可行性。
**Coding 指导:** 仅做 PoC：单 provider、单并发、无 SQLite、无 OneDrive。
**验收方法:** 资源占用测量与故障注入报告。
**验收标准:** 给出 go/no-go 结论与预算边界。
**实现备注:** 新增 `examples/esp32-s3-relay-lite-poc/` 边界 PoC，以固定 `1024` byte chunk、`64 KiB` max object、单 provider callback、单请求在途验证 relay-lite 的最小接口形态；PoC 使用内存 provider，只验证资源/API 边界，不接真实运营商 provider。新增 `docs/esp32-s3-relay-lite-feasibility.md` 给出 go/no-go：client-only 继续 go；relay-lite 仅作为后续真实 provider chunk callback 实验 conditional go；完整 daemon on ESP32-S3 no-go。新增 `scripts/check-esp32-s3-relay-lite-poc.sh` 编译运行 PoC 并拦截 C/H 中的 `onedrive`、`rusqlite`、`gatewayd`、replication host 引用。
**实际验证命令:** `scripts/check-esp32-s3-relay-lite-poc.sh`；`scripts/check-stm32-client-example.sh`；`scripts/check-esp32-s3-client-example.py`；`python3 scripts/license-check.py --skip-cargo-metadata`；`git diff --check`。
**依赖:** PLATFORM-004, PLATFORM-006
**不做事项:** 不承诺近线产品化。
**风险:** 功能剪裁过大导致价值不足。

## RELEASE-001: 个人非商业源码审查流程

**优先级:** P1
**状态:** completed
**目标:** 固化“个人非商业源码审查申请”流程。
**Coding 指导:** 统一申请模板、审批步骤、导出包内容范围。
**验收方法:** 用模拟申请走通一次。
**验收标准:** 流程可重复且可审计。
**实现备注:** 新增 `.github/ISSUE_TEMPLATE/personal-source-review.yml` 公开申请模板，明确 90 天真实个人使用、个人非商业用途、不得商业/托管/再分发/共享/移除声明；新增 `docs/personal-source-review.md` 固化 eligibility、intake、review steps、export scope、grant terms summary 与模拟方法，并在 `docs/github-publication.md` 链接；新增 `scripts/source-review-flow.py --simulate`，用模拟个人申请生成 `target/source-review-flow/simulated-personal-source-review-decision.json`，记录 grant id、请求 fingerprint、决策、scope、`source_exported=false` 与下一步 RELEASE-002 包流程。
**实际验证命令:** `python3 -m py_compile scripts/source-review-flow.py`；`scripts/source-review-flow.py --simulate`；检查 `target/source-review-flow/simulated-personal-source-review-decision.json`；`python3 scripts/license-check.py --skip-cargo-metadata`；`git diff --check`。
**依赖:** COMMERCIAL-LICENSE / docs/github-publication.md
**不做事项:** 不开放默认全量源码直出。
**风险:** 流程不清导致法律与交付风险。

## RELEASE-002: 审查包 fingerprint/manifest

**优先级:** P1
**状态:** completed
**目标:** 为审查包生成 fingerprint 与 manifest。
**Coding 指导:** 对包内文件做哈希清单，记录生成时间、提交号、构建环境。
**验收方法:** 重复打包比对一致性。
**验收标准:** fingerprint 可复核；清单可追踪。
**实现备注:** 新增 `config/source-review-package.json` allowlist，限定个人审查包默认包含法律文件、build metadata、selected source、示例与相关文档；新增 `scripts/build-source-review-package.py`，要求输入 RELEASE-001 生成的 approved grant decision，生成 deterministic `.tar`、`.tar.sha256` 和 `SOURCE-REVIEW-MANIFEST.json`。Manifest 记录 grant id、request fingerprint、git commit、Python/platform build environment、包内文件 size/sha256、file count 与 `package_fingerprint_sha256`。脚本使用固定 tar metadata 与 `--source-date-epoch` 支持重复构建比对。
**实际验证命令:** `python3 -m py_compile scripts/build-source-review-package.py scripts/source-review-flow.py`；`scripts/source-review-flow.py --simulate`；两次执行 `scripts/build-source-review-package.py --decision target/source-review-flow/simulated-personal-source-review-decision.json --out-dir target/source-review-package-{a,b} --source-date-epoch 0`；比对两次 `ccbg-personal-source-review.tar.sha256` 一致；`tar -tf target/source-review-package-a/ccbg-personal-source-review.tar`；`python3 scripts/license-check.py --skip-cargo-metadata`；`git diff --check`。
**依赖:** RELEASE-001
**不做事项:** 不遗漏 license/notice 元信息。
**风险:** 非确定性打包导致哈希漂移。

## RELEASE-003: release provenance 自动化

**优先级:** P1
**状态:** completed
**目标:** 自动生成 release provenance 证明。
**Coding 指导:** 在 CI 注入 commit、构建脚本、产物哈希链；落 `PROVENANCE.md` 更新流程。
**验收方法:** 发布演练并校验 provenance。
**验收标准:** 发布产物均可追溯到源码提交与构建步骤。
**实现备注:** 新增 `scripts/generate-release-provenance.py`，对指定 release artifact 生成 `release-provenance.json` 与 `release-provenance.md`，记录 release name/tag/fingerprint、canonical repo、git commit、dirty 状态、GitHub Actions context、Python/platform build environment、build steps、artifact size/sha256，以及覆盖证据链的 `provenance_sha256`。`PROVENANCE.md` 新增 Automation 章节说明用法；CI `rust` job 新增 `Release provenance smoke`，对 `Cargo.toml` 与 `PROVENANCE.md` 生成 smoke provenance，确保脚本在 Actions 中可运行并捕获 GitHub context。
**实际验证命令:** `python3 -m py_compile scripts/generate-release-provenance.py`；`scripts/generate-release-provenance.py --release-name ci-smoke --tag local --artifact Cargo.toml --artifact PROVENANCE.md --build-step 'cargo test --workspace' --build-step 'python3 scripts/license-check.py' --out-dir target/release-provenance-smoke --source-date-epoch 0`；检查 `target/release-provenance-smoke/release-provenance.json` 与 `.md`；`python3 scripts/license-check.py --skip-cargo-metadata`；`git diff --check`。
**依赖:** RELEASE-002
**不做事项:** 不使用人工补填 provenance。
**风险:** CI 环境变量污染影响可信度。

## RELEASE-004: SPDX/NOTICE/license CI 检查

**优先级:** P0
**状态:** completed
**目标:** 建立 SPDX/NOTICE/license 自动检查。
**Coding 指导:** 扫描依赖许可证、NOTICE 完整性、仓库声明一致性。
**验收方法:** CI 注入违规样例验证能拦截。
**验收标准:** 许可证缺失/冲突会阻断合并。
**验收备注:** 已新增 `scripts/license-check.py`，检查根法律文件、NOTICE/PROVENANCE/TRADEMARKS、代码和 public Cloudflare 静态源文件 SPDX、Cargo manifest `license-file` 边界、误写 `license = "MIT"`、以及 `cargo metadata --locked` 暴露的 registry 依赖 license/license_file 元数据。脚本内置 `--self-test`，用临时 fixture 验证合规样例通过、缺 SPDX 和 Cargo MIT 违规会失败；CI 已在测试前执行 `python3 scripts/license-check.py`。
**实际验证命令:** `python3 scripts/license-check.py --self-test`；`python3 scripts/license-check.py --skip-cargo-metadata`；`python3 scripts/license-check.py`。
**依赖:** CI workflow
**不做事项:** 不把检查仅放本地手工执行。
**风险:** 第三方依赖元数据不完整。

## RELEASE-005: Cloudflare public fingerprint 验收

**优先级:** P2
**状态:** completed
**目标:** 验收 `public/cloudflare` 公共材料 fingerprint 发布链路。
**Coding 指导:** 对静态文件生成 manifest+hash，发布前后对比。
**验收方法:** 本地构建与 Cloudflare 发布后的 hash 校验。
**验收标准:** 公网可校验 fingerprint 与仓库清单一致。
**实现备注:** 新增 `scripts/check-cloudflare-public-fingerprint.py`，读取 `public/cloudflare/.well-known/ccbg-provenance.json` 并与根 `PROVENANCE.md` 的 fingerprint/SHA 对齐，检查 fingerprint 出现在 HTML、manifest、well-known provenance、`_headers`、`assets/app.js`、source map、release note、README，扫描公开目录禁止明显私有/核心/凭证类产物，并生成 `target/cloudflare-public-fingerprint/public-cloudflare-fingerprint-manifest.json`（每个静态文件 size/sha256 与 manifest hash）。脚本支持 `--deployed-base-url`，可在 Cloudflare Pages 发布后逐文件拉取并比对 SHA-256。CI 新增 `Cloudflare public fingerprint check`。新增 `docs/cloudflare-public-fingerprint.md` 固化本地与发布后验收命令。
**实际验证命令:** `python3 -m py_compile scripts/check-cloudflare-public-fingerprint.py`；`scripts/check-cloudflare-public-fingerprint.py`；检查 `target/cloudflare-public-fingerprint/public-cloudflare-fingerprint-manifest.json`；`python3 scripts/license-check.py --skip-cargo-metadata`；`git diff --check`。未执行公网 `--deployed-base-url`，需要 Cloudflare Pages URL 后运行。
**依赖:** RELEASE-002
**不做事项:** 不把私有核心产物发布到公开目录。
**风险:** CDN 缓存导致版本混淆。

## XREG-001: AGI2030 统一注册入口对齐

**优先级:** P0
**状态:** completed
**目标:** 将 CCBG 的商业授权与个人源码审查公开入口收敛到 `register.agi2030.online`，避免 GitHub issue、项目站和注册中心形成并行申请入口。
**Coding 指导:** 核心网关运行时、Admin 本地控制面、provider 授权和 OneDrive/Microsoft App 注册流程不依赖 AGI2030 注册中心；只调整公开站点和源码审查 intake 文档。GitHub issue 模板保留为 fallback triage，不再作为 canonical intake。
**验收方法:** 审计 `public/cloudflare/index.html`、`public/cloudflare/README.md`、`docs/personal-source-review.md` 和 `.github/ISSUE_TEMPLATE/personal-source-review.yml` 中的授权/源码审查入口；确认 `register.agi2030.online` 未被写入 gatewayd 运行时鉴权链路。
**验收标准:** 商业授权和个人源码审查的公开入口均指向 AGI2030 Identity Center；GitHub issue 只标记为兜底分流；Microsoft/OneDrive 应用注册文案不被误改。
**实现备注:** 公开站点授权卡片新增 `https://register.agi2030.online/?product=carrier-cloud-blob-gateway&intent=commercial-authorization` 与 `intent=personal-source-review`；个人源码审查文档改为以 AGI2030 Identity Center 为 canonical intake，GitHub issue 模板提示仅作为 fallback triage。
**实际验证命令:** `rg -n "register\\.agi2030\\.online|personal-source-review|commercial-authorization" public docs .github`；`python3 scripts/license-check.py`；`python3 scripts/check-cloudflare-public-fingerprint.py`。
**依赖:** RELEASE-001, RELEASE-005, CF-001
**不做事项:** 不把用户云盘凭据、gateway 管理登录或 provider OAuth 授权接入注册中心。
**风险:** `register.agi2030.online` 当前 `HEAD /` 返回 404；若外部监控使用 HEAD 健康检查，需要注册项目补 `/healthz` 或修正 HEAD 行为。

## PARKING-ONEDRIVE-001: OneDrive 默认禁用与文档降噪

**优先级:** Parking
**状态:** completed
**目标:** 把 OneDrive 在默认配置与文档中降为禁用/隐藏。
**Coding 指导:** 默认不加入近线 sync/fallback 示例；在文档中标记为 parking optional provider。
**验收方法:** 配置样例与文档审计。
**验收标准:** 新手按默认流程不会启用 OneDrive。
**实现备注:** `config/example.env` 已把 `CCBG_SYNC_TARGETS` 与 `CCBG_FALLBACK_READ_ORDER` 默认改为空，`CCBG_ONEDRIVE_REPLICATION_ENABLED=false`，并把 OneDrive 文案降级为 parking optional provider；软路由部署文档同步改为默认不启用、不加入 sync/fallback。新增 `scripts/check-onedrive-parking.py` 审计默认 env 与关键部署文档，阻断“默认建议 OneDrive/默认备份/默认 fallback”表述回流；CI 已接入该检查。
**实际验证命令:** `python3 -m py_compile scripts/check-onedrive-parking.py scripts/check-onedrive-restore-checklist.py`；`python3 scripts/check-onedrive-parking.py`。
**依赖:** 无
**不做事项:** 不删除已有 OneDrive 实现代码。
**风险:** 旧文档残留“默认备份”表述。

## PARKING-ONEDRIVE-002: OneDrive 真实需求出现后的恢复清单

**优先级:** Parking
**状态:** completed
**目标:** 准备 OneDrive 重新启用 checklist（仅在真实需求触发）。
**Coding 指导:** 列出开关、鉴权、探测、复制、告警、回归测试步骤。
**验收方法:** 桌面演练一次 checklist（不默认上线）。
**验收标准:** 清单完整、可执行、可回滚。
**实现备注:** 新增 `docs/onedrive-parking-restore-checklist.md`，按 Product、Configuration、Authentication、Provider Probe、Replication/Fallback、Observability、Regression、Rollback 八个 gate 固化恢复步骤；明确包默认值仍保持 `CCBG_ONEDRIVE_ENABLED=false` 与 `CCBG_ONEDRIVE_REPLICATION_ENABLED=false`，且 OneDrive 不能在未完成回归前进入默认 sync/fallback。新增 `scripts/check-onedrive-restore-checklist.py` 审计 checklist 必需章节、默认关闭声明、回归命令与回滚步骤；CI 已接入该检查。
**实际验证命令:** `python3 -m py_compile scripts/check-onedrive-parking.py scripts/check-onedrive-restore-checklist.py`；`python3 scripts/check-onedrive-restore-checklist.py`。
**依赖:** PARKING-ONEDRIVE-001
**不做事项:** 不纳入近期里程碑。
**风险:** checklist 过期与 Graph 行为漂移。

## ADMIN-002: Admin 中文化补齐与英文残留清理

**优先级:** P0
**状态:** completed
**目标:** 补齐 Admin 中文文案，重点清理运营商探测窗口、Mobile 登录助手、Limit Probe、日志与诊断中的英文残留。
**Coding 指导:** 优先复用现有 `tr(...)`、`UI_EXACT_TEXT`、`ui_language` 机制，不新增第二套本地化框架；把现有硬编码英文提示、按钮文案、空态说明、错误解释入口统一收口到可翻译字典，避免只改 DOM 初始文本却漏掉运行态 feedback。
**验收方法:** 切换 Admin 到中文后，逐页检查 Dashboard、Providers、Browser/CDP、LLM、Object Browser、Monitoring、Logs；用 `rg` 审计 `index.html` 中和 Mobile/Probe/Log 相关的英文长句。
**验收标准:** 中文模式下只保留专有名词、协议名和必要缩写；运营商探测窗口和 Mobile 助手不再出现整段英文操作提示；运行态 toast/feedback 与空态说明同样可中文显示。
**实现备注:** 复用现有 `UI_EXACT_TEXT` 与 `localizeUiMessage()`，补齐首屏状态、Dashboard 摘要、日志/诊断、SMB 管理、OneDrive OAuth 引导、Mobile/Probe 相关静态提示的中文映射；同时让 topology、auth-capture browser probe、alerts、object placement/reconcile 和 OneDrive setup 的反馈横幅走本地化路径。未改 gateway 运行时逻辑，也未提前处理 Dashboard 布局、Mobile 凭据状态或日志 API 功能。
**实际验证命令:** Admin HTML 内嵌脚本 `node --check`；`git diff --check`；`python3 scripts/license-check.py`；`cargo test -p gatewayd admin_web_contract_routes_are_injected_and_key_calls_use_helpers`；`cargo test -p gatewayd admin_page_exposes_object_actions_panel`。
**依赖:** 现有 Admin HTML 拆分
**不做事项:** 不做多语言文案重写工具链。
**风险:** 运行态错误文案遗漏，导致页面初始中文但交互后回退英文。

## ADMIN-003: Dashboard 顶栏布局、失败对象明细与首页文案

**优先级:** P0
**状态:** completed
**目标:** 调整首页顶栏布局，补齐失败对象明细展示，并更新首页主文案。
**Coding 指导:** 维持现有 status grid 结构，基础宽度改为对象动作表 `4/12`、`Carrier I/O - Start Delay` `2/12`、`Carrier I/O - Rolling / Large Transfer` `6/12`，同步调整响应式断点；首页 hero 文案改为“把运营商网盘封装成S3块存储和SMB文件共享”；失败对象统一显示 provider、target、object、action、failed_at、message，优先前端消费现有 `monitoring.latest_failed_objects/recent_failures`，只有确实缺字段时再补后端。
**验收方法:** 桌面和窄屏分别打开 Admin 首页；检查 Start Delay 卡片明显收窄、右侧 Rolling/Large Transfer 不再拥挤；制造至少一条失败对象，验证“最近失败对象”“最老失败对象”展示对象名和失败时间。
**验收标准:** 顶栏视觉密度更均衡；首页文案更新生效；失败对象不再只显示 age/count，用户能直接看到是哪一个对象、何时失败、往哪个目标失败。
**实现备注:** 顶栏基础布局已固定为对象动作表 `4/12`、Start Delay `2/12`、Rolling/Large Transfer `6/12`，并在 Admin 页面契约测试中锁定；首页主文案已直接使用“把运营商网盘封装成S3块存储和SMB文件共享”。前端新增统一失败对象格式化，Dashboard 的最近/最老失败对象、Monitoring 明细和复制失败表都会显示 provider、target、object、action、failed_at、message；后端 `monitoring.latest_failed_objects/recent_failures` 字段已足够，本任务未改 Rust 运行时 payload。
**依赖:** ADMIN-002
**不做事项:** 不在本任务重做整个 Dashboard 信息架构。
**风险:** 仅靠现有 monitoring payload 无法精确表示 oldest failure 对象时，需要补一个兼容字段。

## ADMIN-004: China Mobile 凭据状态纠偏与助手闭环

**优先级:** P0
**状态:** completed
**目标:** 修正“中国移动已抓到凭据但 UI 仍显示未配置”的状态判断，并打通助手保存后的即时刷新。
**Coding 指导:** 以 `token_present`、`root_folder_id`、`user_domain_id`、runtime health note、browser profile 绑定状态做综合判断；`applyAndSaveMobileAuthOutputs()` 之后强制刷新 provider credential 与 status；将助手反馈改为“已保存/已验证/仍需补字段”等明确状态，避免“Login finished and outputs are ready”与凭据卡“未配置”冲突。
**验收方法:** 在真实 CDP 登录成功场景下执行 Mobile capture/save，随后刷新 Providers 页面与 Dashboard provider 状态。
**验收标准:** 已保存可用 Mobile 凭据时，UI 不再显示“未配置”；助手、凭据卡、provider health 三处状态一致。
**实现备注:** 前端新增 Mobile 可用凭据材料判断，统一把 provider bridge runtime 映射字段、`token_present`/cookie present、`root_folder_id`、`user_domain_id`、浏览器画像、lease 与 provider health 作为状态事实。Mobile 助手现在即使只抓到可复用浏览器画像也会允许填表/保存；保存后会重新拉取 credentials/status，并在 provider test 后再刷新一次状态，让 Dashboard、凭据卡和健康状态重新对齐。Mobile 凭据卡新增“移动凭据状态”和“识别到的材料”，避免已保存材料时仍表现成“未配置”。
**依赖:** ADMIN-002
**不做事项:** 不重写 Mobile browser flow。
**风险:** 前端只看局部字段会与后端健康状态再次分叉。

## ADMIN-005: 真实服务日志页与受控日志 API

**优先级:** P0
**状态:** completed
**目标:** 新增 Admin 日志查看页，并提供受 Admin 鉴权保护的真实 `gatewayd` 进程日志读取接口。
**Coding 指导:** 后端实现进程内有界 ring buffer 或等效轻量日志缓冲，避免直接 tail 任意文件或读取 systemd journal；接口支持按 level、keyword、limit、cursor 查询；前端提供日志页、筛选、搜索、复制、导出与自动刷新。严格限制单条日志长度、总缓存条数/字节数和刷新频率，不为日志页保留无限历史。
**验收方法:** `cargo test -p gatewayd` 覆盖日志缓存与 API 鉴权；手工触发 warn/error/provider failure 后在日志页查到对应记录。
**验收标准:** 日志页能稳定显示运行中的真实日志；未登录或无 Admin 权限无法读取；大量日志不会无限占用内存，默认容量和单条截断有明确上限。
**实现备注:** 已有实现提供进程内 `AdminLogRing` 有界日志缓冲（默认 2000 条、2MiB，总单行截断 2KiB），tracing writer 同时写 stderr 和 ring buffer；受 Admin 鉴权/改密保护的 `GET /api/admin/logs` 支持 `level`、`keyword`、`limit`、`cursor` 查询并限制 limit/keyword。Admin 日志页已提供筛选、搜索、分页加载、复制、导出、自动刷新与 AI 解释入口。本次收尾把日志路由纳入 `admin-api` contract，并让前端通过注入的 `adminApi.routeAdminLogs()` 访问，避免隐藏硬编码接口。
**依赖:** ADMIN-002
**不做事项:** 不做跨重启持久历史日志查询。
**风险:** 日志缓冲过大导致低配宿主额外内存压力。

## ADMIN-006: 前端 AI 解释与配置项问答

**优先级:** P0
**状态:** completed
**目标:** 把“AI解释”改成前端链路，并扩展到错误日志和晦涩配置项。
**Coding 指导:** 不扩展 Rust `/api/llm/error-explain`；前端点击“AI解释”时先脱敏，再请求 Cloudflare FAQ match API，随后在浏览器内用用户当前配置的 LLM endpoint 发起解释；新增统一 modal，支持错误日志和配置项两种上下文；对无 CORS 或未配置 API key 的 endpoint 给出复制 prompt 的降级路径。FAQ 命中、prompt 拼装、对话框状态和解释展示都留在前端，不把这类高层交互状态落到 Rust。
**验收方法:** 在日志页、provider test、limit probe、设置项 help 入口分别触发 AI 解释；断开 LLM endpoint 验证降级提示。
**验收标准:** 不依赖 Rust 的错误解释接口也能完成 FAQ 命中 + LLM 解释；解释请求不发送云盘 token/cookie；配置项可以复用同一套 FAQ 匹配和 prompt 模板。
**实现备注:** Admin 前端已有统一 `ai-explain-modal`，日志页、Provider Test、Limit Probe、凭据诊断和字段 help 都走前端 `buildAiExplainBundle()`：先脱敏上下文，再请求 Cloudflare FAQ match API，随后生成可复制 Prompt，并可尝试从浏览器直连当前启用的 LLM endpoint；CORS 或未配置 endpoint 失败时保留复制 Prompt 降级。前端不调用 Rust `/api/llm/error-explain`；本次收尾把默认 FAQ match endpoint 固定到 `https://carrier-disk-gateway.agi2030.online/api/faq/match`，同时保留 `localStorage.ccbg_front_faq_match_endpoint` 作为本地覆盖。
**依赖:** ADMIN-005, CF-002
**不做事项:** 不引入本地向量数据库。
**风险:** 浏览器直连的 LLM endpoint 若不支持 CORS，需要稳定降级路径。

## CF-001: carrier-disk-gateway.agi2030.online 项目站

**优先级:** P0
**状态:** completed
**目标:** 基于现有 `public/cloudflare/` 建立 `carrier-disk-gateway.agi2030.online` 项目站，作为公开介绍、下载和 FAQ 入口。
**Coding 指导:** 参考 `llm-router.agi2030.online` 的信息结构，但保留当前 public-materials license/provenance 边界；站点内容覆盖产品介绍、能力矩阵、下载安装、FAQ、授权/个人源码审查、Provenance；域名与 Worker/Pages 配置明确绑定到 `agi2030.online`；兼容本机现有 Cloudflare 凭据文件和 GitHub Actions 部署。
**验收方法:** 本地 `wrangler` 预览站点；部署后访问正式域名首页、FAQ、下载页与 `.well-known/ccbg-provenance.json`。
**验收标准:** 站点可从正式自定义域访问；不再保留 OneDrive 作为首页默认能力；页面内容与当前授权边界一致。
**依赖:** RELEASE-005
**不做事项:** 不把核心 Rust 源码或私有运行时配置暴露到公开站点。
**风险:** 现有 `public/cloudflare` 是纯静态结构，若引入 Worker/Functions 需同步维护 fingerprint 检查。
**实现备注:** `public/cloudflare/index.html` 已改为项目站入口，新增 `/faq/`、`/install/` 导航，首页移除 OneDrive 作为默认能力描述；保留 `.well-known/ccbg-provenance.json`、`_headers` 和 meta fingerprint 结构。
**实际验证命令:** `cd public/cloudflare && python3 -m http.server 8788`，访问 `/`、`/faq/`、`/install/`、`/.well-known/ccbg-provenance.json`。

## CF-002: FAQ 目录、匹配 API 与安装入口

**优先级:** P0
**状态:** completed
**目标:** 在 Cloudflare 侧提供 FAQ 目录、FAQ 匹配 API 和下载/安装入口。
**Coding 指导:** FAQ catalog 使用结构化 JSON，字段至少包含 `id/title/summary/keywords/provider/context/config_keys/error_patterns/actions/doc_url`；匹配算法采用关键词、provider、context、error pattern 的加权评分，不引入向量库；安装页优先提供 LXC/Linux、fnOS 实验、OpenWrt 实验三条路径，并公开 package 名称、SHA256、自动安装脚本与风险提示。
**验收方法:** 本地调用 FAQ match API 覆盖错误日志、配置项、provider 三类请求；安装页检查下载链接、SHA256 展示和 profile 说明。
**验收标准:** FAQ 匹配可返回稳定 top hits；安装页信息与仓库现有打包脚本、产物名、宿主支持线一致；OpenWrt 标记为实验，fnOS 标记为实验但高于 OpenWrt 档。
**依赖:** CF-001, PLATFORM-002, PLATFORM-003
**不做事项:** 不在 CF 侧保存用户的 LLM key 或云盘凭据。
**风险:** FAQ schema 漂移导致前端 prompt 组装和匹配结果不稳定。
**实现备注:** 新增 `public/cloudflare/data/faq-catalog.json`；新增 Pages Functions `functions/api/faq/catalog.js` 与 `functions/api/faq/match.js`，匹配算法为关键词/provider/context/config_key/error_pattern 加权评分（无向量库）；安装入口落在 `/install/`，覆盖 LXC/Linux、fnOS 实验、OpenWrt 实验路径与风险说明。
**实际验证命令:** `cd public/cloudflare && wrangler pages dev .`；`curl -sS http://127.0.0.1:8788/api/faq/catalog`；`curl -sS -X POST http://127.0.0.1:8788/api/faq/match -H 'content-type: application/json' -d '{"query":"mobile token expired","provider":"mobile","context":"logs","limit":3}'`。

## PLATFORM-008: fnOS/OpenWrt 实验宿主支持线与安装脚本收敛

**优先级:** P1
**状态:** completed
**目标:** 把 fnOS 与 OpenWrt 的实验支持线、安装说明和自动安装入口收敛到一致文档与脚本。
**Coding 指导:** `fnOS` 作为 NAS/Linux 实验宿主，优先 Docker/Compose 路径；OpenWrt 保留实验支持，明确 `64MB` 仅实验、`128MB` 可测、`256MB+` 推荐；自动安装脚本按宿主 profile 做资源检查、路径提示、危险提示和 dry-run。
**验收方法:** 文档审计 + 安装脚本 `--dry-run`；核对和 `docs/resource-budget.md`、`docs/openwrt-host-profile.md` 的门槛描述一致。
**验收标准:** 站点、脚本、文档三处对 fnOS/OpenWrt 的支持线和定位一致；不会把 OpenWrt 64MB 写成稳定承诺。
**依赖:** CF-002
**不做事项:** 不在本任务承诺 OpenWrt 商店化或 fnOS 应用商店上架。
**风险:** 资源线描述不一致会直接造成安装预期错误。
**实现备注:** 站点安装页 `/install/` 已统一宿主支持线：fnOS 为 NAS/Linux 实验宿主（优先 Docker/Compose 思路），OpenWrt 明确 `64MB` 仅实验、`128MB` 可测、`256MB+` 推荐，并链接现有 `build-openwrt-lite-package.sh`、`deploy/openwrt/install.sh`、`build-lxc-package.sh`。
**实际验证命令:** 访问 `/install/` 校对文案与脚本链接；对照 `docs/openwrt-host-profile.md` 与 `docs/openwrt-lite-deployment.md` 资源线一致性。

## OPS-006: .43 测试机发布与人工验收清单

**优先级:** P0
**状态:** pending
**目标:** 完成上述 TODO 后发布到 `.43` 测试机，并形成一份人工检查清单。
**Coding 指导:** 区分“只改 Admin HTML/CF 站点即可热更新”和“改 Rust 需重新构建/重启”两类发布；发布后按首页、Providers、Mobile、Logs、AI解释、FAQ 站点和下载页逐项验收。
**验收方法:** 部署到 `.43` 后执行 health/admin/status/manual smoke，并记录未通过项。
**验收标准:** `.43` 上 Admin 改动可见，中文化/布局/失败对象/Mobile 状态/日志页/AI解释都能人工验证；正式域名站点可访问 FAQ 与安装页。
**依赖:** ADMIN-006, CF-002
**不做事项:** 不在本任务把 LXC 实机安装自动化跑在生产容器里。
**风险:** 如果同时改动 Rust 和 CF 站点，人工验收时容易把本地缓存与远端部署状态混淆。
**执行备注:** 2026-05-30 已将 release `gatewayd` 发布到 `.43` 的 `/home/walky/apps/ccbg`，远端 binary sha256 为 `fa5d855722b2a6922f08202207841c79d264f839aeb5193c806ef94b916fb174`，进程 PID `619802`。`/healthz` 返回 200 且 `unicom-cloud-drive` healthy；Admin API 未登录返回合同化 401；Admin 根路径跳转 `/login`。详细记录见 `docs/ops-006-43-acceptance.md`。
**阻塞项:** `carrier-disk-gateway.agi2030.online` 和 `ccbg-public.pages.dev` 当前 DNS 解析失败，本机 wrangler 未登录，无法直接发布/绑定 Cloudflare Pages；登录后的 Admin 浏览器人工验收仍需补做。

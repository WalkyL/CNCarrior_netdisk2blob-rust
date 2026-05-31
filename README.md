# carrier-cloud-blob-gateway

面向 Hermes Agent、Open Claw Agent 等编程 Agent 的本地 S3 兼容对象网关。

项目目标是把中国联通、中国电信、中国移动云盘接入为“多账号、多后端”的统一对象层，其中任意时刻只允许指定一个运营商或 `stub` provider 作为唯一写入主云盘，其他被选中的运营商后端可作为异步同步目标，并以 `daemon + MCP server + Skill` 三种交付形态提供给 Agent 使用。OneDrive 相关实现当前保留为延后集成能力，默认禁用并从近期主线隐藏，等出现真实用户需求后再恢复评估。仓库采用“商业核心 + 公开材料 + 个人非商业源码审查申请”模式，不是 MIT，也不是 OSI 开源。

当前仓库已经完成多 provider 工作区、S3 兼容 HTTP 入口、`policy-engine`、`replication-engine`、SQLite 复制状态持久化，以及三大运营商 provider 的基础适配。当前状态是: `gatewayd` 已在 `ListBuckets`、`HeadBucket`、`ListObjectsV2`、`HeadObject`、`GetObject` 上按 `fallback_read_order` 执行读侧 fallback，并通过响应头提示实际数据来源；联通 provider 已打通真实目录列举、下载、上传、对象删除，并把 personal/family scope 映射成 `root`/`family` 两个容器；电信 provider 已打通真实目录列举、流式下载、受控根目录下的 multipart 上传与有界 spool 写路径，但对象级 delete/rename/copy/move 仍待补；移动 provider 已打通真实对象列举、上传与下载，family 视图与对象动作仍待补。OneDrive Graph 读写与 OAuth 会话代码仍在仓库中，但当前阶段不作为默认备份、默认 fallback 或近期完成度目标。控制面当前已支持更直观的 Admin Web、provider 独立凭证存储热注入、provider 级 IPv4/IPv6 策略、auth-capture sidecar / LLM 配置、运行态监控摘要、聚合监控摘要面板、自动刷新控制，以及带 `operator/ticket/notes`、时间范围筛选的对象动作共享审计历史。认证部分仍坚持由操作者显式提供材料或通过受控控制面完成授权，不在服务内实现浏览器会话窃取。

针对网页端经常改版的运营商流程，仓库现在额外引入了三层可替换事实配置: `config/provider-bridges/*.json` 负责 `gatewayd` / Admin Web / auth session 与 provider-specific surface、flow alias、runtime→credential 映射之间的绑定；`config/browser-flows/*.json` 负责页面元素、JS 入口点和关键请求形状；`config/provider-capabilities/*.json` 负责已经证明稳定的 native 请求模板。对“每个云盘后续还要自动探测哪些账号、作用域、读写路径事实”，则额外放进 `config/provider-probes/*.json`。这几层的目的都是把页面和控制面漂移优先收敛成 JSON 改动，而不是重写 Rust 数据面。首个样例是联通桌面站 `pan.wo.cn`，当前已覆盖当前会话抓取、短信登录、个人空间上传准备、个人空间上传、目录创建/删除/重命名/复制/移动这九条真实验证过的网页流程，并为 native `CreateDirectory` / `DeleteFile` 和后续自动探测项维护独立 catalog。

当前项目还明确采用以下硬约束：

- 所有正式 provider 最终都必须支持流式读与流式写；若上游要求显式内容哈希或预分片，只允许使用有界磁盘 spool，不允许整对象内存缓冲。
- provider 页面改版时，优先修改 `browser-flow` / `provider-capabilities` / `provider-probes` 等可替换事实层，不能因为页面细节变化就重写整个 Rust 数据面程序。
- 对象动作执行器、客户端 bucket 派生策略、桶级加密写入策略必须保持可插拔，不能写死成某一家云盘的专有流程。

浏览器执行层当前统一收口到标准 CDP，而不是绑定某个浏览器品牌。`browser-cdp` crate 负责连接可配置的 CDP endpoint、选择 page target、执行基础页面动作，`gatewayd` 则提供 catalog 查询、dry-run 和最小真实 session 执行入口，便于后续把 auth-capture 编排继续外挂出来。

当前针对轻量设备的策略也已明确: `OpenWRT` 优先作为 host 服务宿主，默认通过 `CCBG_MAX_IN_MEMORY_OBJECT_BYTES` 控制非流式对象路径的峰值内存，并通过 SQLite 元数据保留上限控制 flash 增长；`STM32` / `ESP32-S3` 只提供嵌入式 S3 客户端示例，不是完整网关宿主。只有在确认资源充足时，才考虑更小功能集的 ESP32-S3 relay 形态，后续元数据应走更轻量的 `tiny-state-client` 路线而不是继续复用完整 SQLite 宿主形态。

## 当前目标

- 用 Rust 建立一个可长期维护的 Agent-first 边缘对象网关，而不是一次性脚本。
- 将“运营商云盘访问”“本地 S3 兼容 API 暴露”“管理界面”和延后集成 provider 解耦。
- 将“核心网关能力”和“Agent 集成封装”解耦，确保能分别交付为 daemon、MCP 和 Skill。
- 明确采用“单写主云盘 + 多异步同步目标”模型，避免多主写入导致的数据冲突。
- 数据面正式目标兼容 S3 API，优先支持 Agent 常用的最小 S3 子集。
- 一期宿主目标覆盖 `PVE LXC x86/x64`、`Docker x86/x64`、`Podman x86/x64`、`OpenWRT arm64`；`STM32` / `ESP32-S3` 只作为嵌入式 S3 客户端示例。
- 先跑通本地 Ubuntu，后续平滑迁移到 PVE/LXC、软路由和 ARM Linux 设备。
- 统一将监听端口约束在 `60000-65534` 区间，降低与系统常用端口冲突的概率。

## 目录结构

```text
carrier-cloud-blob-gateway/
├── config/
│   ├── browser-flows/   # provider 网页流程描述，供 CDP/auth-capture 执行层复用
│   ├── provider-bridges/ # gateway/auth UI 与 browser-flow 之间的 provider-specific 绑定
│   ├── provider-capabilities/ # provider-native 能力描述，供稳定请求执行层复用
│   ├── provider-probes/ # 每个 provider 的自动探测项描述
│   └── example.env
├── crates/
│   ├── blob-core/        # 抽象对象存储模型与错误定义
│   ├── browser-cdp/      # 基于 Chrome DevTools Protocol 的浏览器执行适配层
│   ├── gatewayd/         # 本地 HTTP 服务入口
│   ├── metadata-store/   # SQLite 元数据与复制状态持久化
│   ├── policy-engine/    # primary/sync/fallback 拓扑校验
│   ├── provider-unicom/  # 中国联通云盘适配层
│   ├── provider-telecom/ # 中国电信云盘适配层
│   ├── provider-mobile/  # 中国移动云盘适配层
│   ├── provider-onedrive/ # OneDrive 延后集成适配层
│   └── replication-engine/ # 异步复制任务队列
├── deploy/
│   └── Containerfile
├── docs/
│   ├── architecture.md
│   ├── agent-packaging.md
│   ├── browser-flow-config.md
│   ├── compatibility-matrix.md
│   ├── component-dependency-map.md
│   ├── detailed-plan.md
│   ├── github-publication.md
│   ├── ports.md
│   ├── provider-matrix.md
│   ├── provider-probes.md
│   ├── s3-compatibility.md
│   └── roadmap.md
├── tools/
│   └── component-ast-map/ # cargo metadata + syn AST 依赖图生成器
└── scripts/
    ├── notify-webhook-receiver-example.py
    └── run-dev.sh
```

后续计划中的 crate:

- `auth-broker`
- `mcp-server`
- `admin-api`
- `admin-ui-web`
- `admin-ui-tui`
- `skills/carrier-cloud-blob-gateway/`

公开仓库基础资产:

- [LICENSE](LICENSE)
- [COMMERCIAL-LICENSE.md](COMMERCIAL-LICENSE.md)
- [PUBLIC-MATERIALS-LICENSE.md](PUBLIC-MATERIALS-LICENSE.md)
- [.github/workflows/ci.yml](.github/workflows/ci.yml)
- [Dockerfile](deploy/Dockerfile)
- [Containerfile](deploy/Containerfile)
- [public/cloudflare](public/cloudflare)

推荐优先阅读的运维文档:

- [docs/auth-step-by-step.md](/home/walky/carrier-cloud-blob-gateway/docs/auth-step-by-step.md:1)
- [docs/router-deployment-guide.md](/home/walky/carrier-cloud-blob-gateway/docs/router-deployment-guide.md:1)
- [docs/object-actions-and-history.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-and-history.md:1)
- [docs/object-actions-api-reference.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-api-reference.md:1)
- [docs/notify-webhook-reference.md](/home/walky/carrier-cloud-blob-gateway/docs/notify-webhook-reference.md:1)
- [docs/unicom-go-live-checklist.md](/home/walky/carrier-cloud-blob-gateway/docs/unicom-go-live-checklist.md:1)
- [docs/unicom-change-record-template.md](/home/walky/carrier-cloud-blob-gateway/docs/unicom-change-record-template.md:1)
- [docs/unicom-phase-closeout-report.md](/home/walky/carrier-cloud-blob-gateway/docs/unicom-phase-closeout-report.md:1)
- [docs/provider-completion-standard.md](/home/walky/carrier-cloud-blob-gateway/docs/provider-completion-standard.md:1)

## 快速启动

```bash
cd /path/to/carrier-cloud-blob-gateway
cp config/example.env .env.local
sed -i "s#^CCBG_UNICOM_TOKEN=.*#CCBG_UNICOM_TOKEN=replace-with-your-own-token#" .env.local
./scripts/run-dev.sh
```

当前默认监听地址为 `127.0.0.1:61080`，该端口规划为本地 S3 兼容数据面入口。

如果宿主是软路由或 OpenWRT 类设备，先看 [docs/router-deployment-guide.md](/home/walky/carrier-cloud-blob-gateway/docs/router-deployment-guide.md:1)。推荐默认保持 `Admin Web`、`OAuth Callback`、`Metrics` 都只监听 `127.0.0.1`，仅按需要显式开放 `S3 API`。

软路由场景建议同时把 `CCBG_DATA_PLANE_MAX_IN_FLIGHT` 控制在 `2~4`，让数据面在并发超限时直接返回 `503`，而不是在小内存宿主上继续堆积请求。
如果客户端有突发请求峰值，还可以再打开 `CCBG_DATA_PLANE_MAX_REQUESTS_PER_SECOND`，做一个更保守的每秒请求阀门。
如果控制面需要被非本机访问，再额外配置 `CCBG_CONTROL_API_KEY`；这和 S3 数据面的 `CCBG_S3_ACCESS_KEY_ID/SECRET` 是两套独立认证。

规划中的同步拓扑:

- `CCBG_PRIMARY_PROVIDER` 指定唯一写入主云盘
- `CCBG_SYNC_TARGETS` 指定异步同步目标，近期主线只默认考虑其他运营商
- OneDrive 不再默认加入同步目标或 fallback 顺序；有真实需求后再按 Parking 清单恢复

当前已落地的拓扑与复制骨架:

- `policy-engine` 负责校验 `primary/sync/fallback` 约束
- `gatewayd` 在 `PutObject` / `DeleteObject` 成功后会入队复制任务，并由后台 worker 消费
- `metadata-store` 使用 SQLite 持久化复制任务与状态摘要，并会裁剪多余的 `completed/failed` 历史，只保留 fallback 必需的最新对象状态和全部 pending job
- `replication-engine` 当前负责内存队列、最近任务记录和启动时 pending job 恢复，最近历史条数可通过环境变量限制
- OneDrive 相关 provider 代码保留但默认禁用，不作为当前阶段验收主线

规划中的端口分配:

- S3 API: `61080`
- Admin Web UI: `61081`
- OneDrive OAuth 本地回调: `61082`（延后集成时才启用）
- Metrics / 扩展健康检查: `61083`
- MCP Streamable HTTP: `61084`（可选，默认优先 stdio）

## 数据面目标

正式目标是兼容 S3 API，而不是长期保留自定义 `/v1/...` 接口。

一期规划中的 S3 子集:

- `ListBuckets`
- `HeadBucket`
- `ListObjectsV2`
- `HeadObject`
- `GetObject`
- `PutObject`
- `DeleteObject`

后续规划:

- `Multipart Upload`
- `CopyObject`
- `Presigned URL`

## 当前已实现接口

已接入的本地 S3 兼容子集:

- `GET /` -> `ListBuckets`
- `HEAD /<bucket>` -> `HeadBucket`
- `GET /<bucket>?list-type=2&prefix=<prefix>&max-keys=<n>` -> `ListObjectsV2`
- `HEAD /<bucket>/<key>` -> `HeadObject`
- `GET /<bucket>/<key>` -> `GetObject`
- `PUT /<bucket>/<key>` -> `PutObject`
- `DELETE /<bucket>/<key>` -> `DeleteObject`

保留的内部调试接口:

- `GET /healthz`
- `GET /__ccbg`
- `GET /__ccbg/providers`
- `GET /__ccbg/replication`
- `GET /v1/containers`
- `GET /v1/objects?container=<name>&prefix=<prefix>&limit=<n>`

当前已落地的 Metrics / Extended Health 接口:

- `GET http://127.0.0.1:61083/healthz` -> 返回扩展健康摘要，包含 `runtime`、`monitoring` 和当前 alerts
- `GET http://127.0.0.1:61083/readyz` -> 返回 `200` / `503`，用于宿主探针判断 primary provider 是否可服务
- `GET http://127.0.0.1:61083/metrics` -> 返回 Prometheus 文本格式指标，覆盖 uptime、open alerts、provider health、replication job 计数和对象动作汇总

当前已落地的控制面认证:

- 浏览器访问 Admin Web 现在优先走本地用户名密码登录，登录成功后改用 `HttpOnly` session cookie，不再依赖把 API key 挂在 URL 上
- `CCBG_ADMIN_USERNAME` 与 `CCBG_ADMIN_PASSWORD_HASH` / `CCBG_ADMIN_PASSWORD_HASH_FILE` 用于浏览器 Admin 登录；本地开发也可临时用 `CCBG_ADMIN_PASSWORD`
- `CCBG_CONTROL_API_KEY` 仍可独立保护脚本访问、机器间调用和指标接口
- 脚本访问可用 `x-api-key: <key>` 或 `Authorization: Bearer <key>`
- S3 数据面仍然单独使用 `CCBG_S3_ACCESS_KEY_ID` + `CCBG_S3_SECRET_ACCESS_KEY` 的 SigV4，不复用控制面 API key

当前已落地的复制人工干预能力:

- Admin Web 的 `Recent Jobs` 表格现在可对最新 failed replication job 直接执行 `Retry`
- Admin Web 的 `Target Status` 表格现在可对某个 target 的 latest failed jobs 执行 `Retry Failed`
- Admin Web 现在额外提供 `Latest Failed Objects` 视图，可按 target 过滤当前仍失败的对象，并导出 JSON / CSV
- Admin Web 现在额外提供 `Operations Overview` 总览卡，集中显示当前主写 provider、异步备份 / fallback 拓扑、复制积压年龄、latest failed object 年龄和 notify 新鲜度
- `Operations Overview` 还会显示数据面并发上限、当前剩余 permit、以及当前每秒请求阀门，便于软路由场景快速判断是否需要继续收紧数据面保护
- `Latest Failed Objects` 现已支持对象关键字和时间窗口过滤，`Monitoring Summary` / notify webhook 也会附带当前 latest failed 对象摘要
- `POST /api/replication/jobs/{job_id}/retry` 只允许重试该对象在对应 target 上“当前最新的一条 failed job”
- `POST /api/replication/targets/{target}/retry-failed` 只会重试该 target 上“当前仍然是最新状态”的 failed jobs
- 重试会把该 job 重新置回 `pending` 并重新入内存队列

当前已落地的外部告警 webhook:

- `CCBG_NOTIFY_WEBHOOK_URL` 配置后，后台会按 `CCBG_NOTIFY_POLL_INTERVAL_SECONDS` 周期检查 alerts
- 仅当 alerts 集合发生变化时才发送 webhook，避免轮询风暴
- webhook 请求体包含 `runtime`、`monitoring` 和 `alerts`
- 复制失败告警现在支持 `CCBG_REPLICATION_FAILED_ALERT_THRESHOLD` 和 `CCBG_REPLICATION_FAILED_ALERT_MIN_AGE_MS` 两个生产化阈值，避免刚失败就立刻噪声告警
- webhook 总会附带 `x-ccbg-notify-event-id` 与 `x-ccbg-notify-timestamp`
- 若配置 `CCBG_NOTIFY_WEBHOOK_SIGNING_SECRET`，还会附带 `x-ccbg-notify-signature-version=v1` 与 `x-ccbg-notify-signature`
- 建议接收端按 `event_id + timestamp` 做幂等与时间窗校验；可直接参考 [docs/notify-webhook-reference.md](/home/walky/carrier-cloud-blob-gateway/docs/notify-webhook-reference.md:1) 和 [scripts/notify-webhook-receiver-example.py](/home/walky/carrier-cloud-blob-gateway/scripts/notify-webhook-receiver-example.py:1)

当前 S3 兼容实现边界:

- 仅验证 header-based SigV4
- 仅保证 `path-style` bucket 访问
- 暂未支持 `Presigned URL`
- 暂未支持 `Multipart Upload`
- OneDrive 映射逻辑保留在延后集成代码中，但默认不进入当前 S3 主线
- 读请求当前会先尝试 primary provider，失败后按 `fallback_read_order` 尝试 sync targets
- 当响应来自备份侧时，会附带 `x-ccbg-source-provider` 和 `x-ccbg-fallback-from`
- 当前自动化测试已覆盖 `stub` backend 与部分 provider mock；运营商 provider 仍未完成全量真实读写回归

当前运行时仍只允许一个 primary provider，但 `sync targets` 的异步复制 worker、per-target 复制状态摘要、基础重试退避，以及 latest-only 语义的单 job / 按 target 批量人工重试都已经落地；复制失败相关监控现在也按“每个对象在每个 target 上的最新状态”统计，避免旧失败在后续成功后继续误报；后续仍可继续演进更完整的死信体系和独立 auth-broker。

一期平台兼容边界:

- `PVE LXC x86/x64`、`Docker x86/x64`、`Podman x86/x64`: 完整宿主目标
- `OpenWRT arm64`: 轻量宿主目标，当前采用 `sqlite-host` 路线
- `STM32`: 嵌入式 S3 客户端示例，不承载完整 daemon
- `ESP32-S3`: 嵌入式 S3 客户端示例；后续按 `tiny-state-client` 或 `relay-lite` 子集评估，详见 `docs/esp32-s3-profile.md`

## 轻量设备资源边界

- `OpenWRT host` 当前可行的关键，不是把 SQLite 完全去掉，而是同时限制对象大小、并发数、内存中的 recent history，以及 SQLite 的历史保留上限。
- `SQLite` 在这里不是最省内存的状态后端，但在 Linux/OpenWRT host 上仍是当前最稳妥的持久化选择，因为 fallback 判定需要可靠的对象级最新状态。
- `ESP32-S3` 不应复用当前 `sqlite-host` 方案；后续应拆成更轻的 `tiny-state-client`，只保留极小状态、配置和必要的 ring buffer。

当前新增的轻量化运行参数:

- `CCBG_REPLICATION_RECENT_LIMIT`
- `CCBG_REPLICATION_MAX_ATTEMPTS`
- `CCBG_REPLICATION_BASE_RETRY_DELAY_MS`
- `CCBG_REPLICATION_MAX_RETRY_DELAY_MS`
- `CCBG_OBJECT_ACTION_HISTORY_LIMIT`
- `CCBG_METADATA_SNAPSHOT_RECENT_LIMIT`
- `CCBG_METADATA_COMPLETED_HISTORY_LIMIT`
- `CCBG_METADATA_FAILED_HISTORY_LIMIT`
- `CCBG_*_IP_FAMILY`
- `CCBG_AUTH_CAPTURE_*`

当前已落地的浏览器流程调试入口:

- `GET /api/browser-flows/catalogs`
- `GET /api/browser-flows/catalog?provider=<provider>&surface=<surface>`
- `GET /api/browser-flows/flow/<flow_id>`
- `POST /api/browser-flows/dry-run`
- `POST /api/browser-flows/session-run`

其中 `session-run` 会优先使用请求体里的 `cdp_endpoint_url` / `cdp_target_selector` / `cdp_target_timeout_ms`，未提供时回退到控制面保存的 `CCBG_AUTH_CAPTURE_CDP_*` 对应配置。这一层的目标是兼容任意支持 CDP 的浏览器或远端 browser host，而不是只兼容 Edge。若 flow 声明了 `prerequisite_flow_id`，`gatewayd` 现在会在同一条 `auth_session_id` 和同一个 CDP page session 内递归执行 prerequisite 链，再执行主 flow。

## 认证边界

- 支持 `CCBG_UNICOM_TOKEN` / `CCBG_UNICOM_TOKEN_FILE`
- 支持 `CCBG_TELECOM_TOKEN` / `CCBG_TELECOM_TOKEN_FILE`
- 支持 `CCBG_TELECOM_BROWSER_ID` / `CCBG_TELECOM_BROWSER_ID_FILE`
- 支持 `CCBG_TELECOM_COOKIE_HEADER` / `CCBG_TELECOM_COOKIE_HEADER_FILE`
- 支持 `CCBG_MOBILE_TOKEN` / `CCBG_MOBILE_TOKEN_FILE`
- OneDrive 相关环境变量仅作为延后集成保留项，不属于当前默认认证路径
- 支持 `CCBG_UNICOM_IP_FAMILY` / `CCBG_TELECOM_IP_FAMILY` / `CCBG_MOBILE_IP_FAMILY`
- 支持 `CCBG_AUTH_CAPTURE_BROKER_URL`、`CCBG_AUTH_CAPTURE_CDP_ENDPOINT_URL`、`CCBG_AUTH_CAPTURE_CDP_TARGET_SELECTOR`、`CCBG_AUTH_CAPTURE_CDP_TARGET_TIMEOUT_MS`、`CCBG_AUTH_CAPTURE_LLM_ENDPOINT`、`CCBG_AUTH_CAPTURE_LLM_MODEL_ID`、`CCBG_AUTH_CAPTURE_LLM_API_KEY`
- OneDrive Web PKCE / Device Code 授权保留为 Parking 恢复项，默认流程不启用
- 成品规划支持 MCP 封装与 Skill 封装
- 不实现浏览器会话自动抓取
- 不在代码里写死账号密码、Cookie、refresh token

如果必须依赖网页会话，建议由你手工在浏览器开发者工具中确认请求头，再以环境变量或受限文件方式注入本服务。

## 文档

- 架构说明见 `docs/architecture.md`
- Agent 交付封装见 `docs/agent-packaging.md`
- 云盘认证新手指南见 `docs/auth-step-by-step.md`
- 组件依赖图见 `docs/component-dependency-map.md`
- 一期兼容矩阵见 `docs/compatibility-matrix.md`
- 详细规划见 `docs/detailed-plan.md`
- ESP32-S3 运行档位见 `docs/esp32-s3-profile.md`
- OpenWRT Host 档位见 `docs/openwrt-host-profile.md`
- 资源预算与算法模型见 `docs/resource-budget.md`
- GitHub 发布规划见 `docs/github-publication.md`
- 端口策略见 `docs/ports.md`
- provider 差异矩阵见 `docs/provider-matrix.md`
- S3 兼容规划见 `docs/s3-compatibility.md`
- 分阶段规划见 `docs/roadmap.md`
- 浏览器流程配置说明见 `docs/browser-flow-config.md`
- 重新生成组件依赖图:
  `cargo run --manifest-path tools/component-ast-map/Cargo.toml -- --workspace-root . --output docs/component-dependency-map.md --json-output docs/component-dependency-map.json`

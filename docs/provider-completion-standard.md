# Provider 完成度标准

这份标准用来定义“某个 provider 做到满”在本项目里的最小可交付含义，避免不同 provider 出现“功能看起来差不多，但接入质量不一致”的情况。

## 1. 目标

一个 provider 只有同时满足下面几个维度，才能被视为达到 `full`:

1. 可以稳定获得可复用认证材料。
2. 可以稳定发现实际存储作用域，而不是只假设单一根目录。
3. 读路径已经打通，并有真实对象 roundtrip 证据。
4. 基础写路径已经打通，并能在当前网关语义下工作。
5. 对象动作语义已经明确，不会在复制/fallback 侧留下脏状态。
6. 健康检查、catalog、文档、测试已经对齐，不依赖“口头知道”。

## 2. 状态定义

- `planned`: 已知要做，但没有稳定事实或实现。
- `partial`: 有部分链路可用，但还不能作为可靠主写候选。
- `full`: 达到本标准，可作为主写候选或同步候选进入系统。

`full` 不等于“永远不再改”，而是表示:

- 当前主流程可交付。
- 上游网页/API 改版时，有 catalog 和测试可回归。
- 后续优化属于增强项，而不是缺功能。

## 3. 必达维度

### 3.1 认证与会话

必须满足:

- provider 能说明当前依赖的是哪些认证材料，例如 `access_token`、`cookie_header`、`browser_id`、OAuth session。
- 认证材料可以通过受控配置或受控浏览器流程注入。
- 对应材料在 `provider-probes` 中有 `confirmed` 探测项。

证据建议:

- `config/provider-probes/<provider>.json`
- `docs/auth-step-by-step.md`

### 3.2 作用域发现

必须满足:

- provider 明确报告实际存储域，例如 `personal | family | shared | unknown`。
- 若存在多个作用域，必须映射成稳定容器，而不是只暴露一个“伪根目录”。
- 健康检查必须反映每个作用域是否真的可用；某个作用域探测失败时不能误报整体健康。

证据建议:

- provider `health()` 返回的 `storage scopes`
- `docs/provider-matrix.md`
- 对应作用域发现 probe

### 3.3 原生读路径

必须满足:

- 容器列举、对象列举、对象读取已经打通。
- 下载路径不是伪实现，必须能拿到真实对象内容。
- 正式读路径必须是流式读；不能以整对象 `collect()` 或 `response.bytes().await` 作为最终数据面实现。
- 至少有一条“对象可读 roundtrip”测试覆盖。

证据建议:

- provider 单测
- `docs/provider-matrix.md`

### 3.4 原生写路径

必须满足:

- 至少支持 `put/upload`、`delete`、`create_directory` 这类基础写能力中的实际项目所需子集。
- 写入后对象能被后续读取或列举验证。
- 正式写路径必须是流式写；不能先把完整对象收进内存再上传。
- 若上游协议要求显式内容哈希、预分片规划或预提交 `partInfos`，允许先把对象流式落到受控磁盘 spool，再以分片方式回放上传；但这只允许使用有界磁盘缓存，不允许退化成整对象内存缓冲。
- 若当前使用浏览器流程采集事实、Rust native 执行请求，两者分工要写清楚。
- 正式写路径不能依赖 CDP 页面、人工保持网页打开、或浏览器 tab 中的临时运行态；浏览器只能帮助取证，不能承接正式数据面。
- provider 侧真实对象必须统一落到单一受控根目录 / 根前缀下，再在其下映射 bucket/key，不能把对象散落到用户云盘根目录。

证据建议:

- `config/provider-capabilities/<provider>-native.json`
- provider 单测
- `docs/browser-flow-config.md`

### 3.5 对象动作

必须满足:

- 明确支持或明确拒绝 `rename/copy/move`。
- 若支持，必须说明边界条件，例如“同目录 rename”或“跨容器 move”。
- 对象动作执行器、加密写入策略、客户端 bucket 派生策略都必须保持可插拔；不能把单一 provider 的固定流程写死成整个系统唯一实现。
- 动作成功后，网关侧复制元数据必须同步更新，不能只改主后端。
- 若控制面暴露对象动作入口，最近执行历史必须以服务端持久化状态为准，并支持回读/清空；不能只依赖浏览器本地缓存。

网关复制语义标准:

- `rename`: `put(new) + delete(old)`
- `copy`: `put(dest)`
- `move`: `put(dest) + delete(src)`

证据建议:

- `BlobBackend` 对应 contract
- `POST /api/object-actions`
- `gatewayd` 测试覆盖
- [docs/object-actions-and-history.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-and-history.md:1)

### 3.6 网关集成

必须满足:

- provider 作为 primary 时能通过 `gatewayd` 正常暴露容器和对象。
- 多容器 provider 不能在网关里退化成单 bucket 视图。
- 与 replication/fallback 的交互语义清晰，不能破坏 metadata gate。
- bucket/key 到 provider 内实际目录的映射必须可预测、可回收、可批量整理；推荐采用 `provider_root/<bucket>/<key>` 或等价结构，而不是直接写到云盘根目录。
- provider 的真实读写表现必须可观测，至少包括首次响应时间、最近一次成功读/写延迟、滚动吞吐，以及最近一次大流量传输均速。
- provider 页面改版时，优先改 `browser-flow` / `provider-capabilities` / `provider-probes` 等可替换事实层，不能因为页面元素变化就重写整个 Rust 数据面程序。
- 路由、fallback、主写选择不应只依赖静态“高速 provider”标签；应优先消费上述观测事实。

证据建议:

- `gatewayd` 集成测试
- `docs/architecture.md`
- `GET /api/status`
- `GET /metrics`

### 3.7 健康、可观测性与 catalog

必须满足:

- provider health 能暴露关键作用域和主要失败原因。
- probe catalog、capability catalog、browser flow catalog 三层边界清晰。
- 已稳定的请求模板放进 capability catalog，而不是继续散落在代码里。

证据建议:

- `docs/provider-probes.md`
- `config/provider-probes/*.json`
- `config/provider-capabilities/*.json`

### 3.8 文档与测试

必须满足:

- provider matrix、架构文档、认证文档已经更新。
- 至少覆盖 provider 单测和网关集成测试。
- 关键边界条件必须有测试，例如 family scope、对象动作、复制语义、degraded health。

补充约定:

- provider 完成度报告统一由 `blob-core` 的 `provider_completion` 测试夹具生成，字段保持稳定:
  - `provider`
  - `overall_expected` / `overall_observed`
  - `coverage_total` / `coverage_full` / `coverage_partial_or_full`
  - 六个维度(`auth_session`、`scope_discovery`、`native_read_path`、`native_write_path`、`object_actions`、`health_catalog_docs`)的 `expected` / `observed` / `notes`
- provider crate 单测只提供本地 mock 的 `health/capabilities` 与已知能力事实，不伪造未实现能力；当前缺口应在报告中体现为 `planned` 或 `partial`。

## 4. 联通当前对照

按这份标准，`unicom` 当前已经达到 `full`:

- 认证与会话: 已确认 token/cookie 注入和浏览器 flow 采集路径。
- 作用域发现: 已支持 personal/family，并映射成 `root` / `family`。
- 原生读路径: 已支持列举、真实下载。
- 原生写路径: 已支持 `upload2C`、删除、建目录。
- 对象动作: 已支持 native `rename/copy/move`，并接入网关对象动作 API。
- 网关集成: 已支持多容器、family bucket、对象动作后的复制元数据同步。
- 健康与 catalog: 已有 probe/capability/browser-flow 三层事实来源。
- 文档与测试: 已对齐并有 provider/gateway 测试覆盖。

当前仍可继续优化，但不再属于“没做到满”:

- `rename` 仍保持“同父目录改名”边界。
- 共享对象动作历史当前仍是轻量审计模型: 只保留最近 12 条，已支持 operator / object / provider / outcome / action / time-window 筛选与 JSON/CSV 导出，但还没有外部系统联动。
- 缺少真实账号的自动化 E2E 回归。

如果要把 `unicom` 作为正式生产主写 provider 上线，建议最后再走一遍:

- [docs/unicom-go-live-checklist.md](/home/walky/carrier-cloud-blob-gateway/docs/unicom-go-live-checklist.md:1)

## 5. OneDrive 当前对照

`onedrive` 以前按旧标准可以视为 `full`，但按当前升级后的“正式数据面必须流式读/写”标准，暂时只能记为 `partial`:

- 认证与会话: 已支持 Web PKCE、Device Code、手工 token、session file 与 refresh token 自动续期。
- 作用域发现: 已明确单 drive 作用域，并通过 `root_prefix/<bucket>/<key>` 暴露稳定 bucket 视图。
- 原生读路径: 已支持列容器、列对象、真实下载。
- 原生写路径: 已支持真实上传、删除，并有对象生命周期 roundtrip 测试。
- 对象动作: 已支持 Graph rename/copy/move，并接入网关对象动作 API 与复制语义。
- 网关集成: 已可作为 sync target 与 fallback backend 参与对象动作后的复制状态更新；按项目策略不作为 primary provider。
- 健康与 catalog: 已有 probe catalog，且能暴露 root prefix / session 状态。
- 文档与测试: 已有 provider 单测、gateway 集成测试与认证文档。

当前仍可继续优化，但不再属于“没做到满”:

- Graph copy 仍依赖异步 monitor URL 轮询，还没有更细粒度的进度可观测性。
- 仍未实现 S3 Multipart Upload 映射。
- 还没有 delta/sync 方向的更完整状态利用。
- 当前读/写实现还没有完成正式流式化改造，因此还不能重新标回升级后标准里的 `full`。

## 6. 后续接入要求

以后新增 provider，建议按下面顺序判定是否达标:

1. 先完成认证、作用域发现、读路径。
2. 再完成基础写路径和 capability catalog。
3. 再把正式读/写路径升级到流式实现，必要时使用有界 spool。
4. 再完成对象动作与网关复制语义。
5. 最后补齐 probe、文档、测试，才允许标记为 `full`。

如果某个 provider 只满足 1-3 步，只能记为 `partial`，不能在文档或控制面里表述成“已完成”。

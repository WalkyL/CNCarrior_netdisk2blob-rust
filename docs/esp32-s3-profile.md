# ESP32-S3 运行档位

## 结论

当前仓库的 `gatewayd` 不适合直接迁移到 `ESP32-S3` 作为完整宿主。

原因不是单点，而是整套运行时假设都偏向 Linux 边缘服务:

- `axum + tokio + tower-http` 的 HTTP 服务模型
- `reqwest + rustls` 的上游 HTTPS 访问
- `rusqlite` 的本地持久化元数据
- 多 provider、后台复制 worker、健康检查和 fallback 判定
- 对象边界虽然已改成 `ObjectBody` 流，但很多 provider 内部仍会整块收集

这套形态对 `PVE LXC`、`Docker`、`Podman`、`OpenWRT arm64` 是合理的，对 `ESP32-S3` 不是。

如果目标是让产品覆盖 `ESP32-S3`，正确做法不是“硬塞当前 daemon 到 MCU”，而是把产品拆成不同运行档位。

所有平台的 RAM / Flash 预算推导见 [resource-budget.md](resource-budget.md)。

## 当前代码的硬约束

当前仓库里最不适合 MCU 的点:

- `gatewayd` 已支持 `UNSIGNED-PAYLOAD` 流式写入，但不是所有签名路径都流式
- `BlobBackend` 已以 `ObjectBody` 表达对象 body，但 provider 内部仍常常收集整块对象
- OneDrive provider 依赖 `reqwest` 和 TLS 栈
- metadata store 依赖 SQLite
- fallback 读取依赖本地复制元数据
- 后台复制 worker 默认常驻

这意味着:

- 小对象可以工作
- 中大对象会迅速放大峰值内存
- 本地 flash 还要承担 SQLite、日志和配置写入
- 如果再叠加 TLS、OneDrive 和 Web UI，MCU 会非常吃力

## 推荐分层

### `full-host`

目标:

- `PVE LXC x86/x64`
- `Docker x86/x64`
- `Podman x86/x64`

包含:

- 完整 `gatewayd`
- SQLite metadata
- 异步复制队列
- OneDrive provider
- 后续 MCP / Admin UI

### `lite-host`

目标:

- `OpenWRT arm64`

包含:

- `gatewayd`
- metadata store
- replication engine
- 可裁剪的 provider 集

裁剪:

- 无 Web UI
- 低并发
- 限制对象尺寸
- 限制日志保留

### `esp32-s3-client`

这是推荐的一期 `ESP32-S3` 形态。

包含:

- 作为本地 S3 客户端
- 或作为 MCP / HTTP 调用端
- 小对象上传下载
- 状态查询

不包含:

- 本地 SQLite
- OneDrive OAuth
- 多 provider 协调
- 本地异步复制 worker
- 本地 fallback 判定

### `esp32-s3-relay-lite`

只有在模块具备较大 `PSRAM` 和足够 flash 时，才考虑这个形态。

包含:

- 单 provider 直通读写
- 极小的本地配置存储
- 极小队列或无队列

仍然不包含:

- OneDrive 本地同步
- SQLite
- 完整 S3 子集
- 多 provider fallback
- Web UI / OAuth broker

## 建议的资源预算

下面的数字不是芯片理论上限，而是建议的工程预算上限。超过这些预算，就不应继续把 `ESP32-S3` 当目标宿主。

### `esp32-s3-client`

- 常驻堆内存目标: `< 256KB`
- 峰值内存目标: `< 512KB`
- 本地持久化目标: `< 128KB`
- 单次对象缓冲: `8KB-32KB`
- 并发上传/下载: `1`

### `esp32-s3-relay-lite`

前提:

- 明确有可用 `PSRAM`
- 明确有足够 flash
- 关闭 OneDrive 本地能力

预算:

- 常驻内存目标: `< 1.5MB`
- 峰值内存目标: `< 3MB`
- 本地配置和状态: `< 512KB`
- 可选环形日志: `< 1MB`
- 单次对象缓冲: `32KB` 默认，最多 `64KB`
- 并发请求: `1`
- 后台任务: 最多 `1`

## ESP32-S3 的非谈判约束

如果真的要跑在 `ESP32-S3`，下面这些约束必须接受:

1. 必须放弃当前完整宿主定位。
2. 必须把对象读写改成流式或分块，不允许整对象 `Bytes` 常驻。
3. 必须移除 SQLite 依赖，改成 NVS / LittleFS / 超轻量自定义日志结构。
4. 必须把 OneDrive 同步和 OAuth 挪到更强的宿主。
5. 必须把 Web UI 和复杂控制面移除。
6. 必须严格限制并发数和对象尺寸。
7. 必须接受“ESP32-S3 版功能子集”和 Linux 主版不完全等价。

## 对当前仓库的具体改造建议

### 1. 先拆 feature profile

建议新增:

- `full`
- `lite`
- `esp-client`
- `esp-relay`

要求:

- `reqwest`
- `rusqlite`
- `tower-http`
- `tracing-subscriber`
- OneDrive provider

都不能默认进入 `esp-*` profile。

### 2. 把对象 API 改成流式

当前 `BlobBackend` 以整块 `Bytes` 传对象，不适合 MCU。

需要改成:

- 分块读取
- 分块写入
- 明确 chunk size
- 明确最大对象大小

在 `ESP32-S3` 上，优先支持:

- 小对象直接缓冲
- 大对象仅分块转发

### 3. 元数据层抽象化

当前只有 SQLite，不适合 MCU。

需要引入:

- `MetadataStore` trait
- `sqlite` 实现
- `memory` 实现
- `nvs` 或 `littlefs` 实现

ESP 档位至少要支持:

- 最近少量同步状态
- 极小容量 ring buffer
- 崩溃后可恢复的最小配置

### 4. 把 OneDrive 下沉到强宿主

OneDrive:

- TLS 成本高
- OAuth 成本高
- Graph 读写路径复杂

因此 OneDrive 不应在 `ESP32-S3` 本地实现。

合理方式:

- Linux 主机处理 OneDrive
- `ESP32-S3` 只调用上一级本地网关

### 5. 限制数据面语义

`ESP32-S3` 档位建议只保留:

- `GetObject`
- `PutObject`
- `HeadObject`

可选:

- 极小 `ListObjectsV2`

不建议保留:

- 多 target fallback
- 异步复制 worker
- 大量状态接口

## 推荐的一期策略

如果你希望最终覆盖 `ESP32-S3`，建议的一期策略是:

1. 保持当前 `gatewayd` 继续服务于 Linux / OpenWRT。
2. 明确 `ESP32-S3` 一期只做 `client-only`。
3. 二期再评估是否值得做 `esp32-s3-relay-lite`。
4. 在开始 `esp32-s3-relay-lite` 之前，先完成流式对象 API 改造。

当前 client-only 参考实现见 [esp32-s3-client-only.md](esp32-s3-client-only.md)。它复用 portable S3 client 核心，并用 ESP-IDF 的 mbedTLS / `esp_http_client` 做板级适配。

## 当前建议

对于你现在这个项目，最稳妥的判断是:

- `ESP32-S3` 可以成为重要目标客户设备
- 但不应成为当前完整 Rust daemon 的直接宿主
- 正确方向是“Linux 主网关 + ESP32-S3 极简客户端”

只有在你确认:

- 模块有足够 `PSRAM`
- 允许严格裁剪功能
- 接受功能不等价

的前提下，才值得继续规划 `esp32-s3-relay-lite`

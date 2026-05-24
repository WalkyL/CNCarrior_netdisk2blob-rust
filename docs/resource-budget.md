# 资源预算与算法模型

## 目标

本文件把当前代码的资源占用拆成两部分:

- 已在本机 `x86_64` 上测得的基线值
- 按当前算法和数据路径推导出的 RAM / Flash 预算

用途:

- 判断哪些平台适合跑 `host`
- 判断哪些平台只能跑 `client`
- 给 `OpenWRT`、`ESP32-S3`、`STM32` 的裁剪策略提供统一依据

## 实测基线

测量时间:

- `2026-04-25`

测量环境:

- Ubuntu `x86_64`
- `cargo build --release -p gatewayd`
- `stub` primary provider
- `RUST_LOG=warn`
- 无 sync target

当前测得:

- `gatewayd` release 二进制: `9,662,376` bytes，约 `9.22 MiB`
- `strip` 后二进制: `7,793,288` bytes，约 `7.43 MiB`
- 空载 RSS: `7,332 KiB`
- 空 SQLite metadata DB: `12,288` bytes，约 `12 KiB`

说明:

- 这些数字是当前代码的参考基线，不是 `OpenWRT arm64`、`musl`、`PVE container`、`ESP32-S3` 的直接实测值。
- 真正决定小设备可用性的，不是空载 RSS，而是对象 body 的非流式内存路径和对象状态元数据的累计规模。

## 当前资源占用由哪些算法决定

### 1. 对象 body 边界已改成流式，但还不是全链路流式

当前代码的对象模型已经改成 `ObjectBody` 流:

- `blob-core::ObjectPayload.body: ObjectBody`
- `blob-core::PutObjectRequest.body: ObjectBody`

这意味着:

- `gatewayd` 对 `UNSIGNED-PAYLOAD` 上传可以直接把请求体按流传给 backend
- `GET` 响应也可以直接把 backend 的对象流转回 HTTP 响应体
- 但很多 provider 内部仍会在本地把对象收集成整块，再交给上游 SDK/HTTP 客户端

这次流式改动带来的收益是:

- `gatewayd` 不再要求所有 `PUT` 先经 `axum::body::Bytes` 整块缓冲
- replication 路径也不再要求中间层必须拿到整块 `Bytes`

但当前系统仍不是“全链路零缓冲流式”，对象大小上限和并发预算依然重要。

### 2. `PUT` 路径已经分成两类

当前 `PUT` 路径分为:

- `UNSIGNED-PAYLOAD`: 网关层流式透传
- 其他显式 payload hash: 仍会先收集整个 body 再校验签名

因此单个上传对象的峰值内存不再是单一模型:

- 流式 `UNSIGNED-PAYLOAD PUT`:
  `peak_put_streaming ~= base_rss + protocol_overhead + provider_buffering`
- 非流式签名 `PUT`:
  `peak_put_buffered ~= base_rss + 1 * object_size + protocol_overhead`

如果 backend/provider 内部仍整块收集，则仍要按接近一份对象大小估算。

### 3. `GET` 路径对网关已经是流式回传

读取时会先拿到 `ObjectPayload.body` 流，之后直接作为响应体返回。

但如果 provider 下载阶段已经把对象整块读入内存，则 provider 侧峰值仍接近一份对象大小。

`peak_get ~= base_rss + object_size + 0.25~0.75 MiB`

### 4. replication worker 通常是一份 body，加上 HTTPS/TLS 开销

后台复制现在在网关/复制层可以直接传递对象流，但目标 provider 仍可能整块收集。

因此复制峰值依然保守按“一份对象 + 上游缓冲”估算更稳妥。

因此对单个复制 worker，峰值内存近似为:

`peak_replication ~= base_rss + object_size + 0.25~1.0 MiB`

如果目标是 `onedrive`，建议按上界估算。

### 5. 元数据裁剪算法只裁剪历史，不裁剪“最新对象状态”

当前 SQLite 保留策略如下:

1. 保留所有 `pending` 任务。
2. 对每个 `(target, bucket, key)`，保留最新一条任务记录。
3. 对剩余的 `completed` 历史，仅保留最新 `CCBG_METADATA_COMPLETED_HISTORY_LIMIT` 条。
4. 对剩余的 `failed` 历史，仅保留最新 `CCBG_METADATA_FAILED_HISTORY_LIMIT` 条。
5. 删除其他冗余历史。

这个算法的意义:

- 不破坏 fallback 所依赖的对象级最新状态判定
- 控制“最近成功/失败历史”无限增长
- 控制调试窗口和本地状态快照的体积

这个算法的边界:

- 它只能限制“额外历史”，不能限制“唯一对象数量”。
- 只要系统仍需要按对象判断 fallback，SQLite 里就至少要保留每个对象在每个 target 上的一条最新状态。
- 因此当前 DB 体积仍会随“被复制过的唯一对象数”线性增长。

这意味着:

- `OpenWRT` 可以承载“小到中等热数据集”的 host
- 但不适合“海量唯一对象、长期不清理”的归档型工作负载

### 6. sync target 数量会放大队列和元数据，而不是线性放大单次请求内存

一次 `PUT` 对 `T` 个 sync targets 的影响:

- 会产生 `T` 条 replication job
- SQLite 里会多出 `T` 组对象状态
- 内存里的 pending queue 也会多出 `T` 条任务

但单个请求的对象 body 内存峰值，主要仍由对象大小本身决定。

## RAM 预算公式

记号:

- `L = CCBG_MAX_IN_MEMORY_OBJECT_BYTES`
- `W = 同时进行的 PUT 请求数`
- `G = 同时进行的 GET 请求数`
- `R = replication worker 数`
- `Q = 当前 pending job 条数`
- `Hr = CCBG_REPLICATION_RECENT_LIMIT`
- `Hs = CCBG_METADATA_SNAPSHOT_RECENT_LIMIT`

当前代码下，可用一个保守模型估算:

`peak_rss ~= base_rss + (1 * W + 1 * G + 1 * R) * L + queue_overhead + tls_overhead`

其中:

- `base_rss` 当前参考值约为 `7~9 MiB`
- `queue_overhead` 主要来自 `pending_jobs` 和 `recent_jobs`
- `tls_overhead` 主要来自 OneDrive / HTTPS provider 的请求缓冲

### `pending_jobs` 和 `recent_jobs` 的内存特征

每个 `ReplicationJob` 包含:

- `target`
- `bucket`
- `key`
- `etag`
- `content_type`
- `last_error`
- 若干整数和枚举

保守估算:

- 普通短 key 场景: `0.3~0.8 KiB / job`
- 长 key / 长错误信息场景: `0.8~1.5 KiB / job`

因此:

- `Q=128` 时，队列本身通常在 `40~200 KiB`
- `Hr=64` 时，recent history 常见在 `20~100 KiB`

相对于对象 body，这部分通常不是主风险。

### 当前 OpenWRT 档位的可操作估算

如果采用 [openwrt-lite.env](../config/openwrt-lite.env):

- `L = 4 MiB`
- `R = 1`
- `Hr = 16`
- `Hs = 16`

则可按下面几种场景估算:

1. 空载:
   约 `7~9 MiB`
2. 单个 `GET 4 MiB`:
   约 `11~14 MiB`
3. 单个 `PUT 4 MiB`:
   约 `11~14 MiB`
4. 单个 replication worker 复制 `4 MiB` 对象:
   约 `11~16 MiB`
5. 最坏重叠场景: `1 PUT + 1 GET + 1 replication`
   约 `19~27 MiB`

这也是为什么当前判断是:

- `128 MiB RAM` 可以作为 OpenWRT host 的可运行下限
- `256 MiB RAM` 才是更稳妥的推荐线

### 并发的现实边界

当前代码已经限制了:

- replication worker 数量
- 单对象内存上限
- recent history 和 SQLite 历史窗口

当前代码还没有显式限制或完全流式化:

- 入站 S3 请求并发数
- 非 `UNSIGNED-PAYLOAD` PUT 的整包签名校验
- 多数 provider 内部的整块收集

所以对小内存宿主，必须把下面这些当成部署前提，而不是代码默认保证:

- Agent 侧控制并发
- 小宿主尽量单用户或低并发
- `OpenWRT` 上按 `W<=1`、`G<=1` 预算

## Flash / 可写存储预算公式

当前宿主上的持久化占用主要由这几部分组成:

`flash_total ~= binary + config + secrets + sqlite_db + logs`

### 1. 二进制

当前 `x86_64` 基线:

- release: `9.22 MiB`
- stripped: `7.43 MiB`

工程上对其他 Linux 宿主的保守预算建议:

- `full-host`: 预留 `10~16 MiB`
- `lite-host`: 预留 `8~12 MiB`

### 2. 配置与 secrets

这部分通常很小:

- `.env` 级别配置通常 `< 16 KiB`
- token 文件通常 `< 16 KiB`

即使留出备份副本，通常也可按 `< 128 KiB` 预算。

### 3. SQLite metadata DB

空库基线当前约 `12 KiB`。

之后 DB 大小主要由三部分组成:

`sqlite_db ~= db_base + latest_object_state_rows + bounded_history_rows + page_slack`

可以保守估算:

- 每条状态记录按 `1~2 KiB` 预算
- `page_slack` 按额外 `25%~100%` 余量预算

更实用的保守公式:

`sqlite_db_budget ~= 12 KiB + 2 KiB * row_count`

其中:

`row_count ~= pending_jobs + latest_object_state_rows + completed_history_limit + failed_history_limit`

关键点:

- `completed_history_limit` 和 `failed_history_limit` 已经可控
- `latest_object_state_rows` 当前仍取决于“被复制过的唯一对象总数”

### OpenWRT 档位的 SQLite 保守预算

按 `openwrt-lite.env`:

- `completed_history_limit = 64`
- `failed_history_limit = 64`

如果当前热数据集约为:

- `100` 个唯一对象
- `1` 个 sync target
- `pending_jobs <= 32`

则保守估算:

- `row_count ~= 32 + 100 + 64 + 64 = 260`
- `sqlite_db_budget ~= 12 KiB + 260 * 2 KiB ~= 532 KiB`

实际部署建议:

- 按 `1~4 MiB` 给 SQLite 预留空间

如果唯一对象数增长到:

- `1,000` 级别，DB 可很容易进入数 MiB
- `10,000` 级别，DB 可进入数十 MiB

这就是当前架构对低 flash 设备的硬边界。

### 4. 日志

对 `OpenWRT`，日志往往比 SQLite 更容易失控。

因此建议:

- `RUST_LOG=warn`
- 尽量输出到 `tmpfs`
- 不在小 flash 设备上长期保留高频文本日志

## 设备级预算与结论

## `PVE LXC x86/x64`

- 角色: `full-host`
- 当前结论: 适合
- RAM 下限: `128 MiB`
- RAM 推荐: `256 MiB+`
- 可写存储建议: `64 MiB+`
- 说明:
  - 适合完整 `daemon + SQLite + replication + OneDrive`
  - 适合作为主部署目标

## `Docker x86/x64`

- 角色: `full-host`
- 当前结论: 适合
- RAM 下限: `128 MiB`
- RAM 推荐: `256 MiB+`
- 镜像与持久卷建议: `128 MiB+`
- 说明:
  - 资源逻辑与 `PVE LXC` 接近
  - 还需加上基础镜像和容器层开销

## `Podman x86/x64`

- 角色: `full-host`
- 当前结论: 适合
- RAM 下限: `128 MiB`
- RAM 推荐: `256 MiB+`
- 镜像与持久卷建议: `128 MiB+`
- 说明:
  - 与 Docker 基本同级
  - 适合 rootless 和更保守的宿主环境

## `OpenWRT arm64`

- 角色: `lite-host`
- 当前结论: 可行，但只适合轻量 host 档位
- RAM 下限: `128 MiB`
- RAM 推荐: `256 MiB+`
- Flash / overlay 下限: `16 MiB`
- Flash / overlay 推荐: `32 MiB+`
- 推荐配置:
  - `CCBG_MAX_IN_MEMORY_OBJECT_BYTES=4194304`
  - `CCBG_REPLICATION_WORKERS=1`
  - `CCBG_REPLICATION_RECENT_LIMIT=16`
  - `CCBG_METADATA_SNAPSHOT_RECENT_LIMIT=16`
  - `CCBG_METADATA_COMPLETED_HISTORY_LIMIT=64`
  - `CCBG_METADATA_FAILED_HISTORY_LIMIT=64`
  - `CCBG_ONEDRIVE_ENABLED=false`
  - `RUST_LOG=warn`
- 说明:
  - 当前应视为 `sqlite-host` 路线
  - 适合小到中等热数据集
  - 不适合高并发、大对象、海量唯一对象归档

## `x86/arm64 软路由`

- 角色: 视实际系统而定，可按 `full-host` 或 `lite-host`
- 当前结论: 可行
- RAM 推荐:
  - `128 MiB` 起步按 `lite-host`
  - `256 MiB+` 可接近 `full-host`
- Flash / 可写存储建议: `32 MiB+`
- 说明:
  - 如果实际运行环境接近 OpenWRT，则按 OpenWRT 档位保守预算
  - 如果是完整 Linux 发行版，则可按容器宿主预算

## `ESP32-S3`

- 角色: `client-only` 为一期默认
- 当前结论: 不适合运行当前完整 `gatewayd`
- host 结论: 不支持
- client 预算目标:
  - 常驻 RAM: `< 256 KiB`
  - 峰值 RAM: `< 512 KiB`
  - 本地持久化: `< 128 KiB`
  - 单次对象缓冲: `8~32 KiB`
- 说明:
  - 后续应走 `tiny-state-client`
  - 不应继续复用完整 SQLite 宿主
  - 如果未来进入 `relay-lite`，也必须先完成流式对象 API

## `STM32`

- 角色: `client-only`
- 当前结论: 不适合运行当前完整 `gatewayd`
- host 结论: 不支持
- client 预算目标:
  - RAM: `32~128 KiB+`，视芯片型号而定
  - 本地状态: 极小配置，不建议本地 durable queue
  - 单次对象缓冲: `4~16 KiB`
- 说明:
  - 推荐只做本地 S3 / MCP 调用端
  - 不承担 SQLite、OneDrive、复制队列、fallback 判定

## 当前架构对小设备成立的前提

要让当前仓库在“小内存、小 flash”设备上站得住，前提不是“它已经完全轻量化”，而是接受下面这些条件:

1. 仅在 Linux 类设备上运行 host。
2. 小宿主严格限制对象大小。
3. 小宿主严格限制 replication worker 数。
4. 小宿主关闭高成本的 OneDrive 和 Web UI。
5. 小宿主把日志压到最低。
6. 当前 SQLite 裁剪仅能控制历史窗口，不能消除唯一对象数带来的 DB 线性增长。
7. `ESP32-S3` 和 `STM32` 当前只按客户端预算规划。

## 下一步建议

若目标是进一步覆盖更多小设备，优先级应为:

1. 把对象路径改成真正的流式读写。
2. 为 `gatewayd` 增加入站请求并发限制。
3. 把 SQLite 的“对象最新状态”从 job 历史里拆出来，改成更紧凑的 object-state 表。
4. 为 `ESP32-S3` 实现 `tiny-state-client`，不要复用当前 `sqlite-host`。

# OpenWRT Host 档位

## 目标

这个档位的目标不是承载完整控制面，而是在多数 `OpenWRT` 设备上稳定运行一个本地 host 服务。

优先级:

1. 稳定驻留
2. 可预测的内存上限
3. 可控的 flash 占用
4. 足够的 S3 host 能力
5. 再考虑 OneDrive 和更多同步目标

## 当前实现的现实边界

当前 `gatewayd` 已经可以作为 OpenWRT host 的基础，但它仍然是“非流式对象路径”:

- `PUT` 请求体会整块进入内存
- `GET` / fallback / replication 也会整块读出对象
- SQLite 仍用于复制状态持久化

因此 OpenWRT 档位的关键不是追求完整功能，而是限制:

- 对象大小
- 并发
- 同步目标数量
- 日志和 SQLite 增长

## 建议资源线

### 可运行下限

- RAM: `128MB`
- Flash / 可写存储: `16MB+`
- CPU: `arm64`

这个档位下应当:

- 只开一个 host 服务
- 不开 Web UI
- 复制 worker 限制为 `1`
- 限制单对象内存上限
- 谨慎启用 OneDrive

### 推荐线

- RAM: `256MB+`
- Flash / 可写存储: `32MB+`
- 有稳定 overlay / extroot

这个档位下:

- 可启用 carrier primary
- 可启用少量 sync target
- 可启用受控 fallback
- 可保留 SQLite 元数据

## 当前实测参考

基于当前 x86_64 release 构建，现有代码大致表现为:

- `gatewayd` release 二进制约 `9.2MB`
- strip 后约 `7.4MB`
- `stub` 空载 RSS 约 `6.7MB`

这些数字不能直接等同于 OpenWRT `arm64`，但可以说明:

- 空载本身不是最大问题
- 真正的风险来自对象 body 的整块缓冲

## OpenWRT 建议配置

推荐从 [openwrt-lite.env](../config/openwrt-lite.env) 起步。

资源预算和算法推导见 [resource-budget.md](resource-budget.md)。

关键建议:

- `CCBG_REPLICATION_WORKERS=1`
- `CCBG_REPLICATION_RECENT_LIMIT=16`
- `CCBG_METADATA_SNAPSHOT_RECENT_LIMIT=16`
- `CCBG_METADATA_COMPLETED_HISTORY_LIMIT=64`
- `CCBG_METADATA_FAILED_HISTORY_LIMIT=64`
- `CCBG_MAX_IN_MEMORY_OBJECT_BYTES=4194304`
- `RUST_LOG=warn`
- 默认先不启用 OneDrive
- 先只启用一个 primary provider

## 为什么要限制对象内存上限

当前代码已经支持:

- `CCBG_MAX_IN_MEMORY_OBJECT_BYTES`
- `CCBG_REPLICATION_RECENT_LIMIT`
- `CCBG_METADATA_SNAPSHOT_RECENT_LIMIT`
- `CCBG_METADATA_COMPLETED_HISTORY_LIMIT`
- `CCBG_METADATA_FAILED_HISTORY_LIMIT`

这个限制会作用于:

- `PUT`
- `GET`
- fallback 读取
- replication worker 的整块复制

意义:

- 给 OpenWRT 设备一个明确的峰值边界
- 避免大对象直接把 host 服务顶死
- 在流式重构完成前，先用“限制对象大小”换稳定性
- 在不破坏 fallback 最新状态判定的前提下，裁剪多余的 SQLite 历史

## OpenWRT Host 建议功能集

保留:

- 本地 S3 兼容 API
- 单 primary provider
- 受控 fallback
- 最小复制状态持久化
- stdio MCP

建议关闭或暂缓:

- Web UI
- OAuth broker
- 多个高成本 sync target
- 高并发复制
- 大对象操作

## 和 ESP32-S3 的关系

你的目标可以这样拆:

- `OpenWRT` 负责 host 服务
- `ESP32-S3` 在资源允许时做轻客户端
- `ESP32-S3` 资源不够时只做 `client-only`

这样能保证:

- Host 逻辑留在更强的 Linux 设备上
- MCU 侧只承受最小调用成本
- 不把 OneDrive、SQLite、TLS、后台复制都堆到 MCU 上

## 下一步建议

如果 OpenWRT 要成为一等 host 平台，建议后续按这个顺序推进:

1. 保持当前对象内存上限机制
2. 增加 OpenWRT 专用默认配置
3. 增加 SQLite 体积控制和日志裁剪
4. 把对象路径改成流式读写
5. 再评估是否值得让 `ESP32-S3` 进入 `relay-lite`

# ADR-003: 关键应用使用第三云盘远端写前日志与检查点恢复

## 状态
Accepted

Stage-1 implemented（关键应用 PUT、checkpoint + replay、按 checkpoint 清理远端 committed WAL）

## 日期
2026-05-26

## 背景

当前网关已经具备：

- 主写 provider + 异步复制目标的对象写入链路
- placement / logical object / protection plan / replication jobs 的本地持久化
- 控制面全量备份与恢复

但这还不能完全覆盖一个关键故障模型：

1. 关键应用把对象写到 `A` 云盘，并要求再复制到 `B` 云盘。
2. 网关主机在写入过程中或写入完成后、本地最新元数据与复制队列尚未稳定落盘前突然崩溃，甚至整机硬件损坏。
3. 管理员随后只能依赖“最近一次控制面备份”和仍然留在云盘上的事实来恢复。

在这个模型下，如果没有额外的远端小日志，系统会遇到几类问题：

- 主对象可能已经写入 `A`，但 placement 还没落盘
- 本地复制队列可能丢失，导致 `B` 永远补不到
- 本地可能残留“写到一半”的元数据，和真实云盘状态不一致
- 仅靠定时控制面备份，控制面 `RPO` 仍然受备份间隔约束

## 决策

### 1. 只对关键应用启用远端 WAL

这套机制不是全局默认强制开启，而是对被显式标记为关键的应用启用。

应用匹配以应用 ID 为准，由控制面策略决定。

### 2. WAL 固定写到第三个云盘的专用目录

关键写入启用 WAL 时：

- 对象真实字节仍写入主写 / 副本云盘
- 小日志写入第三个 provider
- 日志目录固定在托管根目录下的专用子目录
- 不参与普通对象 placement

### 3. 恢复模型采用 `checkpoint + WAL replay`

控制面备份继续作为检查点（checkpoint）。

远端 WAL 则保存检查点之后、尚未被下一次备份覆盖的关键写入事实。

恢复顺序固定为：

1. 恢复最近一个控制面备份
2. 读取其中记录的 `checkpoint_lsn`
3. 扫描第三云盘上的远端 WAL
4. 只回放 `lsn > checkpoint_lsn` 的记录

### 4. WAL 记录的是“状态与意图”，不是对象字节

远端 WAL 只保存小型 JSON 记录，至少包含：

- `lsn`
- `tx_id`
- `phase`
- 应用 ID
- bucket / key
- home provider
- logical object 元数据
- protection plan
- 变更前本地元数据快照

它不保存对象字节本体。

因此它的目标是：

- 重建 placement / logical object / protection plan
- 重建丢失的复制任务
- 回滚崩溃前残留的半成品本地元数据

而不是在任何情况下凭空恢复对象内容。

### 5. 恢复判定以“云盘对象是否真实存在”为准

对每条 WAL：

- 如果 home provider 上的对象存在：
  - 视为对象写入已经至少成功到 home provider
  - 重建本地元数据
  - 按 protection plan 补回缺失的复制任务
- 如果 home provider 上的对象不存在：
  - 视为这次写入没有完成
  - 用 WAL 中记录的“变更前元数据”回滚本地状态

### 6. 日志清理按 `checkpoint_lsn`，不按时间段

不允许按“某个时间段以前”简单删日志。

原因：

- 时间戳不能严格代表某条日志是否已被备份覆盖
- 备份可能失败、部分成功或晚于预期
- 主机时钟不可靠时更容易误删

因此：

- 每次成功生成控制面备份后，记录这次备份所覆盖的 `checkpoint_lsn`
- 只有 `phase=committed` 且 `lsn <= checkpoint_lsn` 的 WAL 才允许进入清理范围

### 7. 第一阶段只保护关键 `PUT`

第一阶段实现范围限定为：

- 关键应用的对象 `PUT`
- 写前生成 `prepare`
- 写成后更新为 `committed`
- 启动时做最小 replay / rollback

暂不在第一阶段直接覆盖：

- 删除事务
- rename / move / copy 事务
- 同 key 覆盖写的旧版本保留
- 历史对象显式迁移 / 加密重写事务

## 后果

### 正面影响

- 关键应用的控制面恢复 `RPO` 不再仅由自动备份间隔决定
- 主机崩溃后可依据“检查点 + WAL”修补关键对象元数据
- 不需要把对象字节额外写入本地磁盘，符合软路由 / TF 卡场景
- WAL 位于第三云盘独立目录，和普通对象命名空间分离

### 负面影响

- 每次关键写入会多一次第三云盘小对象写入
- 启动恢复逻辑更复杂
- WAL 本身也需要可观测、可审计与清理策略
- 第一阶段仍然不能承诺“任意对象操作都像数据库一样严格回滚”

## 直接要求

后续实现至少必须满足：

- WAL 默认关闭，按应用 ID 显式启用
- 远端 WAL 目录与普通对象目录隔离
- checkpoint 进入控制面备份元数据
- replay 只处理 `lsn > checkpoint_lsn`
- home 对象存在时补元数据与复制意图
- home 对象不存在时回滚本地残留元数据
- WAL 清理必须按 `checkpoint_lsn` 判定

## 参考

- [docs/gateway-backup-and-restore.md](../gateway-backup-and-restore.md)
- [docs/gateway-write-ahead-log.md](../gateway-write-ahead-log.md)
- [docs/resource-budget.md](../resource-budget.md)

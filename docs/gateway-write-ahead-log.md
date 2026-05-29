# 关键应用远端写前日志

## 目标

这套机制服务的是“关键应用写入后的控制面恢复”，不是对象字节归档。

它解决的是这样一类故障：

1. 关键应用把对象写入主云盘，并要求补副本。
2. 网关主机在写到一半、或写完但本地最新状态还没稳定前突然崩溃。
3. 管理员需要依赖最近一次备份和第三云盘上的小日志，把关键对象状态恢复出来。

## 基本模型

系统把恢复拆成两层：

### 1. checkpoint

也就是现有的控制面加密备份。

checkpoint 保存：

- 配置
- 云盘凭据
- 托管加密密钥
- placement / logical object / protection plan
- pending replication jobs
- 本地 WAL 状态，包括 `checkpoint_lsn` 和 `last_replayed_lsn`

### 2. 远端 WAL

WAL 是写到第三个云盘专用目录里的小 JSON 记录。

它不保存对象字节，只保存：

- `lsn`
- `tx_id`
- `phase`
- 应用 ID
- bucket / key
- home provider
- logical object 目标状态
- protection plan 目标状态
- 变更前本地元数据

## 为什么不是“时间段删日志”

如果按时间删日志，会有几个风险：

- 备份可能失败，但时间已经过去了
- 某次备份生成得慢，不能说明时间更早的 WAL 已经被覆盖
- 主机时钟不准时更容易误删

因此清理规则必须是：

- 备份成功时记录它覆盖到的 `checkpoint_lsn`
- 只有 `lsn <= checkpoint_lsn` 的 `committed` WAL 才可以删除

## WAL 存放位置

远端 WAL 固定放在托管根目录下的专用子目录。

示意：

- `root/<managed-root>/gateway-wal/...`

这条路径：

- 不参与普通对象 placement
- 不用于用户对象浏览
- 只服务于关键应用恢复

## 事务阶段

第一阶段先做两种状态：

### `prepare`

表示：

- 这次关键写入已经被登记
- 系统准备把对象写到 home provider
- 如果此时主机崩溃，后续恢复器至少知道这次写入“打算做什么”

### `committed`

表示：

- home 对象已经存在
- 本地目标元数据已经有足够信息可被恢复器重建

第一阶段不要求 WAL 在远端再区分更多状态。

## 启动恢复规则

启动时，恢复器执行：

1. 读取本地 / 备份恢复后的 `checkpoint_lsn`
2. 扫描远端 WAL 目录
3. 只处理 `lsn > replay_floor_lsn` 的记录（`replay_floor_lsn = max(last_replayed_lsn, checkpoint_lsn)`）

对每条记录：

### home 对象存在

如果记录中的 home provider 上真实对象存在，则：

- 重建 placement
- 重建 logical object
- 重建 protection plan
- 对缺失的同步目标重新补回复制任务

### home 对象不存在

如果 home provider 上真实对象不存在，则：

- 认为这次写入未完成
- 删除崩溃前可能残留的本地半成品元数据
- 如果 WAL 中记录了变更前状态，则恢复旧状态

## 它能恢复什么

能恢复：

- 主对象已写入，但 placement 丢了
- 主对象已写入，但 protection plan / logical object 丢了
- 主对象已写入，但复制队列丢了
- 主机崩溃导致本地留下不应存在的半成品元数据

## 它不能单独恢复什么

不能恢复：

- 还没成功写到任何云盘的对象字节
- 没有旧版本保护时的严格“覆盖写回滚”
- 已经物理删除后的对象内容

这些能力需要额外的：

- 隐藏版本
- 两阶段重写
- tombstone + 延迟物理删除

## Admin 控制面

管理入口里提供了“关键应用远端写前日志”卡片：

- 显式开关与日志云盘选择
- 关键应用 ID 勾选（必须显式选中，空白不匹配任何应用）
- 手动刷新远端 WAL 状态
- 手动清理已 checkpoint 的 `committed` WAL

默认不会随着总览自动刷新远端 WAL 目录，只有进入该卡片或点击刷新时才会扫描，以降低软路由/TF 卡场景下的额外负担。

## 清理语义

备份成功后，网关会推进 `checkpoint_lsn`，并尝试删除远端里 `lsn <= checkpoint_lsn` 的 `committed` WAL。

如果日志云盘不支持删除：

- 不会阻断备份成功
- 会把“需要手工清理”状态保留下来
- 管理入口会显示清理受阻与原因

## 第一阶段实现范围（当前）

当前已实现：

- 关键应用 `PUT` 进入 WAL（`prepare` → `committed`）
- 启动 replay / rollback
- `last_replayed_lsn` 高水位持久化
- checkpoint 驱动的远端 WAL 清理
- Admin 卡片中的策略配置、状态刷新与清理动作

后续再扩展：

- 删除事务
- rename / move / copy
- 历史对象显式迁移
- 加密重写 / 解密重写
- 已 checkpoint WAL 自动清理

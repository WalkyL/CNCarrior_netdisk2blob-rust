# 网关备份与恢复

## 目标

这套备份能力服务的是 `Carrier Cloud Drive Storage Gateway` 的控制面恢复，不是云盘对象数据本体归档。

设计目标有三条：

1. 让网关在软路由、N100、ARM64、TF 卡等资源紧张环境下仍能稳定备份和恢复。
2. 让恢复过程尽量是一键式整体恢复，而不是人工拼装一堆零散 JSON。
3. 让对象数据迁移、重写、加密重写这类高成本动作，和控制面备份彻底分离。

## 备份颗粒度

当前备份颗粒度是“控制面全量快照”。

备份包含：

- 拓扑、路由、高速提供方、写入目标
- S3 应用账号与权限
- 内容策略、加密配置、SMB 插件配置
- 云盘凭据、浏览器会话、provider lease 信息
- 网关托管加密密钥
- placement、logical object、protection plan、待执行复制队列

这里的 “S3 应用账号” 指的是应用级 `access_key_id + secret_access_key`。恢复后，Admin Web 仍然默认只在列表里显示 `secret_access_key_present`；需要现场回填或交付给外部应用时，再按单个应用显式显示或复制明文凭据。

备份不包含：

- 云盘里的对象字节
- 各 provider 上的真实文件内容

原因很直接：

- 对象数据本体体积大，放进这类备份会显著抬高内存、磁盘、网络和总时长。
- 控制面恢复追求一致性和低成本；对象数据迁移追求显式、可审计、可回滚。这两件事不应混在一起。

## 与远端 WAL 的关系

对于被标记为关键的应用，系统会额外引入“第三云盘远端 WAL”。

这时：

- 控制面备份仍然是 checkpoint
- 第三云盘 WAL 负责保存 checkpoint 之后的关键写入事实
- 恢复顺序变成“先恢复 checkpoint，再回放 `checkpoint_lsn` 之后的 WAL”

要注意：

- WAL 依然不保存对象字节
- WAL 保存的是“状态与写入意图”，不是大对象内容
- 因此它解决的是关键对象元数据与复制意图恢复，不是对象字节全量归档

这套设计的完整说明单独放在 [docs/gateway-write-ahead-log.md](./gateway-write-ahead-log.md)。

当前实现里，控制面备份成功后会自动推进 WAL `checkpoint_lsn`，并尝试清理远端里 `lsn <= checkpoint_lsn` 的 `committed` 记录；若目标云盘不支持删除，系统会保留“需手工清理”状态并在 Admin WAL 卡片里提示。

## RPO / RTO 建议

这里要把“控制面状态”和“对象数据本体”分开看。

### 对象数据本体

已经成功写入运营商云盘的对象数据，本质上不依赖本机磁盘保存，因此对象字节的 `RPO` 接近 `0`。

### 控制面状态

控制面状态的 `RPO` 取决于自动备份间隔。也就是：

- 你把自动备份间隔设成 `24 小时`，控制面 `RPO` 就是 `24 小时`
- 你把自动备份间隔设成 `60 分钟`，控制面 `RPO` 就是 `60 分钟`

建议值：

- 轻度家用、策略很少变：`24 小时`
- 日常活跃使用、经常改策略或凭据：`60 分钟`
- 上线切换窗口、频繁变更：`15 分钟`

### 恢复时间目标

建议把 `RTO` 目标设成：

- `10 到 30 分钟`
  前提是 provider 凭据仍然有效，恢复后无需重新扫码或重新短信登录
- `30 到 90 分钟`
  当 provider 凭据失效，需要重新人工认证时

## 归档格式

网关备份归档是：

1. 先把控制面快照序列化为 JSON
2. 再做 `gzip` 压缩
3. 再用用户提供的备份密码做密钥派生
4. 最后使用 `ChaCha20-Poly1305` 分块加密

归档扩展名：

- `.ccbgbak`

恢复时必须重新输入同一个备份密码。

## 备份密码

自动备份使用的密码不会写入控制面 JSON。

它会单独保存在本地私密文件中：

- 位于 `CCBG_CREDENTIALS_DIR` 下
- Unix 下权限会收紧为 `0600`

这样做的目的，是避免：

- 控制面 JSON 本身泄露时连同备份密码一起泄露
- 备份文件和解密口令同时被打包走

## 自动备份落点

自动备份支持两个落点：

### 本地副本

默认本地目录：

- `data/gateway-backups`

可以自定义到更耐写的磁盘路径。

### 云端副本

云端副本固定写到托管根目录下的专用路径：

- `root/<managed-root>/gateway-backups/<filename>.ccbgbak`

例如：

- `root/ccbg-managed-0979cb0a/gateway-backups/ccbg-backup-gateway-a-1779540062756.ccbgbak`

这条路径是专门为网关备份保留的，不参与普通对象 placement。

## 恢复语义

恢复是“整体覆盖式”的。

恢复后会替换：

- 当前控制面配置
- 当前云盘凭据
- 当前托管密钥
- 当前 placement / logical object / protection plan
- 当前待执行复制队列
- 当前本地 WAL 状态，例如 `checkpoint_lsn`

恢复不会直接删除云盘里已经存在的真实对象，也不会自动迁移对象数据本体。

## 资源开销考虑

### 内存

备份归档优先走文件流，不应把整份归档长期常驻内存。

### 本地磁盘

自动备份只保留最近若干份，超过保留数会自动清理，避免长期占满 TF 卡或系统盘。

### 云端空间

云端副本也只保留最近若干份。

如果目标 provider 不支持删除，旧云端备份可能无法自动清理，需要管理员手工处理。

同样地，远端 WAL 清理也依赖目标 provider 的删除能力；不支持删除时不会中断备份，但需要在运维窗口手工回收旧 WAL 文件。

## 什么时候不该把它当成“完整灾备”

如果你要解决的是下面这些问题，这套控制面备份并不等价于最终答案：

- 需要把对象数据整体迁移到另一家云盘
- 需要对既有对象做加密重写或解密重写
- 需要在 provider 之间补写旧对象副本
- 需要回收旧 provider 上的死数据

这些都属于显式对象迁移 / 重写 / 对账能力，而不是控制面备份本身。

## OPS-005 月度演练（离线）

本演练只做本地离线校验，不要求在 LXC/实机现场安装。它的目标是让操作者把 checkpoint、credential、WAL 与 metadata 四类恢复证据打包到一个可搬运目录中，在桌面或新宿主上用同一个检查器完成恢复前校验；真实 LXC 安装与服务启动 smoke 由操作者在测试机手动执行。

演练输入目录建议固定为：

```text
target/backup-restore-drill/
  checkpoint/checkpoint-summary.json
  credential/credential-inventory.json
  wal/wal-records.json
  metadata/metadata-snapshot.json
  report/drill-input.json
```

字段约束（最小集）：

- `checkpoint/checkpoint-summary.json`
  - `checkpoint_lsn`（int）
  - `replay_from_lsn`（int，且必须等于 `checkpoint_lsn + 1`）
- `credential/credential-inventory.json`
  - `entries`（非空数组）
  - 每个 entry 至少包含 `provider`、`credential_ref`
  - `contains_secret_material` 不能为 `true`
- `wal/wal-records.json`
  - 非空数组
  - 必须存在 `phase=committed` 且 `lsn > checkpoint_lsn` 的记录，证明 checkpoint 后 WAL 回放路径可测
- `metadata/metadata-snapshot.json`
  - `logical_object_count`（int）
  - `placement_count`（int）
  - `pending_replication_jobs`（int）
- `report/drill-input.json`
  - `drill_id`、`checkpoint_backup_file`、`restore_target`、`operator` 均为非空字符串

## 演练步骤

1. 准备演练输入目录（推荐放在 `target/backup-restore-drill/`）。
2. 运行离线校验：
   - `python3 scripts/check-backup-restore-drill.py --drill-root target/backup-restore-drill`
3. 如需先验证脚本本身，可生成最小离线样例并校验：
   - `python3 scripts/check-backup-restore-drill.py --drill-root target/backup-restore-drill-smoke --write-sample`
4. 读取输出 `target/backup-restore-drill/report/drill-check-result.json`。缺文件或字段错误也会生成该报告，便于归档失败证据。
5. 使用 [docs/backup-restore-drill-report-template.md](./backup-restore-drill-report-template.md) 填写本次演练记录（RTO/RPO、异常与改进项）。

## 建议验收口径

- checkpoint：`replay_from_lsn = checkpoint_lsn + 1`。
- credential：凭据清单完整且无 secret material。
- WAL：存在 checkpoint 之后的 committed 记录可用于恢复回放。
- metadata：关键计数字段齐全，可用于恢复后 smoke 前后对比。
- 报告：每月至少一次，报告中记录本次日期、恢复目标、RTO、RPO 与改进项。

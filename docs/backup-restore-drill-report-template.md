# backup/restore 演练报告模板（OPS-005）

> 说明：本模板用于月度离线演练记录，覆盖 checkpoint、credential、WAL、metadata 恢复证据，不要求 LXC/实机安装。

## 1. 基本信息

- 演练编号（drill_id）：
- 演练日期（YYYY-MM-DD）：
- 执行人（operator）：
- 恢复目标（restore_target）：
- 使用备份文件（checkpoint_backup_file）：
- 演练输入目录：

## 2. 目标与范围

- 本次目标：
- 范围内：
  - checkpoint 恢复路径校验
  - credential 恢复清单校验
  - WAL 回放起点与记录校验
  - metadata 快照完整性校验
- 范围外：
  - 对象字节全量归档/迁移
  - LXC/实机安装演练

## 3. 演练输入摘要

- checkpoint_lsn：
- replay_from_lsn：
- credential entries 数量：
- WAL records 数量：
- committed 且 lsn > checkpoint_lsn 的记录数量：
- logical_object_count：
- placement_count：
- pending_replication_jobs：

## 4. 执行命令与结果

- 命令：
  - `python3 scripts/check-backup-restore-drill.py --drill-root <drill_root>`
- 输出文件：
  - `<drill_root>/report/drill-check-result.json`
- 校验结论：
  - [ ] 通过
  - [ ] 未通过
- 失败项（code/message）：

## 5. RTO / RPO 记录

- 目标 RTO：
- 实际 RTO：
- 目标 RPO：
- 实际 RPO：
- 是否满足目标：
  - [ ] 是
  - [ ] 否

## 6. 异常与处置

- 异常描述：
- 根因判断：
- 临时处置：
- 永久修复建议：

## 7. 验收结论

- checkpoint： [ ] 通过 / [ ] 不通过
- credential： [ ] 通过 / [ ] 不通过
- WAL： [ ] 通过 / [ ] 不通过
- metadata： [ ] 通过 / [ ] 不通过
- 本月演练结论： [ ] 通过 / [ ] 不通过

## 8. 后续行动

- [ ] 更新运维手册
- [ ] 更新告警阈值
- [ ] 追加自动化校验项
- [ ] 下月复测

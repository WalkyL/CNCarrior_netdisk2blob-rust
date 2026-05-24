# 联通变更记录模板

这份模板用于记录“联通作为 primary provider 的一次正式变更”。

建议每次正式变更都新建一份副本，例如:

- `docs/records/unicom-change-2026-05-14.md`

如果当前仓库还没有专门的 `docs/records/` 目录，也可以先放到团队自己的变更系统里，但建议保留 markdown 原文。

在开始填写前，先完成:

- [docs/unicom-go-live-checklist.md](/home/walky/carrier-cloud-blob-gateway/docs/unicom-go-live-checklist.md:1)

如果你要先看当前阶段到底收到了什么程度，再回来填写模板，先看:

- [docs/unicom-phase-closeout-report.md](/home/walky/carrier-cloud-blob-gateway/docs/unicom-phase-closeout-report.md:1)

---

## 1. 基本信息

- 变更标题:
- 变更编号:
- 日期:
- 开始时间:
- 结束时间:
- 操作者:
- 复核人:
- 宿主环境:
- 当前部署形态:
  - `PVE LXC x86/x64`
  - `Docker x86/x64`
  - `Podman x86/x64`
  - `OpenWRT arm64`
  - 其他:

## 2. 变更目标

本次变更目标:

- [ ] 联通首次作为 primary provider 切流
- [ ] 联通凭证轮换
- [ ] 联通 `Family ID` 更新
- [ ] `root` / `family` 读写验证
- [ ] 对象动作预演
- [ ] fallback / sync target 验证
- [ ] 其他:

简述本次变更要达成的结果:

```text
在这里写 3-6 行，说明这次为什么改、改什么、预期结果是什么。
```

## 3. 变更前状态

### 3.1 当前 primary / topology

- 变更前 `CCBG_PRIMARY_PROVIDER`:
- 变更前 `CCBG_SYNC_TARGETS`:
- 变更前 `CCBG_FALLBACK_READ_ORDER`:

### 3.2 联通凭证来源

- [ ] `Unicom` 标签页直接注入
- [ ] `CCBG_UNICOM_TOKEN_FILE`
- [ ] `CCBG_UNICOM_COOKIE_HEADER_FILE`
- [ ] 其他:

- Token 更新时间:
- Cookie 更新时间:
- 是否依赖手工 `Family ID`:
- `Family ID` 值:

### 3.3 变更前检查结论

- [ ] 已完成 [docs/unicom-go-live-checklist.md](/home/walky/carrier-cloud-blob-gateway/docs/unicom-go-live-checklist.md:1)
- [ ] 已导出一份变更前共享历史
- [ ] 已保存一份变更前 `GET /api/status`
- [ ] 已确认 provider health 正常

变更前摘要:

```text
在这里写变更前状态摘要，例如:
- 联通 root/family 读取正常
- OneDrive sync target 已启用
- 当前 fallback 未开启
```

## 4. 具体变更内容

### 4.1 配置改动

列出本次修改的关键配置:

```dotenv
CCBG_PRIMARY_PROVIDER=
CCBG_SYNC_TARGETS=
CCBG_FALLBACK_READ_ORDER=
CCBG_UNICOM_TOKEN_FILE=
CCBG_UNICOM_COOKIE_HEADER_FILE=
CCBG_UNICOM_FAMILY_ID=
CCBG_OBJECT_ACTION_HISTORY_LIMIT=
```

### 4.2 控制面操作

本次在 Admin Web 中执行的动作:

- [ ] 更新 `Unicom` 标签页中的 China Unicom 配置
- [ ] 点击 `Test Now`
- [ ] 验证 `root` 读取
- [ ] 验证 `family` 读取
- [ ] 执行 upload
- [ ] 执行 delete
- [ ] 执行 rename
- [ ] 执行 copy
- [ ] 执行 move
- [ ] 导出共享历史
- [ ] 清空共享历史
- [ ] 调整 topology
- [ ] 其他:

### 4.3 对象动作明细

如果本次执行了对象动作，按下面格式记录:

```text
动作 1:
- action:
- bucket/source_bucket:
- key/source_key:
- destination/new_key:
- 预期结果:
- 实际结果:

动作 2:
- action:
- bucket/source_bucket:
- key/source_key:
- destination/new_key:
- 预期结果:
- 实际结果:
```

## 5. 验证结果

### 5.1 Provider Health

- [ ] `China Unicom` health 正常
- [ ] personal scope 可见
- [ ] family scope 可见
- [ ] scope 容器映射符合预期

摘要:

```text
在这里写 provider health 的关键结论。
```

### 5.2 读写验证

- [ ] `root` list 正常
- [ ] `root` get 正常
- [ ] `root` put 正常
- [ ] `root` delete 正常
- [ ] `family` list 正常
- [ ] `family` get 正常
- [ ] `family` put 正常

摘要:

```text
在这里写读写验证结论。
```

### 5.3 对象动作验证

- [ ] rename 正常
- [ ] copy 正常
- [ ] move 正常
- [ ] before/after 检查结果可信
- [ ] 共享历史记录完整

摘要:

```text
在这里写对象动作验证结论。
```

### 5.4 复制与 fallback 验证

- [ ] 本次未启用 sync/fallback
- [ ] sync target 写入正常
- [ ] fallback 读验证正常
- [ ] 对象动作后的复制元数据语义符合预期

摘要:

```text
在这里写复制与 fallback 结论。
```

## 6. 证据清单

建议把证据按下面格式挂上:

- 变更前 `GET /api/status`:
- 变更后 `GET /api/status`:
- 变更前共享历史导出:
- 变更后共享历史导出:
- Provider Health 截图 / JSON:
- 成功 upload 证据:
- 成功 rename/copy/move 证据:
- 关键日志片段:

## 7. 风险与异常

### 7.1 预期内风险

```text
例如:
- 联通 rename 仍仅支持同父目录
- family scope 依赖当前会话里的 Family ID
```

### 7.2 实际异常

```text
在这里写本次实际遇到的问题。
如果没有，写“无”。
```

## 8. 回滚记录

- 是否触发回滚:
- 回滚开始时间:
- 回滚结束时间:
- 回滚动作:
- 回滚后状态:

如未回滚:

```text
未回滚。说明为什么没有触发回滚条件。
```

## 9. 最终结论

- [ ] 变更成功
- [ ] 部分成功，但有已知风险
- [ ] 失败，已回滚

最终结论摘要:

```text
用 5-10 行写清楚:
- 本次是否成功
- 联通当前是否可作为正式 primary provider 使用
- 还有哪些风险没有消除
- 后续动作是什么
```

## 10. 后续跟进项

1.
2.
3.

## 11. 相关文档

- [docs/unicom-go-live-checklist.md](/home/walky/carrier-cloud-blob-gateway/docs/unicom-go-live-checklist.md:1)
- [docs/object-actions-and-history.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-and-history.md:1)
- [docs/object-actions-api-reference.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-api-reference.md:1)
- [docs/auth-step-by-step.md](/home/walky/carrier-cloud-blob-gateway/docs/auth-step-by-step.md:225)

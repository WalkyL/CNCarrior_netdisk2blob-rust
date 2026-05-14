# 联通阶段收尾报告

## 1. 范围

这份报告用于对“联通能力做到阶段性交付”的当前状态做一次收口。

这里的“阶段收尾”指的不是项目整体结束，而是:

- 联通 provider 已从“概念接入”进入“可作为正式 primary provider 候选”的阶段
- 控制面、对象动作、共享历史和运维文档已经形成闭环
- 后续工作主要进入“生产化加强”和“自动化回归”阶段

## 2. 本阶段目标

本阶段目标原本聚焦在三件事:

1. 把联通原生读写链路做完整。
2. 把联通对象动作与网关复制语义做完整。
3. 把联通相关运维文档、上线清单和留档模板做完整。

## 3. 当前完成情况

### 3.1 联通 provider 能力

当前已完成:

- 真实目录列举
- 真实下载
- `upload2C` 上传
- 对象删除
- native `rename`
- native `copy`
- native `move`
- personal / family scope 发现
- `root` / `family` 容器映射

这意味着联通当前已经不是“只能看目录”的接入状态，而是具备:

- 基础读路径
- 基础写路径
- 多容器视图
- 对象动作能力

### 3.2 gatewayd 集成能力

当前已完成:

- 联通可作为当前 primary provider
- `root` / `family` 容器可通过网关暴露
- 对象动作 API 已打通
- 对象动作后复制元数据会同步更新
- Admin Web 已提供对象动作面板
- Admin Web 已提供 before/after 检查
- Admin Web 已提供共享历史、筛选、导出、清空
- 共享历史已从浏览器本地状态迁到服务端 control-plane 状态

### 3.3 历史与审计能力

当前已完成:

- 历史服务端持久化
- 历史保留上限配置化
- `GET /api/status` 暴露 `object_action_history`
- `GET /api/status` 暴露 `object_action_history_limit`
- `POST /api/object-actions/history/clear`
- Admin Web 支持 action / outcome / provider 筛选
- Admin Web 支持导出当前筛选结果

对应配置:

- `CCBG_OBJECT_ACTION_HISTORY_LIMIT`

### 3.4 文档资产

当前已补齐的联通相关文档资产:

- [docs/provider-completion-standard.md](/home/walky/carrier-cloud-blob-gateway/docs/provider-completion-standard.md:1)
- [docs/auth-step-by-step.md](/home/walky/carrier-cloud-blob-gateway/docs/auth-step-by-step.md:1)
- [docs/object-actions-and-history.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-and-history.md:1)
- [docs/object-actions-api-reference.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-api-reference.md:1)
- [docs/unicom-go-live-checklist.md](/home/walky/carrier-cloud-blob-gateway/docs/unicom-go-live-checklist.md:1)
- [docs/unicom-change-record-template.md](/home/walky/carrier-cloud-blob-gateway/docs/unicom-change-record-template.md:1)

这代表联通不只是“代码能跑”，而是已经具备:

- 接入说明
- 运维手册
- API 契约
- 上线前检查清单
- 变更后留档模板

## 4. 测试与验证结论

当前与联通阶段收尾直接相关的结论:

- `cargo test -p gatewayd` 已全量通过
- 当前全量结果是 `51 passed; 0 failed`

覆盖重点包括:

- provider health 中的 family scope 呈现
- `root` / `family` 容器对外暴露
- 对象动作 `rename/copy/move`
- 对象动作历史持久化
- 对象动作历史按上限截断
- Admin Web 暴露对象动作与共享历史能力

## 5. 当前明确边界

这部分不是缺陷，而是当前阶段需要明说的边界。

### 5.1 联通 rename 边界

当前联通 `rename`:

- 只支持同父目录改名

因此:

- 跨目录调整应使用 `move`

### 5.2 family 仍依赖有效会话事实

当前 `family` 容器虽然已经进入正式能力范围，但仍要注意:

- 自动发现或手工注入的 `Family ID` 必须来自有效会话
- 正式上线前必须重新确认 `family` 可读

### 5.3 shared history 仍是轻量审计模型

当前共享历史已经足够运维使用，但还不是完整审计系统。

当前已经具备:

- `operator / ticket / notes` 审计字段
- 按 action / outcome / provider / operator / object / time-window 的筛选
- JSON / CSV 导出
- 控制面自动刷新与最后刷新状态提示
- 控制面监控摘要卡片，可直接汇总 alerts、provider 健康、复制失败和最近失败事件

当前尚未具备:

- 对接外部告警系统后的跨实例聚合统计
- 审计字段和对象历史的外部系统联动

## 6. 当前阶段判断

按项目自己的完成度标准看，`unicom` 当前已经达到:

- `full`

原因是:

- 认证与会话有明确注入路径
- 作用域发现已支持 personal / family
- 原生读路径已打通
- 原生写路径已打通
- 对象动作与复制语义已打通
- 网关集成已覆盖多容器与对象动作元数据同步
- 文档与测试已经基本成套

但“达到 full”不等于“可以停止打磨”。

它表示:

- 当前主流程可交付
- 继续投入的重点从“补缺功能”转向“提升生产化质量”

## 7. 建议作为阶段收尾的结论

可以把当前联通状态定义为:

> 联通已经完成一期功能闭环，可以作为正式 primary provider 候选进入受控上线与运维阶段。

这句话背后的前提是:

- 上线前仍需逐项走 checklist
- 正式变更仍需保留 change record
- 真实生产稳定性仍依赖新鲜凭证、family scope 验证和对象动作审计留痕

## 8. 下一阶段建议

### 8.1 高优先级

1. 接入真实账号 E2E 回归。
2. 把审计字段接到外部变更系统或告警系统。
3. 给控制面补跨实例聚合与外部告警联动。

### 8.2 中优先级

1. 给 `family` 相关浏览器 flow 补更完整变体。
2. 把联通正式变更记录沉淀到单独的 `docs/records/` 目录规范。
3. 继续压实 OpenWRT 档位的保守参数和实机验证。

### 8.3 低优先级

1. 更细粒度的历史聚合视图。
2. 更细粒度的控制面趋势视图。
3. 更漂亮但非关键的 UI 打磨。

## 9. 建议保留的阶段资产

作为本阶段结束时，建议固定保留这些资产:

1. 一份当前通过的测试结论
2. 一份联通 go-live checklist 空模板
3. 一份联通 change record 模板
4. 一份对象动作共享历史导出样例
5. 一份当前 control-plane / provider health 截图或 JSON

## 10. 相关文档索引

- [docs/provider-completion-standard.md](/home/walky/carrier-cloud-blob-gateway/docs/provider-completion-standard.md:1)
- [docs/auth-step-by-step.md](/home/walky/carrier-cloud-blob-gateway/docs/auth-step-by-step.md:1)
- [docs/object-actions-and-history.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-and-history.md:1)
- [docs/object-actions-api-reference.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-api-reference.md:1)
- [docs/unicom-go-live-checklist.md](/home/walky/carrier-cloud-blob-gateway/docs/unicom-go-live-checklist.md:1)
- [docs/unicom-change-record-template.md](/home/walky/carrier-cloud-blob-gateway/docs/unicom-change-record-template.md:1)

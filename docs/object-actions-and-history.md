# 对象动作与共享历史运维手册

这份文档专门说明 `gatewayd` 里的对象动作控制面，包括:

- `rename`
- `copy`
- `move`
- before/after 对象检查
- 服务端共享历史
- 联通当前边界
- 运维建议与故障排查

如果你只想知道“联通现在做到什么程度”，先看:

- [docs/provider-completion-standard.md](/home/walky/carrier-cloud-blob-gateway/docs/provider-completion-standard.md:1)
- [docs/provider-matrix.md](/home/walky/carrier-cloud-blob-gateway/docs/provider-matrix.md:1)

如果你需要先完成联通认证材料注入，再回来执行对象动作，先看:

- [docs/auth-step-by-step.md](/home/walky/carrier-cloud-blob-gateway/docs/auth-step-by-step.md:1)

如果你需要接口字段、请求体和返回契约，去看:

- [docs/object-actions-api-reference.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-api-reference.md:1)

## 1. 当前能力概览

当前 `gatewayd` 已支持:

- `POST /api/object-actions`
- `POST /api/object-actions/history/clear`
- `GET /api/status` 返回 `object_action_history`
- `GET /api/status` 返回运行态 `runtime` 摘要
- `GET /api/status` 返回聚合监控 `monitoring` 摘要
- Admin Web 里的对象动作面板
- Admin Web 里的 `Monitoring Summary` 监控摘要卡片
- before/after 对象状态检查
- 服务端持久化共享历史
- 历史筛选、JSON/CSV 导出、清空
- operator / ticket / notes 审计字段
- 控制面自动刷新

当前共享历史的几个关键事实:

- 历史以服务端 control-plane 状态为准，不再依赖浏览器 `localStorage`
- 清空历史会影响所有访问同一网关控制面的操作者
- 历史长度有上限，超出会从最旧记录开始裁剪
- 历史导出的是“当前筛选结果”，不是无条件全量导出
- 单条历史现在可附带 `operator`、`ticket`、`notes`

当前已完成对象动作接入的 provider 包括:

- `unicom`
- `onedrive`

运行策略上需要额外注意:

- `POST /api/object-actions` 始终作用于“当前 primary provider”
- 本项目当前只允许运营商 provider 或 `stub` 作为 primary
- 因此 OneDrive 虽然已具备 provider 侧对象动作实现，但运行时定位仍是异步备份 / 可选 fallback 目标，而不是主写后端

## 2. 动作语义

### 2.1 rename

输入形状:

```json
{
  "action": "rename",
  "bucket": "family",
  "key": "shared/note.txt",
  "new_key": "shared/renamed.txt"
}
```

网关复制语义:

- `rename = put(new) + delete(old)`

含义不是“只改主后端名称”，而是:

1. 当前 primary provider 先执行 rename。
2. 动作成功后，网关会补一条针对新 key 的复制 `put` 元数据。
3. 同时补一条针对旧 key 的复制 `delete` 元数据。

这样做的目的，是让 fallback / backup 侧最终也能收敛到新对象名，而不是留下旧对象脏状态。

### 2.2 copy

输入形状:

```json
{
  "action": "copy",
  "source_bucket": "family",
  "source_key": "shared/renamed.txt",
  "destination_bucket": "root",
  "destination_key": "docs/copied.txt"
}
```

网关复制语义:

- `copy = put(dest)`

含义是:

1. primary provider 上生成目标对象。
2. 网关把目标对象视为一条新的写入事实。
3. backup / fallback 侧后续只需要复制目标对象，不会删除源对象。

### 2.3 move

输入形状:

```json
{
  "action": "move",
  "source_bucket": "root",
  "source_key": "docs/copied.txt",
  "destination_bucket": "family",
  "destination_key": "shared/moved.txt"
}
```

网关复制语义:

- `move = put(dest) + delete(src)`

它和 `rename` 的差别主要在于:

- `rename` 偏向“同一逻辑对象改名”
- `move` 明确允许跨目录，甚至跨容器

但从复制元数据角度看，两者都需要“新增目标 + 清理源对象”。

## 3. 联通当前边界

联通当前已经支持:

- `root` / `family` 两个容器
- 真实下载
- `upload2C` 上传
- 对象删除
- native `rename/copy/move`

当前仍然要明确的边界:

- 联通 `rename` 当前只支持“同父目录改名”
- 若需要跨目录调整，应该优先使用 `move`
- 跨 `root` / `family` 的 `copy` 或 `move` 需要确认目标容器是有意的
- `copy` / `move` 的目标 key 如果已存在，可能被覆盖

控制面已经把这些风险前置到执行预览里，但操作者仍然要对目标路径负责。

## 4. OneDrive 当前边界

OneDrive 当前已经支持:

- `root_prefix/<bucket>/<key>` bucket 映射
- 真实 Graph 读写删
- Graph rename/copy/move

当前要明确的实现边界:

- `rename` / `move` 通过 Graph `PATCH` 更新 `name` / `parentReference`
- `copy` 通过 Graph async copy 完成，控制面成功返回前会等待 monitor URL 轮询完成
- 目标 key 如果已存在，Graph 或 provider 语义可能返回冲突，而不是静默覆盖

## 5. Admin Web 操作说明

### 5.1 入口

打开 Admin Web 后，进入 `Object Actions` 卡片。

这块会提供:

- 动作类型选择
- bucket/key 输入
- 执行预览
- before/after 检查结果
- 共享历史
- 审计输入: `operator` / `ticket` / `notes`

### 5.2 执行前预览

执行前预览会明确提示:

- 当前 primary provider
- 计划动作
- 复制语义
- 风险警告

联通下常见警告包括:

- rename 跨父目录
- copy/move 跨容器
- 目标对象可能被覆盖
- no-op rename / no-op move

如果预览已经显示风险，而这不是你预期的动作，先不要执行。

### 5.3 before/after 检查

每次动作执行后，页面会显示被影响对象的 before/after 检查结果。

检查结果会按对象展示:

- label
- bucket/key
- 变化摘要
- before 概览
- after 概览
- provider 级变化

这一步的目的不是替代真正的数据校验，而是让操作者快速看到:

- 对象是否还存在
- 哪个 provider 可读
- fallback gate 是否改变
- 元数据是否已经转向目标 key

### 5.4 共享历史

共享历史现在提供:

- action 筛选
- outcome 筛选
- primary provider 筛选
- operator 筛选
- bucket/key 检索
- 时间范围筛选
- 导出当前筛选结果
- 导出 CSV
- 清空整份共享历史

注意:

- 历史是“这个网关实例的共享状态”
- 不是“当前浏览器 tab 的私有状态”
- 不是“某一个操作者的个人历史”

### 5.5 控制面自动刷新

Admin Web 顶部现在提供:

- `Auto-refresh dashboard`
- `Refresh Every (s)`
- `Last refresh` 状态摘要

这套自动刷新默认只轮询 `GET /api/status`，用于持续观察:

- provider health
- replication queue
- monitoring summary
- runtime 摘要
- 共享历史

### 5.7 Monitoring Summary

Admin Web 现在额外提供 `Monitoring Summary` 卡片，聚合展示:

- open alerts 数量
- provider 健康计数: healthy / degraded / unavailable
- replication pending / failed 概览
- 对象动作失败数与最近一次对象动作时间
- 最近失败事件列表，来源包括失败的对象动作和失败的复制任务

这块摘要的目标不是替代详细表格，而是让运维先快速判断当前实例是否处在需要人工介入的状态。

为了避免干扰操作中的表单，自动刷新不会在后台每一轮都强制重载 provider credentials 和 pending verification prompts；这些区域仍以手工触发或显式动作后的刷新为主。

### 5.6 导出

点击 `Export Shared History` 会导出一份 JSON 文件。

导出内容包含:

- `exported_at`
- `history_limit`
- `filters`
- `entries`

它更适合:

- 事后审计
- 给另一个工程师复盘
- 附到问题单或变更记录里

点击 `Export Shared History CSV` 会导出一份面向审计表格的 CSV。

它更适合:

- 交给运维或变更流程做留档
- 在表格工具里筛选 operator / ticket / object
- 做阶段性复盘

### 5.7 清空

点击 `Clear Shared History` 会调用:

```text
POST /api/object-actions/history/clear
```

这会清空 control-plane 文件里的共享历史。

适合的使用场景:

- 一轮演练完成后，准备开始新的正式操作
- 需要把测试历史与生产历史切开
- 希望缩短运维复盘时的噪音

不适合的使用场景:

- 还没有导出证据就直接清空
- 多个操作者同时使用同一网关时未经沟通直接清空

## 6. 历史持久化模型

对象动作共享历史保存在 control-plane 文件里。

关键配置:

- `CCBG_CONTROL_PLANE_FILE`
- `CCBG_OBJECT_ACTION_HISTORY_LIMIT`

设计目的:

- 网关重启后仍然保留最近历史
- 多个浏览器访问同一控制面时看到同一份历史
- 不把控制面状态绑定到单个浏览器环境

当前每条历史大致包含:

- `executed_at_unix_ms`
- `primary_provider`
- `action`
- `operator`
- `ticket`
- `notes`
- `description`
- `outcome`
- `message`
- `warnings`
- `references`

`references` 用来表达“这次动作影响了哪些对象，以及这些对象发生了什么变化”。

## 6. 历史上限配置

环境变量:

```dotenv
CCBG_OBJECT_ACTION_HISTORY_LIMIT=12
```

当前建议:

- 通用宿主: `12`
- 多人共用但操作频率不高: `12 ~ 32`
- OpenWRT / 轻量宿主: `8`

选择原则:

- 值太小: 容易丢掉刚刚可用于复盘的证据
- 值太大: control-plane 文件会持续增长，UI 也更嘈杂

当前项目默认值:

- `config/example.env`: `12`
- `config/openwrt-lite.env`: `8`

## 7. 与复制 / fallback 的关系

对象动作不是孤立的 UI 行为，它会直接影响:

- 复制任务
- fallback 判定
- 对象可读性检查

要点如下:

1. primary provider 动作成功，不代表 backup 已经同步完成。
2. 控制面会立即补齐对应复制元数据。
3. fallback 仍然受对象最新复制状态约束，不会因为动作成功就无条件放开。
4. before/after 检查展示的是“当前观察到的状态”，不是未来最终一致状态的承诺。

因此，正式运维时建议把对象动作分成两步看:

1. 先确认 primary provider 上动作结果正确。
2. 再确认复制状态是否按预期收敛。

## 8. 推荐运维标准

建议把对象动作操作规范化成下面这套顺序:

1. 先确认当前 primary provider 和目标 bucket/key。
2. 先看执行预览，不要直接点运行。
3. 涉及联通 rename 时，先确认是否同父目录。
4. 涉及 `family` 容器时，先确认 family scope 已正常发现。
5. 执行后先看 before/after 检查，再看共享历史。
6. 正式操作前后都导出一次历史，保留审计证据。
7. 一轮测试结束后再清空共享历史，不要在中途清。

## 9. 常见问题

### 10.1 为什么 rename 成功率不稳定

先区分是哪一层失败:

- 页面预览已经警告“联通 rename 仅支持同父目录”
- provider 返回上游错误
- token / cookie 已经过期

如果是跨目录 rename，需要改成 `move`。

### 10.2 为什么历史里没有我刚刚更早的记录

优先检查:

- 是否已经超过 `CCBG_OBJECT_ACTION_HISTORY_LIMIT`
- 是否有人清空了共享历史
- 是否当前看的是筛选后的结果

### 10.3 为什么历史里出现 `missing before/after snapshot`

这表示网关在某一侧对象状态检查时没有拿到完整快照。

常见原因:

- 对象在动作前后极快变化
- provider 状态查询失败
- 认证材料已过期

这不一定表示对象动作本身失败，但表示审计证据不完整，需要继续查原始 provider 状态。

### 10.4 为什么 `family` 容器不可用

通常要检查:

- 联通 token 是否仍有效
- `Family ID` 是否能自动发现
- 是否需要手工注入 `Family ID`

相关认证和联通接入步骤见:

- [docs/auth-step-by-step.md](/home/walky/carrier-cloud-blob-gateway/docs/auth-step-by-step.md:225)

### 10.5 为什么动作成功但 fallback 读还不正常

因为动作成功只说明 primary provider 已处理完成。

如果 backup / fallback 侧复制还没完成，或者旧删除任务还在 pending，fallback 仍可能暂时不可读。这属于最终一致性模型内的正常现象。

## 10. 当前未覆盖的增强项

这部分不是“缺功能”，而是后续可以继续增强:

- 审计字段与外部变更/告警系统联动
- 更细粒度的聚合统计视图
- 接入真实账号 E2E 回归

如果后续要继续提升“联通做到满”的运维质量，优先级最高的是:

1. 真实账号 E2E
2. 审计字段对接外部系统
3. 更细粒度聚合视图

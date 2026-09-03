# 对象管理与批量删除/移动设计

**日期：** 2026-09-04

**状态：** 已按 Sol 两轮审阅修订，待用户审阅

## 背景

Admin Web 的“对象与文件操作”页已经能从当前读来源浏览对象，也已经有网关托管的单对象删除、重命名、复制和移动能力。但当前列表行操作主要是“检查”，用户需要把已经看到的 bucket/key 再次填入下方表单，才能执行动作。

用户真正要完成的是：

1. 找到需要整理的文件；
2. 选择一个或多个文件；
3. 移动或删除；
4. 看到明确的执行结果。

本设计把当前对象列表变成管理入口，首版只优化两个高频任务：批量删除和批量移动。

## 设计原则

### 第一性原理

普通操作者不需要先理解 provider、placement、复制拓扑或 WAL。界面应围绕“找文件、选文件、执行整理、确认结果”组织。

### 易用性

- 从当前列表直接选择，不重复输入已显示的 bucket/key；
- 单对象和批量对象使用同一套管理交互；
- 批量移动只填写目标桶和目标目录，自动保留每个源对象的文件名；
- 删除前显示数量、对象摘要和真实影响；
- 执行后按对象显示结果，不把部分失败伪装成整体成功。

### 简约性

- 未选中对象时不显示批量工具栏；
- 真实云盘对象删除与 Placement 元数据清理使用不同入口和文案；
- 删除、移动是主路径；rename/copy 收进高级操作；
- 不新增 provider 选择器、重复对象输入表单或多余确认层。

## 方案与范围

采用“增强现有对象列表”的方案。它复用当前浏览、对象状态、网关动作和共享历史，减少导航和重复实现。

本次包含：

- 当前结果列表的多选、当前结果全选和清除选择；
- 单对象“管理”入口；
- 批量删除；
- 批量移动到目标桶和目录，并保留 basename；
- 服务端预检、短期 selection/plan；
- 网关内对象变更锁；
- 幂等提交、逐项结果和批次历史摘要；
- Admin API、Rust 行为测试、DOM/state 测试和运维文档。

本次不包含：

- 递归文件夹操作、服务端全量搜索或未加载对象的隐式全选；
- 回收站、撤销删除或用户指定底层 provider；
- 批量 copy、批量 rename；
- provider crate 或 BlobBackend 条件移动协议改造；
- .43/.49 部署、真实凭据注入和正式发布。

## 并发与一致性边界

本版采用实用方案，不向 provider 要求当前抽象没有的条件移动或 CAS 能力。

### 网关内保护

增加一把网关内对象变更锁。以下现有 Admin 路径和新批量执行必须使用它：

- POST /api/object-actions；
- POST /api/object-actions/batch；
- POST /api/object-reconcile/execute 的实际执行分支；
- POST /api/object-placement/delete-stale；
- POST /api/object-placement/delete-stale-bulk。

批量执行从最终预检开始一直持有锁，直到本批次结束。预览、状态查询和历史导出不持有该锁。数据面 S3 PUT/DELETE、后台 replication worker 和绕过网关的 provider 客户端不在锁内。

### 产品保证

产品保证精确定义为：

- 最终网关预检观察到任何冲突时，在第一项写入前阻止整个批次；
- 受同一把锁保护的其他 Admin 对象元数据动作不会插入最终预检与执行之间；
- 外部 S3/provider 写入或其他网关实例造成的竞态不宣称为分布式原子保证；
- 如果下一个对象开始前发现外部变化，停止剩余未开始项并返回逐项状态；已经完成的项不自动撤销；
- 预览、执行结果和批次历史都带有 non_atomic_warning 与可读的 consistency_note，明确外部竞态边界。

锁最多等待 5 秒；超时返回 409、reason code batch_busy 和 Retry-After: 5，不开始任何对象动作。

锁忙、ledger 容量不足或 ledger 持久化失败都属于批次级错误，不产生对象动作历史。它们分别使用 batch_busy、batch_ledger_full 和 batch_ledger_unavailable。

## UI 设计

### 对象列表与选择状态

“浏览桶和对象 Key（当前读来源）”继续作为主列表。每行显示：

- 选择框；
- object key；
- 大小；
- 修改时间；
- “管理”按钮。

前端保存：

- selection_id；
- bucket、prefix；
- 列表 read source/provider 与 fallback source；
- 加载时间；
- 已加载对象的 canonical bucket/key 和身份快照（etag、size、last_modified）。

selection_id 是服务端短期快照标识，不是授权机制。当前结果全选只选择已经渲染的行，不代表服务端未加载的对象。单次最多选择 100 项。

下列任一事件发生时，必须清空选择、丢弃 selection_id、关闭批量面板并清除当前批次预览：

- 切换 bucket；
- 修改 prefix 并重新浏览；
- 重载桶列表或对象列表；
- read source/fallback source 发生变化；
- 列表请求失败；
- 页面重新加载；
- 操作完成或部分失败。

### 单对象管理

点击某行“管理”打开统一管理面板，自动填入该行的 bucket/key，并先显示对象状态。面板提供：

- 移动；
- 删除；
- 查看详细状态；
- 进入高级操作。

同一面板既服务单对象，也服务批量对象；不要求复制 bucket/key。现有 rename/copy 表单收进“高级对象操作”，不再作为删除/移动的主入口。

### 批量工具栏

至少选中一项后，在列表上方显示：

    已选 3 项    [移动] [删除] [清除选择]

工具栏明确写“当前已加载结果”，避免用户误认为会作用于未加载对象。

### 批量移动

移动面板只要求：

- 目标桶；
- 目标目录。

目标目录为空表示目标桶根目录。每个源 key 取最后一个路径片段作为 basename：

    source:      root/photos/2026/a.jpg
    destination: family/archive/
    result:      family/archive/a.jpg

预览逐项显示：

- source bucket/key；
- 当前列表 read source；
- 实际动作 home provider；
- destination bucket/key；
- ready/no-op/conflict 状态；
- 警告和 reason code。

如果 canonical source 与 destination 完全相同，先验证 source identity、home-provider 解析和目标桶存在，再分类为 no_op；该项不参加目标对象存在冲突判断，也不调用远端 move。一个批次可以混合 no_op 与真正移动项；全部为 no_op 时仍可生成计划，但界面提示“没有需要移动的对象”。

移动最终预检还必须对每个 action home provider 验证 destination bucket：调用 `head_container(destination_bucket)` 成功且 backend 的写入/删除能力可用。provider 不得在批量动作中隐式创建目标桶。

目标桶不存在返回 reason code `destination_bucket_missing`；无法读取、权限不足或无法确认目标桶返回 `destination_bucket_unverifiable`；能力不足返回 `provider_unsupported`。这些状态都会在第一项写入前阻止整个移动批次。

移动目标冲突包括：

- 实际动作 backend 上目标对象已存在；
- 网关已有目标 placement、logical object 或 protection plan；
- 本批次两个源对象生成同一目标 key。

发现任何上述冲突时，批次在第一项写入前停止。目标 metadata 残留不能由批量移动隐式覆盖。

### 批量删除

点击“删除”后先调用服务端预检；预检通过后才打开唯一的删除确认面板。预检冲突直接展示冲突列表，不打开可继续的确认框。

确认面板显示：

- “即将删除 N 个云盘对象”；
- 前 5 个对象的 bucket/key，剩余数量；
- “这会删除云盘对象及网关元数据，并产生相应复制状态”；
- 取消和确认删除按钮。

不要求输入确认短语。请求期间禁用所有批量操作按钮。
确认面板只确认当前预检计划；如果计划在提交时过期或发生状态变化，服务端返回冲突并要求重新预览，不自动继续。

删除预检逐项分类：

- ready：对象仍存在且身份与 selection 快照一致；
- already_missing：远端对象已经不存在，执行时只尝试清理可清理的网关 metadata；
- stale_conflict：对象仍存在但身份已经改变；
- unverifiable：无法取得足够身份信息。

任何 stale_conflict 或 unverifiable 都阻止整个删除批次。already_missing 可以进入计划，但执行结果不能伪装成发生过远端删除。删除是 best-effort 收敛，不承诺删除某个已被外部替换的历史版本。

Placement 卡片中的“删除残留 Placement”保持独立入口；它只清理网关元数据，不删除云盘文件。

## API 与数据流

### 对象列表快照

GET /api/object-browser/objects 成功响应新增：

- selection_id：不可猜测的短期服务端快照标识；
- selection_expires_at：过期时间。

selection 快照有效期固定为 5 分钟，服务端最多保留 32 个未过期快照。快照保存 bucket、prefix、read source 和当前返回对象的 canonical key 与身份快照。它只约束 Admin UI 选中的范围，不能替代服务端重新读取对象。

### Home provider 解析

对每个对象，动作 backend 按以下顺序确定：

1. 有持久化 placement 时，placement provider 是唯一权威 home provider；
2. 没有 placement 时，使用最终预检时的当前 primary provider；
3. 列表 read source/fallback source 只用于展示和 selection 绑定，不决定动作 backend；
4. 如果列表来源能读到对象，但权威 home provider 不能读到，返回 home_resolution_conflict；
5. 如果 preview 与 execute 之间 primary 发生变化，最终预检将计划视为过期并拒绝执行。

### 认证主体与操作者标签

Admin 鉴权中间件在请求 extension 中放入服务端解析的 authenticated principal：浏览器会话记录为 admin:<username>；机器凭据路径只记录 admin-api-key，不记录或回显 key 内容。该 principal 来自服务端会话或鉴权路径，不能由请求 body 覆盖。

批量请求中的 operator 字段保留为向后兼容的可选操作者备注标签，不是可信身份；服务端 trim 并限制长度后原样作为 operator_label 保存。UI 将其标为“操作者备注（可选）”。批次响应和历史同时保存 authenticated_principal 与 operator_label。现有单对象历史中的 operator 字段继续表示客户端提供的标签，单对象合同不因本功能改变。

### 批量预检

新增 POST /api/object-actions/batch/preview。

请求：

    {
      "action": "move",
      "selection_id": "selection-opaque-id",
      "objects": [
        { "bucket": "root", "key": "photos/a.jpg" },
        { "bucket": "root", "key": "photos/b.jpg" }
      ],
      "destination_bucket": "family",
      "destination_prefix": "archive/"
    }

action 只允许 delete 或 move。对象必须是 selection 对应当前结果的子集；服务端按 canonical bucket/key 去重并拒绝重复项。客户端不提交或覆盖身份字段，服务端从快照和实际 backend 重新读取身份。

预检成功返回 plan_id、plan_expires_at、规范化后的对象映射和逐项状态。计划有效期固定为 5 分钟。move 预检对每个 action home provider 调用 head_container(destination_bucket)；目标桶不存在、不可验证或 backend 不具备写入/删除能力时不生成计划。

预检失败使用 409 Conflict，表示没有任何对象动作开始；字段、重复项或数量错误使用 400 Bad Request。所有可预期错误返回：

    {
      "code": "batch_preflight_failed",
      "message": "Batch did not start.",
      "action": "move",
      "items": [
        {
          "ordinal": 0,
          "source": { "bucket": "root", "key": "photos/a.jpg" },
          "destination": { "bucket": "family", "key": "archive/a.jpg" },
          "status": "conflict",
          "reason_code": "destination_exists",
          "message": "Destination exists on the action provider."
        }
      ]
    }

移动预检中，源不存在使用 source_missing，源身份变化使用 source_changed，无法取得身份使用 source_unverifiable；这些状态都会阻止整个移动计划。删除预检允许 already_missing，但不允许 stale_conflict 或 unverifiable。

预检成功的逐项结构至少包含：

    {
      "ordinal": 0,
      "source": { "bucket": "root", "key": "photos/a.jpg" },
      "read_source": "telecom",
      "home_provider": "unicom",
      "destination": { "bucket": "family", "key": "archive/a.jpg" },
      "status": "ready",
      "warnings": []
    }

删除预检使用同一接口和同一错误合同；already_missing 是可执行计划状态，stale_conflict 和 unverifiable 会让整个计划失败。

### 批量执行

新增 POST /api/object-actions/batch。请求必须带 Idempotency-Key header，并且 body 中的 batch_id 必须与 header 完全相同：

    {
      "plan_id": "plan-opaque-id",
      "batch_id": "550e8400-e29b-41d4-a716-446655440000",
      "operator": "alice",
      "ticket": "CHG-2026-0903",
      "notes": "整理归档"
    }

客户端只生成 canonical lowercase UUID v4。服务端拒绝不匹配以下规则的 key：

    ^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$

对象集合、目标映射和 action 全部来自 plan_id，客户端不能在执行请求中改写。
服务端收到请求后先执行幂等 key 的 compare-and-reserve；reserve 或首次 ledger 持久化失败时，若尚未开始对象动作则返回 500、reason code batch_ledger_unavailable。任何对象动作开始后若中间结果无法持久化，立即停止剩余项，保留已完成项并尝试持久化 interrupted ledger；持久化成功时返回 409 和 batch_recovery_required 恢复合同，持久化失败时返回 500 和 batch_ledger_unavailable，绝不自动重放。

执行流程：

1. 以最多 5 秒的有界等待取得对象变更锁；
2. 验证 plan 未过期；
3. 在锁内执行最终预检；
4. 最终预检通过后，按 canonical source bucket/key 稳定排序，逐项调用复用后的单对象动作核心；
5. 每完成一个对象，就更新幂等 ledger 的逐项结果；
6. 完成后保存最终响应、批次历史摘要并释放锁。


最终预检失败返回 409 Conflict，不开始任何对象动作。执行过程中在下一个对象开始前发现目标出现、源身份变化或其他冲突时，停止剩余项并标记为 not_started；已完成项保留。

执行结果每项固定包含：

- ordinal；
- source；
- read_source；
- home_provider；
- 可选 destination；
- status；
- 可选 reason_code；
- 可选 message。

status 只允许：

- completed；
- already_missing；
- no_op；
- failed；
- stale_conflict；
- not_started。

响应至少包含：

    {
      "batch_id": "550e8400-e29b-41d4-a716-446655440000",
      "action": "move",
      "authenticated_principal": "admin:alice",
      "operator_label": "alice",
      "requested": 2,
      "completed": 1,
      "already_missing": 0,
      "no_op": 0,
      "failed": 1,
      "stale_conflict": 0,
      "not_started": 0,
      "non_atomic_warning": true,
      "consistency_note": "External writers are outside the gateway mutation lock.",
      "results": [
        {
          "ordinal": 0,
          "source": { "bucket": "root", "key": "photos/a.jpg" },
          "read_source": "telecom",
          "home_provider": "unicom",
          "destination": { "bucket": "family", "key": "archive/a.jpg" },
          "status": "completed"
        },
        {
          "ordinal": 1,
          "source": { "bucket": "root", "key": "photos/b.jpg" },
          "read_source": "telecom",
          "home_provider": "unicom",
          "destination": { "bucket": "family", "key": "archive/b.jpg" },
          "status": "failed",
          "reason_code": "provider_unsupported",
          "message": "Provider does not support move."
        }
      ]
    }

计数不变量为：

    requested == completed + already_missing + no_op + failed + stale_conflict + not_started

跨对象批量不声明事务性，不自动撤销已经成功的对象。预检失败不产生动作历史；already_missing、no_op 和 not_started 不伪造远端成功。

### 幂等 ledger

幂等 ledger 持久化到 control-plane state，使用向后兼容的默认字段，不保存凭据。初始限制：

- 最多 64 个未过期批次记录；
- 所有状态记录按最后更新时间保留 24 小时；
- 状态为 in_progress、completed、interrupted；interrupted 是批次级状态，不是普通逐项 status；
- compare-and-reserve 在 control-plane mutex 内完成，并通过现有原子 control-plane 文件写入持久化。

同一 key 的行为：

- completed 且计划/审计指纹相同：返回原最终响应；
- in_progress：返回 409、reason code batch_in_progress 和 Retry-After: 5，不并发执行；
- 指纹不同：返回 409、reason code idempotency_key_reused；
- 服务重启后发现 in_progress：转为 interrupted。恢复响应使用独立的批次级合同，不使用普通执行计数不变量：

    {
      "batch_id": "550e8400-e29b-41d4-a716-446655440000",
      "action": "move",
      "state": "interrupted",
      "code": "batch_recovery_required",
      "requested": 2,
      "saved_count": 1,
      "unresolved_count": 1,
      "saved_results": [
        {
          "ordinal": 0,
          "source": { "bucket": "root", "key": "photos/a.jpg" },
          "status": "completed"
        }
      ],
      "unresolved_items": [
        {
          "ordinal": 1,
          "source": { "bucket": "root", "key": "photos/b.jpg" },
          "destination": { "bucket": "family", "key": "archive/b.jpg" },
          "reason_code": "recovery_required",
          "message": "Provider result was not durably recorded before restart."
        }
      ],
      "non_atomic_warning": true,
      "consistency_note": "The gateway cannot determine whether the in-flight provider call committed before restart."
    }

其中 saved_results 只包含已经持久化的终态项；当前动作及其后的项进入 unresolved_items，不能被标为 completed 或 failed。interrupted 是批次级 ledger 状态，不是普通逐项 status。恢复合同的不变量为 requested = saved_count + unresolved_count = saved_results.length + unresolved_items.length；unresolved_items 只包含 ordinal、source、可选 destination、reason_code=recovery_required 和 message，不伪造远端结果。原 Idempotency-Key 在该记录保留期间始终返回同一恢复合同，不允许重放或改写；操作者必须重新检查对象并创建新的 selection、plan 和 UUID 才能继续未完成工作。

所有 ledger 记录按最后更新时间保留 24 小时后自动清理，包括 interrupted 记录；在保留期内原 key 不可复用。清理只删除 ledger 记录，不对云盘对象执行动作。此功能不提供人工清除接口；如果操作者已完成外部核查，只需重新浏览并创建新的 selection、plan 和 UUID。

### 批次历史

每个批量执行请求写入一条摘要历史；批量预检失败不写历史，不写入 100 条互相争抢全局上限的单项记录。摘要包含：

- batch_id；
- action 和批次标记；
- requested、各状态计数；
- authenticated_principal；
- operator_label（客户端 operator 字段的非可信备注值）；
- ticket、notes；
- non_atomic_warning 和 consistency_note；
- 最多 100 项的完整结果，包括 source、destination、read_source、home_provider、status、reason_code 和 message；这些结果直接嵌入摘要，不是指向另一个未定义的存储。

摘要的结构等价于：

    {
      "batch_id": "550e8400-e29b-41d4-a716-446655440000",
      "action": "move",
      "batch": true,
      "authenticated_principal": "admin:alice",
      "operator_label": "alice",
      "requested": 2,
      "completed": 1,
      "already_missing": 0,
      "no_op": 0,
      "failed": 1,
      "stale_conflict": 0,
      "not_started": 0,
      "non_atomic_warning": true,
      "consistency_note": "External writers are outside the gateway mutation lock.",
      "items": [
        {
          "ordinal": 0,
          "source": { "bucket": "root", "key": "photos/a.jpg" },
          "destination": { "bucket": "family", "key": "archive/a.jpg" },
          "status": "completed"
        },
        {
          "ordinal": 1,
          "source": { "bucket": "root", "key": "photos/b.jpg" },
          "destination": { "bucket": "family", "key": "archive/b.jpg" },
          "status": "failed",
          "reason_code": "provider_unsupported",
          "message": "Provider does not support move."
        }
      ]
    }

现有 object_action_history_limit 只限制批次摘要数量。单对象 API 的历史语义保持不变。批次 outcome 只有在 failed、stale_conflict、not_started 都为零时才为 success，否则为 failed，以兼容现有监控汇总。预检失败不写入批次历史；ledger 中断状态只通过恢复合同保留，待人工重新检查后再建立新的批次历史。

## 路径与身份规范

### 路径

- bucket 必须是当前列表返回的精确容器名；去除首尾空白后不能为空，不能包含 /、\ 或控制字符；
- object key 必须是相对、非空、以 / 分隔的 key；拒绝首尾 /、连续 //、\、控制字符以及 . 和 .. 路径段；
- destination prefix 为空表示目标桶根目录；非空时拒绝首个 /、\、连续 //、控制字符和 .、.. 路径段，最后 canonicalize 为一个尾随 /；
- source basename 必须非空，生成 destination 后再次执行全部 key 校验；
- 网关不做大小写折叠或 Unicode 规范化，规范化后的 UTF-8 字符串按精确匹配比较。

### 身份

对象 identity 使用 ObjectInfo 的 etag、size、last_modified：

- 空或全空白的 optional text 视为 absent；
- 非空 etag 保留原始字符串（包括引号），按精确字符串比较，并与 size 一起形成 fingerprint；
- 没有 etag 时，必须有 size 和非空 last_modified，last_modified 保留原始字符串并按精确字符串比较；
- 不解析日期、不移除 ETag 引号、不做 provider 间格式转换；
- 无法取得上述最低身份信息时标为 unverifiable，不得进入破坏性批量执行；
- 没有 etag 时的 size+last_modified 仍是 best-effort 身份，不宣称跨 provider 的强一致版本检查。

## 错误与安全处理

- 客户端和服务端都验证空选择、重复对象、路径、selection/plan 有效期和 100 项上限；
- selection 过期或提交对象不属于 selection 时返回 selection_expired 或 selection_mismatch；
- 移动任一冲突、删除任一 stale/unverifiable 在最终预检阶段发现时零写入；
- 删除远端 NotFound 沿用现有成功收敛语义，结果明确标为 already_missing；
- 执行期间不自动重试、不自动回滚已完成项；
- 完成或部分失败后刷新对象列表、状态和共享历史，并清空选择；
- 沿用现有 Admin 鉴权和错误脱敏边界，不在前端输出凭据或敏感响应体。

稳定 reason code 至少包括：

selection_expired、selection_mismatch、plan_expired、invalid_path、duplicate_object、duplicate_destination、source_missing、source_changed、source_unverifiable、home_resolution_conflict、destination_exists、destination_metadata_exists、destination_bucket_missing、destination_bucket_unverifiable、provider_unsupported、batch_busy、batch_ledger_full、batch_ledger_unavailable、batch_in_progress、idempotency_key_reused、batch_recovery_required、recovery_required。

## 实现边界

预期修改：

- crates/gatewayd/assets/admin/index.html
  - 列表选择状态；
  - batch toolbar；
  - 单对象管理面板；
  - 移动预览、删除确认、冲突展示；
  - 请求禁用、幂等 key、逐项结果和刷新；
  - advanced object action 区域。
- crates/gatewayd/src/main.rs
  - selection snapshot 和 plan cache；
  - batch DTO、预检和执行路由；
  - object mutation lock；
  - 结构化单对象动作核心；
  - durable idempotency ledger；
  - batch history payload；
  - 路由、竞态和错误测试。
- docs/object-actions-api-reference.md
  - batch preview/execute、状态和 reason code。
- docs/object-actions-and-history.md
  - 批量删除/移动、冲突、best-effort 和部分失败说明。

不修改 provider crate 或 BlobBackend 协议，不改变现有单对象 API 请求和响应合同。新增 control-plane 字段必须使用 serde 默认值兼容旧文件。

## 测试策略

### Rust 行为测试

至少覆盖：

1. 多对象删除正确更新远端对象、metadata、复制任务和批次摘要；
2. 批量移动按目标目录和 basename 生成目标 key；
3. 任一远端目标、目标 metadata 或重复目标 key 冲突时零写入；
4. no-op 优先级正确，可与真实移动项混合；
5. selection/plan 过期、primary 改变、home provider 不可读和 source identity 改变时拒绝执行；
6. stale-placement 删除与 reconcile 执行路径和批量动作共享同一变更锁；
7. 执行期间单项失败时计数、逐项错误和 not_started 正确；
8. 删除已不存在对象返回 already_missing 并清理可清理 metadata；
9. 幂等 key 的 compare-and-reserve、重复完成、处理中、复用冲突和重启恢复行为；
10. 路径规则拒绝尾随 slash、//、.、..、反斜杠和空 basename；
11. 100 项上限、空字段、非法 UUID 和未认证请求被拒绝。

### Admin 行为测试

测试真实 state/render 行为，而不是只检查字符串：

- 多选、当前结果全选和清除选择；
- 切换 bucket/prefix、刷新或来源变化后选择清空；
- 行管理按钮预填对象；
- 移动预览显示 read source、home provider、目标 basename 和冲突状态；
- no-op 与真实移动混合预览；
- 删除预检成功后才出现唯一确认框；
- 409 预检不会发起执行请求；
- 请求期间按钮禁用并复用同一 UUID 幂等 key；
- 部分失败结果逐项可见，完成后刷新列表。

如果仓库当前没有浏览器测试依赖，增加只使用 Node 标准库的 DOM/state harness，测试选择生命周期、预览状态和结果渲染；现有 Rust HTML contract test 继续用于确认生产 HTML 包含入口和 API wiring，但不能单独作为交互测试。

## 验收标准

用户能够在同一个对象列表中：

1. 选择一个或多个当前已加载对象；
2. 一键打开移动或删除；
3. 移动时只填写目标桶和目标目录，并保留每个文件名；
4. 看到列表来源与实际动作归属的区别；
5. 在任意可观察冲突时看到清晰原因且没有任何批次写入；
6. 删除前看到明确对象摘要和“删除云盘对象及网关元数据”的语义；
7. 操作后看到每个对象的最终状态；
8. 不需要手工复制 bucket/key，也不会把 Placement 元数据清理误认为云盘删除；
9. 在普通网关内并发 Admin 操作下不会让批量预检和受保护的元数据动作交叉；外部竞态以明确警告呈现。

## 方案取舍

采用增强现有对象列表的方案，并使用网关内锁、短期 selection/plan、最终预检、durable idempotency ledger 和批次摘要历史来控制风险。

不采用独立对象管理页面，因为它会重复列表、筛选和状态信息；不采用仅每行菜单，因为它不能为批量选择、目标路径和整体结果提供清晰承载；不采用 provider 条件移动协议，因为用户已选择实用方案，当前 provider 抽象也没有这种原子能力。

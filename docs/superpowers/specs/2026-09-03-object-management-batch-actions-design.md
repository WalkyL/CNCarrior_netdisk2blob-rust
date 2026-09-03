# 对象管理与批量删除/移动设计

日期：2026-09-04

状态：已按 Sol 最终复审意见修订，待用户审阅

## 背景

Admin Web 的“对象与文件操作”页已经能从当前读来源浏览对象，也已经有网关托管的单对象删除、重命名、复制和移动能力。但当前列表行操作主要是“检查”，用户需要把已经看到的 bucket/key 再次填入下方表单，才能执行动作。

用户真正要完成的是：找到文件、选择文件、移动或删除、确认结果。本设计把当前对象列表变成管理入口，首版只优化批量删除和批量移动两个高频任务。

## 设计原则

### 第一性原理

普通操作者不需要先理解 provider、placement、复制拓扑或 WAL。界面围绕“找文件、选文件、整理、确认结果”组织。

### 易用性

- 从当前列表直接选择，不重复输入已经显示的 bucket/key；
- 单对象和批量对象使用同一套管理交互；
- 批量移动只填写目标桶和目标目录，自动保留文件名；
- 删除前显示数量、对象摘要和真实影响；
- 执行后按对象显示结果，不把部分失败伪装成整体成功。

### 简约性

- 未选中对象时不显示批量工具栏；
- 真实云盘对象删除与 Placement 元数据清理使用不同入口和文案；
- 删除、移动是主路径，rename/copy 收进高级操作；
- 不新增 provider 选择器、重复对象输入表单或多余确认层。

## 方案与范围

采用增强现有对象列表的方案，复用当前浏览、对象状态、网关动作和共享历史。

本次包含：

- 当前结果列表的多选、当前结果全选和清除选择；
- 单对象“管理”入口；
- 批量删除；
- 批量移动到目标桶和目录，并保留 basename；
- 服务端 selection/plan 预检、网关内变更锁、幂等提交、逐项结果和批次历史摘要；
- Admin API、Rust 行为测试、DOM/state 测试和运维文档。

本次不包含：

- 递归文件夹操作、服务端全量搜索或未加载对象的隐式全选；
- 回收站、撤销删除或用户指定底层 provider；
- 批量 copy、批量 rename；
- provider crate 或 BlobBackend 条件移动协议改造；
- .43/.49 部署、真实凭据注入和正式发布。

## 并发与一致性边界

本版采用实用方案，不向当前 provider 抽象要求条件移动或 CAS 能力。

### 网关内保护

增加一把网关内对象变更锁。以下路径必须使用它：

- POST /api/object-actions；
- POST /api/object-actions/batch；
- POST /api/object-reconcile/execute 的实际执行分支；
- POST /api/object-placement/delete-stale；
- POST /api/object-placement/delete-stale-bulk；
- POST /api/control-plane/topology。

批量执行从最终预检开始一直持有锁，直到本批次结束。预览、状态查询和历史导出不持有该锁。数据面 S3 PUT/DELETE、后台 replication worker 和绕过网关的 provider 客户端不在锁内。

### 产品保证

- 最终网关预检观察到冲突时，在第一项写入前阻止整个批次；
- 受同一把锁保护的其他 Admin 对象元数据动作不会插入最终预检与执行之间；
- 外部 S3/provider 写入或其他网关实例造成的竞态不属于本版的分布式原子保证；
- 如果下一个对象开始前发现外部变化，停止剩余未开始项并返回逐项状态；已经完成的项不自动撤销；
- 预览、执行结果和批次历史带有 non_atomic_warning 与 consistency_note。

锁最多等待 5 秒。超时返回 409、reason code batch_busy 和 Retry-After: 5，不开始对象动作。

## UI 设计

### 对象列表与选择状态

“浏览桶和对象 Key（当前读来源）”继续作为主列表。每行显示选择框、object key、大小、修改时间和“管理”按钮。

前端保存 selection_id、bucket、prefix、read source/fallback source、加载时间，以及已加载对象的 canonical bucket/key 和身份快照（etag、size、last_modified）。selection_id 是服务端短期快照标识，不是授权机制。当前结果全选只选择已经渲染的行，不代表服务端未加载的对象。单次最多选择 100 项。

selection 快照有效期固定为 5 分钟，服务端最多保留 32 个未过期快照。

以下任一事件发生时，清空选择、丢弃 selection_id、关闭批量面板并清除预览：

- 切换 bucket；
- 修改 prefix 并重新浏览；
- 重载桶列表或对象列表；
- read source/fallback source 发生变化；
- 列表请求失败；
- 页面重新加载；
- 操作完成或部分失败。

### 单对象管理

点击某行“管理”打开统一管理面板，自动载入该对象状态，并提供移动、删除、详细状态和高级操作。单对象和批量对象使用同一套面板，不要求复制 bucket/key。

### 批量工具栏

至少选中一项时，在列表上方显示：

    已选 3 项    [移动] [删除] [清除选择]

工具栏明确写“当前已加载结果”。

### 批量移动

移动面板只要求目标桶和目标目录。目标目录为空表示目标桶根目录。每个 source key 取最后一个路径片段作为 basename：

    source:      root/photos/2026/a.jpg
    destination: family/archive/
    result:      family/archive/a.jpg

预览逐项显示 source、read source、home provider、destination、状态、警告和 reason code。

如果 canonical source bucket/key 与生成的 destination bucket/key 完全相同，先验证 source identity、home provider 解析和目标桶存在，再分类为 no_op；该项不参加目标存在冲突判断，也不调用远端 move。批次可以混合 no_op 和真实移动项；全部为 no_op 时仍可生成计划，但界面提示没有需要移动的对象。

移动目标冲突包括实际动作 backend 上目标对象已存在、网关已有目标 placement/logical object/protection plan，以及本批次两个源对象生成同一目标 key。目标 metadata 残留不能被隐式覆盖。

最终预检还必须对每个 action home provider 验证 destination bucket：head_container 成功且 backend 具备写入和删除能力。目标桶不存在、不可验证或能力不足时不生成计划。

### 批量删除

点击删除后先调用服务端预检；预检通过后才打开唯一的删除确认面板。预检冲突直接展示冲突列表，不打开可继续的确认框。

确认面板显示删除数量、前五个对象的 bucket/key、剩余数量，以及“这会删除云盘对象及网关元数据，并产生相应复制状态”。不要求输入确认短语，请求期间禁用所有批量操作按钮。确认面板只确认当前预检计划；如果计划过期或状态变化，服务端返回冲突并要求重新预览。

删除预检逐项分类为 ready、already_missing、stale_conflict、unverifiable：

- ready：对象仍存在且身份与 selection 快照一致；
- already_missing：远端对象已经不存在，执行时只尝试清理可清理的网关 metadata；
- stale_conflict：对象仍存在但身份已经改变；
- unverifiable：无法取得足够身份信息。

任何 stale_conflict 或 unverifiable 都阻止整个删除批次。already_missing 可以进入计划，但结果不能伪装成发生过远端删除。删除是 best-effort 收敛，不承诺删除某个已被外部替换的历史版本。

Placement 卡片中的删除残留 Placement 入口保持独立；它只清理网关元数据，不删除云盘文件。

## API 与数据流

### 对象列表快照

GET /api/object-browser/objects 成功响应新增 selection_id 和 selection_expires_at。服务端在有界缓存中保存 bucket、prefix、read source、当前返回对象的 canonical key 和身份快照。selection 只约束 Admin UI 选中的范围，不能替代服务端重新读取对象。

### Home provider 解析

每个对象的 action backend 按以下顺序确定：

1. 有持久化 placement 时，placement provider 是唯一权威 home provider；
2. 没有 placement 时，使用最终预检时的当前 primary provider；
3. 列表 read source/fallback source 只用于展示和 selection 绑定，不决定 action backend；
4. 如果列表来源能读到对象，但权威 home provider 不能读到，返回 home_resolution_conflict；
5. 如果 preview 与 execute 之间 primary 发生变化，最终预检将计划视为过期并拒绝执行。preview 将 topology_fingerprint 写入 plan；拓扑更新也取得同一把对象变更锁并更新 fingerprint，最终预检在锁内比较两者。

### 认证主体与操作者标签

Admin 鉴权中间件为请求提供服务端解析的 authenticated_principal：浏览器会话记录为 admin:<username>，机器凭据路径只记录 admin-api-key，不记录或回显 key 内容。该值不能由 body 覆盖。

批量请求中的 operator 字段是向后兼容的可选操作者备注标签，不是可信身份。服务端 trim 并限制长度后保存为 operator_label。UI 将它标为操作者备注（可选）。批次响应和历史同时保存 authenticated_principal 与 operator_label。

### 批量预检

新增 POST /api/object-actions/batch/preview。action 只允许 delete 或 move。请求包含 selection_id 和 objects；move 另需 destination_bucket 与 destination_prefix。

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

objects 必须是 selection 对应当前结果的子集。服务端按 canonical bucket/key 去重并拒绝重复项；客户端不提交或覆盖身份字段，服务端从快照和实际 action backend 重新读取身份。

预检成功返回 plan_id、plan_expires_at、topology_fingerprint、规范化后的对象映射和逐项状态。计划有效期固定为 5 分钟；plan 同时保存 action、selection_id、规范化对象集合、目标映射和 topology_fingerprint。move 预检对每个 action home provider 调用 head_container(destination_bucket)；目标桶不存在、不可验证或 backend 不具备写入/删除能力时不生成计划。

预检失败使用 409 Conflict，表示没有对象动作开始；字段、重复项或数量错误使用 400 Bad Request。可预期错误至少包含 code、message、action 和 items。item 至少包含 ordinal、source、destination（如适用）、status、reason_code 和 message。

移动预检中，源不存在使用 source_missing，源身份变化使用 source_changed，无法取得身份使用 source_unverifiable；这些状态阻止整个移动计划。删除预检允许 already_missing，但不允许 stale_conflict 或 unverifiable。

预检成功的逐项结构至少包含 ordinal、source、read_source、home_provider、destination（如适用）、status 和 warnings。只有没有冲突的计划才返回 plan_id。

reason code destination_bucket_missing 和 destination_bucket_unverifiable 用于目标桶检查。通用 write/delete 能力缺失时预检使用 provider_unsupported；当前 BlobBackend 没有独立的 move capability 位，因此 provider 的 move_object 在执行阶段返回 NotImplemented 时，按逐项 provider_unsupported 失败处理，不把它误报为目标冲突。selection 过期或提交对象不属于 selection 时分别使用 selection_expired 和 selection_mismatch。

### 批量执行

新增 POST /api/object-actions/batch。请求必须带 Idempotency-Key header，body 的 batch_id 必须与 header 完全相同。客户端只生成 canonical lowercase UUID v4，服务端拒绝不匹配以下规则的值：

    ^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$

    {
      "plan_id": "plan-opaque-id",
      "batch_id": "550e8400-e29b-41d4-a716-446655440000",
      "operator": "alice",
      "ticket": "CHG-2026-0903",
      "notes": "整理归档"
    }

objects、destination 映射和 action 全部来自 plan_id，客户端不能在执行请求中改写。

执行顺序：

1. 解析认证、Idempotency-Key 和请求字段；
2. 查找已有 ledger；matching completed/rejected/interrupted 直接返回对应保存结果，matching in_progress 返回 batch_in_progress；不同 fingerprint 返回 idempotency_key_reused；
   获取锁后必须再次查 ledger；如果等待期间已有请求完成 reservation，按该次读取到的状态处理，不继续使用旧的空查找结果；
3. 以最多 5 秒的有界等待取得对象变更锁；锁忙返回 batch_busy，不创建 ledger；
4. 在锁内验证 plan 未过期、topology_fingerprint 未变化并执行最终预检；plan_expired、topology_changed、冲突、目标桶错误或路径错误直接返回对应零写入响应；
5. 所有零写入检查通过后，compare-and-reserve 并持久化 in_progress ledger；
6. 在第一个 provider 变更调用（move、delete 或其他会改变远端对象的调用）即将发生前，先持久化 provider_call_started_at；head/list/health 等只读预检调用不设置该字段；成功后才允许调用 provider；
7. 按 canonical source bucket/key 稳定顺序逐项执行，完成每项后持久化 ledger 结果；
8. 保存最终响应和批次历史摘要，释放锁。

如果 reservation 后、首个 provider 变更调用前发生错误，持久化 state=rejected、终态 HTTP status 和错误 payload；rejected 响应使用统一的批次错误 envelope，包含 batch_id、state=rejected、code、message 和 items=[]，同 key 相同 fingerprint 后续请求重放完全相同的 HTTP status/body。如果 provider_call_started_at 已持久化后中间结果无法持久化，停止剩余项并尽力保存 interrupted ledger；不自动重放。

批量每一项必须调用与现有单对象 POST /api/object-actions 共用的结构化动作核心。move 的状态迁移和失败顺序必须与现有单对象 core 一致：先捕获 source/target metadata 和 protection plan，准备 destination/source WAL，暂存 destination placement，再调用 provider move；成功后删除 source placement、迁移 logical/protection metadata，并为 destination 入队 replication put、为 source 入队 replication delete。远端动作后的 metadata 或复制入队失败沿用现有 rollback_move_after_failure 语义，结果标为 failed 并保留 side-effect/rollback 说明；不得只移动远端对象而留下 source metadata。

WAL 提交完成标记沿用现有 mark_gateway_write_ahead_log_committed_or_warn 语义：提交标记失败只记录 WAL commit warning 和运行时告警，不回滚已经成功的远端 move/delete，也不把该对象改判为 failed。结构化动作核心要把 warning 回传给批量 item 的 warnings 字段和批次历史；单对象 API 的成功/告警语义保持不变。

批量 delete 同样复用单对象 delete 核心，包括 NotFound 的 already_missing 收敛、metadata 清理、复制 delete、WAL 和现有回滚报告。

最终预检失败返回 409，不开始对象动作。执行过程中在下一个对象开始前发现目标出现、源身份改变或其他冲突时，停止剩余项并标记 not_started；已完成项保留。

执行结果每项固定包含 ordinal、source、read_source、home_provider、可选 destination、status、可选 reason_code、可选 message 和 warnings。status 只允许 completed、already_missing、no_op、failed、stale_conflict、not_started。

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
          "status": "completed",
          "warnings": []
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

### 幂等 ledger

幂等 ledger 持久化到 control-plane state，使用 serde 默认值兼容旧文件，不保存凭据。每条记录包含 batch_id、plan/request fingerprint、state、provider_call_started_at、逐项结果、终态 HTTP status 和终态 payload。

初始限制：最多 64 个记录；所有状态按最后更新时间保留 24 小时；状态为 in_progress、completed、rejected、interrupted。rejected 和 interrupted 是批次级状态，不是普通逐项 status。ledger 容量不足返回 503、reason code batch_ledger_full；持久化失败返回 500、reason code batch_ledger_unavailable；两者都不开始对象动作。compare-and-reserve 在 control-plane mutex 内完成，并通过现有原子 control-plane 文件写入持久化。

provider_call_started_at 初始为空。只有在第一个 provider 变更调用即将发生前成功持久化该字段后，才允许开始远端对象动作；head/list/health 等只读预检调用不会设置该字段。

同一 key 的行为：

- completed 且 fingerprint 相同：返回原最终响应；
- rejected 且 fingerprint 相同：返回原零写入错误响应；
- in_progress：返回 409、reason code batch_in_progress 和 Retry-After: 5，不并发执行；
- interrupted：返回 409、reason code batch_recovery_required 和恢复合同；
- fingerprint 不同：返回 409、reason code idempotency_key_reused。

服务重启时，provider_call_started_at 为空的 in_progress 记录安全转为 rejected，code=batch_not_started，说明尚未开始 provider 动作；provider_call_started_at 非空的记录才转为 interrupted。

interrupted 恢复响应使用独立的批次级合同，不使用普通执行计数不变量：

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

恢复合同不变量为 requested = saved_count + unresolved_count = saved_results.length + unresolved_items.length。unresolved_items 不能被标为 completed 或 failed，只包含 ordinal、source、可选 destination、reason_code=recovery_required 和 message。原 key 在记录保留期间始终返回同一恢复合同，不允许重放或改写；操作者必须重新检查对象并创建新的 selection、plan 和 UUID。

所有 ledger 记录按最后更新时间保留 24 小时后自动清理，包括 rejected 和 interrupted；在保留期内原 key 不可复用。清理只删除 ledger 记录，不对云盘对象执行动作。

### 批次历史

每个批量执行请求写入一条摘要历史；批量预检失败不写历史。摘要包含 batch_id、action、batch=true、authenticated_principal、operator_label、requested 和各状态计数、ticket、notes、non_atomic_warning、consistency_note，以及最多 100 项直接嵌入的完整结果和 warnings。

批次 outcome 只有在 failed、stale_conflict、not_started 都为零时才为 success，否则为 failed。预检失败不产生批次历史；ledger 中断状态只通过恢复合同保留。旧 control-plane 历史记录读取时，新增 authenticated_principal、operator_label、batch 和 items 字段使用默认值：authenticated_principal 为空、operator_label 取旧 operator、batch=false；旧 references 继续按原单对象语义展示。

## 路径与身份规范

### 路径

- bucket 必须是当前列表返回的精确容器名；去除首尾空白后不能为空，不能包含 /、反斜杠或控制字符；
- object key 必须是相对、非空、以 / 分隔的 key；拒绝首尾 /、连续 //、反斜杠、控制字符以及 . 和 .. 路径段；
- destination prefix 为空表示目标桶根目录；非空时拒绝首个 /、反斜杠、连续 //、控制字符和 .、.. 路径段，最后 canonicalize 为一个尾随 /；
- source basename 必须非空，生成 destination 后再次执行全部 key 校验；
- 网关不做大小写折叠或 Unicode 规范化，canonical UTF-8 字符串按精确匹配比较。

### 身份

对象 identity 使用 ObjectInfo 的 etag、size、last_modified：

- 空或全空白的 optional text 视为 absent；
- 非空 etag 保留原始字符串（包括引号），按精确字符串比较，并与 size 一起形成 fingerprint；
- 没有 etag 时，必须有 size 和非空 last_modified，last_modified 保留原始字符串并按精确字符串比较；
- 不解析日期、不移除 ETag 引号、不做 provider 间格式转换；
- 无法取得最低身份信息时标为 unverifiable，不得进入破坏性批量执行；
- 没有 etag 时的 size+last_modified 仍是 best-effort 身份，不宣称跨 provider 的强一致版本检查。

## 错误与安全处理

客户端和服务端都验证空选择、重复对象、路径、目标桶、selection/plan 有效期和 100 项上限。预检失败、锁忙、ledger 错误和恢复错误都返回稳定的 machine-readable code、message 和适用的 item/批次信息。

v1 reason code 是封闭枚举：selection_expired、selection_mismatch、plan_expired、topology_changed、invalid_path、duplicate_object、duplicate_destination、source_missing、source_changed、source_unverifiable、home_resolution_conflict、destination_exists、destination_metadata_exists、destination_bucket_missing、destination_bucket_unverifiable、provider_unsupported、batch_busy、batch_ledger_full、batch_ledger_unavailable、batch_in_progress、idempotency_key_reused、batch_not_started、batch_recovery_required、recovery_required。未来新增 code 必须通过 Admin API 版本变更；旧 UI 将未知 code 显示为通用错误并阻止继续。

## 实现边界

预期修改：

- crates/gatewayd/assets/admin/index.html：选择状态、批量工具栏、单对象管理面板、移动预览、删除确认、冲突展示、按钮禁用、幂等 key、逐项结果和刷新；
- crates/gatewayd/src/main.rs：selection snapshot、plan cache、batch DTO、预检与执行路由、对象变更锁、结构化单对象动作核心、持久化幂等 ledger、批次历史 payload 和测试；
- docs/object-actions-api-reference.md：批量预检/执行、状态和 reason code；
- docs/object-actions-and-history.md：批量删除/移动、冲突、best-effort、部分失败和恢复说明。

不修改 provider crate 或 BlobBackend 协议，不改变现有单对象 API 请求和响应合同。新增 control-plane 字段使用 serde 默认值兼容旧文件。新增批量路由必须同步登记到 crates/admin-api/src/lib.rs 的 Admin route contract 和 DTO kind。

## 测试策略

### Rust 行为测试

至少覆盖：

1. 多对象删除正确更新远端对象、metadata、复制任务和批次摘要；
2. 批量移动按目标目录和 basename 生成目标 key；
3. 任一远端目标、目标 metadata、目标桶或重复目标 key 冲突时零写入；
4. no-op 优先级正确，可与真实移动项混合；
5. selection/plan 过期、primary 改变、home provider 不可读和 source identity 改变时拒绝执行；
6. stale-placement 删除、reconcile 执行、topology 更新和单对象动作共享同一变更锁，并验证 topology_fingerprint；
7. 执行期间单项失败时计数、逐项错误和 not_started 正确；
8. 删除已不存在对象返回 already_missing 并清理可清理 metadata；
9. ledger compare-and-reserve、重复完成、rejected、处理中、复用冲突和重启恢复行为；
10. provider_call_started_at 不被只读预检设置，且 marker 写入失败时不调用 provider；
11. move 成功后的 destination/source metadata、WAL 准备和 replication 状态与单对象核心一致，预提交失败时保留现有 rollback 语义；
12. WAL commit finalization warning 不回滚已成功对象，并在批量结果和历史中可见；
13. 路径规则拒绝尾随 slash、//、.、..、反斜杠和空 basename；
14. 100 项上限、空字段、非法 UUID、未认证请求和响应计数不变量被验证。

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
- 部分失败结果逐项可见，完成后刷新列表；
- interrupted 恢复结果明确显示不可自动重放。

如果仓库当前没有浏览器测试依赖，增加只使用 Node 标准库的 DOM/state harness，测试选择生命周期、预览状态和结果渲染；现有 Rust HTML contract test 继续确认生产 HTML 包含入口和 API wiring，但不能单独作为交互测试。

## 验收标准

用户能够在同一个对象列表中：

1. 选择一个或多个当前已加载对象；
2. 一键打开移动或删除；
3. 移动时只填写目标桶和目标目录，并保留每个文件名；
4. 看到列表来源与实际动作归属的区别；
5. 在任意网关可观察冲突时看到清晰原因且没有批次写入；
6. 删除前看到明确对象摘要和真实删除语义；
7. 操作后看到每个对象的最终状态；
8. 不需要手工复制 bucket/key，也不会把 Placement 清理误认为云盘删除；
9. 普通网关内并发 Admin 操作不会让受保护的批量预检和对象元数据动作交叉；外部竞态以明确警告呈现。

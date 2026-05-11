# 浏览器流程配置

## 为什么要有这层

运营商云盘网页会持续改版，但大多数改动并不值得直接改 Rust provider 逻辑。

这层配置的目标是把下面这些“易变信息”抽出来:

- 页面 URL 和页面角色
- 关键 DOM 元素的 selector
- 需要点击、填值、发事件、注入文件的步骤
- 页面自带 Vue/JS 入口点
- 应该观察到的关键网络请求和响应字段

这样做的结果是:

- 页面小改版时，优先改 `config/browser-flows/*.json`
- provider crate 继续只负责稳定的对象存储语义和上游协议
- 后续 `auth-capture` sidecar / CDP 执行层可以共享同一套流程描述

## 当前结构

当前仓库提供了一个最小可校验模型:

- Rust 类型: [crates/blob-core/src/browser_flow.rs](/home/walky/workspaces/carrier-cloud-blob-gateway/crates/blob-core/src/browser_flow.rs:1)
- 联通样例: [config/browser-flows/unicom-web.json](/home/walky/workspaces/carrier-cloud-blob-gateway/config/browser-flows/unicom-web.json:1)

并且现在已经有了最小的“目录级 loader + 查询接口”基础:

- `blob-core::BrowserFlowCatalogCollection` 可以从 `config/browser-flows/` 目录批量加载 catalog，并按 `provider/surface` 查找
- `blob-core::BrowserFlowCatalog::bind_flow(...)` / `BrowserFlowCatalogCollection::bind_flow(...)` 可以把 `flows[].inputs`、`runtime.*` 和 `preset_refs` 绑定成一份已解析模板的执行计划
- `blob-core::DryRunBrowserFlowExecutor` 可以把绑定后的执行计划展开成逐步的 `Planned` 报告，方便在接入真实 CDP 执行层之前先验证 catalog、输入绑定和预期请求
- `blob-core::BrowserFlowSession` / `BrowserFlowSessionExecutor` 已经定义了真实执行时需要的通用浏览器动作边界，例如 `navigate`、`click`、`set_input`、`set_files`、`wait_for_request`、`wait_for_page`
- `gatewayd` 的 auth-capture 控制面现在已经能保存 CDP 配置字段，例如 `cdp_endpoint_url`、`cdp_target_selector`、`cdp_target_timeout_ms`，后续真实 transport 将优先消费这些字段，而不是写死某个浏览器
- `gatewayd` 只读暴露:
  - `GET /api/browser-flows/catalogs`
  - `GET /api/browser-flows/catalog?provider=unicom&surface=pan.wo.cn-web`
  - `GET /api/browser-flows/flow/{flow_id}`
- `gatewayd` 现在还暴露一个服务层 dry-run 入口:
  - `POST /api/browser-flows/dry-run`
  - body 里显式传 `provider`、`surface`、`flow_id`、`inputs`、`runtime`
  - 返回值只包含执行报告，不回显绑定后的 secret context
- `gatewayd` 现在也暴露一个最小真实 session 执行入口:
  - `POST /api/browser-flows/session-run`
  - `GET /api/browser-flows/session/{session_id}`
  - body 里显式传 `provider`、`surface`、`flow_id`、`inputs`、`runtime`
  - 可选传 `auth_session_id`，把多次请求收口到同一条人工认证会话
  - 可选覆盖 `cdp_endpoint_url`、`cdp_target_selector`、`cdp_target_timeout_ms`
  - 如果请求里不传，会回退到 control-plane 保存的 auth-capture CDP 配置
  - 底层 transport 统一走 `browser-cdp` crate，不绑定某个具体浏览器品牌
  - 如果缺少必填 `inputs.*`，当前会返回 `status=awaiting_input` 并自动创建对应的 auth-capture prompts；待提示项回答后，可带同一个 `auth_session_id` 重试
  - `GET /api/browser-flows/session/{session_id}` 可轮询当前会话的 `pending/awaiting_input/answered/resumed/completed/failed` 状态、关联 prompts、最近 report 和 last_error
  - 当 flow 成功完成后，`gatewayd` 当前会把 `flows[].outputs` 里的 `script_value` / `url` 输出写回同一条 auth session 的 `runtime`，供后续复用同一个 `auth_session_id` 的 flow 继续绑定 `{{runtime.*}}`
  - 当 flow 声明了 `prerequisite_flow_id`，`gatewayd` 会在同一条 `auth_session_id` 和同一个 CDP page session 内先执行 prerequisite flow，再执行主 flow
  - 当前这一步还没有实现完整的 `response_field` / `request_field` 抓取；先覆盖登录后页面内可直接读取的 token / URL / store state

当前 schema version 为 `1`。

## 配置文件表达什么

一份浏览器流程配置由这些部分组成:

- `pages`: 页面身份和 URL pattern
- `elements`: 关键元素和 selector 候选
- `requests`: 关键请求的 URL、header、字段和成功码
- `operations`: 页面内 JS/Vue 入口点
- `flows`: 组合后的业务流程

`flows` 当前支持的步骤类型包括:

- `navigate`
- `click`
- `set_input`
- `invoke_operation`
- `set_files`
- `dispatch_events`
- `wait_for_request`
- `wait_for_page`
- `wait`

## 联通样例现在覆盖了什么

当前联通样例覆盖八条主路径，其中 personal root 上传已按当前执行器能力拆成“准备 uploader 上下文”和“真正附加文件上传”两段:

1. `unicom_sms_login`
2. `unicom_prepare_personal_root_upload`
3. `unicom_personal_root_upload`
4. `unicom_create_directory`
5. `unicom_delete_entry`
6. `unicom_rename_entry`
7. `unicom_copy_entry`
8. `unicom_move_entry`

其中上传流程记录了一个关键约束:

- 必须先调用页面动作组件的 `goUpload(false)`
- 再向 `#global-uploader-btn input[type=file]` 注入文件
- 然后等待真实 `upload2C` 请求

这是为了复用网页自身已经准备好的:

- `params.fileInfo`
- 页面内已初始化的 `directoryId`
- 页面内已初始化的 `spaceType`
- 加密后的 `fileInfo`
- uploader 内部续传/分片逻辑

当前 checked-in 的上传 flow 只承诺 personal root 路径。

- family/private upload context 还没有拆成独立 flow
- 在它们的 uploader 上下文、额外 token 和页面切换动作完成实测前，不再通过一个泛化 upload flow 预先要求额外 runtime
- `unicom_prepare_personal_root_upload` 负责调用页面的 `goUpload(false)` 并把 `batch_no`、`directory_id`、`personal_space_type` 写回同一条 auth session runtime
- `unicom_personal_root_upload` 负责在这条已准备好的 uploader 上下文里附加本地文件并等待真实上传请求

目录创建、删除、重命名、复制、移动这些写路径当前也已经有了实测事实:

- `createDir()` 会组装 `spaceType`、`parentDirectoryId`、`directoryName`
- `action-dialog.deleteFile()` 会组装 `spaceType`、`vipLevel`、`dirList`、`fileList`
- `renameFileOrDirectory()` 会组装 `spaceType`、`type`、`fileType`、`id`、`name`
- `action-dialog.onSubmit()` 在 `copy` 分支会组装 `targetDirId`、`sourceType`、`targetType`、`dirList`、`fileList`
- `action-dialog.onSubmit()` 在 `move` 分支会组装 `targetDirId`、`sourceType`、`targetType`、`dirList`、`fileList`，家庭空间时还会带 `fromFamilyId`
- 两者最终都走 `/wohome/dispatcher`

这里还有两个值得固化到配置层的页面约束:

- `copy/move` 不能只伪造 `dirIds/fileIds`，父组件需要先经过 `handleSelectionChange()`，再让 `handleCopy()` / `handleMove()` 从 `multipleSelection` 派生 `dialogData`
- `move` 提交前页面会先跑一次 `QueryDirectorys` 预查询，因此配置里的 JS operation 保留了一个短暂等待来匹配真实页面节奏

## 维护规则

更新这类配置时，遵守下面几条:

1. 不要提交真实 token、cookie、手机号、短信码。
2. 只保存符号占位符，例如 `{{inputs.sms_code}}`、`{{runtime.access_token}}`。
3. selector 优先写多候选，而不是只赌一个易碎 class name。
4. 关键网络请求要把“真正决定成功”的字段写清楚。
5. 如果页面流程必须先调用某个 JS/Vue 方法，不要把它降级成纯 selector 点击说明。
6. 对登录态、client id、family id 这类可从页面上下文直接读取的值，优先记录成 `outputs[].kind=script_value`，避免在 provider 代码里二次猜测。

## 与 provider 代码的边界

这层不是 `provider-unicom` 的替代品。

更准确地说:

- `browser-flow` 负责“怎么在浏览器里完成流程”
- `provider-unicom` 负责“怎么在服务里表达联通对象读写能力”

当后续联通写路径真正落进 Rust provider 时，优先复用这层产出的稳定事实:

- 上传前必须准备的上下文
- `upload2C` 的关键字段
- `CreateDirectory` / `DeleteFile` / `RenameFileOrDirectory` / `CopyFile` / `MoveFile` 这类动作的请求形状

## 下一步建议

这份最小结构已经能支撑继续扩展，下一批优先项应该是:

1. 给真正的 CDP / `auth-capture` 执行层接入这套 catalog loader 和 `bind_flow(...)` 解析结果，而不是再手写 provider-specific 文件路径和模板替换。
2. 让 `auth-capture` sidecar 把待输入手机号、短信码、验证码统一映射到 `flows[].inputs`。
3. 继续补联通 family/private space 变体，以及后续电信/移动 provider 的网页 flow catalog。

目前仓库已经有最小可用的真实 CDP transport 和 session 执行链，但还没有完整的 auth-capture 编排层。

换句话说:

- 现在已经可以校验 catalog、加载 catalog、查询 flow、绑定模板、输出 dry-run step report
- 现在也可以通过 `gatewayd` 直接请求一份 flow 的 dry-run execution report，拿来验证输入是否齐全、预期请求是否匹配
- 现在已经有了可 mock 的 session executor，可以真正逐步执行这些步骤
- 现在已经可以把这个 session executor 接到任意 CDP 兼容浏览器会话上
- 但还没有把“短信码输入、验证码等待、人工确认、失败恢复”这类 auth-capture 编排完整落成独立 sidecar

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

- 页面与控制面之间的 provider-specific 绑定，优先改 `config/provider-bridges/*.json`
- 页面小改版时，优先改 `config/browser-flows/*.json`
- provider crate 继续只负责稳定的对象存储语义和上游协议
- 后续 `auth-capture` sidecar / 真实 CDP 执行层可以共享同一套流程描述；这仍然只是可插拔 capture 层，不是 core 的长期常驻依赖

## 这层明确不负责什么

这里需要刻意收紧边界，避免后续实现走偏:

- `browser-flow` / CDP 负责抓取事实，不负责承载正式对象能力。
- 它适合做:
  - 登录态抓取
  - 页面结构校验
  - 真实请求/字段/响应契约采样
  - provider 尚未 native 化前的人工探针
- 它不适合做:
  - 依赖某个已打开网页长期存活的正式上传/下载实现
  - 把真实业务写入长期绑在某个浏览器 tab 上
  - 以“浏览器里能点通”为由跳过 provider crate 内的稳定实现

必须遵守的原则:

1. 如果浏览器页被关闭、刷新、跳转后能力就消失，这条能力仍然只能算 `probe/discovery`，不能算 provider 已完成。
2. `set_files` 这类步骤只用于探测页面上传契约、验证 selector、观察真实请求，不应被当成长期生产写路径的最终形态。
3. 一旦某个上传/删除/重命名/复制/移动契约已经明确，正式能力应迁回 `gatewayd` / provider crate 内执行。
4. 文档、测试、控制面文案都要把“页面探针”和“正式能力”区分开，不能混写成一个概念。

## 当前结构

当前仓库提供了一个最小可校验模型:

- Rust 类型: [crates/blob-core/src/browser_flow.rs](/home/walky/workspaces/carrier-cloud-blob-gateway/crates/blob-core/src/browser_flow.rs:1)
- 联通样例: [config/browser-flows/unicom-web.json](/home/walky/workspaces/carrier-cloud-blob-gateway/config/browser-flows/unicom-web.json:1)
- provider bridge 类型: [crates/blob-core/src/provider_bridge.rs](/home/walky/workspaces/carrier-cloud-blob-gateway/crates/blob-core/src/provider_bridge.rs:1)
- 移动 bridge 样例: [config/provider-bridges/mobile-web.json](/home/walky/workspaces/carrier-cloud-blob-gateway/config/provider-bridges/mobile-web.json:1)

并且现在已经有了最小的“目录级 loader + 查询接口”基础:

- `blob-core::BrowserFlowCatalogCollection` 可以从 `config/browser-flows/` 目录批量加载 catalog，并按 `provider/surface` 查找
- `blob-core::ProviderBridgeCatalogCollection` 可以从 `config/provider-bridges/` 目录批量加载 bridge catalog，并按 `provider` 查找
- `blob-core::BrowserFlowCatalog::bind_flow(...)` / `BrowserFlowCatalogCollection::bind_flow(...)` 可以把 `flows[].inputs`、`runtime.*` 和 `preset_refs` 绑定成一份已解析模板的执行计划
- `blob-core::DryRunBrowserFlowExecutor` 可以把绑定后的执行计划展开成逐步的 `Planned` 报告，方便在接入真实 CDP 执行层之前先验证 catalog、输入绑定和预期请求
- `blob-core::BrowserFlowSession` / `BrowserFlowSessionExecutor` 已经定义了真实执行时需要的通用浏览器动作边界，例如 `navigate`、`click`、`set_input`、`set_files`、`wait_for_request`、`wait_for_page`
- `gatewayd` 的 auth-capture 控制面现在已经能保存 CDP 配置字段，例如 `cdp_endpoint_url`、`cdp_target_selector`、`cdp_target_timeout_ms`，后续真实 transport 将优先消费这些字段，而不是写死某个浏览器或把浏览器宿主机常驻进 core
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
  - 当 flow 成功完成后，`gatewayd` 当前会把 `flows[].outputs` 里的 `script_value` / `dom_text` / `url` 输出写回同一条 auth session 的 `runtime`，供后续复用同一个 `auth_session_id` 的 flow 继续绑定 `{{runtime.*}}`
  - `POST /api/browser-flows/session-run` 的返回值当前也会直接带回这次会话最新的 `runtime`，方便控制面或 auth-broker 读取“点击前文本 / 点击后文本 / 是否进入倒计时”这类可插拔校验结果，而不用再写 provider-specific API
  - 当 flow 声明了 `prerequisite_flow_id`，`gatewayd` 会在同一条 `auth_session_id` 和同一个 CDP page session 内递归执行 prerequisite chain，再执行主 flow
  - 当前这一步还没有实现完整的 `response_field` / `request_field` 抓取；先覆盖登录后页面内可直接读取的 token / URL / store state

当前 schema version 为 `1`。

## 配置文件表达什么

一份浏览器流程配置由这些部分组成:

- `pages`: 页面身份和 URL pattern
- `elements`: 关键元素和 selector 候选
- `requests`: 关键请求的 URL、header、字段和成功码
- `operations`: 页面内 JS/Vue 入口点
- `flows`: 组合后的业务流程

`flows[].inputs[]` 目前除了 `id / label / kind / required / secret / description` 之外，还支持:

- `transient: true`
  - 适合图形验证码这类短时输入
  - `gatewayd` 不会把这类值长期保留在 auth session 里反复复用
  - `solve_visual_captcha` 在单次执行里也只会消费一次这类手工值，避免后续 captcha step 误填上一个 challenge

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

## `provider-bridge` 负责什么

`browser-flow` 负责页面动作本身，但它不应该承担控制面字段命名和 provider-specific UI 分支。

这部分现在单独抽到 `config/provider-bridges/*.json`，主要描述:

- `surface`
- `flow_aliases`
- `runtime_credential_mappings`
- `browser_profile`
- `logged_in_probes`

这层的边界是:

- 如果页面 flow id 改了，但动作语义没变，优先改 `provider-bridges`
- 如果 selector、JS 入口点、页面 URL pattern 变了，优先改 `browser-flows`
- 如果稳定 native 请求模板的静态字段变了，优先改 `provider-capabilities`
- 只有当上游正式对象协议或 provider-native 语义变化时，才应该回到 Rust provider / gateway 执行器改代码

## 联通样例现在覆盖了什么

当前联通样例覆盖九条主路径，其中 personal root 上传已按当前执行器能力拆成“当前会话抓取”“准备 uploader 上下文”和“真正附加文件上传”三段:

1. `unicom_sms_login`
2. `unicom_capture_current_session`
3. `unicom_prepare_personal_root_upload`
4. `unicom_personal_root_upload`
5. `unicom_create_directory`
6. `unicom_delete_entry`
7. `unicom_rename_entry`
8. `unicom_copy_entry`
9. `unicom_move_entry`

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
- `unicom_capture_current_session` 负责在已登录的 `file_list_all` 页面上直接抓取 `access_token`、`family_id`、`client_id` 和 `current_url`
- `unicom_personal_root_upload` 负责在这条已准备好的 uploader 上下文里附加本地文件并等待真实上传请求

目录创建、删除、重命名、复制、移动这些写路径当前也已经有了实测事实:

- `createDir()` 会组装 `spaceType`、`parentDirectoryId`、`directoryName`
- `action-dialog.deleteFile()` 会组装 `spaceType`、`vipLevel`、`dirList`、`fileList`
- `renameFileOrDirectory()` 会组装 `spaceType`、`type`、`fileType`、`id`、`name`
- `action-dialog.onSubmit()` 在 `copy` 分支会组装 `targetDirId`、`sourceType`、`targetType`、`dirList`、`fileList`
- `action-dialog.onSubmit()` 在 `move` 分支会组装 `targetDirId`、`sourceType`、`targetType`、`dirList`、`fileList`，家庭空间时还会带 `fromFamilyId`
- 两者最终都走 `/wohome/dispatcher`

这些事实现在已经同步沉淀进联通 native capability catalog 和 provider 代码:

- `CreateDirectory`
- `DeleteFile`
- `RenameFileOrDirectory`
- `CopyFile`
- `MoveFile`

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
7. 如果要做“点击前后页面是否发生了预期变化”的校验，优先把它做成独立 browser flow，并用 `prerequisite_flow_id` 抓取前态、主 flow 抓取后态，再把结果记录到 `outputs[].kind=dom_text` 或 `script_value`。
8. 不同运营商的校验逻辑不要写死在 Rust 里。联通可以看 `.change-code`，电信/移动可以看别的按钮、提示条、倒计时或错误提示，只要各自定义自己的 flow outputs 即可。
9. 图形验证码、一次性 challenge 这类值不要当成可长期复用输入；在 `flows[].inputs[]` 里显式标 `transient: true`。

## 与 provider 代码的边界

这层不是 `provider-unicom` 的替代品。

更准确地说:

- `browser-flow` 负责“怎么在浏览器里完成流程”
- `provider-unicom` 负责“怎么在服务里表达联通对象读写能力”
- `provider-capabilities` 负责“哪些已经稳定的 native 请求可以配置化复用”
- `provider-probes` 负责“后续自动探测应该逐项确认哪些事实”

再强调一次:

- 浏览器流程是取证层，不是最终能力层。
- provider crate 才是正式读写语义的长期落点。
- 如果某个实现只能在 CDP 活页存在时工作，它最多只能帮助我们拿到契约，不能作为完成态保留。

当后续联通写路径真正落进 Rust provider 时，优先复用这层产出的稳定事实:

- 上传前必须准备的上下文
- `upload2C` 的关键字段
- `CreateDirectory` / `DeleteFile` / `RenameFileOrDirectory` / `CopyFile` / `MoveFile` 这类动作的请求形状

当前仓库已经开始把其中一部分稳定动作继续下沉到独立 capability catalog，例如:

- [config/provider-capabilities/unicom-native.json](/home/walky/workspaces/carrier-cloud-blob-gateway/config/provider-capabilities/unicom-native.json:1)

这类文件适合承载:

- dispatcher 操作名
- 默认静态字段
- capability id 到稳定请求模板的映射

这样网页小改版时，优先改 `browser-flows`；若只是 stable native 动作的静态字段微调，则优先改 `provider-capabilities`，而不是直接改 Rust provider。

## 下一步建议

这份最小结构已经能支撑继续扩展，下一批优先项应该是:

1. 给真正的 CDP / `auth-capture` 执行层接入这套 catalog loader 和 `bind_flow(...)` 解析结果，而不是再手写 provider-specific 文件路径和模板替换。
2. 让 `auth-capture` sidecar 把待输入手机号、短信码、验证码统一映射到 `flows[].inputs`。
3. 继续补联通 family space 的网页 flow catalog 变体，以及后续电信/移动 provider 的网页 flow catalog。当前 native provider 已支持 `family` bucket，但浏览器流程样例仍以 personal space 为主。

目前仓库已经有最小可用的真实 CDP transport 和 session 执行链，但它仍然是可插拔的 capture 组件，不是必须常驻的 core 运行时；还没有完整的 auth-capture 编排层。

换句话说:

- 现在已经可以校验 catalog、加载 catalog、查询 flow、绑定模板、输出 dry-run step report
- 现在也可以通过 `gatewayd` 直接请求一份 flow 的 dry-run execution report，拿来验证输入是否齐全、预期请求是否匹配
- 现在已经有了可 mock 的 session executor，可以真正逐步执行这些步骤
- 现在已经可以把这个 session executor 接到任意 CDP 兼容浏览器会话上
- 但还没有把“短信码输入、验证码等待、人工确认、失败恢复”这类 auth-capture 编排完整落成独立 sidecar

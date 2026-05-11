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
- `gatewayd` 只读暴露:
  - `GET /api/browser-flows/catalogs`
  - `GET /api/browser-flows/catalog?provider=unicom&surface=pan.wo.cn-web`
  - `GET /api/browser-flows/flow/{flow_id}`
- `gatewayd` 现在还暴露一个服务层 dry-run 入口:
  - `POST /api/browser-flows/dry-run`
  - body 里显式传 `provider`、`surface`、`flow_id`、`inputs`、`runtime`
  - 返回值只包含执行报告，不回显绑定后的 secret context

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

当前联通样例覆盖七条真实验证过的主路径:

1. `unicom_sms_login`
2. `unicom_personal_root_upload`
3. `unicom_create_directory`
4. `unicom_delete_entry`
5. `unicom_rename_entry`
6. `unicom_copy_entry`
7. `unicom_move_entry`

其中上传流程记录了一个关键约束:

- 必须先调用页面动作组件的 `goUpload(false)`
- 再向 `#global-uploader-btn input[type=file]` 注入文件
- 然后等待真实 `upload2C` 请求

这是为了复用网页自身已经准备好的:

- `params.fileInfo`
- `directoryId`
- `spaceType`
- 加密后的 `fileInfo`
- uploader 内部续传/分片逻辑

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

目前还没有真实的浏览器执行器落地在仓库里。

换句话说:

- 现在已经可以校验 catalog、加载 catalog、查询 flow、绑定模板、输出 dry-run step report
- 现在也可以通过 `gatewayd` 直接请求一份 flow 的 dry-run execution report，拿来验证输入是否齐全、预期请求是否匹配
- 但还没有把 `click/set_input/set_files/wait_for_request` 这些步骤真正驱动到 CDP 会话上

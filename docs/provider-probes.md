# Provider Probes

`config/provider-probes/*.json` 用来描述“每个云盘后续要自动探测什么”。

如果要判断某个 provider 是否已经“做到满”，请同时对照:

- [docs/provider-completion-standard.md](/home/walky/carrier-cloud-blob-gateway/docs/provider-completion-standard.md:1)

这层和 `browser-flows`、`provider-capabilities` 的分工不同:

- `browser-flows`: 页面元素、页面动作、浏览器里的真实流程
- `provider-capabilities`: 已稳定的 native 请求模板和默认字段
- `provider-probes`: 为了把账号、作用域、读写路径自动摸清，需要逐项探测的目标清单

建议每个 probe item 至少包含这些字段:

- `id`: 稳定探测项标识
- `status`: `confirmed | partial | planned`
- `category`: `auth | scope_discovery | native_read | native_write | upload | metadata`
- `transport`: `native_http | browser_flow | browser_capture | manual_or_browser_capture | oauth_or_manual_token` 之类的人机都能读懂的枚举
- `goal`: 这项探测到底想确认什么
- `prerequisites`: 依赖哪些前置探测或材料
- `artifacts`: 成功后应该拿到哪些事实
- `config_targets`: 成功后这些事实应该落到哪里

当前已经补了四份初始 catalog:

- [config/provider-probes/unicom.json](/home/walky/workspaces/carrier-cloud-blob-gateway/config/provider-probes/unicom.json:1)
- [config/provider-probes/telecom.json](/home/walky/workspaces/carrier-cloud-blob-gateway/config/provider-probes/telecom.json:1)
- [config/provider-probes/mobile.json](/home/walky/workspaces/carrier-cloud-blob-gateway/config/provider-probes/mobile.json:1)
- [config/provider-probes/onedrive.json](/home/walky/workspaces/carrier-cloud-blob-gateway/config/provider-probes/onedrive.json:1)

下一步自动化时，建议直接把这层接到:

1. `gatewayd` 的控制面 catalog API。
2. `auth-capture` / CDP sidecar 的任务编排。
3. provider 健康检查和 capability 探测报告。

这样以后上游改版，优先改 probe catalog、browser flow catalog 或 capability catalog，而不是先动 Rust 主逻辑。

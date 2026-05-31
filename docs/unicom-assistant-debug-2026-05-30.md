# 联通短信助手联调修复记录（2026-05-30）

## 背景

这次问题发生在 Admin Web 的“中国联通登录助手”。

用户看到的现象分两段：

1. 从联通 tab 点击可以打开助手浮窗。
2. 但在浮窗里继续点击 `Start SMS Login + Validate` 时，没有继续驱动 CDP 浏览器，也没有打开联通登录页，看起来像“按钮没反应”。

同一阶段里，后端 `session-run` API 已经证明能真实驱动联通网页请求短信验证码，所以问题需要继续往前端控制链和运行时部署路径上收敛。

## 直接根因

最后确认是两个前端问题叠加：

### 1. 浮窗点击路径调用了缺失的 helper

文件:

- [crates/gatewayd/assets/admin/index.html](/home/walky/workspaces/carrier-cloud-blob-gateway/crates/gatewayd/assets/admin/index.html:13944)

联通浮窗点击 `Start SMS Login + Validate` 后，会先进入一个启动前探测分支。

当时页面里调用了:

- `probeBrowserEndpointsWithFlowTargetHint(...)`

但这段 helper 并没有实际定义在页面脚本里。

结果是：

- 点击事件本身已经触发
- 但 JS 在真正发出 `/api/browser-flows/session-run` 之前就抛异常
- 从用户视角看，就是“按钮没反应，CDP 也没反应”

真实浏览器回归时，页面反馈里直接出现了：

- `probeBrowserEndpointsWithFlowTargetHint is not defined`

这就是当时最直接的前端阻断点。

### 2. 联通启动前 CDP 预探测过严

补上 helper 后，浮窗按钮已经能继续执行，但又暴露出第二层拦截：

- 页面会先向 `/api/browser-cdp/probe` 发起预探测
- 如果 probe 返回失败，前端会直接 `return`
- 这样真实的 `/api/browser-flows/session-run` 根本没有机会执行

而联通这条链路的后端已经有“新建空白页 -> 自动跳到登录页”的兜底能力。

也就是说：

- probe 失败不代表真实 flow 一定不能跑
- 让 probe 失败直接阻断真正登录动作，会把本来可恢复的链路拦死

这正是“CDP 明明开着，但点按钮没反应”的第二层原因。

## 联通 selector 侧同时确认过的事实

这次联调里还确认了联通网页自己的发送验证码按钮匹配问题。

文件:

- [config/browser-flows/unicom-web.json](/home/walky/workspaces/carrier-cloud-blob-gateway/config/browser-flows/unicom-web.json:1)

之前的 `login.send_code_button` 选择器会误命中外层容器，比如：

- `.el-form-item__content`
- `.el-tabs__content`

真实可点击节点实际是短信验证码输入框旁边的：

- `DIV.change-code`

另外，联通页面发送按钮状态不只会显示：

- `发送验证码`
- `获取验证码`

还会出现：

- `重新发送`

这两个事实已经同步进 browser flow config，所以后端 flow 现在能真实点中按钮，并正确识别倒计时或重发状态。

## 修复内容

### 前端修复

文件:

- [crates/gatewayd/assets/admin/index.html](/home/walky/workspaces/carrier-cloud-blob-gateway/crates/gatewayd/assets/admin/index.html:13428)
- [crates/gatewayd/assets/admin/index.html](/home/walky/workspaces/carrier-cloud-blob-gateway/crates/gatewayd/assets/admin/index.html:13944)

做了两处修改：

1. 新增通用 helper `probeBrowserEndpointsWithFlowTargetHint(...)`
   - 输入是 browser endpoints、flow catalog 和 flow id
   - 自动按 endpoint 已配置 selector 或 flow catalog 默认 target selector 做 probe
   - 这层保持通用，不把联通逻辑硬编码进 Rust

2. 联通 `Start SMS Login + Validate` 启动前探测改为“提示但不阻断”
   - 仍然保留 probe 结果反馈
   - 但不再因为 probe 失败就提前 `return`
   - 让真实 `/api/browser-flows/session-run` 继续执行

这样做的原因很明确：

- 能力仍然保持可插拔
- 流程控制留在 browser flow / admin 前端配置层
- 不需要把联通的特例塞回后端 provider 主逻辑

### flow config 修复

文件:

- [config/browser-flows/unicom-web.json](/home/walky/workspaces/carrier-cloud-blob-gateway/config/browser-flows/unicom-web.json:1)

修复点：

1. `login.send_code_button` 只匹配短信验证码输入附近的可见 `.change-code`
2. 发送按钮文本匹配补充 `重新发送`
3. 发送后校验逻辑允许识别倒计时、成功提示和重发状态

## .43 测试机上额外踩到的部署点

这次还有一个很容易误判的运行时覆盖机制。

`.43` 上的 `gatewayd` 并不总是直接使用二进制内嵌的 admin HTML。

`gatewayd` 运行时会优先尝试读取外置模板：

- `/home/walky/apps/ccbg/assets/admin/index.html`

只有找不到外置模板时，才退回到二进制内嵌的 `include_str!` 版本。

这意味着：

1. 只替换远端 `gatewayd` 二进制，未必能更新前端页面
2. 如果外置 `assets/admin/index.html` 还是旧版本，浏览器仍然会拿到旧 JS

这次就是先看到：

- 远端 `unicom-web.json` 已更新
- 远端 `gatewayd` 也已替换重启
- 但实际页面源码里仍然存在旧的联通逻辑

最后追到原因，就是 `.43` 使用了外置 admin 页面覆盖。

所以本次部署时两样都要同步：

1. `/home/walky/apps/ccbg/gatewayd`
2. `/home/walky/apps/ccbg/assets/admin/index.html`

## 回归结果

在真实 CDP 浏览器 `http://192.168.1.36:9222` 上做了联通回归。

### 回归一：确认浮窗按钮确实触发前端请求

在已登录的 Admin 页面里，打开联通助手浮窗，点击：

- `Start SMS Login + Validate`

观察到前端依次发出：

1. `/api/browser-cdp/probe`
2. `/api/browser-flows/session-run`

这说明“浮窗按钮点击链”已经恢复，不再是无响应状态。

### 回归二：确认 CDP 真实打开联通登录页

同一次回归里，CDP target 列表里出现了新页面：

- `https://pan.wo.cn/login`

说明真实后端 flow 已经接管浏览器，并完成：

1. 新开页/空白页
2. 导航到联通登录页

这就是用户要求的那条链路：

1. 助手窗口立即弹出
2. CDP 浏览器里新开一个空白页/新标签
3. 然后自动跳到联通登录页

现在第 3 步已恢复。

## 结论

这次问题的本质不是运营商网页完全失效，而是：

1. 联通浮窗路径上的前端 helper 缺失
2. 启动前 probe 过严，把本来能跑的后端 flow 拦死
3. `.43` 存在外置 admin 页面覆盖，导致只发二进制时修复不会生效

修完后，联通浮窗链路已经恢复到：

- 浮窗点击可触发真实请求
- CDP 可新开并跳到联通登录页
- 后续短信登录流程继续由 browser flow + auth session 承接

## 后续建议

1. 移动和电信继续沿用同一套排查顺序：
   - 先看浮窗按钮有没有发 `/api/browser-flows/session-run`
   - 再看 CDP 是否新开目标页
   - 最后再看运营商页面 selector / iframe / prompt 细节

2. 任何 `.43` 上的前端修复，都要先确认运行时是否存在外置：
   - `/home/walky/apps/ccbg/assets/admin/index.html`

3. 对登录助手这类前端流程，尽量把“可失败但不致命”的预探测都设计成：
   - warning / hint
   - 而不是 hard block

这样更符合当前“能力可插拔、最小化 provider-specific 硬编码”的边界。

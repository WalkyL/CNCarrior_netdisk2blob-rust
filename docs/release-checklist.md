# CCBG Release Checklist

这份清单用于把当前版本从 `.43` 测试机推进到正式 release。

目标很简单：

- release 产物自洽
- `.43` 作为 release candidate 做一次全量验证
- 所有阻塞项清零后再开始正式 release

建议每次正式发布都复制一份，按实际结果勾选并留档。

## 1. Release 产物

- [x] Git 工作区干净
- [x] 已有明确 release 候选提交：`460a27f`
- [x] `gatewayd` 与 Admin HTML 作为同一发布物交付
- [x] LXC 包包含 `assets/admin/index.html`
- [x] OpenWRT lite 包包含 `assets/admin/index.html`
- [x] 容器镜像构建文件已显式复制 Admin HTML
- [ ] 生成本次正式 release 的 provenance 文件
- [ ] 生成本次正式 release 的对外交付包与 SHA256

## 2. 本地质量门

- [x] `cargo check -p gatewayd --quiet`
- [x] `cargo test -p gatewayd --quiet`
- [x] `cargo test -p browser-cdp --quiet`
- [x] `cargo test -p blob-core --quiet`
- [x] `git diff --check`
- [ ] 需要时补跑完整 workspace 级测试

## 3. `.43` 自动 smoke

测试机：`192.168.1.43`

- [x] `GET http://192.168.1.43:61080/healthz` 返回 `200 OK`
- [x] `GET http://192.168.1.43:61081/` 返回 `302 /login`
- [x] 已确认 `.43` Admin API 登录态可取到 `api/status`
- [x] `GET /api/showcase?limit=3` 返回 `200`
- [x] `POST /api/browser-cdp/open-endpoint-info` 返回 `200`
- [x] `POST /api/browser-cdp/probe` 返回 `200`

## 4. 公共站点 smoke

- [x] `https://carrier-disk-gateway.agi2030.online/faq/` 返回 `200`
- [x] `https://carrier-disk-gateway.agi2030.online/install/` 返回 `200`
- [x] `https://carrier-disk-gateway.agi2030.online/api/faq/catalog` 返回 `200`
- [x] `https://carrier-disk-gateway.agi2030.online/api/faq/match` 返回 `200`

## 5. `.43` 全量人工验收

这一节是当前真正的 release gate。CLI 不替代浏览器人工确认。

### 5.1 顶部与基础导航

- [ ] 顶部第一行布局正确：站点标题、语言选择、生态项目展示栏同一行
- [ ] showcase 展示位内容可见且可点击
- [ ] 中文语言下关键页面不再残留明显英文文案
- [ ] Dashboard / Providers / 联通 / 电信 / 移动 / Browser-CDP / Logs 可正常切换

### 5.2 Browser / CDP

- [ ] CDP 端点列表可加载
- [ ] “打开识别页”会在目标浏览器打开端点识别页
- [ ] 识别页能明确显示 endpoint 信息和 port，避免用户连错浏览器
- [ ] Probe 结果与当前真实浏览器标签页一致

### 5.3 联通助手

- [ ] 切到 `联通` tab 时不再自动弹出引导浮窗
- [ ] 点击 `Open Guided Window` 可正常打开联通助手窗口
- [ ] 点击 `Start SMS Login + Validate` 后，CDP 浏览器会新开页并跳到联通登录页
- [ ] 已登录联通页面时可直接抓当前会话
- [ ] 联通助手主要文案为中文
- [ ] 发送短信验证码、继续提交验证码、抓取输出三条主链都可用

### 5.4 电信助手

- [ ] 电信助手窗口主要文案为中文
- [ ] 可进入账号登录 / 短信登录正确路径
- [ ] `Start SMS Login + Validate` 可驱动真实电信登录页
- [ ] 发送验证码、继续提交验证码、抓取当前会话可用

### 5.5 移动助手

- [ ] 移动助手窗口主要文案为中文
- [ ] `Start SMS Login` / 引导流程可驱动真实移动登录页
- [ ] 已登录页面时可直接抓当前会话
- [ ] 上传探针 / 捕获相关面板可正常使用

### 5.6 Provider / Health / Logs

- [ ] Provider Health 正常显示 personal / family 等 scope 信息
- [ ] Dashboard 能显示关键运行状态、告警和对象动作摘要
- [ ] Logs 页面可加载
- [ ] Logs 的级别 / 来源 / 搜索过滤可用
- [ ] AI 解释入口可打开统一 modal，FAQ 命中和复制 Prompt 可用

### 5.7 对象与控制面

- [ ] 对象动作入口可见
- [ ] shared history / object action history 可加载与导出
- [ ] 复制失败与 retry 路径可见
- [ ] 控制面中文化没有明显断裂

## 6. Release 决策门

只有在下面条件同时满足时才开始正式 release：

- [ ] `.43` 全量人工验收全部通过
- [ ] release 产物与 SHA256 已生成
- [ ] provenance 已生成
- [ ] 回滚包 / 上一个稳定版本可用
- [ ] 发布记录已落文档

## 7. 回滚准备

- [ ] 保留上一版 `gatewayd` 二进制或完整安装包
- [ ] 保留上一版 Admin HTML
- [ ] 保留上一版配置与 control-plane 快照
- [ ] 明确正式发布失败时的回滚执行人和步骤

## 8. 当前结论

截至 2026-05-31：

- 自动化 smoke 已基本通过
- release 产物“一体化打包 Rust 程序 + Admin HTML”已落地
- 当前唯一明确阻塞项是 `.43` 上的全量人工验收尚未完成

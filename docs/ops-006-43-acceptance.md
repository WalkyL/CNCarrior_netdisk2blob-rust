# OPS-006: .43 测试机发布与验收记录

## 状态

- 执行时间: 2026-05-30 00:30 +0800
- 结论: `.43` 测试机 Rust/Admin 包已发布并通过基础 smoke；公共站点 Worker/Assets 已上传，`carrier-disk-gateway.agi2030.online` 已绑定并通过 FAQ/安装页/API smoke；登录后的 Admin 人工验收仍需补齐。
- 测试机: `walky@192.168.1.43`
- 应用目录: `/home/walky/apps/ccbg`
- 运行方式: `nohup ./gatewayd`，由 `gatewayd.pid` 记录 PID；现有 `ccbg.service` 未管理当前进程。

## 发布记录

本次发布属于 Rust/Admin 资产变更，需要重新构建并替换 `gatewayd`。

注意：

- `.43` 这台测试机当前运行时会优先读取外置 admin 模板 `/home/walky/apps/ccbg/assets/admin/index.html`
- 所以凡是涉及 Admin 前端 JS/HTML 的修复，不能只替换 `gatewayd` 二进制
- 必须同时检查并同步外置 admin 页面，否则浏览器仍会拿到旧前端逻辑
- 这属于测试机当前部署遗留，不应作为正式 release 约定；正式发布物必须把 `gatewayd` 与 Admin HTML 一起打包交付

- 构建命令: `cargo build --release -p gatewayd`
- 发布前远端 binary sha256: `606629d55f72211f343a59eaed985dac4b03a554d59d3b0854275856ee78bb6f`
- 发布后远端 binary sha256: `fa5d855722b2a6922f08202207841c79d264f839aeb5193c806ef94b916fb174`
- 备份文件: `/home/walky/apps/ccbg/gatewayd.prev.20260530-002837.ops006`
- 新进程 PID: `619802`

当前监听端口:

- `0.0.0.0:61080`: data/health
- `0.0.0.0:61081`: Admin Web/API
- `127.0.0.1:61083`: metrics

## 自动 smoke

### Health

命令:

```bash
curl -sS -i --max-time 5 http://192.168.1.43:61080/healthz
```

结果:

- HTTP `200 OK`
- backend: `unicom-cloud-drive`
- status: `healthy`
- notes 中包含 `auth_probe_status=accepted` 与 `auth_probe_rsp_desc=成功`

### Admin API 未登录合同

命令:

```bash
curl -sS -i --max-time 5 http://192.168.1.43:61081/api/status
```

结果:

- HTTP `401 Unauthorized`
- body: `{"error":"admin login required","code":"unauthorized","api_version":"2026-05-26"}`

### Admin 根路径保护

命令:

```bash
curl -sS -i --max-time 5 http://192.168.1.43:61081/
```

结果:

- HTTP `302 Found`
- location: `/login`

### 运行日志

命令:

```bash
ssh walky@192.168.1.43 'cd /home/walky/apps/ccbg; tail -80 gatewayd.err'
```

结果: `gatewayd.err` 当前为空。

## 人工验收清单

以下项目需要使用 Admin 登录态在浏览器中检查，CLI 未替代执行:

- 首页/Dashboard: 中文化首屏、对象动作入口、失败对象明细可见。
- Providers: provider 状态、失败复制重试、告警查看路径可用。
- Mobile: 已抓到 token/cookie/browser profile/root/user domain 时，助手、凭据卡和 provider health 不再分叉。
- Logs: `/api/admin/logs` 驱动的日志页可加载，级别/来源/搜索过滤可用。
- AI 解释: 错误日志与配置项入口均能打开统一 modal；FAQ 命中、Prompt 复制和前端 LLM 降级路径可用。

## 公共站点检查

OPS-006 要求正式域名站点可访问 FAQ 与安装页。本次检查已通过。

权威 DNS 与公共 DNS 已返回 Cloudflare A 记录:

```bash
dig +short @1.1.1.1 carrier-disk-gateway.agi2030.online A
dig +short @8.8.8.8 carrier-disk-gateway.agi2030.online A
dig +short @corey.ns.cloudflare.com carrier-disk-gateway.agi2030.online A
```

当前本机默认解析器仍可能有短时缓存滞后；正式域名 HTTP smoke 使用 `--resolve carrier-disk-gateway.agi2030.online:443:104.21.64.196` 验证。

```bash
curl -sS -L -i --max-time 10 https://carrier-disk-gateway.agi2030.online/faq/
curl -sS -L -i --max-time 10 https://carrier-disk-gateway.agi2030.online/install/
```

结果:

- `/`: HTTP 200，返回 `Carrier Cloud Blob Gateway` 首页。
- `/faq/`: HTTP 200，返回 `CCBG FAQ` 页面。
- `/install/`: HTTP 200，返回 `CCBG Install` 页面。
- `/api/faq/catalog`: HTTP 200，返回 version `2026-05-28`、count `5`。
- `/api/faq/match`: HTTP 200，mobile `missing root_folder_id token expired` 查询命中 `mobile-credential-configured-but-unavailable`。

### Cloudflare 发布补充记录

本机 Cloudflare 凭据文件 `/home/walky/.stock-rag-cf-secrets.env` 可用，但不覆盖 Pages 项目管理权限；本次改走 Worker + Assets fallback。

已完成:

- 新增 Worker fallback 入口 `public/cloudflare/worker.js`，复用 FAQ catalog/match 合同。
- 新增 Worker 专用配置 `public/cloudflare/wrangler.worker.toml`，保留原 Pages 配置不变。
- 使用 `target/cloudflare-public-assets` staging 目录上传公开静态资产，排除 `functions/`、`worker.js`、`wrangler.toml`。
- `wrangler deploy -c target/wrangler-ccbg-public-domain.toml --assets target/cloudflare-public-assets --domain carrier-disk-gateway.agi2030.online` 已上传 Worker 与公开静态资产，并绑定 custom domain。
- Worker 最新可见部署版本: `918c2c76-7b3a-4d0c-9b82-81e3c6d67867`。

未完成:

- 使用 committed `public/cloudflare/wrangler.worker.toml` 复现部署时，应按 README 通过 CLI `--domain carrier-disk-gateway.agi2030.online` 绑定；不要在该 toml 里写 `routes`，否则 wrangler 会走 zone Workers routes 接口。后续正式流程改为 GitHub Actions：`test` 分支部署测试 Worker + Assets，`main` 分支部署生产 Worker + Assets。

本地 Worker smoke:

- `http://127.0.0.1:8788/`: HTTP 200。
- `http://127.0.0.1:8788/faq/`: HTTP 200。
- `http://127.0.0.1:8788/install/`: HTTP 200。
- `http://127.0.0.1:8788/api/faq/catalog`: 返回 version `2026-05-28`、count `5`。
- `http://127.0.0.1:8788/api/faq/match`: mobile `missing root_folder_id token expired` 查询命中 `mobile-credential-configured-but-unavailable`。

### 解析与交叉影响判断

补充检查:

- `carrier-disk-gateway.agi2030.online` 已绑定到 `ccbg-public` custom domain。
- `ccbg-public.pages.dev` 无 DNS 记录。
- `register.agi2030.online` 可解析到 Cloudflare。
- `/home/walky/register-agi2030/wrangler.toml` 只绑定 `register.agi2030.online`，未覆盖 `carrier-disk-gateway.agi2030.online`。
- 当前仓库 `public/cloudflare/wrangler.toml` 只有 Pages 项目名 `ccbg-public`，未声明自定义域名绑定。
- 当前 token 可通过 wrangler `whoami`，但 Pages 项目查询权限不足；Worker custom domain 已通过 CLI `--domain` 成功绑定。

判断: 当前更像是 CCBG 公共 Pages 项目/自定义域名未发布或未绑定，不是 `register.agi2030.online` 项目直接覆盖了 CCBG 域名；但两个项目同属 `agi2030.online` zone，补绑定时需要确认 Cloudflare zone 里的 DNS/Workers/Pages route 没有互相占用。

## 下一步

1. 等本机默认 DNS 缓存刷新后，不带 `--resolve` 复测正式域名。
2. 使用 Admin 登录态完成 `.43:61081` 浏览器人工验收。

## 2026-05-31 增量修复记录

- 已补发 `.43` 外置 Admin 模板 `/home/walky/apps/ccbg/assets/admin/index.html`，覆盖联通/电信/移动登录助手近期前端修复。
- 已确认 `.43` 当前健康检查仍为 `200 OK`：

```bash
curl -sS -i --max-time 5 http://192.168.1.43:61080/healthz
```

- 已确认 `.43` Admin 根路径仍按预期跳转登录页：

```bash
curl -sS -i --max-time 5 http://192.168.1.43:61081/
```

- 本次增量包含两条关键运行时结论：
  - 联通短信助手浮窗内点击 `Start SMS Login + Validate` 已恢复触发真实 `/api/browser-flows/session-run`
  - 进入 `联通` tab 时不再自动弹出引导浮窗，只有用户显式点开或流程进入进行态时才开窗

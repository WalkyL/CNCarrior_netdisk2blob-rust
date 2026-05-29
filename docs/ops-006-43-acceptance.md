# OPS-006: .43 测试机发布与验收记录

## 状态

- 执行时间: 2026-05-30 00:30 +0800
- 结论: `.43` 测试机 Rust/Admin 包已发布并通过基础 smoke；登录后的 Admin 人工验收与正式公共域名访问仍需补齐。
- 测试机: `walky@192.168.1.43`
- 应用目录: `/home/walky/apps/ccbg`
- 运行方式: `nohup ./gatewayd`，由 `gatewayd.pid` 记录 PID；现有 `ccbg.service` 未管理当前进程。

## 发布记录

本次发布属于 Rust/Admin 资产变更，需要重新构建并替换 `gatewayd`。

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

OPS-006 要求正式域名站点可访问 FAQ 与安装页。本次检查未通过:

```bash
curl -sS -L -i --max-time 10 https://carrier-disk-gateway.agi2030.online/faq/
curl -sS -L -i --max-time 10 https://carrier-disk-gateway.agi2030.online/install/
```

结果均为 DNS 解析失败: `Could not resolve host: carrier-disk-gateway.agi2030.online`。

补充检查:

- `carrier-disk-gateway.agi2030.online` 无 DNS 记录。
- `ccbg-public.pages.dev` 无 DNS 记录。
- `register.agi2030.online` 可解析到 Cloudflare。
- `/home/walky/register-agi2030/wrangler.toml` 只绑定 `register.agi2030.online`，未覆盖 `carrier-disk-gateway.agi2030.online`。
- 当前仓库 `public/cloudflare/wrangler.toml` 只有 Pages 项目名 `ccbg-public`，未声明自定义域名绑定。
- 本机 wrangler 未登录: `wrangler whoami` 返回 `You are not authenticated. Please run wrangler login.`

判断: 当前更像是 CCBG 公共 Pages 项目/自定义域名未发布或未绑定，不是 `register.agi2030.online` 项目直接覆盖了 CCBG 域名；但两个项目同属 `agi2030.online` zone，补绑定时需要确认 Cloudflare zone 里的 DNS/Workers/Pages route 没有互相占用。

## 下一步

1. 登录 Cloudflare/wrangler，创建或确认 `ccbg-public` Pages 项目。
2. 将 `public/cloudflare` 发布为 Pages assets，并启用 Pages Functions。
3. 绑定 `carrier-disk-gateway.agi2030.online` 到该 Pages 项目。
4. 复测 `/faq/`、`/install/`、`/api/faq/catalog`、`/api/faq/match`。
5. 使用 Admin 登录态完成 `.43:61081` 浏览器人工验收。

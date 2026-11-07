# LuCI 插件开发规范（官方资料整理）

> 来源：OpenWrt `openwrt/luci` 官方仓库 `applications/luci-app-example`（README.md + structure.md + 实际文件），
> 抓取日期 2026-11-07。这是 LuCI 官方推荐的现代（JavaScript client-rendered）插件开发规范。
> 官方 wiki（openwrt.org）当时被 Anubis 反爬拦截，本总结以 GitHub 源码为准。

## 1. 官方标准目录结构

```
luci-app-<appname>/
├── Makefile                                     # 必选：license、依赖包声明
├── README.md                                    # 推荐：应用说明、测试指引
├── htdocs/luci-static/resources/view/<appname>/*.js   # JS 视图（client-rendered）
├── root/
│   ├── etc/uci-defaults/80_<appname>            # 可选：启动时创建默认 /etc/config/<appname>
│   └── usr/share/
│       ├── luci/menu.d/luci-app-<appname>.json  # 必选：菜单映射
│       └── rpcd/
│           ├── acl.d/luci-app-<appname>.json    # 必选：ACL 权限声明
│           └── ucode/<appname>.uc               # 可选：RPC 后端脚本
└── po/templates/<appname>.pot                   # 翻译模板
```

- 起点是 LuCI 仓库的 `applications/` 目录，必须创建名为 `luci-app-<appname>` 的文件夹。
- 至少一个 `Makefile`（定义 license + 依赖）；`README.md` 推荐。

## 2. 菜单注册（menu.d/*.json）

文件放 `root/usr/share/luci/menu.d/luci-app-<appname>.json`，名字 = 应用文件夹名 + `.json`。

```json
{
  "admin/<appname>/": {
    "title": "<App 标题>",
    "order": 60,
    "action": { "type": "firstchild" },
    "depends": { "acl": [ "luci-app-<appname>" ] }
  },
  "admin/<appname>/<page>": {
    "title": "<页面标题>",
    "order": 1,
    "action": { "type": "view", "path": "<appname>/<viewfile>" }
  }
}
```

- 目录节点用 `action: {type: "firstchild"}`，子页面用 `action: {type: "view", path: "<app>/<view>"}`。
- 每个菜单项通过 `"depends": {"acl": ["luci-app-<appname>"]}` 门控。

## 3. ACL 权限声明（acl.d/*.json）

文件放 `root/usr/share/rpcd/acl.d/luci-app-<appname>.json`。

```json
{
  "luci-app-<appname>": {
    "description": "Grant access to ...",
    "read": {
      "uci": [ "<config>" ],
      "ubus": { "luci.<app>": [ "method1", "method2" ] },
      "file": { "/etc/<app>.json": [ "read" ] }
    },
    "write": {
      "uci": [ "<config>" ],
      "file": { "/etc/<app>.json": [ "write" ] }
    }
  }
}
```

**硬约束**：ACL 未正确授权时，Web UI 显示 **"Access denied"**。修复 ACL 文件后需 **`restart rpcd`**。

- `read > ubus > <namespace>` 是允许调用的 RPC API 列表。
- `read > uci > ["<config>"]` / `write > uci > ["<config>"]` 控制 UCI 读写（映射到 `/etc/config/<config>`）。
- ACL 是**全局**的：声明 `luci-app-<appname>` 只是命名约定，不阻止 Web UI 里其他代码修改同一 UCI 节点。

## 4. JS 视图（htdocs/luci-static/resources/view/<appname>/*.js）

现代 LuCI 用 **client-rendered JS**（不是旧式 `.htm` 模板）。核心模式：

```js
'use strict';
'require uci';      // 或 'require view', 'require rpc', 'require fs'
'require view';

return view.extend({
  load: function () {
    return Promise.all([ uci.load('<config>') ]);   // load 是 Promise
  },
  render: function (data) {
    var body = E([ E('h2', _('标题')) ]);           // E() 构建 DOM
    // ...
    return body;
  }
});
```

- 视图文件**没有** `.htm` 后缀（`structure.md` 里的例子全是 `.js`：`form.js`、`htmlview.js`、`rpc.js`、`file-jsonmap.js`）。
- `E(node_type, {attrs}, [children])` 构建 DOM；`load()` 返回 Promise 数组，结果传入 `render()`。
- `form.Map` / `form.JSONMap` / `rpc` / `fs` 是常用模块。
- **缩进用 tab**；格式化用 `js-beautify -t -a -j -w 110 -r <filename>`。

## 5. RPC 后端（可选）

- RPCd 脚本放 `root/usr/share/rpcd/ucode/<app>.uc`，通过 ubus 调用。
- 对应 ACL 声明 `read > ubus > luci.<app> > [method...]`。
- 示例 `rpc.js` 展示 RPC 调用 + 只读 JSONMap 展示。

## 6. UCI defaults（可选）

- 放 `root/etc/uci-defaults/80_<app>`，启动时创建默认 `/etc/config/<app>`。
- 数字前缀控制顺序（如 `80_example`）。

## 7. 翻译

- 用 **Weblate**（不要直接编辑 `.po`）。
- 重新生成模板：`./build/i18n-scan.pl applications/luci-app-<app> > applications/luci-app-<app>/po/templates/<app>.pot`。
- 需要 GNU gettext（`msguniq`）。

## 8. 安装与缓存

- **安装后登出/登入** Web 界面强制刷新缓存（README 明确："log out of the web interface and back in to force a cache refresh"）——**不是重启 uhttpd**。
- 通过 git 安装：`scp -r root/* root@<router>:/` + `scp -r htdocs/* root@<router>:/www/` + 执行 UCI defaults。
- 通过包安装：`apk add luci-app-example`，访问 Web UI，菜单出现该应用。

## 9. 代码规范（CONTRIBUTING.md）

- 提交信息：`<component>: <小写开头描述>`（如 `luci-mod-admin-full: wifi channel selection on STA networks`）。
- 每条 commit 必须含 `Signed-off-by`（真实姓名，不用 noreply）。
- 只 cherry-pick 安全/修复到 release 分支（`openwrt-xx.yy`），不加新包、不做大升级。
- 不用 `git push --force`；有疑问走 PR。

## 10. 对 CCBG 嵌入方案的关键结论

1. **现代 LuCI 用 `menu.d/*.json` 注册菜单**，不是旧式 `controller/*.lua`（修正了此前基于旧版 LuCI 的假设）。
2. **安装后登出/登入刷新缓存**，**不重启 uhttpd**——官方规范明确此点。
3. **ACL 是硬门槛**：LuCI 页面若要访问 CCBG 的 `/api/*`，必须在 `acl.d/luci-app-ccbg.json` 授权，否则 "Access denied"；且通常需 **`restart rpcd`**。
4. **官方结构面向「LuCI 原生 JS 页面」**（UCI/RPC 访问），**没有 iframe 内嵌独立服务的先例**——"嵌入现有 1.3MB Admin SPA" 不属于标准 LuCI 应用形态，需要 gatewayd 侧配合（frame-ancestors / API base），而非纯 LuCI 插件规范覆盖。

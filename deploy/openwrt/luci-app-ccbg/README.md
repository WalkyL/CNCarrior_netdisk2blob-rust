# luci-app-ccbg

LuCI 插件包：把 CCBG Gateway 的 Admin 管理页面以 iframe 嵌入 OpenWRT LuCI 界面。

## 方案说明（ADR / 最终裁决：方案 A）

- LuCI 页面（`htdocs/luci-static/resources/view/ccbg/admin.js`）渲染一个 iframe，
  src 用当前 LuCI 来源的 `scheme://host:61081/` 动态构造。
- gatewayd 的 Admin 服务监听 `CCBG_ADMIN_BIND_ADDR`（默认 `127.0.0.1:61081`，
  LuCI 嵌入需改为 LAN 可达地址）。
- gatewayd 必须配置 `CCBG_ADMIN_FRAME_ANCESTORS` 为 LuCI 的来源（如
  `http://192.168.1.1`），否则 CSP `frame-ancestors 'none'` + `X-Frame-Options: DENY`
  会阻止 iframe 加载。
- 纯 HTTP LAN 下还必须 `CCBG_ADMIN_COOKIE_SECURE=false`，否则浏览器拒绝发送
  session cookie，登录后立即失效。若配置了 `http://` 来源且 `CCBG_ADMIN_COOKIE_SECURE`
  仍为默认 `true`，gatewayd 启动时会打印一条 advisory warning（不会阻止启动）。
  `CCBG_ADMIN_COOKIE_SECURE` 同时是 Admin 传输层信号的来源：`true` 表示 Admin
  端点走 HTTPS，`false` 表示纯 HTTP；CSRF 同源判定据此比较 Origin 的 scheme。
- `CCBG_ADMIN_FRAME_ANCESTORS` 只接受 `scheme://host[:port]` 形式；**任一非法项
  （如 `*`、`'self'`、`http:`、带路径/查询/用户信息的地址、含 CSP 分隔符或非 ASCII
  的主机名、WHATWG 数字形式但非规范点分四段的 IPv4、非法或未加括号的 IPv6、非法端口）
  会使整个配置校验失败并拒绝启动**，而不是被静默丢弃。这样拼写错误不会在无提示的
  情况下削弱或禁用 `frame-ancestors` 指令。主机校验只接受浏览器（WHATWG）序列化的
  形式：点分四段 IPv4、十六进制压缩的 IPv6（IPv4 映射/兼容地址也保留纯十六进制，
  而非点分十进制），且默认端口（`:80`/`:443`）会被规范化省略，使配置与浏览器发出的
  `Origin` 头精确一致。

## 打包

本包 `PKGARCH:=all`（纯 JS/JSON，架构无关），一份 `.ipk` 同时覆盖 x86_64 和 arm。
在 OpenWRT 构建树中：

```
make package/luci-app-ccbg/compile
```

生成的 `luci-app-ccbg_1.0-1_all.ipk` 用 `opkg install` 安装到目标设备。

## 安装与刷新

- 安装后**登出/登入** LuCI 界面以刷新菜单/ACL 缓存（LuCI 官方规范），**不需要**
  重启 uhttpd 或 rpcd。
- gatewayd 服务本身如需应用配置变更，允许 `service ccbg restart`（用户接受的边界）。

## 文件

```
htdocs/luci-static/resources/view/ccbg/admin.js   # iframe 视图
root/usr/share/luci/menu.d/luci-app-ccbg.json      # 菜单
root/usr/share/rpcd/acl.d/luci-app-ccbg.json       # ACL（菜单可见性）
Makefile                                           # PKGARCH:=all
```

> 本包只负责 LuCI 侧展示；gatewayd 二进制按架构单独出包（x86_64 / arm）。

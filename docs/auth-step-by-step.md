# 云盘认证 Step by Step 指南

## 这份文档是给谁看的

这份文档是写给:

- 不熟悉开发工具的用户
- 第一次给 `carrier-cloud-blob-gateway` 配认证的人
- 需要把自己的云盘账号接入本地网关的人

## 先说结论

当前仓库的认证边界是:

- 不自动抓浏览器登录态
- 不自动窃取 Cookie
- 不自动拦截网页流量
- 只接受你手工提供的认证材料

当前支持的输入方式:

- `CCBG_*_TOKEN`
- `CCBG_*_TOKEN_FILE`
- `CCBG_*_COOKIE_HEADER`（运营商 provider 预留字段）

最推荐的做法:

1. 不把 token 直接写在命令行里。
2. 优先把 token 保存到本地文件。
3. 再把 `*_TOKEN_FILE` 写进 `.env.local`。

## 你需要准备什么

开始之前，请准备:

- 你的云盘账号已经能在浏览器里正常打开
- 一台能运行本项目的 Linux 主机
- Chrome、Edge 或其他带开发者工具的浏览器
- 一个只给自己看的 token 文件目录

推荐先创建一个专用目录:

```bash
mkdir -p /home/walky/.config/ccbg/credentials
chmod 700 /home/walky/.config/ccbg/credentials
```

## 最重要的安全规则

1. 只处理你自己账号的认证信息。
2. 不要把 token 发给别人。
3. 不要把 token 贴到公开 issue、聊天记录或截图里。
4. 不要把 token 写进 Git 仓库。
5. 用完浏览器开发者工具后，可以关闭标签页并重新登录一次，降低误泄露风险。

## 浏览器里找 token 的通用方法

下面的步骤，联通、电信、移动和 OneDrive 都通用。

### 第 1 步: 先正常登录网页版云盘

1. 打开对应云盘的官方网站。
2. 完成登录。
3. 确认你已经能看到文件列表，不要停在登录页。

### 第 2 步: 打开开发者工具

1. 在当前浏览器页面按 `F12`。
2. 点开 `Network` 或“网络”标签。
3. 勾选 `Preserve log` 或“保留日志”。
4. 在过滤条件里优先选择 `Fetch/XHR`。

### 第 3 步: 刷新页面

1. 按浏览器刷新按钮。
2. 等文件列表重新显示出来。
3. 观察网络请求列表。

### 第 4 步: 找“列文件”或“读文件”的请求

优先找这些类型的请求:

- 打开首页后加载文件列表的请求
- 点击某个目录后刷新目录内容的请求
- 预览、下载或查看文件详情的请求

通常这些请求最容易带上真实认证信息。

### 第 5 步: 打开请求详情

1. 在左侧点开某一条请求。
2. 看 `Headers` 或“标头”。
3. 找 `Request Headers` 或“请求标头”。

### 第 6 步: 找认证字段

常见位置:

- `Authorization: Bearer xxxxx`
- `Cookie: xxxxx`
- 某个自定义 token header

对这个项目来说，最优先找的是:

- `Authorization`
- 明显像 access token 的字段
- 其次再记录 `Cookie`

### 第 7 步: 保存到文件

推荐格式:

- token 文件只放 token 本体
- cookie 文件放完整 `Cookie` 请求头值

例如:

```bash
chmod 700 /home/walky/.config/ccbg/credentials
printf '%s\n' 'replace-with-token' > /home/walky/.config/ccbg/credentials/example.token
chmod 600 /home/walky/.config/ccbg/credentials/example.token
```

如果你拿到的是:

- `Authorization: Bearer abcdefg`

通常应把 `abcdefg` 写进 token 文件，不要连 `Bearer ` 一起写进去。

如果你拿到的是:

- `Cookie: foo=1; bar=2`

则把 `foo=1; bar=2` 作为 cookie 内容保存。

## 中国联通云盘

### 当前代码支持到什么程度

当前 `provider-unicom` 还是 scaffold:

- 已有配置结构
- 已有 token / token file / cookie header 入口
- 当前 health 检查只验证“token 是否存在”
- 真实上游接口映射还没有完成

这意味着:

- 你现在可以先把认证材料准备好
- 但今天还不能承诺联通云盘真实读写已经完全打通

### 联通云盘 Step by Step

1. 在浏览器打开中国联通云盘网页，并登录你自己的账号。
2. 进入能看到文件列表的页面。
3. 按 `F12` 打开开发者工具。
4. 进入 `Network`。
5. 勾选 `Preserve log`。
6. 选择 `Fetch/XHR`。
7. 刷新页面。
8. 找请求域名里接近 `panservice.mail.wo.cn` 的请求。
9. 点开一条“列文件”或“查看目录”的请求。
10. 在 `Request Headers` 里找 `Authorization`。
11. 如果有 `Authorization: Bearer xxxxx`，把 `xxxxx` 保存到:

```bash
printf '%s\n' 'replace-with-unicom-token' > /home/walky/.config/ccbg/credentials/unicom.token
chmod 600 /home/walky/.config/ccbg/credentials/unicom.token
```

12. 如果请求里还有 `Cookie`，也建议一并保存，供后续真实 provider 接入时备用:

```bash
printf '%s\n' 'replace-with-unicom-cookie' > /home/walky/.config/ccbg/credentials/unicom.cookie
chmod 600 /home/walky/.config/ccbg/credentials/unicom.cookie
```

13. 打开你的 `.env.local`，加入:

```dotenv
CCBG_UNICOM_TOKEN_FILE=/home/walky/.config/ccbg/credentials/unicom.token
CCBG_UNICOM_COOKIE_HEADER=
```

14. 如果你确实需要连 cookie 一起配，可以把 cookie 文件内容手工粘到:

```dotenv
CCBG_UNICOM_COOKIE_HEADER=replace-with-unicom-cookie
```

### 联通云盘验证

启动服务后，可以看 provider 健康状态:

```bash
curl -s http://127.0.0.1:61080/__ccbg/providers
```

你当前应该关注:

- `backend=unicom-cloud-drive`
- `auth_source=file`

注意:

- 现在 `provider-unicom` 的健康检查主要看 token 是否已提供
- 它还不是“联通真实接口已经可用”的证明

## 中国电信天翼云盘

### 当前代码支持到什么程度

当前 `provider-telecom` 也是 scaffold:

- 已有配置结构
- 已有 token / token file / cookie header 入口
- 当前 health 检查只验证“token 是否存在”
- 真实上游接口映射还没有完成

### 电信云盘 Step by Step

1. 打开中国电信天翼云盘网页版并登录。
2. 进入文件列表页面。
3. 按 `F12`。
4. 打开 `Network`。
5. 选择 `Fetch/XHR`。
6. 刷新页面。
7. 找域名里接近 `cloud.189.cn` 的文件列表请求。
8. 点开请求详情。
9. 在 `Request Headers` 里找 `Authorization`、自定义 token 字段或 `Cookie`。
10. 如果看到 `Authorization: Bearer xxxxx`，保存 `xxxxx`:

```bash
printf '%s\n' 'replace-with-telecom-token' > /home/walky/.config/ccbg/credentials/telecom.token
chmod 600 /home/walky/.config/ccbg/credentials/telecom.token
```

11. 如果你还看到了稳定的 `Cookie`，建议也保存备用:

```bash
printf '%s\n' 'replace-with-telecom-cookie' > /home/walky/.config/ccbg/credentials/telecom.cookie
chmod 600 /home/walky/.config/ccbg/credentials/telecom.cookie
```

12. 在 `.env.local` 里加入:

```dotenv
CCBG_TELECOM_TOKEN_FILE=/home/walky/.config/ccbg/credentials/telecom.token
CCBG_TELECOM_COOKIE_HEADER=
```

13. 如果需要，也可以手工补:

```dotenv
CCBG_TELECOM_COOKIE_HEADER=replace-with-telecom-cookie
```

### 电信云盘验证

```bash
curl -s http://127.0.0.1:61080/__ccbg/providers
```

检查:

- `backend=telecom-cloud-drive`
- `auth_source=file`

同样注意:

- 当前只代表“token 已配置”
- 不代表“天翼云盘真实读写已经完成验证”

## 中国移动云盘

### 当前代码支持到什么程度

当前 `provider-mobile` 也是 scaffold:

- 已有配置结构
- 已有 token / token file / cookie header 入口
- 当前 health 检查只验证“token 是否存在”
- 真实上游接口映射还没有完成

### 中国移动云盘 Step by Step

1. 打开中国移动云盘网页版并登录。
2. 进入文件列表页面。
3. 按 `F12`。
4. 打开 `Network`。
5. 选择 `Fetch/XHR`。
6. 刷新页面。
7. 找域名里接近 `yun.139.com` 的请求。
8. 找到“列目录”或“查看文件”的请求。
9. 打开 `Headers`。
10. 找 `Authorization`、token 字段或 `Cookie`。
11. 如果拿到 `Bearer` token，只保存 token 本体:

```bash
printf '%s\n' 'replace-with-mobile-token' > /home/walky/.config/ccbg/credentials/mobile.token
chmod 600 /home/walky/.config/ccbg/credentials/mobile.token
```

12. 如果页面同时依赖 cookie，也把 cookie 记下来备用:

```bash
printf '%s\n' 'replace-with-mobile-cookie' > /home/walky/.config/ccbg/credentials/mobile.cookie
chmod 600 /home/walky/.config/ccbg/credentials/mobile.cookie
```

13. 在 `.env.local` 里加入:

```dotenv
CCBG_MOBILE_TOKEN_FILE=/home/walky/.config/ccbg/credentials/mobile.token
CCBG_MOBILE_COOKIE_HEADER=
```

14. 如果后续真实 provider 需要 cookie，再补:

```dotenv
CCBG_MOBILE_COOKIE_HEADER=replace-with-mobile-cookie
```

### 中国移动云盘验证

```bash
curl -s http://127.0.0.1:61080/__ccbg/providers
```

检查:

- `backend=mobile-cloud-drive`
- `auth_source=file`

## OneDrive

### 当前代码支持到什么程度

当前 `provider-onedrive` 比三家运营商更完整:

- 支持真实 Graph API 读写删
- 支持 `root_prefix/<bucket>/<key>` 映射
- 当前仍要求你显式提供 access token
- 还没有把“官方 OAuth 引导界面”做进当前仓库

这意味着:

- OneDrive 目前已经是可工作的后端
- 但 token 仍需要你手工提供

### OneDrive Step by Step

1. 在浏览器打开 OneDrive 网页并登录你自己的账号。
2. 确认你已经能看到文件列表。
3. 按 `F12`。
4. 打开 `Network`。
5. 选择 `Fetch/XHR`。
6. 刷新页面。
7. 找请求域名为 `graph.microsoft.com`，或者和 OneDrive 文件列表相关的请求。
8. 点开请求详情。
9. 在 `Request Headers` 里找:

- `Authorization: Bearer xxxxx`

10. 复制 `Bearer ` 后面的 token 本体。
11. 保存到本地文件:

```bash
printf '%s\n' 'replace-with-onedrive-token' > /home/walky/.config/ccbg/credentials/onedrive.token
chmod 600 /home/walky/.config/ccbg/credentials/onedrive.token
```

12. 在 `.env.local` 里加入:

```dotenv
CCBG_ONEDRIVE_ENABLED=true
CCBG_ONEDRIVE_TOKEN_FILE=/home/walky/.config/ccbg/credentials/onedrive.token
CCBG_ONEDRIVE_ROOT_PREFIX=carrier-cloud-blob-gateway
```

13. 如果你知道自己的 OneDrive drive id，也可以继续配置:

```dotenv
CCBG_ONEDRIVE_DRIVE_ID=
```

不知道就先留空。

### OneDrive 验证

```bash
curl -s http://127.0.0.1:61080/__ccbg/providers
```

你应该重点看:

- `backend=onedrive`
- `auth_source=file`
- `status=healthy` 或 `status=degraded`

OneDrive 的状态解释:

- `healthy`: token 可用，且根路径已可访问
- `degraded`: token 已读到，但根路径可能还没创建或 Graph 访问不完整
- `unavailable`: token 缺失、已过期或 Graph 请求失败

## 把认证写进 `.env.local`

下面是一个最常见的写法:

```dotenv
CCBG_PRIMARY_PROVIDER=unicom
CCBG_SYNC_TARGETS=onedrive
CCBG_FALLBACK_READ_ORDER=onedrive

CCBG_UNICOM_TOKEN_FILE=/home/walky/.config/ccbg/credentials/unicom.token
CCBG_UNICOM_COOKIE_HEADER=

CCBG_ONEDRIVE_ENABLED=true
CCBG_ONEDRIVE_TOKEN_FILE=/home/walky/.config/ccbg/credentials/onedrive.token
CCBG_ONEDRIVE_ROOT_PREFIX=carrier-cloud-blob-gateway
```

## 最后一步: 启动和检查

```bash
cd /home/walky/carrier-cloud-blob-gateway
./scripts/run-dev.sh
```

启动后检查:

```bash
curl -s http://127.0.0.1:61080/__ccbg/providers
curl -s http://127.0.0.1:61080/healthz
```

## 常见问题

### 1. 我看到了 `Authorization: Bearer ...`，到底要复制哪一部分

只复制 `Bearer ` 后面的那一长串 token 本体。

### 2. 我只看到了 Cookie，没有看到 Authorization

先把 Cookie 保存下来。

但要注意:

- 当前三家运营商 provider 还没完成真实接口接入
- 当前 health 检查仍要求至少有一个 token 输入
- 如果页面完全没有 Bearer token，只能先把你找到的最稳定认证字段记录下来，等真实 provider 接入时再确认最终需要哪一项

### 3. token 配好了，为什么还是不可用

常见原因:

- token 已经过期
- 复制时多复制了 `Bearer `
- 复制时带了换行、空格或引号
- OneDrive token 没有足够权限
- 当前运营商 provider 还没有完成真实上游 API 映射

### 4. 为什么推荐 `TOKEN_FILE`，不推荐直接写 `TOKEN`

因为:

- 更不容易泄露到 shell 历史
- 更不容易误提交到仓库
- 更容易单独做权限控制

### 5. 后面会不会做成真正的“一键登录”

计划里会做:

- OneDrive 官方授权流程
- 管理界面引导
- 更稳定的认证管理

但不会做:

- 自动窃取浏览器登录态
- 自动抓取别的程序的 session

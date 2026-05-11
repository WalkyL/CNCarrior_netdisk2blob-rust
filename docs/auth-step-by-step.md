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

补充一点:

- 对需要浏览器辅助的 provider，仓库现在会把识别出来的页面元素、流程步骤和关键请求沉淀到 `config/browser-flows/*.json`
- 这类文件是给后续 `auth-capture` / CDP 执行层消费的，不保存真实 token、cookie 或验证码

当前支持的输入方式:

- Admin Web 中按 provider 独立保存认证字段
- `CCBG_*_TOKEN`
- `CCBG_*_TOKEN_FILE`
- `CCBG_*_COOKIE_HEADER`（运营商 provider 预留字段）
- `CCBG_UNICOM_COOKIE_HEADER_FILE`
- `CCBG_TELECOM_COOKIE_HEADER_FILE`
- `CCBG_TELECOM_BROWSER_ID`
- `CCBG_TELECOM_BROWSER_ID_FILE`

最推荐的做法:

1. 先给服务配置一个独立的 `CCBG_CREDENTIALS_DIR`。
2. 再通过 Admin Web 把各 provider 的 token / cookie / OneDrive OAuth 必要字段写入各自 JSON 文件。
3. 如果后续启用 auth capture sidecar，需要在 Admin Web 的 `Auth Capture / LLM` 卡片里先填好 `Broker URL`，以及 CDP 入口配置，例如 `CDP Endpoint URL`、可选 `CDP Target Selector`、可选 `CDP Target Timeout`，再按需填写 `LLM Endpoint` / `LLM Model ID` / `LLM API Key`。
3. 只有在没有浏览器或不方便打开管理页时，再退回 `*_TOKEN_FILE`。

## 你需要准备什么

开始之前，请准备:

- 你的云盘账号已经能在浏览器里正常打开
- 一台能运行本项目的 Linux 主机
- Chrome、Edge 或其他带开发者工具的浏览器
- 一个只给自己看的 token 文件目录

推荐先创建一个专用目录:

```bash
mkdir -p $HOME/.config/ccbg/credentials
chmod 700 $HOME/.config/ccbg/credentials
```

然后在 `.env.local` 里指定:

```dotenv
CCBG_CREDENTIALS_DIR=$HOME/.config/ccbg/credentials
```

这个目录下现在会按 provider 分开落盘:

- `unicom.json`
- `telecom.json`
- `mobile.json`
- `onedrive.json`

## 最推荐的接入方式: Admin Web 独立注入

这套方式最适合不熟悉命令行、又希望后续能在网页里直接修改认证信息的用户。

### 第 1 步: 先启动网关

确认你的端口配置已经在 `60000-65534` 范围内，例如:

```dotenv
CCBG_BIND_ADDR=127.0.0.1:61080
CCBG_ADMIN_BIND_ADDR=127.0.0.1:61081
CCBG_AUTH_CALLBACK_BIND_ADDR=127.0.0.1:61082
CCBG_CREDENTIALS_DIR=$HOME/.config/ccbg/credentials
```

### 第 2 步: 打开 Admin Web

1. 在浏览器访问 `http://127.0.0.1:61081/`
2. 找到页面中的 `Provider Credentials`
3. 你会看到:

- `China Unicom`
- `China Telecom`
- `China Mobile`
- `Microsoft OneDrive`

### 第 3 步: 按 provider 粘贴认证字段

每个 provider 只改自己的卡片:

- 联通: `Access Token`、`Cookie Header`、可选 `Family ID`
- 电信: `Browser ID`、`Cookie Header`、可选 `Access Token`、可选 `Root Folder ID`
- 移动: `Access Token`、`Cookie Header`
- OneDrive: `Client ID`、`Tenant`、`Drive ID`、`Redirect URL`、可选 `Manual Access Token`

### 第 4 步: 点击 `Save Credentials`

保存后会发生 3 件事:

1. 当前 provider 的字段写入它自己的 JSON 文件。
2. 网关会立刻热重建该 provider 的 backend。
3. 新请求会马上使用新认证字段，不需要重启进程。

### 第 5 步: 用页面里的 `Test Now` 验证

保存后，回到 `Provider Health` 区域:

1. 找到对应 provider。
2. 点击 `Test Now`。
3. 看返回结果是 `healthy`、`degraded` 还是 `unavailable`。

### 第 6 步: 清空某个覆盖值

如果你想撤销网页里保存过的某个字段:

1. 把该输入框清空。
2. 再次点击 `Save Credentials`。
3. 该字段会从 provider JSON 中移除，运行时退回 env/default 值。

## 最重要的安全规则

1. 只处理你自己账号的认证信息。
2. 不要把 token 发给别人。
3. 不要把 token 贴到公开 issue、聊天记录或截图里。
4. 不要把 token 写进 Git 仓库。
5. 用完浏览器开发者工具后，可以关闭标签页并重新登录一次，降低误泄露风险。

## 浏览器里找 token 的通用方法

下面的步骤，联通、电信、移动和 OneDrive 都通用。

如果运营商网页登录过程中需要你输入手机号、短信验证码、图形验证码或其他确认信息:

- 不应该让后台流程一直卡着不动。
- 后续 auth-broker 应该把这个输入请求推到 Admin Web 的 `Pending Verification Inputs` 卡片。
- 你在网页里填完以后，再由 auth-broker 继续后续步骤。

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
chmod 700 $HOME/.config/ccbg/credentials
printf '%s\n' 'replace-with-token' > $HOME/.config/ccbg/credentials/example.token
chmod 600 $HOME/.config/ccbg/credentials/example.token
```

如果你拿到的是:

- `Authorization: Bearer abcdefg`

通常应把 `abcdefg` 写进 token 文件，不要连 `Bearer ` 一起写进去。

如果你拿到的是:

- `Cookie: foo=1; bar=2`

则把 `foo=1; bar=2` 作为 cookie 内容保存。

## 中国联通云盘

### 当前代码支持到什么程度

当前 `provider-unicom` 已经打通到联通桌面站真实 `QueryAllFiles` 流程:

- 已有 token / token file / cookie header 入口
- health 检查会真实请求 `pan.wo.cn` 桌面站实际使用的 `/wohome/dispatcher`
- 当前默认 probe 会调用真实文件 API `QueryAllFiles`
- 现在已经支持把联通云盘根目录映射成单个 S3 bucket: `root`
- 现在已经支持 `ListBuckets` / `ListObjectsV2` / `/v1/containers` / `/v1/objects`
- 现在已经支持真实下载与对象删除
- 上传和其余写入接口映射还没有完成

另外，联通桌面站当前已经有一份独立的浏览器流程样例配置:

- [config/browser-flows/unicom-web.json](/home/walky/workspaces/carrier-cloud-blob-gateway/config/browser-flows/unicom-web.json:1)

它记录了:

- 短信登录页需要操作的元素
- 个人空间上传时必须先调用的页面 JS 入口点
- 目录创建和删除时复用的 Vue 组件入口
- `upload2C`、`/wohome/dispatcher` 等关键请求的字段要求

后续如果 `pan.wo.cn` 页面小改版，优先改这份配置，而不是直接改 provider 逻辑。

这意味着:

- 你已经可以用联通 token 在本地列出 bucket、读取对象并删除对象
- 但今天还不能承诺联通云盘上传、重命名、复制、移动等 native 写路径已经完全打通

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
10. 在 `Request Headers` 里优先找:

- `Access-Token: xxxxx`
- `accesstoken: xxxxx`
- 如果你看到的是 `Access-Token`，也一并记下来

11. 对 `pan.wo.cn` 桌面站，最重要的是:

- `accesstoken`
- `Origin: https://pan.wo.cn`
- `Referer: https://pan.wo.cn/`

12. 再打开请求的 `Request Payload`，确认 URL 是:

- `https://panservice.mail.wo.cn/wohome/dispatcher`

13. 在 payload 里确认:

- `header.channel` 通常是 `wohome`
- `body.secret` 或 `body.key` 至少有一个为 `true`

14. 这个桌面站在前端启动时，默认 `clientId` 是 `1001000021`。如果你在请求 payload 解密前后、URL 参数、页面初始化参数里能看到 `clientId`，优先保留原值；如果没有看到，当前项目默认按 `1001000021` 处理。

15. 把 token 值保存到:

```bash
printf '%s\n' 'replace-with-unicom-token' > $HOME/.config/ccbg/credentials/unicom.token
chmod 600 $HOME/.config/ccbg/credentials/unicom.token
```

16. 如果请求里还有 `Cookie`，也建议一并保存，供后续真实 provider 接入时备用:

```bash
printf '%s\n' 'replace-with-unicom-cookie' > $HOME/.config/ccbg/credentials/unicom.cookie
chmod 600 $HOME/.config/ccbg/credentials/unicom.cookie
```

17. 打开你的 `.env.local`，加入:

```dotenv
CCBG_UNICOM_TOKEN_FILE=$HOME/.config/ccbg/credentials/unicom.token
CCBG_UNICOM_COOKIE_HEADER=
CCBG_UNICOM_COOKIE_HEADER_FILE=
CCBG_UNICOM_REQUEST_ORIGIN=https://pan.wo.cn
CCBG_UNICOM_REQUEST_REFERER=https://pan.wo.cn/
CCBG_UNICOM_DISPATCHER_CLIENT_ID=1001000021
CCBG_UNICOM_DISPATCHER_CHANNEL=wohome
CCBG_UNICOM_AUTH_PROBE_STYLE=wohome-secret
CCBG_UNICOM_AUTH_PROBE_OPERATION=QueryAllFiles
CCBG_UNICOM_AUTH_PROBE_BODY_JSON={"spaceType":"0","parentDirectoryId":"0","pageNum":0,"pageSize":50,"sortRule":0}
```

如果你更想走网页方式，也可以不写 `CCBG_UNICOM_TOKEN_FILE`，而是在 `Provider Credentials -> China Unicom` 卡片里把 token 和 cookie 直接粘进去。保存后会写入 `CCBG_CREDENTIALS_DIR/unicom.json`，并立即热生效。

如果你同时有联通家庭云，并且希望控制面也把家庭空间探测出来，可以额外填写:

- `Family ID (Optional)`

如果你不确定 `Family ID`，可以先留空。当前版本会尝试通过 `QueryFamilyGroups` 自动发现；自动发现失败时，再手工补这个字段。

18. 如果你确实需要连 cookie 一起配，可以把 cookie 文件内容手工粘到:

```dotenv
CCBG_UNICOM_COOKIE_HEADER=replace-with-unicom-cookie
```

或者更推荐直接指向 cookie 文件:

```dotenv
CCBG_UNICOM_COOKIE_HEADER_FILE=$HOME/.config/ccbg/credentials/unicom.cookie
```

### 联通云盘验证

启动服务后，可以看 provider 健康状态:

```bash
curl -s http://127.0.0.1:61080/__ccbg/providers
```

你当前应该关注:

- `backend=unicom-cloud-drive`
- `auth_source=file`
- `auth_probe=QueryAllFiles`
- `auth_probe_style=wohome-secret`
- `auth_probe_status=accepted`

注意:

- 现在 `provider-unicom` 的健康检查已经会真实请求联通桌面站 `pan.wo.cn` 的 `/wohome/dispatcher`
- 它还不是“联通真实接口已经可用”的证明

如果 health 仍然报 `RSP_CODE=9999`，下一份最有价值的材料是:

- 该请求的 `Request Payload`
- 该请求的 `Response`
- 如果浏览器还带了 `Cookie`，把 cookie 一并提供

如果页面提示:

- `missing environment variable: CCBG_UNICOM_TOKEN`

说明你还没有把联通 token 真正保存进去。最简单的修复方法就是打开 `Provider Credentials -> China Unicom`，把 `Access Token` 粘进去后点击 `Save Credentials`。

如果页面提示:

- `HTTP 401`
- `RSP_CODE=9999`

通常说明联通 token 已经过期，或者当前浏览器头信息和你复制出来的 token 已经不匹配。此时应该:

1. 重新打开 `https://pan.wo.cn/`
2. 刷到能看到文件列表
3. 重新抓一条 `QueryAllFiles` 请求
4. 复制新的 `accesstoken`
5. 再次保存到 `China Unicom` 卡片

## 中国电信天翼云盘

### 当前代码支持到什么程度

当前 `provider-telecom` 已经打通到天翼云盘网页版真实文件列表和下载直链流程:

- 已支持 `Browser ID`、`Cookie Header`、可选 `Access Token`
- 已支持 health 真实请求 `listFiles.action`
- 已支持把天翼云盘根目录映射成单个 S3 bucket: `root`
- 已支持 `ListBuckets` / `ListObjectsV2` / `head_object` / `get_object`
- 当前仍然是只读 provider，还没有完成写入、删除和上传

### 电信云盘 Step by Step

1. 打开中国电信天翼云盘网页版并登录。
2. 进入文件列表页面。
3. 按 `F12`。
4. 打开 `Network`。
5. 选择 `Fetch/XHR`。
6. 刷新页面。
7. 找域名接近 `cloud.189.cn`，并且路径像下面这样的文件列表请求:

- `/api/open/file/listFiles.action`

8. 点开这条请求。
9. 先看 `Request Headers`，把下面两项记下来:

- `Browser-Id`
- `Cookie`

10. `Browser-Id` 一般长得像一串很长的十六进制或指纹值，例如:

- `f4e5af5fe716a785d4a7277eb2c11eea`

11. 建议先把 `Browser-Id` 单独保存到文件:

```bash
printf '%s\n' 'replace-with-telecom-browser-id' > $HOME/.config/ccbg/credentials/telecom.browser_id
chmod 600 $HOME/.config/ccbg/credentials/telecom.browser_id
```

12. 再把完整 `Cookie` 请求头值保存到文件:

```bash
printf '%s\n' 'replace-with-telecom-cookie' > $HOME/.config/ccbg/credentials/telecom.cookie
chmod 600 $HOME/.config/ccbg/credentials/telecom.cookie
```

13. 如果你还想把网页里可能用到的 `AccessToken` 一并留作兼容备用，可以继续搜:

- `AccessToken`
- `accessToken`
- `getAccessToken`

如果你确实拿到了稳定的 `AccessToken`，再把它额外保存:

```bash
printf '%s\n' 'replace-with-telecom-token' > $HOME/.config/ccbg/credentials/telecom.token
chmod 600 $HOME/.config/ccbg/credentials/telecom.token
```

14. 在 `.env.local` 里加入:

```dotenv
CCBG_TELECOM_BROWSER_ID_FILE=$HOME/.config/ccbg/credentials/telecom.browser_id
CCBG_TELECOM_COOKIE_HEADER_FILE=$HOME/.config/ccbg/credentials/telecom.cookie
CCBG_TELECOM_TOKEN_FILE=$HOME/.config/ccbg/credentials/telecom.token
```

如果你没有拿到 `AccessToken`，也可以先不配 `CCBG_TELECOM_TOKEN_FILE`。当前版本主流程以 `Browser ID + Cookie` 为主，`Access Token` 主要作为“上游未来收紧时的兼容备用”。

15. 如果你更想走网页方式，也可以直接打开 `Provider Credentials -> China Telecom`，把:

- `Browser ID`
- `Cookie Header`
- 可选 `Access Token (Optional)`
- 可选 `Root Folder ID`

粘进去。保存后会写入 `CCBG_CREDENTIALS_DIR/telecom.json`，并立即热生效。

如果你只是接个人云，`Root Folder ID` 通常保持默认 `-11` 即可，不需要改。

16. 如果你不想用 `*_FILE`，也可以直接手工写环境变量:

```dotenv
CCBG_TELECOM_COOKIE_HEADER=replace-with-telecom-cookie
CCBG_TELECOM_BROWSER_ID=replace-with-telecom-browser-id
CCBG_TELECOM_TOKEN=replace-with-optional-telecom-token
```

### 电信云盘验证

最简单的验证方式是打开 Admin Web，找到 `China Telecom`，点击 `Test Now`。

如果你更习惯命令行，也可以执行:

```bash
curl -s http://127.0.0.1:61080/__ccbg/providers
```

检查:

- `backend=telecom-cloud-drive`
- `status=healthy`
- 能看到 `root_entry_count=...` 这类真实目录探测信息

同样注意:

- 当前版本已经完成真实读取链路验证
- 当前还不代表“写入、删除、上传”已经完成

如果页面提示:

- `InvalidSessionKey`
- `cookieUserSession is null or invalid`

这几乎总是说明电信网页登录态过期了。处理方法是:

1. 在同一个浏览器里重新打开 `https://cloud.189.cn/`
2. 确认已经回到文件列表页
3. 刷新页面
4. 重新抓最新的 `Browser-Id`
5. 重新抓最新的 `Cookie`
6. 回到 Admin Web，把 `China Telecom` 卡片里的 `Browser ID` 和 `Cookie Header` 全部替换后保存

如果你是在 IPv4 浏览器会话里抓到这些字段，而网关所在主机同时有 IPv4 和 IPv6，建议把:

```dotenv
CCBG_TELECOM_IP_FAMILY=ipv4
```

固定下来，避免网页会话因为出口协议不同而失效。

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
printf '%s\n' 'replace-with-mobile-token' > $HOME/.config/ccbg/credentials/mobile.token
chmod 600 $HOME/.config/ccbg/credentials/mobile.token
```

12. 如果页面同时依赖 cookie，也把 cookie 记下来备用:

```bash
printf '%s\n' 'replace-with-mobile-cookie' > $HOME/.config/ccbg/credentials/mobile.cookie
chmod 600 $HOME/.config/ccbg/credentials/mobile.cookie
```

13. 在 `.env.local` 里加入:

```dotenv
CCBG_MOBILE_TOKEN_FILE=$HOME/.config/ccbg/credentials/mobile.token
CCBG_MOBILE_COOKIE_HEADER=
```

如果你更想走网页方式，也可以直接打开 `Provider Credentials -> China Mobile`，把 token / cookie 粘进去。保存后会写入 `CCBG_CREDENTIALS_DIR/mobile.json`，并立即热生效。

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
- 已内置官方 OAuth 引导
- 已支持把 OAuth session 持久化到本地文件
- 当 session 文件里包含 `refresh_token` 时，provider 会自动续期 access token

这意味着:

- OneDrive 目前已经是可工作的后端
- 已经可以直接用浏览器或终端完成授权
- 旧的“手工贴 access token”方式仍可作为兜底

### OneDrive Step by Step

#### 方式 A: 内置网页授权

1. 先在 `.env.local` 或 systemd `runtime.env` 里配置:

```dotenv
CCBG_ADMIN_MODE=web
CCBG_ADMIN_BIND_ADDR=127.0.0.1:61081
CCBG_AUTH_CALLBACK_BIND_ADDR=127.0.0.1:61082

CCBG_ONEDRIVE_ENABLED=true
CCBG_ONEDRIVE_CLIENT_ID=replace-with-your-entra-app-client-id
CCBG_ONEDRIVE_REDIRECT_URL=http://127.0.0.1:61082/auth/onedrive/callback
CCBG_ONEDRIVE_AUTH_BASE_URL=https://login.microsoftonline.com
CCBG_ONEDRIVE_SCOPES=offline_access Files.ReadWrite User.Read openid profile
CCBG_ONEDRIVE_SESSION_FILE=$HOME/.config/ccbg/credentials/onedrive-session.json
CCBG_ONEDRIVE_ROOT_PREFIX=carrier-cloud-blob-gateway
```

如果你不想手改环境变量，也可以在 `Provider Credentials -> Microsoft OneDrive` 卡片里填写 `Client ID`、`Tenant`、`Drive ID`、`Redirect URL`。这些字段会写入 `CCBG_CREDENTIALS_DIR/onedrive.json`，新授权流程会立即读取它们。

2. 创建 session 文件目录:

```bash
mkdir -p $HOME/.config/ccbg/credentials
```

3. 启动服务后，打开管理界面:

```text
http://127.0.0.1:61081/
```

4. 点击 `Connect OneDrive In Browser`。
5. 按微软官方页面提示登录并授权。
6. 浏览器会跳到 `61082` 上的回调页。
7. 如果页面显示 `OneDrive Connected`，说明 session 已写入本地文件。
8. 这个 session 文件里会保存:

- `access_token`
- `refresh_token`
- `expires_at_unix`

9. 后续 `provider-onedrive` 会优先读取这个 session 文件，并在 access token 快过期时自动刷新。

#### 方式 B: 终端 / SSH Device Code 授权

1. 先保留上面的 OneDrive 基础配置，但你可以把默认模式改成:

```dotenv
CCBG_ADMIN_MODE=terminal
CCBG_ONEDRIVE_USE_DEVICE_CODE=true
```

2. 启动服务后，在 SSH 终端执行:

```bash
curl -s -X POST http://127.0.0.1:61081/api/auth/onedrive/device/start | jq
```

3. 你会拿到几项关键信息:

- `flow_id`
- `user_code`
- `verification_uri`
- `message`

4. 按输出里的 `verification_uri` 打开浏览器。
5. 输入 `user_code`。
6. 登录并同意授权。
7. 回到终端轮询状态:

```bash
curl -s http://127.0.0.1:61081/api/auth/onedrive/device/<flow_id> | jq
```

8. 当 `status` 变成 `completed`，说明 session 已写入 `CCBG_ONEDRIVE_SESSION_FILE`。

#### 方式 C: 旧的手工 token 兜底

如果你的环境暂时还没准备好微软 OAuth 应用，也可以继续手工放 token:

```bash
printf '%s\n' 'replace-with-onedrive-token' > $HOME/.config/ccbg/credentials/onedrive.token
chmod 600 $HOME/.config/ccbg/credentials/onedrive.token
```

然后配置:

```dotenv
CCBG_ONEDRIVE_TOKEN_FILE=$HOME/.config/ccbg/credentials/onedrive.token
```

这种方式能跑，但不能像 session 文件那样自动刷新。

#### 可选项

如果你知道自己的 OneDrive `drive id`，也可以继续配置:

```dotenv
CCBG_ONEDRIVE_DRIVE_ID=
```

不知道就先留空，默认走 `me/drive`。

### OneDrive 验证

```bash
curl -s http://127.0.0.1:61080/__ccbg/providers
```

你应该重点看:

- `backend=onedrive`
- `auth_source=file` 或 `auth_source=env`
- `status=healthy` 或 `status=degraded`

OneDrive 的状态解释:

- `healthy`: session / token 可用，且根路径已可访问
- `degraded`: 授权已完成，但根路径还没创建，或 Graph 访问只部分可用
- `unavailable`: session / token 缺失、已过期且刷新失败，或 Graph 请求失败

## 把认证写进 `.env.local`

下面是一个最常见的写法:

```dotenv
CCBG_CONTROL_PLANE_FILE=$HOME/.config/ccbg/control-plane.json

CCBG_PRIMARY_PROVIDER=unicom
CCBG_SYNC_TARGETS=onedrive
CCBG_FALLBACK_READ_ORDER=onedrive

CCBG_UNICOM_TOKEN_FILE=$HOME/.config/ccbg/credentials/unicom.token
CCBG_UNICOM_COOKIE_HEADER=

CCBG_ONEDRIVE_ENABLED=true
CCBG_ONEDRIVE_CLIENT_ID=replace-with-your-entra-app-client-id
CCBG_ONEDRIVE_SESSION_FILE=$HOME/.config/ccbg/credentials/onedrive-session.json
CCBG_ONEDRIVE_ROOT_PREFIX=carrier-cloud-blob-gateway
CCBG_ONEDRIVE_REPLICATION_ENABLED=true
CCBG_ONEDRIVE_FALLBACK_ENABLED=true
CCBG_ONEDRIVE_POLICY_MODE=memory_only
CCBG_ONEDRIVE_MEMORY_BUCKETS=agent-memory
CCBG_ONEDRIVE_MEMORY_PREFIXES=memory/,sessions/
```

## 在网页里控制主写和 OneDrive 范围

打开:

```text
http://127.0.0.1:61081/
```

你现在会看到两类控制项:

- `Saved Primary / Sync Topology`
- `OneDrive Backup Scope`

它们的语义不一样:

1. `Saved Primary / Sync Topology`
- 这里可以指定哪个云盘是唯一主写。
- 也可以指定哪些云盘参与异步同步，以及 fallback 顺序。
- 这部分会写入 `CCBG_CONTROL_PLANE_FILE`。
- 热切换版本中，这部分保存后会立即影响新的读写请求。
- 已经入队的旧复制 job 仍然继续绑定它们创建时的旧源 provider。

2. `OneDrive Backup Scope`
- 这里可以直接控制是否把新对象异步复制到 OneDrive。
- 这里也可以控制是否允许从 OneDrive 做 fallback 读取。

3. `Auth Capture / LLM`
- 这里可以配置 auth-broker 的地址。
- 如果你准备让 sidecar 调用模型分析网页流程，也在这里配置 `LLM Endpoint`、`LLM Model ID` 和可选 `LLM API Key`。
- 如果 sidecar 在登录运营商云盘时需要手机号、短信码或验证码，输入框会出现在 `Pending Verification Inputs` 区域，而不是静默挂住。
- `scope_mode=all` 代表所有对象都可参与。
- `scope_mode=memory_only` 代表只允许你指定的 memory bucket 或 memory prefix 参与。
- 这部分保存后会立即影响后续新写入和新的 fallback 判定。

如果你只想把 Hermes / OpenClaw 的记忆部分备份到 OneDrive，推荐这样配:

- `scope_mode=memory_only`
- `memory_buckets=<你自己定义的记忆 bucket 名称>`
- 或 `memory_prefixes=<你自己定义的记忆前缀>`

项目不会规定默认 memory bucket 名称，bucket 和 prefix 由用户自己决定。

## 最后一步: 启动和检查

```bash
cd /path/to/carrier-cloud-blob-gateway
./scripts/run-dev.sh
```

启动后检查:

```bash
curl -s http://127.0.0.1:61080/__ccbg/providers
curl -s http://127.0.0.1:61080/healthz
```

## 常见问题

### 1. 我到底应该优先用哪种 OneDrive 方式

优先顺序建议是:

- 内置网页授权
- Device Code 授权
- 最后才是手工 access token

原因:

- 前两种能获得 `refresh_token`
- 能自动续期
- 更适合长期跑异步备份

### 2. Device Code 已授权，为什么还要轮询状态

因为服务端需要等微软 token endpoint 返回最终 token，然后把 session 落到本地文件。

只有 `status=completed` 才算真正写入成功。

### 3. token / session 配好了，为什么还是不可用

常见原因:

- `CCBG_ONEDRIVE_CLIENT_ID` 配错
- `CCBG_ONEDRIVE_REDIRECT_URL` 和微软应用注册里不一致
- session 文件目录没有写权限
- 你的 token / session 没有 `Files.ReadWrite` 一类的 Graph 权限
- 旧的 access token 已过期，且没有 `refresh_token`

### 4. 为什么推荐 `TOKEN_FILE`，不推荐直接写 `TOKEN`

因为:

- 更不容易泄露到 shell 历史
- 更不容易误提交到仓库
- 更容易单独做权限控制

### 5. 现在还需不需要手工抓浏览器里的 `Authorization: Bearer ...`

大多数情况下已经不需要。

只有在下面这些情况，才建议你临时手工抓 token:

- 你还没创建自己的微软 OAuth 应用
- 你只想做一次性快速联调
- 你所在环境暂时不能开放回调端口，也不方便做 Device Code

无论如何，项目都不会做:

- 自动窃取浏览器登录态
- 自动抓取别的程序 session

### 6. 为什么主写云盘切换不是立刻生效

热切换版本里，复制队列中的每个 job 都会单独记录“创建当时的源 provider”。

这意味着:

- 新的写入会立即走新主写
- 旧的 pending job 继续从旧主写读取
- 热切换不会把旧 job 串到新主写后端

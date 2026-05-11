# Provider 差异矩阵

## 当前目标 provider

| Provider | 产品 | 角色 | 默认入口 | 认证方式 | 当前状态 |
| --- | --- | --- | --- | --- | --- |
| `unicom` | 中国联通云盘 | 主写候选 / 同步候选 | `https://panservice.mail.wo.cn` | 手工 token / cookie 注入 | 已打通桌面站 `QueryAllFiles` 认证、对象列举与真实下载；已探测 personal scope，并可通过 `familyId` 或 `QueryFamilyGroups` 自动发现 family scope；写入待补 |
| `telecom` | 天翼云盘 | 主写候选 / 同步候选 | `https://cloud.189.cn` | 手工 `Browser ID` / cookie 注入，可选 token 兼容备用 | 已打通 `listFiles.action`、对象列举与真实下载；已探测 personal scope，并通过 `getUserInfoForPortal.action` 拉取容量；当前只读，写入待补 |
| `mobile` | 中国移动云盘 | 主写候选 / 同步候选 | `https://yun.139.com` | 手工 token / cookie 注入 | 已建立适配骨架，待确认实际网页接口 |
| `onedrive` | Microsoft OneDrive | 默认备份同步目标 / 可选 fallback | Microsoft Graph / OneDrive API | 内置 Web PKCE / Device Code / 手工 token 兜底 | 已支持最小 Graph 读写删、session 文件落盘与自动 refresh token 续期 |

## 接入策略

### 共同点

- 运营商 provider 先按网页会话方式建模
- 运营商 provider 通过显式 token 或本地 token 文件注入
- OneDrive 走官方授权流
- 同一时刻只允许一个运营商 provider 作为主写
- 其他 provider 可按配置作为 sync targets
- 先做只读能力，再做写入和分片上传
- 都要保留超时、分页、重试配置

### 可能差异

- 认证头字段名称可能不同
- Cookie 依赖程度可能不同
- 目录与文件元数据结构可能不同
- 下载接口可能存在防盗链、风控或签名参数
- 个人空间 / 家庭空间 / 共享空间 的入口与容量接口可能不同
- 网页会话可能与 IPv4 / IPv6 出口绑定
- 上传流程可能使用预签名 URL、分片 ticket 或独立上传域名
- OneDrive 侧对象 ID、目录路径和 delta/sync 语义与运营商 provider 明显不同

## 控制面要求

- 每个 provider 的认证必要项必须单独存放，并能通过 Admin Web 修改注入。
- Admin Web 必须能展示 provider 的验证输入队列:
  - 手机号
  - 短信验证码
  - 图形验证码
  - 其他网页登录步骤
- provider 必须支持独立的出站 IP family:
  - `auto`
  - `ipv4`
  - `ipv6`
- 当前 Web 控制面已把 provider 返回的 `storage scopes` 直接渲染成卡片视图，展示:
  - `personal | family | shared | unknown`
  - 可写性
  - root/container 映射
  - 根层对象计数
  - 总容量 / 已用 / 剩余
- 仍未确认真实容量接口的 scope 必须明确显示为 `unknown`，不能伪造数值。

## 建议的适配顺序

1. 先完成 OneDrive provider，因为它决定默认备份同步和可选 fallback 的底座。
2. 再完成你当前最容易验证的一家运营商，建议联通，作为首个 primary provider。
3. 抽取公共模型后，再接入第二家，作为可选 sync target。
4. 第三家放在公共上传逻辑和错误模型稳定之后再接。

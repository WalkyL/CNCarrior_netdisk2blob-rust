# Provider 差异矩阵

## 当前目标 provider

| Provider | 产品 | 角色 | 默认入口 | 认证方式 | 当前状态 |
| --- | --- | --- | --- | --- | --- |
| `unicom` | 中国联通云盘 | 主写候选 / 同步候选 | `https://panservice.mail.wo.cn` | 手工 token / cookie 注入 | 已打通桌面站 `QueryAllFiles` 认证、对象列举、真实下载、`upload2C` 上传，以及 native `CreateDirectory` / `DeleteFile` / `RenameFileOrDirectory` / `CopyFile` / `MoveFile`；已探测 personal scope，并可通过 `familyId` 或 `QueryFamilyGroups` 自动发现 family scope，当前映射成 `root` / `family` 两个容器；浏览器流程配置仍覆盖短信登录、上传上下文与页面动作事实采集 |
| `telecom` | 天翼云盘 | 主写候选 / 同步候选 | `https://cloud.189.cn` | 手工 `Browser ID` / cookie 注入；家庭云需要 `Access Token` + `Family ID` | 已打通 `listFiles.action`、对象列举、真实下载、受控根目录 multipart 原生上传，以及 personal/family 回收站软删除；已探测 personal scope，并通过 `getUserInfoForPortal.action` 拉取容量；配置 `family_id` 后映射 `family` 容器并支持列举/读取/上传/删除；对象动作已支持同 scope 内 copy/move 与同父目录 rename，跨 personal/family copy/move 与跨父目录 rename 仍明确拒绝 |
| `mobile` | 中国移动云盘 | 主写候选 / 同步候选 | `https://yun.139.com` | 手工 token / cookie 注入 | 已打通 `file/list`、`file/create`、`file/complete`、`file/getDownloadUrl`，支持 `root_prefix/<bucket>/<key>` 托管目录映射下的对象列举、真实上传与真实下载；上传链路已按上游约束改成“`file/create` 首批最多 100 个 `partInfos`，其余分片通过 `file/getUploadUrl` 补齐”；同时已兼容上游返回 `exist=true` 且 `uploadId` 缺失的同名对象覆盖分支，会先删除受控根目录下的同名旧对象并短暂重试 `file/create`；但 `.49` 上 2026-06-05 的 16 GiB 实测仍被上游以 `code=04010319 / Insufficient Rights` 拒绝，因此当前不能宣称中国移动已经验证通过超大文件上传；`getFamilyDiskInfo` 只作为 family 容量事实来源，只有同时配置/捕获真实 `family_root_folder_id` 且该根可列举时才暴露稳定 `family` 容器；对象动作已支持 native `delete/rename/move`，并支持 capability-gated native `copy`（当前仅同 scope 根且同名复制；跨 scope 或 copy+rename 明确返回 `NotImplemented`） |
| `onedrive` | Microsoft OneDrive | Parking / 延后集成 | Microsoft Graph / OneDrive API | 内置 Web PKCE / Device Code / 手工 token 兜底 | 已有实验性 Graph 读写与 OAuth 会话实现，但当前阶段默认禁用并隐藏；不作为默认异步备份、默认 fallback 或近期 provider completion 目标，等出现真实用户需求后再恢复评估 |

## 接入策略

### 共同点

- 运营商 provider 先按网页会话方式建模
- 运营商 provider 通过显式 token 或本地 token 文件注入
- OneDrive 仅在后续真实需求触发时再恢复官方授权流集成
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
- 若后续恢复 OneDrive，需重新评估对象 ID、目录路径和 delta/sync 语义与运营商 provider 的差异
- 网页执行层需要保留一份可更新的 selector / JS 入口点 / 请求形状配置，而不是把这些细节固定在代码里

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

1. 先完成你当前最容易验证的一家运营商，建议联通，作为首个 primary provider。
2. 抽取公共模型后，再接入第二家，作为可选 sync target。
3. 第三家放在公共上传逻辑和错误模型稳定之后再接。
4. OneDrive 放入 Parking，等真实用户需求明确后再按单独恢复清单处理。

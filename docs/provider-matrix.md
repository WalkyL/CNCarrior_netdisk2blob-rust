# Provider 差异矩阵

## 当前目标 provider

| Provider | 产品 | 角色 | 默认入口 | 认证方式 | 当前状态 |
| --- | --- | --- | --- | --- | --- |
| `unicom` | 中国联通云盘 | 主写候选 / 同步候选 | `https://panservice.mail.wo.cn` | 手工 token / cookie 注入 | 已建立适配骨架，待确认实际网页接口 |
| `telecom` | 天翼云盘 | 主写候选 / 同步候选 | `https://cloud.189.cn` | 手工 token / cookie 注入 | 已建立适配骨架，待确认实际网页接口 |
| `mobile` | 中国移动云盘 | 主写候选 / 同步候选 | `https://yun.139.com` | 手工 token / cookie 注入 | 已建立适配骨架，待确认实际网页接口 |
| `onedrive` | Microsoft OneDrive | 默认备份同步目标 / 最终 fallback | Microsoft Graph / OneDrive API | 显式 access token 注入，后续补 PKCE / Device Code | 已支持最小 Graph 读写删与 bucket-folder 映射，OAuth broker 待补 |

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
- 上传流程可能使用预签名 URL、分片 ticket 或独立上传域名
- OneDrive 侧对象 ID、目录路径和 delta/sync 语义与运营商 provider 明显不同

## 建议的适配顺序

1. 先完成 OneDrive provider，因为它决定默认备份同步和最终 fallback 的底座。
2. 再完成你当前最容易验证的一家运营商，建议联通，作为首个 primary provider。
3. 抽取公共模型后，再接入第二家，作为可选 sync target。
4. 第三家放在公共上传逻辑和错误模型稳定之后再接。

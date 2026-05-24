# S3 兼容规划

## 目标

数据面正式目标是兼容 S3 API，使 Hermes Agent、Open Claw Agent 以及其他使用现成 S3 SDK 的 Agent 可以直接接入，而不需要理解运营商云盘的差异。

## 兼容边界

这是“本地 S3 兼容对象网关”，不是完整 AWS 控制面复刻。

一期目标:

- 自定义 endpoint
- SigV4 本地鉴权
- path-style bucket 访问
- 常用对象读写接口

明确不在一期承诺:

- IAM
- ACL
- STS
- Bucket Policy
- Event Notification
- Object Lock
- SSE-KMS
- Website Hosting
- 完整 AWS 错误码矩阵

## 一期 S3 子集

### Bucket 级

- `ListBuckets`
- `HeadBucket`
- `ListObjectsV2`

### Object 级

- `HeadObject`
- `GetObject`
- `PutObject`
- `DeleteObject`

### 后续阶段

- `Multipart Upload`
- `CopyObject`
- `Presigned URL`
- 更完整的 `Range GET`

## 地址风格

一期默认只保证 `path-style`:

```text
http://127.0.0.1:61080/<bucket>/<key>
```

原因:

- 对本地 endpoint 兼容性更稳定
- 不依赖额外 DNS
- 对 Agent、容器和软路由部署更简单

`virtual-hosted-style` 可留到后续阶段。

## 认证模型

### 本地 S3 认证

网关本地维护一套独立的 S3 access key / secret key，用来给 Agent 做 SigV4 鉴权。

这套凭据:

- 不等于运营商云盘凭据
- 不等于 OneDrive 授权凭据
- 只用于 Agent 与本地网关之间

### 上游认证

- 运营商云盘继续使用 token / cookie
- OneDrive 继续使用 OAuth

## Bucket 映射策略

### 一期默认策略

S3 bucket 是本地逻辑命名空间，不直接等价于上游 provider 的原生 bucket 概念。

建议一期采用:

- bucket 绑定一个默认 `primary provider`
- bucket 继承全局 `sync targets`
- 后续再支持 per-bucket 覆盖

## 元数据映射

需要注意:

- `ETag` 不保证等于上游源文件的 MD5
- `Last-Modified` 可能来自网关统一元数据层
- multipart 场景下 `ETag` 更不应被假定为 MD5

## 错误模型

数据面应尽量返回 S3 风格错误响应，特别是常见场景:

- `NoSuchBucket`
- `NoSuchKey`
- `AccessDenied`
- `InvalidAccessKeyId`
- `SignatureDoesNotMatch`
- `InternalError`

## Fallback 读取

当前读侧 fallback 只覆盖已实现的一期 S3 读接口:

- `ListBuckets`
- `HeadBucket`
- `ListObjectsV2`
- `HeadObject`
- `GetObject`

当前行为:

- 先读取 primary provider
- 若 primary 读取失败，则按 `fallback_read_order` 顺序尝试已配置的 sync target
- 若 fallback 成功，响应头会带 `x-ccbg-source-provider`
- 若实际命中了备份侧，响应头还会带 `x-ccbg-fallback-from`

这意味着:

- fallback 是“尽力读取”，不是复制完成保证
- 当对象尚未异步复制完成时，fallback 仍可能返回 `NoSuchKey`
- 当 primary 删除已完成但备份侧删除仍在异步传播时，fallback 可能短暂读到旧对象
- Agent 应读取响应头，而不是假设所有成功响应都来自 primary

## 与控制面的边界

S3 数据面只负责:

- bucket / object 操作
- 本地 S3 鉴权
- 对象读写语义

控制面继续走独立接口处理:

- OneDrive 授权
- provider 健康状态
- primary provider 与 sync targets 配置
- 告警
- 复制任务管理

## 对 Agent 的价值

S3 兼容带来的直接好处:

1. Agent 可直接复用成熟 S3 SDK。
2. 现成工具链更容易接入。
3. STM32 和 OpenWRT 侧更容易只实现最小客户端逻辑。
4. MCP 和 Skill 可以围绕更稳定的 bucket/object 语义封装。

## 实施顺序建议

1. 先落最小 S3 子集，不做自定义数据面继续扩张。
2. 优先支持 `ListObjectsV2`、`GetObject`、`PutObject`。
3. 完成本地 SigV4 校验。
4. 完成 bucket 到 provider policy 的映射。
5. 再补 multipart upload。

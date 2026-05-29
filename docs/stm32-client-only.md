# STM32 Client-Only 集成说明

STM32 一期定位是 CCBG 的客户端，不是宿主。它只调用附近 Linux/OpenWRT 网关暴露的本地 S3 数据面。

示例代码在 [examples/stm32-client-only](../examples/stm32-client-only/README.md)。

## 能力边界

支持:

- `HeadObject`
- `GetObject`
- `PutObject`
- 固定大小 chunk 读写
- SigV4 header 认证
- 有界 timeout / retry

不支持:

- 在 STM32 上运行 `gatewayd`
- SQLite 元数据
- OneDrive OAuth / Graph
- provider 凭证存储
- 异步复制 worker
- Admin Web
- MCP stdio

## 推荐配置

网关侧给 STM32 单独分配 S3 key，不复用管理员 key:

```dotenv
CCBG_BIND_ADDR=0.0.0.0:61080
CCBG_S3_ACCESS_KEY_ID=ccbg-stm32
CCBG_S3_SECRET_ACCESS_KEY=<board-specific-secret>
CCBG_PRIMARY_PROVIDER=stub
CCBG_ONEDRIVE_ENABLED=false
CCBG_ONEDRIVE_REPLICATION_ENABLED=false
```

STM32 固件侧:

- endpoint: `http://<gateway-lan-ip>:61080`
- region: `us-east-1`
- bucket: `root`
- max object: 默认 `32 KiB`
- IO chunk: 默认 `1024` bytes
- concurrency: `1`
- retry: `1-2` 次，超时后返回上层业务

## 签名模型

示例使用 `AWS4-HMAC-SHA256` header 认证，并固定签名头集合:

```text
host;x-amz-content-sha256;x-amz-date
```

`PutObject` 使用:

```text
x-amz-content-sha256: UNSIGNED-PAYLOAD
```

这样 STM32 不需要先把整个对象读入内存再计算 body hash；HTTP transport 只要能按 `Content-Length` 分块发送即可。

## 移植点

需要由板级工程提供:

- `sha256_hex`
- `hmac_sha256`
- `utc_now`
- `http_request`

mbedTLS 移植建议:

- `sha256_hex` 使用 `mbedtls_sha256`
- `hmac_sha256` 使用 `mbedtls_md_hmac`
- `utc_now` 使用 RTC / SNTP 后的 UTC 时间
- `http_request` 使用 LwIP、cellular modem SDK 或板级 HTTP client

## 验收方法

本仓库 host 侧语法检查:

```bash
scripts/check-stm32-client-example.sh
```

板级联调:

1. 网关启动并确保 `GET /healthz` 可访问。
2. 在 `root/stm32/demo.txt` 准备一个小对象。
3. 固件调用 `HeadObject`，期望 HTTP `200`。
4. 固件调用 `GetObject`，分块写入 caller-provided sink。
5. 固件调用 streaming `PutObject`，对象大小不超过 `32 KiB`。
6. 用桌面 S3 客户端或网关日志确认对象可读。

通过标准:

- 三类请求均成功
- 单次对象缓冲不超过 `CCBG_STM32_IO_CHUNK_BYTES`
- 超时和重试不会无限阻塞主循环
- 固件内不保存运营商 provider 凭证或控制面 API key

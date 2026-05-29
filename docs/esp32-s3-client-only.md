# ESP32-S3 Client-Only 集成说明

ESP32-S3 一期定位和 STM32 一致：它是 CCBG 的客户端，不是网关宿主。

示例代码在 [examples/esp32-s3-client-only](../examples/esp32-s3-client-only/README.md)。

## 能力

示例基于 ESP-IDF:

- mbedTLS: SHA256 / HMAC-SHA256
- `esp_http_client`: HTTP transport
- 复用 portable C client: `examples/stm32-client-only/ccbg_stm32_client.c`
- `PutObject`
- `HeadObject`
- `GetObject`
- `UNSIGNED-PAYLOAD` streaming upload

默认资源边界:

- `CCBG_ESP32S3_IO_CHUNK_BYTES=1024`
- `CCBG_ESP32S3_MAX_OBJECT_BYTES=32 KiB`
- 单并发
- 有界 timeout / retry

## 不做事项

ESP32-S3 client-only 示例不包含:

- `gatewayd`
- SQLite
- replication engine
- OneDrive
- provider credentials
- Admin Web
- MCP stdio

需要 MCP 能力时，推荐让 Linux/OpenWRT host 或 Agent 侧使用 MCP；ESP32-S3 只走本地 S3 数据面。

## 验收

Host 侧结构检查:

```bash
scripts/check-esp32-s3-client-example.py
```

ESP-IDF 侧:

```bash
cd examples/esp32-s3-client-only
idf.py set-target esp32s3
idf.py menuconfig
idf.py build
```

板级联调通过标准:

- 网络和 SNTP 已就绪
- `PutObject` 上传小对象成功
- `HeadObject` 对同一对象成功
- `GetObject` 分块读取成功
- heap 监控未出现对象大小级别的整块分配
- 固件不包含运营商 provider 凭证或控制面 API key

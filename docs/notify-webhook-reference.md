# Notify Webhook 接入参考

这份文档说明 `gatewayd` 外发告警 webhook 的接收端要求，以及仓库内置的参考接收器脚本怎么使用。

如果你只想先确认网关本身有没有在发 webhook，先看:

- [docs/object-actions-and-history.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-and-history.md:1)
- [docs/object-actions-api-reference.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-api-reference.md:1)

如果你需要服务端配置项，去看:

- [config/example.env](/home/walky/carrier-cloud-blob-gateway/config/example.env:1)

## 1. 协议概览

配置 `CCBG_NOTIFY_WEBHOOK_URL` 后，`gatewayd` 会按 `CCBG_NOTIFY_POLL_INTERVAL_SECONDS` 周期评估当前 alerts。

当前行为:

- 只有 alerts 集合发生变化时才发送
- 请求方法固定为 `POST`
- 请求体固定为 `application/json`
- 无论是否启用签名，都会发送:
  - `x-ccbg-notify-event-id`
  - `x-ccbg-notify-timestamp`
- 只有启用 `CCBG_NOTIFY_WEBHOOK_SIGNING_SECRET` 或 `CCBG_NOTIFY_WEBHOOK_SIGNING_SECRET_FILE` 时，才额外发送:
  - `x-ccbg-notify-signature-version: v1`
  - `x-ccbg-notify-signature`

当前请求体包含:

- `event_id`
- `service`
- `emitted_at_unix_ms`
- `runtime`
- `monitoring`
- `alerts`

## 2. 签名规则

签名算法:

- `hex(HMAC_SHA256(secret, "<timestamp>.<sha256(body)>"))`

其中:

- `timestamp` 对应 `x-ccbg-notify-timestamp`
- `body` 是 HTTP 原始请求体字节，不是重排后的 JSON
- `sha256(body)` 使用小写十六进制编码
- 输出签名也是小写十六进制编码

因此接收端必须先拿到原始 body，再计算摘要和 HMAC，不能先把 JSON 解析后再重新序列化。

## 3. 接收端最低要求

建议至少做以下校验:

1. 校验 `x-ccbg-notify-event-id` 存在。
2. 校验 `x-ccbg-notify-timestamp` 可解析，并且落在可接受时间窗内。
3. 如果你启用了签名:
   - 校验 `x-ccbg-notify-signature-version == v1`
   - 校验 `x-ccbg-notify-signature`
   - 用常量时间比较函数比对签名
4. 先按 `event_id` 做幂等去重，再往企业微信、飞书、PagerDuty 或内部告警系统转发。

推荐的时间窗:

- 局域网或同机部署: `60-300s`
- 经公网反代: `300-600s`

## 4. 参考接收器脚本

仓库提供了一个最小参考实现:

- [scripts/notify-webhook-receiver-example.py](/home/walky/carrier-cloud-blob-gateway/scripts/notify-webhook-receiver-example.py:1)

它做的事情只有四件:

- 读取原始请求体
- 校验 `event_id` 和 `timestamp`
- 在配置 secret 时校验 HMAC 签名
- 把解析后的 payload 打到标准输出，方便你再接下游系统

运行示例:

```bash
cd /path/to/carrier-cloud-blob-gateway
python3 ./scripts/notify-webhook-receiver-example.py \
  --host 127.0.0.1 \
  --port 61110 \
  --secret 'replace-with-notify-secret' \
  --max-age-seconds 300
```

启动后会先输出一行监听状态 JSON，例如:

```json
{"listening": "http://127.0.0.1:61110", "signature_verification": true, "timestamp_max_age_seconds": 300, "dedupe_note": "persist event_id before forwarding to downstream systems"}
```

收到 webhook 后会输出一行事件 JSON，例如:

```json
{"received_at_unix_ms": 1710000001234, "event_id": "abc123", "timestamp_unix_ms": 1710000001200, "alert_count": 2, "payload": {"service": "carrier-cloud-blob-gateway"}}
```

## 5. 网关侧配置示例

```bash
CCBG_NOTIFY_WEBHOOK_URL=http://127.0.0.1:61110/
CCBG_NOTIFY_WEBHOOK_SIGNING_SECRET=replace-with-notify-secret
CCBG_NOTIFY_POLL_INTERVAL_SECONDS=15
```

如果你只想先做联调、不启用验签，可以只配:

```bash
CCBG_NOTIFY_WEBHOOK_URL=http://127.0.0.1:61110/
```

这时接收端仍应校验:

- `x-ccbg-notify-event-id`
- `x-ccbg-notify-timestamp`
- 时间窗

## 6. 去重点

参考脚本不会内置持久化去重，因为每个接收系统的存储约束不同。

实际部署时建议:

- 把 `event_id` 写入 Redis / SQLite / 本地 KV
- 至少保留 `24h`
- 只有在 `event_id` 首次出现时才继续转发

如果你的告警系统本身支持幂等键，也可以直接把 `event_id` 当幂等键透传。

## 7. 常见失败场景

- `400 missing x-ccbg-notify-event-id`: 说明接收端前面有代理错误改写，或请求并非来自当前网关实现
- `400 stale webhook timestamp`: 说明接收端主机时钟漂移过大，或 webhook 长时间滞留
- `400 unsupported x-ccbg-notify-signature-version`: 说明接收端需要升级以支持新的签名版本
- `401 signature mismatch`: 说明 secret 不一致，或中间层改写了 body

最容易出错的点是“先 parse JSON 再 stringify 后验签”。当前协议要求按原始 body 字节验签，不能重排字段。

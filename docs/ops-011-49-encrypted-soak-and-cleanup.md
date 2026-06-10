# OPS-011: `.49` 96 MiB LXC 加密压测与脏数据清理

## 状态

- 执行时间: 2026-06-09
- 目标: 在 `.49` 的低内存 LXC 环境上验证加密写路径稳定性，并清理压测残留对象
- 结论: 加密写路径可稳定运行；内存增幅有限，主要成本是时延上升；压测脏数据已从云盘和本地元数据清理干净

## 环境

- 宿主形态: Debian LXC
- 内存预算: `96 MiB`
- 运行组件:
  - `gatewayd`
  - SMB sidecar
  - FUSE 已暴露，可执行 `rclone mount` 读环
- 加密方式:
  - 仅为本轮压测临时创建 gateway-managed key
  - 仅为压测前缀创建临时 encryption profile
  - 仅为压测前缀创建临时 content policy
  - 测试结束后全部回收

## 压测矩阵

### 明文对照

- 10 分钟 soak
- `8 MiB` 对象
- `4` 并发 worker
- 同时保留 FUSE 读环

结果:

- `269/269` 成功
- Gateway RSS:
  - `p50 48.1 MiB`
  - `p95 64.3 MiB`
  - `max 74.7 MiB`
- SMB sidecar RSS:
  - `p50 11.0 MiB`
  - `p95 18.4 MiB`
  - `max 20.2 MiB`
- S3 端到端时延:
  - `p50 8.5 s`
  - `p95 13.3 s`
  - `max 20.5 s`
- `MemAvailable` 最低约 `6.6 MiB`

### 加密路径

加密写入先做命中验证，再跑长时间压力:

- 验证对象被持久化为:
  - `encrypted=true`
  - `algorithm=aes_256_gcm`
  - `chunk_plaintext_bytes=1048576`
  - `stored_size > plaintext_size`
- 10 分钟 soak
- `8 MiB` 对象
- `4` 并发 worker
- 同时保留 FUSE 读环
- 额外补一轮 `16 MiB` spooled smoke

结果:

- `216/216` 成功
- 额外 `16 MiB` spooled smoke: `4/4` 成功
- Gateway RSS:
  - `p50 51.9 MiB`
  - `p95 65.9 MiB`
  - `max 75.8 MiB`
- SMB sidecar RSS:
  - `p50 12.3 MiB`
  - `p95 17.4 MiB`
  - `max 32.1 MiB`
- S3 端到端时延:
  - `p50 10.7 s`
  - `p95 15.7 s`
  - `max 24.2 s`
- `MemAvailable` 最低约 `6.9 MiB`

## 对比结论

- 加密路径稳定性:
  - 明文与加密两轮 soak 都是 `0` 失败
  - `16 MiB` spooled 加密写读删也全部通过
- 加密对网关内存的影响有限:
  - Gateway RSS `p50` 约 `+8%`
  - Gateway RSS `p95` 约 `+2.4%`
  - Gateway RSS `max` 约 `+1.5%`
- 加密的主要成本是时延:
  - 时延 `p50` 约 `+25.8%`
  - 时延 `p95` 约 `+17.4%`
  - 相同 10 分钟窗口内完成的操作数少于明文路径
- 对 `96 MiB` LXC 的判断:
  - 可以作为低内存验证环境
  - 不是舒适的长期生产预算
  - 日常运行仍应优先使用 `256 MiB+`
  - 低内存场景继续建议把 `CCBG_DATA_PLANE_MAX_IN_FLIGHT` 控制在 `2~4`

## 清理结果

压测结束后，对云盘中的压测残留对象执行了显式收敛。

发现的残留对象:

- `stress/local-preflight-*`
- 两轮 `stress/memory-soak-*`

清理结果:

- `stress/` 前缀残留对象共 `12` 个，已全部删除
- `ListObjectsV2 prefix=stress/` 复查结果为 `0` 个对象
- 本地元数据复查结果:
  - `logical_objects where key like 'stress/%' = 0`
  - `object_placements where key like 'stress/%' = 0`
  - `object_protection_plans where key like 'stress/%' = 0`
- 临时加密配置复查结果:
  - `/api/encryption-keys = []`
  - `/api/encryption-profiles = []`
  - `/api/content-policies = []`

## 验收结论

- `.49` 当前可以承载加密路径验证
- 公开文案可以安全表述为:
  - 在 `96 MiB` LXC 上做过明文与加密低内存压力验证
  - 加密路径稳定，但吞吐/时延劣于明文
  - `96 MiB` 适合实验或验收，不建议作为常态生产预算

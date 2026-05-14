# 联通正式上线检查清单

这份文档面向“准备把联通作为正式 primary provider 使用”的场景。

目标不是介绍背景，而是给你一份可逐项打勾的上线前检查表，避免出现:

- token 能看目录但不能稳定读写
- `family` 容器未确认就直接上线
- 对象动作能点但复制语义没验证
- fallback / shared history 没有审计证据

如果你还没完成联通凭证接入，先看:

- [docs/auth-step-by-step.md](/home/walky/carrier-cloud-blob-gateway/docs/auth-step-by-step.md:225)

如果你需要对象动作的运维说明和 API 契约，继续看:

- [docs/object-actions-and-history.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-and-history.md:1)
- [docs/object-actions-api-reference.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-api-reference.md:1)

## 1. 使用方式

建议在正式切流前复制一份本清单，把每项标成:

- `[x]` 已确认
- `[ ]` 未确认
- `N/A` 当前不适用

建议至少保留:

- 勾选后的清单副本
- 一份共享历史导出
- 一份 provider health / status 截图或 JSON

## 2. 宿主与部署检查

### 2.1 基础宿主

- [ ] 宿主平台已确认，属于本期支持范围，例如 `PVE LXC x86/x64`、`Docker x86/x64`、`Podman x86/x64` 或 `OpenWRT arm64`
- [ ] 端口规划已确认，没有与现有服务冲突
- [ ] `61080` 数据面入口已确认
- [ ] `61081` Admin Web 入口已确认
- [ ] 若启用 OneDrive 授权，`61082` 回调口已确认
- [ ] 管理界面未直接暴露到公网，或已有受控网络边界

### 2.2 轻量宿主附加项

如果是 `OpenWRT arm64`:

- [ ] 已从 [config/openwrt-lite.env](/home/walky/carrier-cloud-blob-gateway/config/openwrt-lite.env:1) 起步，而不是直接照搬完整宿主配置
- [ ] `CCBG_MAX_IN_MEMORY_OBJECT_BYTES` 已按设备内存确认
- [ ] `CCBG_REPLICATION_WORKERS=1` 或等效保守设置已确认
- [ ] `CCBG_OBJECT_ACTION_HISTORY_LIMIT` 已按轻量档位确认，默认建议 `8`
- [ ] `RUST_LOG=warn` 或其他低噪音日志级别已确认

## 3. 配置与文件检查

- [ ] `CCBG_PRIMARY_PROVIDER=unicom` 已确认
- [ ] `CCBG_CONTROL_PLANE_FILE` 路径可写
- [ ] `CCBG_CREDENTIALS_DIR` 路径可写
- [ ] `CCBG_METADATA_DB_PATH` 路径可写
- [ ] `CCBG_OBJECT_ACTION_HISTORY_LIMIT` 已明确设置或接受默认值
- [ ] 已确认是否启用 `CCBG_SYNC_TARGETS`
- [ ] 已确认是否启用 `CCBG_FALLBACK_READ_ORDER`
- [ ] 若启用 OneDrive，同步与 fallback 策略已明确

## 4. 联通凭证与会话检查

- [ ] 联通 `Access Token` 已确认是新鲜的，不是历史缓存值
- [ ] 如需要，`Cookie Header` 已同步更新
- [ ] `Origin=https://pan.wo.cn` 与 `Referer=https://pan.wo.cn/` 已保持正确
- [ ] `Provider Credentials -> China Unicom` 卡片里的值与预期一致
- [ ] 如用文件注入，`CCBG_UNICOM_TOKEN_FILE` 路径已确认
- [ ] 如用 cookie 文件注入，`CCBG_UNICOM_COOKIE_HEADER_FILE` 路径已确认
- [ ] 如当前依赖手工 `Family ID`，已确认该值来自当前可用会话

## 5. Provider Health 检查

- [ ] Admin Web 中 `China Unicom` 的 provider health 可正常加载
- [ ] `provider_test` 或 `Test Now` 返回的不是认证失败
- [ ] personal scope 可见
- [ ] 若需要家庭空间，family scope 可见
- [ ] health 返回的 scope 容器映射与预期一致:
  - `root`
  - `family`
- [ ] 如果 family scope 缺失，已经先处理，不带病上线

## 6. 联通读路径检查

### 6.1 root 容器

- [ ] 能列出 `root` bucket / container
- [ ] 能列出 `root` 下至少一个已知对象
- [ ] 能读取 `root` 下至少一个已知对象

### 6.2 family 容器

如果正式环境需要家庭空间:

- [ ] 能列出 `family` bucket / container
- [ ] 能列出 `family` 下至少一个已知对象
- [ ] 能读取 `family` 下至少一个已知对象

如果正式环境不需要家庭空间:

- [ ] 已明确记录“本次上线不依赖 `family` 容器”

## 7. 联通写路径检查

### 7.1 上传

- [ ] 已在 `root` 容器完成至少一次真实上传
- [ ] 上传后可立即读取该对象
- [ ] 如正式环境依赖 `family`，已在 `family` 容器完成至少一次真实上传

### 7.2 删除

- [ ] 已在非关键测试对象上验证删除
- [ ] 删除后对象确实不可再读取

## 8. 对象动作检查

### 8.1 rename

- [ ] 已验证同父目录 rename
- [ ] 已确认操作者知道“联通 rename 仅支持同父目录”
- [ ] 若需要跨目录调整，运维手册中已约定使用 `move`

### 8.2 copy

- [ ] 已验证至少一次 copy
- [ ] 已确认目标 key 冲突覆盖风险

### 8.3 move

- [ ] 已验证至少一次 move
- [ ] 若正式环境涉及跨容器移动，已做过预演

### 8.4 对象动作页面

- [ ] 执行预览能正常显示风险提示
- [ ] before/after 检查结果能正常渲染
- [ ] 共享历史能记录本次动作

## 9. 共享历史与审计检查

- [ ] `GET /api/status` 中能看到 `object_action_history`
- [ ] `GET /api/status` 中能看到 `object_action_history_limit`
- [ ] Admin Web 中 action/outcome/provider 筛选可正常工作
- [ ] `Export Shared History` 可正常导出 JSON
- [ ] 导出的 JSON 已实际打开检查过，不是空文件
- [ ] 正式切流前已保留一份共享历史导出
- [ ] 运维团队已约定谁有权限清空共享历史

## 10. 复制与 fallback 检查

如果本次上线启用了同步目标:

- [ ] 写入后能看到复制任务生成
- [ ] 复制状态不会长期卡在失败且无人处理
- [ ] 已验证对象动作后复制元数据语义正确:
  - rename -> `put(new) + delete(old)`
  - copy -> `put(dest)`
  - move -> `put(dest) + delete(src)`

如果本次上线启用了 fallback:

- [ ] 已确认 `fallback_read_order`
- [ ] 已验证至少一条 fallback 可读路径
- [ ] 已确认 fallback 不是“理论开启”，而是对象状态已真正允许

如果本次上线未启用同步 / fallback:

- [ ] 已明确记录“本次上线只验证 primary provider，不启用 fallback”

## 11. 变更窗口前最后确认

- [ ] 当前 primary provider 确认是 `unicom`
- [ ] 没有残留测试对象会影响正式命名空间
- [ ] 联通 token 不是昨天或更早抓的旧值
- [ ] family scope 如需使用，刚刚做过实际读取验证
- [ ] 共享历史已清晰区分“测试阶段”和“正式阶段”
- [ ] 正式动作前已导出一份基线历史

## 12. 回滚准备

- [ ] 已明确如果联通读写异常，回滚方案是什么
- [ ] 如拓扑需要回切，已确认可以快速改回原 primary provider
- [ ] 已保留最近一次可工作的 provider 凭证材料
- [ ] 已确认 control-plane 文件路径，必要时可直接留证排障
- [ ] 已保留:
  - 一份 provider health 证据
  - 一份共享历史导出
  - 一份关键失败请求/报错截图或日志

## 13. 上线后 15 分钟观察项

- [ ] 新写入对象能够稳定读取
- [ ] 新写入对象没有持续认证失败
- [ ] 复制队列没有快速堆积失控
- [ ] 如果启用了 fallback，没有出现异常误切
- [ ] 共享历史持续增长符合预期，没有异常失败风暴

## 14. 建议最小上线证据包

建议至少保存以下材料:

1. 勾选后的这份清单
2. `Provider Health` 截图或 JSON
3. 一份 `GET /api/status` 导出
4. 一份共享历史导出 JSON
5. 一次成功 upload / rename / move 的操作证据

如果团队需要标准化留档，建议直接套用:

- [docs/unicom-change-record-template.md](/home/walky/carrier-cloud-blob-gateway/docs/unicom-change-record-template.md:1)

如果你需要查看当前阶段的整体收尾结论，继续看:

- [docs/unicom-phase-closeout-report.md](/home/walky/carrier-cloud-blob-gateway/docs/unicom-phase-closeout-report.md:1)

## 15. 不建议直接上线的信号

出现下面任一项，建议先不要切正式流量:

- [ ] token 只能偶尔列目录，但读写不稳定
- [ ] `family` 是正式依赖，但还没验证成功
- [ ] rename 边界没和操作者说清楚
- [ ] 对象动作历史还没导出过一次
- [ ] fallback 已配置，但从未做过真实验证
- [ ] OpenWRT 设备内存过小，却还保留大对象和高并发配置

## 16. 相关文档

- [docs/auth-step-by-step.md](/home/walky/carrier-cloud-blob-gateway/docs/auth-step-by-step.md:225)
- [docs/object-actions-and-history.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-and-history.md:1)
- [docs/object-actions-api-reference.md](/home/walky/carrier-cloud-blob-gateway/docs/object-actions-api-reference.md:1)
- [docs/openwrt-host-profile.md](/home/walky/carrier-cloud-blob-gateway/docs/openwrt-host-profile.md:1)
- [docs/provider-completion-standard.md](/home/walky/carrier-cloud-blob-gateway/docs/provider-completion-standard.md:1)
- [docs/unicom-change-record-template.md](/home/walky/carrier-cloud-blob-gateway/docs/unicom-change-record-template.md:1)

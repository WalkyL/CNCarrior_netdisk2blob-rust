# `.49` 中国移动覆盖写入失败分析

日期: `2026-06-06`

## 1. 结论

`ccbg49` 当前已经不再卡在中国联通失效登录，而是进入了中国移动写路径。
本文件记录的是修复前排查结论。修复前的剩余问题是:

- 当 S3 `PUT` 覆盖一个已经存在的对象 key 时
- 上游中国移动 `file/create` 可能返回:
  - `success=true`
  - `rapidUpload=false`
  - `exist=true`
  - `uploadId=null`
- 当前 `provider-mobile` 把这个返回当成异常，直接报:
  - `China Mobile file/create uploadId was missing`

这会导致:

- 新 key 的写入可能成功
- 已存在 key 的覆盖写入失败
- `stock-rag-bridge-rust` 的增量图发布在 `.49` 上无法稳定工作，因为图分片会反复覆盖同一批对象 key

这不是 `.49` 的部署配置问题，也不是 `stock-rag-bridge-rust` 本身的对象筛选问题，而是 `carrier-cloud-blob-gateway` 的 `provider-mobile` 没有实现中国移动这条覆盖写入分支。

2026-06-06 项目内跟进状态:

- `provider-mobile` 已实现该覆盖写入分支。
- `MobileFileCreateData` 现在兼容解析上游 `exist` 字段和 `exists` 字段名。
- 新增回归测试 `put_object_retries_after_existing_object_blocks_mobile_create_upload_id` 和 `put_object_deletes_existing_same_name_files_before_mobile_create` 已通过。
- `.49` 已更新到 `gatewayd 0.1.6`，并通过 S3 同 key 重复 `PUT` / `GET` / `DELETE` 实机验收。

## 2. 业务影响

受影响链路:

- `host42` -> `ccbg49` S3
- `stock-rag-bridge-rust graph-publish-run`
- 所有走中国移动 provider、且会覆盖既有对象 key 的 S3 `PUT`

直接影响:

- 增量图发布无法稳定完成
- 已存在对象的重复发布失败
- `host42` 无法把图构建结果可靠地持续落到 `.49` 挂载的 S3

## 3. 已验证事实

### 3.1 上游路由已切到 mobile

此前 `.49` 的主写 provider 是 `unicom`，但联通账号失效，`/healthz` 已显示:

- `QueryAllFiles returned RSP_CODE=1001`

后续已完成:

- `primary_provider` 改成 `mobile`
- `write_targets` 改成只写 `mobile`
- `stock-rag-bridge-rust/graph/%` 既有对象的 `object_placements.provider` 已从 `unicom` 调整到 `mobile`

因此当前失败已确认不是“还在写联通”，而是“已经写到移动，但移动覆盖分支没处理”。

### 3.2 `host42` 增量选择已经正确

`stock-rag-bridge-rust` 侧已验证:

- `graph_rebuild_plan.json` 的增量 bucket 选择 bug 已修复
- `graph_publish` 已只发布受影响 bucket
- 单 ticker 增量重建只会触碰少量分片文件

因此当前失败不是“误走全量发布”。

### 3.3 现象只在覆盖写入时出现

已确认:

- 对某些新对象 key 直接 `PUT` 可以成功
- 对已存在 key 重复 `PUT` 时，`graph-publish-run` 报错:

```text
China Mobile file/create uploadId was missing
```

### 3.4 上游中国移动真实返回已被手工复现

对中国移动 `file/create` 的重放观察到两类行为:

1. 新文件名:

- 返回正常 `uploadId`
- 可继续走 multipart 上传

2. 已存在同名文件:

- 返回 `success=true`
- 返回 `rapidUpload=false`
- 返回 `exist=true`
- 但 `uploadId=null`

这说明“`rapidUpload=false` 就一定有 `uploadId`”这个假设不成立。

## 4. 当前代码根因

问题点在:

- `crates/provider-mobile/src/lib.rs`
- `create_mobile_upload(...)`
- `MobileFileCreateData`

当前逻辑等价于:

1. 调 `file/create`
2. 如果 `rapidUpload=true`，允许没有 `uploadId`
3. 否则强制要求 `uploadId` 必须存在
4. 一旦为空，直接报错

但中国移动实际上还有第三条合法分支:

- `rapidUpload=false`
- `exist=true`
- `uploadId=null`

当前 `provider-mobile` 没有为这个分支定义覆盖语义，所以 S3 `PUT` 无法覆盖既有对象。

## 5. 建议修复方案

目标不是在 `.49` 临时打补丁编译，而是让 `CCBG` 项目按正式发布流程修复。项目代码已按下面的最小方案落地；`.49` 仍需等待正式发布包更新后验收。

建议最小修复:

1. 在 `MobileFileCreateData` 中解析上游 `exist` 字段，并兼容 `exists` 字段名。
2. 在 `create_mobile_upload(...)` 中识别:
   - `rapidUpload == false`
   - `uploadId == null/empty`
   - `exist == true`
3. 命中该分支后:
   - 在当前 `parentFileId` 下按同名文件查找既有对象
   - 通过 native `file/batchDelete` 删除同名旧对象
   - 短暂重试 `file/create`
4. 重试后如果仍然没有 `uploadId`，再返回稳定错误。

这样做的原因:

- S3 `PUT` 语义本来就是允许覆盖
- `ccbg` 在中国移动侧已经把对象限制在受控根目录下
- 删除同 key 旧对象再重试，是当前 `.49` 实机验证过的覆盖实现；曾测试“新上传完成后再删旧对象”，真实移动端会导致随后 `GET` 变成 `404`，因此未采用

## 6. 建议修改点

建议 `CCBG` 项目组至少改以下位置:

- `crates/provider-mobile/src/lib.rs`

建议新增/调整:

- `MobileFileCreateData.exists: Option<bool>`，通过 serde 兼容解析 `exist` / `exists`
- `find_child_file_id(parent_file_id, file_name)` 之类的 helper
- `create_mobile_upload(...)` 的覆盖分支处理

建议增加回归测试:

- 场景: `file/create` 第一次返回 `exist=true` 且 `uploadId=null`
- 预期:
  - provider 先调用 `file/batchDelete`
  - 然后重试 `file/create`
  - 最终上传完成

## 7. 验收标准

`CCBG` 项目修复后，至少要满足以下验收:

1. 单元/集成测试

- `provider-mobile` 新增覆盖写入回归测试通过
- 既有移动上传测试不回归

2. `.49` 本地 S3 验证

- 对一个已存在 key 执行重复 `PUT`
- 不再出现:

```text
China Mobile file/create uploadId was missing
```

3. `host42` 实际业务验证

- 在 `2026-06-06` 当前环境下重新执行:

```bash
set -a
source /etc/default/stock-rag-rust-bridge
set +a
/root/apps/stock-rag-bridge-rust/bin/stock-rag-bridge-rust graph-publish-run --graph-root /home/walky/graphrag-stock-project/live/graphrag-stock
```

- 对已经存在的图分片 key 能成功覆盖发布

4. 非目标不误报

- 新 key 写入仍然正常
- `rapidUpload=true` 的秒传分支不受影响
- 16 GiB 大文件权限问题不应被误判为本问题已解决

## 8. 非目标

这次修复不包含:

- 修复中国移动 `04010319 / Insufficient Rights` 的 16 GiB 大文件限制
- 在 `.49` 上安装 Rust 编译环境
- 修改 `stock-rag-bridge-rust` 的发布逻辑来绕过 `CCBG`

## 9. 发布要求

不要在 `.49` 直接装编译环境修。

按 `CCBG` 既有发布约束执行:

- 在正式 Linux build host 上构建新 release
- 生成标准发布包
- 再替换 `.49` 的 `/opt/ccbg/bin/gatewayd`

参考:

- [ops-007-47-release-build-host.md](ops-007-47-release-build-host.md)

## 10. 关联背景

此前 `.49` 相关验收与移动大文件限制记录见:

- [ops-008-49-lxc-smb-stub-removal.md](ops-008-49-lxc-smb-stub-removal.md)

本问题的核心不是大文件权限，而是“已存在对象的覆盖写入分支未实现”。

## 11. 项目修复跟进

代码改动:

- `crates/provider-mobile/src/lib.rs`
  - `MobileFileCreateData.exists` 通过 serde 兼容解析 `exist` / `exists`
  - `create_mobile_upload(...)` 在 `rapidUpload=false`、`uploadId` 缺失、`exist=true` 时进入覆盖分支
  - 覆盖分支会在当前 `parentFileId` 下查找同名旧文件，调用 native `file/batchDelete` 删除旧文件，再对 `file/create` 做短暂重试
  - 重试后仍缺 `uploadId` 时返回稳定错误

本地验证:

```bash
cargo test -p provider-mobile
```

结果:

```text
test result: ok. 23 passed
```

`.49` 实机验收:

- 部署版本: `gatewayd 0.1.6`
- Provenance fingerprint: `ccbg-0.1.6-walky-20260606`
- 部署后二进制 SHA256: `89fa4d7e0e9c20d32bda5a728f77eb0e50291fcfa131053493e468fba467720d`
- 部署包 SHA256: `03151f6dcd00cde1fc5754eedc256f5f366f1bbb7c7971c8b5ab3f65f654d65f`
- 验证脚本: `/tmp/ccbg_s3_overwrite_verify_1536.py`
- 验证 key: `root/ccbg-mobile-overwrite-verify-20260606-1536b.txt`
- 验证结果:
  - 第一次 `PUT` 返回 `200`
  - 同 key 第二次 `PUT` 返回 `200`
  - 随后 `GET` 返回第二次写入内容 `ccbg mobile overwrite verify second`
  - 清理 `DELETE` 返回 `204`
- `.49` health: `systemctl is-active ccbg` 为 `active`，`http://127.0.0.1:61080/healthz` 为 healthy，backend 为 `mobile-cloud-drive`
- SMB sidecar: `state=running`，`listener_ready=true`，`mounted_share_count=1`，`enabled_share_count=1`，`last_error=null`

剩余待验收:

- 在 `host42` 重新跑 `stock-rag-bridge-rust graph-publish-run`

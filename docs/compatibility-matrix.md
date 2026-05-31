# 一期兼容矩阵

## 目标

一期目标不是“所有平台都运行同一个完整版进程”，而是按平台能力分层兼容:

- 官方宿主兼容
- 实验宿主兼容
- 客户端兼容

## 平台矩阵

| 目标 | 角色 | 一期定位 | 说明 |
| --- | --- | --- | --- |
| PVE LXC `x86/x64` | 官方宿主 | 支持 | 优先级最高，适合作为主部署目标，systemd 后台常驻 |
| Docker `x86/x64` | 官方宿主 | 支持 | 使用 `deploy/Dockerfile` |
| Podman `x86/x64` | 官方宿主 | 支持 | 使用 `deploy/Containerfile` 或 `deploy/Dockerfile` |
| Windows `x86_64` | 官方宿主 | 支持 | 原生发布包，默认 Scheduled Task 后台常驻路径 |
| macOS `x86_64` | 官方宿主 | 支持 | 原生发布包，默认 `launchd` user agent |
| macOS `arm64` | 官方宿主 | 支持 | 原生发布包，默认 `launchd` user agent |
| fnOS | 实验宿主 | 实验 | 优先复用 Docker / Linux 路径，不承诺应用商店上架 |
| OpenWRT `arm64` | 实验宿主 | 实验 | 优先 `daemon + stdio MCP`，管理界面按资源情况裁剪 |
| STM32 | 客户端/从设备 | 支持 | 不承载完整 daemon，作为本地 S3/MCP 的调用端 |
| ESP32-S3 | 客户端/从设备 | 支持 | 默认按 `client-only` 兼容处理，不承载完整 daemon |

## 宿主分级

### 官方宿主

包含:

- `gatewayd`
- `policy-engine`
- `metadata-store`
- `replication-engine`
- 运营商 provider adapters
- `mcp-server`
- 可选 `admin-ui-web`

一期目标:

- PVE LXC `x86/x64`
- Docker `x86/x64`
- Podman `x86/x64`
- Windows `x86_64`
- macOS `x86_64`
- macOS `arm64`

发布包要求:

- Rust `gatewayd` 二进制和 `assets/admin/index.html` 必须在同一个 artifact 内。
- Windows/macOS 原生包使用 [build-native-package.sh](../scripts/build-native-package.sh) 生成。
- macOS 默认安装到用户级 `launchd`；Windows 默认安装为无额外依赖的后台常驻任务。

### 实验宿主

包含:

- `gatewayd`
- `policy-engine`
- `metadata-store`
- `replication-engine`
- `mcp-server` 优先 `stdio`

可选裁剪:

- 禁用 `admin-ui-web`
- 降低并发 worker 数
- 限制缓存和日志保留

一期目标:

- fnOS
- OpenWRT `arm64`

参考:

- [openwrt-host-profile.md](openwrt-host-profile.md)
- [resource-budget.md](resource-budget.md)

### 客户端兼容

包含:

- 调用本地 S3 API
- 调用 MCP
- 小对象上传下载
- 状态查询

不包含:

- Web UI
- SQLite 元数据层
- OneDrive OAuth 控制面
- 多 provider 协调

一期目标:

- STM32

## STM32 兼容定义

STM32 一期兼容的正确解释是:

- 作为 `carrier-cloud-blob-gateway` 的 S3 / MCP 客户端
- 调用宿主机暴露的本地接口
- 处理少量对象、短请求和状态查询

不是:

- 在 STM32 上运行完整 Rust daemon
- 在 STM32 上承载 OneDrive 授权与复制引擎

如果后续需要 STM32 原生接入，建议单独提供:

- 极简 HTTP client
- 二进制协议或串口桥接
- 极小内存占用的对象上传下载适配层

当前 client-only 示例见 [stm32-client-only.md](stm32-client-only.md)，它只覆盖本地 S3 `HeadObject` / `GetObject` / streaming `PutObject`，并要求板级工程注入 HTTP、UTC 时间、SHA256 与 HMAC-SHA256。

如果目标进一步扩展到 `ESP32-S3`，请额外参考 [esp32-s3-profile.md](esp32-s3-profile.md)。该档位默认仍不应承载完整 daemon。
所有平台的 RAM / Flash 预算推导见 [resource-budget.md](resource-budget.md)。

## 容器兼容要求

### PVE LXC

- 容器内部保留 `61080-61084`
- 挂载数据库目录、日志目录和 secrets
- 优先 Debian/Ubuntu 基础镜像

### Docker

- 默认使用 [Dockerfile](../deploy/Dockerfile)
- 支持多阶段构建
- 适合开发和 CI 验证

### Podman

- 默认使用 [Containerfile](../deploy/Containerfile)
- 兼容 rootless 模式
- 适合软路由和更保守的宿主环境

## 一期验收标准

1. PVE LXC `x86/x64` 可运行完整 daemon。
2. Docker 和 Podman 镜像都可成功构建并启动。
3. OpenWRT `arm64` 能运行轻量化部署配置。
4. Windows `x86_64` 原生包包含 `gatewayd.exe`、Admin HTML 和后台常驻安装脚本。
5. macOS `x86_64` / `arm64` 原生包包含 `gatewayd`、Admin HTML 和 `launchd` 安装脚本。
6. STM32 / ESP32-S3 有明确的客户端兼容接口，不再被误认为完整宿主目标。

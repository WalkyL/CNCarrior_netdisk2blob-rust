# PVE/LXC 部署包验收

这份清单用于在 Proxmox VE LXC 或普通 Debian/Ubuntu LXC guest 中部署 `gatewayd`。

默认包只跑 `stub` provider，`CCBG_ONEDRIVE_ENABLED=false`，不包含真实 provider 凭证。真实上线前必须编辑 `/etc/ccbg/ccbg.env`，替换 S3 secret、控制面 API key、primary provider 与凭证路径。

## 构建部署包

```bash
scripts/build-lxc-package.sh
```

如果已经有 `target/release/gatewayd`，可跳过本地构建:

```bash
scripts/build-lxc-package.sh --skip-build
```

输出:

- `target/lxc-package/ccbg-lxc-package.tar.gz`
- `target/lxc-package/ccbg-lxc-package.tar.gz.sha256`
- 包内 `MANIFEST.sha256`

## LXC guest 建议

- OS: Debian 12 或 Ubuntu 22.04+
- 网络: bridge 到受控 LAN，先只开放 `61080`
- 挂载/备份点:
  - `/etc/ccbg`: env 与 catalog 配置
  - `/var/lib/ccbg`: SQLite、control-plane、provider credentials、spool
  - `/var/log/ccbg`: 日志目录
  - `/opt/ccbg/backups`: 升级前二进制备份
- 端口:
  - `61080`: S3 API，可按需暴露到 LAN
  - `61081`: Admin Web，默认 `127.0.0.1`
  - `61082`: OAuth callback，默认 `127.0.0.1`
  - `61083`: Metrics/readyz，默认 `127.0.0.1`

## 安装

在 LXC guest 中:

```bash
tar -xzf ccbg-lxc-package.tar.gz
cd ccbg-lxc-package
sudo scripts/install.sh
```

安装脚本会:

- 创建 `ccbg` system user/group
- 安装 `/opt/ccbg/bin/gatewayd`
- 安装 `/opt/ccbg/assets/admin/index.html`
- 安装 `/etc/systemd/system/ccbg.service`
- 首次写入 `/etc/ccbg/ccbg.env`
- 保留已有 `/etc/ccbg/ccbg.env`，并把新样例写成 `.package-<timestamp>`
- 升级前备份旧二进制到 `/opt/ccbg/backups/`
- `systemctl enable --now ccbg.service`

正式 release 约定:

- `gatewayd` 与 Admin HTML 必须作为同一部署包交付
- 运行时会优先读取二进制同前缀下的 `assets/admin/index.html`
- 不应依赖测试机上人工散落的外置模板覆盖来完成正式发布

## 验收

```bash
sudo systemctl status ccbg.service --no-pager
sudo scripts/smoke.sh
```

验收标准:

- `ccbg.service` 为 active/running
- `GET /healthz` 返回 200
- `GET /readyz` 返回 200
- SigV4 `ListBuckets` 返回 200
- `/etc/ccbg/ccbg.env` 中 `CCBG_ONEDRIVE_ENABLED=false`

## 升级

1. 上传新 `ccbg-lxc-package.tar.gz` 到 LXC guest。
2. 解包后运行 `sudo scripts/install.sh`。
3. 运行 `sudo scripts/smoke.sh`。
4. 查看 `journalctl -u ccbg.service -n 100 --no-pager`。

## 回滚

安装脚本会在升级前保存旧二进制。回滚最近一次备份:

```bash
sudo scripts/rollback.sh
sudo scripts/smoke.sh
```

回滚指定备份:

```bash
sudo scripts/rollback.sh /opt/ccbg/backups/gatewayd.<sha>.<timestamp>
```

## 上线前必须改

- `CCBG_S3_SECRET_ACCESS_KEY`
- `CCBG_CONTROL_API_KEY`
- `CCBG_PRIMARY_PROVIDER`
- 对应 provider 的 credential 文件或 Admin Web 保存凭证
- 如需 LAN 访问 Admin/Metrics，必须通过受控反代、SSH tunnel 或管理 VLAN，不要默认裸露

OneDrive 仍是 parking provider。除非有真实需求和单独回归，不要把 `onedrive` 加入默认 sync/fallback。

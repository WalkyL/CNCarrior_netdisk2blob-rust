# PVE/LXC 部署包验收

这份清单用于在 Proxmox VE LXC 或普通 Debian/Ubuntu LXC guest 中部署 `gatewayd`。

默认包以 `unicom` 作为主 provider，但不包含真实 provider 凭证；未注入凭据前 provider 会显示 unavailable。真实上线前必须编辑 `/etc/ccbg/ccbg.env`，替换 S3 secret、控制面 API key、primary provider 与凭证路径。

## 构建部署包

```bash
scripts/build-lxc-package.sh
```

如果已经有 `target/release/gatewayd`，可跳过本地构建:

```bash
scripts/build-lxc-package.sh --skip-build
```

LXC 包必须包含 Linux ELF `gatewayd`，不能包含 Windows `gatewayd.exe`。在 Windows 发版工作站上构建 LXC 包时，必须显式传入 Linux target 或已经构建好的 Linux 二进制:

```bash
scripts/build-lxc-package.sh --target x86_64-unknown-linux-gnu
scripts/build-lxc-package.sh --binary target/gatewayd-linux-x86_64
```

如果 Windows 主机本地已经准备好 `Podman` 和 `localhost/product-build-runner:latest`，优先走这个两段式流程，避免把新的 `gatewayd.exe` 和旧的 Linux ELF 混在一起:

```bash
scripts/build-linux-release-in-podman.sh --target x86_64-unknown-linux-gnu --package gatewayd
scripts/build-lxc-package.sh --skip-build --target x86_64-unknown-linux-gnu
```

打包脚本会用 `file` 检查选中的二进制；如果不是 ELF，会直接失败，避免把 Windows 二进制误打进 LXC 包。

输出:

- `target/lxc-package/ccbg-lxc-package.tar.gz`
- `target/lxc-package/ccbg-lxc-package.tar.gz.sha256`
- 包内 `MANIFEST.sha256`

## LXC guest 建议

- OS: Debian 12 或 Ubuntu 22.04+
- 网络: bridge 到受控 LAN，至少开放 `61080` 和 `61081`
- 挂载/备份点:
  - `/etc/ccbg`: env 与 catalog 配置
  - `/var/lib/ccbg`: SQLite、control-plane、provider credentials、spool
  - `/var/log/ccbg`: 日志目录
  - `/opt/ccbg/backups`: 升级前二进制备份
- 端口:
  - `61080`: S3 API，可按需暴露到 LAN
  - `61081`: Admin Web，官方 LXC 包默认 `0.0.0.0`
  - `61082`: OAuth callback，默认 `127.0.0.1`
  - `61083`: Metrics/readyz，默认 `127.0.0.1`

## 安装

在 LXC guest 中:

```bash
tar --no-same-owner -xzf ccbg-lxc-package.tar.gz
cd ccbg-lxc-package
sudo scripts/install.sh
```

安装 profile:

- `sudo scripts/install.sh --s3-only`: 只安装并启动 S3 gateway。这是默认模式。
- `sudo scripts/install.sh --enable-smb-sidecar`: 安装 `rclone`、`samba`、`fuse3` 等 SMB sidecar 依赖，写入 sidecar 脚本与 systemd units，把 `/etc/ccbg/ccbg.env` 和已有 control-plane 中的 SMB 开关打开，并立即运行一次 reconcile。

`--enable-smb-sidecar` 会把 Admin 里的 SMB 能力打开并准备自动挂接。安装后即使还没有 SMB 用户或 share，sidecar 也会先启动一个由 CCBG 管理的 `smbd`，默认监听 `0.0.0.0:445`。第一次使用时，用户进入 Admin 的 SMB 页面添加一个 SMB 用户即可；如果没有手工创建 share，控制面会自动生成 `CCBGRoot` 默认 root 共享，保存后由 systemd path/timer 自动重试并收敛到可用共享。

从当前实现开始，`ccbg-smb-sidecar-sync.service` 只负责 reconcile；长时间运行的 `smbd` 和
`rclone mount` 会被放进独立的 transient systemd units，不再留在 `sync.service` 的 cgroup 里。

SMB sidecar 默认挂载根目录是 `/mnt/ccbg/smb/mounts`，配置和 runtime data 位于 `/var/lib/ccbg/smb-sidecar/`。这个挂载点避开 Ubuntu/Debian LXC 中 `fusermount3` AppArmor profile 对 `/srv`、`/var/lib` 等自定义 mount point 的常见拦截。

如果部署在 PVE/LXC 或其它容器里，真实挂载 `CCBGRoot` 这类 rclone-backed SMB share 还需要 guest 能访问 `/dev/fuse`。没有 `/dev/fuse` 时，`--enable-smb-sidecar` 仍会安装依赖、启用 sidecar units，并先启动 CCBG 管理的 `smbd` 监听 `0.0.0.0:445`；但用户保存 SMB 用户并自动生成 `CCBGRoot` 后，rclone mount 会停在 FUSE 前置条件，`status.json.last_error` 会提示容器需要暴露 `/dev/fuse`。

PVE host 侧的最小开通步骤：

```bash
# 在 PVE 宿主机 shell 中执行；把 104 改成实际 CTID
pct stop 104
cat >> /etc/pve/lxc/104.conf <<'EOF'
features: fuse=1,nesting=1
lxc.cgroup2.devices.allow: c 10:229 rwm
lxc.mount.entry: /dev/fuse dev/fuse none bind,create=file,optional 0 0
EOF
pct start 104
```

容器重启后，在 guest 中重新收敛 sidecar：

```bash
mkdir -p /run/samba/ncalrpc
systemctl start ccbg-smb-sidecar-sync.service
python3 /opt/ccbg/scripts/ccbg-smb-sidecar.py status
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
- 默认把 Admin Web 打开到 `http://<LXC-IP>:61081/`
- 仅在 `--enable-smb-sidecar` profile 下安装并启用 `ccbg-smb-sidecar.path`、`ccbg-smb-sidecar.timer` 和 `ccbg-smb-sidecar-sync.service`

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
- `GET /readyz` 返回 200；如果配置了 `CCBG_CONTROL_API_KEY`，必须带 `x-api-key: <key>` 或 `?api_key=<key>`
- SigV4 `ListBuckets` 返回 200
- 浏览器访问 `http://<LXC-IP>:61081/` 返回登录页或 Admin Web
- `/etc/ccbg/ccbg.env` 中 `CCBG_ONEDRIVE_ENABLED=false`

`scripts/smoke.sh` 会自动读取 `/etc/ccbg/ccbg.env` 里的 `CCBG_CONTROL_API_KEY`，并用
`x-api-key` 探测 metrics `readyz`。如果你想手工验证，可直接运行:

```bash
curl -H "x-api-key: $(grep '^CCBG_CONTROL_API_KEY=' /etc/ccbg/ccbg.env | cut -d= -f2-)" http://127.0.0.1:61083/readyz
```

SMB sidecar profile 额外验收:

```bash
sudo systemctl status ccbg-smb-sidecar.path ccbg-smb-sidecar.timer --no-pager
sudo systemctl start ccbg-smb-sidecar-sync.service
sudo cat /var/lib/ccbg/smb-sidecar/status.json
sudo systemctl list-units 'ccbg-smb-sidecar-*.service' --no-pager
```

验收标准:

- `--s3-only` 安装后 `ccbg-smb-sidecar.*` units 不应被启用。
- `--enable-smb-sidecar` 安装后 path/timer 应启用。
- SMB 配置完整时，`status.json` 应显示 `state=running`、`listener_ready=true`，且 share mount 计数等于启用 share 数。
- SMB 用户/share 尚未配置时，`status.json` 应显示 `state=running`、`listener_ready=true`、share 计数为 `0`，且 Admin 的 SMB Runtime 区块能展示当前监听状态。
- `ccbg-smb-sidecar-sync.service` 可以是 `inactive (dead)`；真正的长生命周期进程应出现在独立的 `ccbg-smb-sidecar-*.service` transient units 中。
- LXC guest 准备挂载 share 前应能访问 `/dev/fuse`；否则 `CCBGRoot` 会自动生成，但 share mount 不会成功。

## 升级

1. 上传新 `ccbg-lxc-package.tar.gz` 到 LXC guest。
2. 使用 `tar --no-same-owner -xzf ccbg-lxc-package.tar.gz` 解包后运行 `sudo scripts/install.sh`。
3. 运行 `sudo scripts/smoke.sh`。
4. 查看 `journalctl -u ccbg.service -n 100 --no-pager`。

Windows + Podman 本地构建主机的推荐升级流程:

```bash
scripts/build-linux-release-in-podman.sh --target x86_64-unknown-linux-gnu --package gatewayd
scripts/build-lxc-package.sh --skip-build --target x86_64-unknown-linux-gnu
scp target/lxc-package/ccbg-lxc-package.tar.gz root@<guest-ip>:/tmp/ccbg-lxc-package.tar.gz
ssh root@<guest-ip>
rm -rf /tmp/ccbg-lxc-package
mkdir -p /tmp/ccbg-lxc-package
cd /tmp/ccbg-lxc-package
tar --no-same-owner -xzf /tmp/ccbg-lxc-package.tar.gz
cd ccbg-lxc-package
./scripts/install.sh --enable-smb-sidecar
```

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
- `61080` 是 S3 API，不是浏览器管理页；浏览器管理页在 `61081`

OneDrive 仍是 parking provider。除非有真实需求和单独回归，不要把 `onedrive` 加入默认 sync/fallback。

## 验收记录

- 2026-06-03 `.49` LXC + SMB sidecar + no-stub 验收见 [ops-008-49-lxc-smb-stub-removal.md](ops-008-49-lxc-smb-stub-removal.md)。
- 2026-06-09 `.49` `96 MiB` LXC 的明文/加密压力验证与压测脏数据清理见 [ops-011-49-encrypted-soak-and-cleanup.md](ops-011-49-encrypted-soak-and-cleanup.md)。
- 2026-06-17 `.49` carrier login browser-flow LLM repair 部署见 [ops-012-49-browser-flow-llm-repair-deploy.md](ops-012-49-browser-flow-llm-repair-deploy.md)。
- 2026-06-24 `.49` MCP 公开发现、应用存储映射字段和 affiliation 写入偏好部署见 [ops-013-49-mcp-storage-affiliation-deploy.md](ops-013-49-mcp-storage-affiliation-deploy.md)。

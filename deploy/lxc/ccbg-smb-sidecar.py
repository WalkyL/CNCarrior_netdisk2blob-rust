#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
from __future__ import annotations

import grp
import hashlib
import json
import os
import pathlib
import pwd
import shlex
import shutil
import signal
import socket
import subprocess
import sys
import time
from typing import Any


GROUP_NAME = "ccbg-smb"
STATUS_SCHEMA_VERSION = 1
RUNTIME_SPEC_VERSION = 2
SMBD_UNIT_NAME = "ccbg-smb-sidecar-smbd.service"


def now_ms() -> int:
    return int(time.time() * 1000)


def read_env_file(path: pathlib.Path) -> dict[str, str]:
    values: dict[str, str] = {}
    if not path.exists():
        return values
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        values[key.strip()] = value.strip()
    return values


def resolve_paths() -> dict[str, pathlib.Path]:
    env_file = pathlib.Path(os.environ.get("CCBG_ENV_FILE", "/etc/ccbg/ccbg.env"))
    env_values = read_env_file(env_file)
    control_plane_file = pathlib.Path(
        env_values.get("CCBG_CONTROL_PLANE_FILE", "/var/lib/ccbg/control-plane.json")
    )
    control_plane_dir = control_plane_file.resolve().parent
    state_root = control_plane_dir / "smb-sidecar"
    status_file = state_root / "status.json"
    metadata_file = state_root / "managed-runtime.json"
    return {
        "env_file": env_file,
        "control_plane_file": control_plane_file,
        "state_root": state_root,
        "status_file": status_file,
        "metadata_file": metadata_file,
    }


def ensure_dir(path: pathlib.Path) -> None:
    path.mkdir(parents=True, exist_ok=True)


def run_checked(
    args: list[str], *, input_text: str | None = None
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        input=input_text,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=True,
    )


def read_json(path: pathlib.Path) -> Any:
    if not path.exists():
        return None
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def atomic_write_text(path: pathlib.Path, contents: str, mode: int = 0o600) -> None:
    ensure_dir(path.parent)
    temp_path = path.with_name(f".{path.name}.tmp-{os.getpid()}")
    with temp_path.open("w", encoding="utf-8") as handle:
        handle.write(contents)
        handle.flush()
        os.fsync(handle.fileno())
    os.chmod(temp_path, mode)
    os.replace(temp_path, path)
    os.chmod(path, mode)


def write_json(path: pathlib.Path, payload: Any, mode: int = 0o600) -> None:
    serialized = json.dumps(payload, indent=2, ensure_ascii=True) + "\n"
    atomic_write_text(path, serialized, mode)


def write_text(path: pathlib.Path, contents: str, mode: int = 0o600) -> None:
    atomic_write_text(path, contents, mode)


def pid_exists(pid: int | None) -> bool:
    if not pid or pid <= 0:
        return False
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def normalize_pid(value: Any) -> int:
    try:
        return int(value or 0)
    except (TypeError, ValueError):
        return 0


def managed_unit_name(role: str, identifier: str | None = None) -> str:
    if not identifier:
        return SMBD_UNIT_NAME
    slug = "".join(char.lower() if char.isalnum() else "-" for char in identifier).strip("-")
    slug = "-".join(part for part in slug.split("-") if part) or "default"
    suffix = hashlib.sha1(identifier.encode("utf-8")).hexdigest()[:8]
    return f"ccbg-smb-sidecar-{role}-{slug}-{suffix}.service"


def systemd_run_available() -> bool:
    return shutil.which("systemd-run") is not None and shutil.which("systemctl") is not None


def systemd_unit_main_pid(unit_name: str) -> int | None:
    if not unit_name or not systemd_run_available():
        return None
    result = subprocess.run(
        ["systemctl", "show", unit_name, "--property", "MainPID", "--value"],
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return None
    pid = normalize_pid(result.stdout.strip())
    return pid or None


def stop_systemd_unit(unit_name: str | None) -> None:
    if not unit_name or not systemd_run_available():
        return
    subprocess.run(
        ["systemctl", "stop", unit_name],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    subprocess.run(
        ["systemctl", "reset-failed", unit_name],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def start_transient_unit(unit_name: str, command: list[str], log_path: pathlib.Path) -> int:
    ensure_dir(log_path.parent)
    stop_systemd_unit(unit_name)
    shell_command = (
        f"exec >>{shlex.quote(str(log_path))} 2>&1; exec "
        + " ".join(shlex.quote(part) for part in command)
    )
    run_checked(
        [
            "systemd-run",
            "--collect",
            "--quiet",
            "--service-type=exec",
            "--unit",
            unit_name,
            "/bin/bash",
            "-lc",
            shell_command,
        ]
    )
    time.sleep(1.0)
    pid = systemd_unit_main_pid(unit_name)
    if not pid or not pid_exists(pid):
        raise RuntimeError(f"managed unit {unit_name} exited immediately; see {log_path}")
    return pid


def local_ipv4_interface_specs() -> list[str]:
    specs = ["127.0.0.1/8"]
    try:
        result = subprocess.run(
            ["ip", "-o", "-4", "addr", "show", "scope", "global", "up"],
            check=False,
            capture_output=True,
            text=True,
            timeout=3,
        )
    except (OSError, subprocess.TimeoutExpired):
        return specs

    for line in result.stdout.splitlines():
        parts = line.split()
        try:
            cidr = parts[parts.index("inet") + 1]
        except (ValueError, IndexError):
            continue
        if cidr.startswith("127."):
            continue
        specs.append(cidr)
    return list(dict.fromkeys(specs))


def terminate_pid(pid: int | None) -> None:
    if not pid_exists(pid):
        return
    try:
        os.kill(pid, signal.SIGTERM)
    except OSError:
        return
    for _ in range(25):
        if not pid_exists(pid):
            return
        time.sleep(0.2)
    try:
        os.kill(pid, signal.SIGKILL)
    except OSError:
        return


def process_cmdline(pid: int) -> list[str]:
    try:
        raw = (pathlib.Path("/proc") / str(pid) / "cmdline").read_bytes()
    except (FileNotFoundError, PermissionError):
        return []
    return [part.decode("utf-8", errors="replace") for part in raw.split(b"\0") if part]


def choose_fusermount() -> str | None:
    for name in ("fusermount3", "fusermount"):
        path = shutil.which(name)
        if path:
            return path
    return None


def unmount_path(path: pathlib.Path) -> None:
    fusermount = choose_fusermount()
    if fusermount and path.exists():
        subprocess.run([fusermount, "-u", str(path)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run([fusermount, "-uz", str(path)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    subprocess.run(["umount", str(path)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    subprocess.run(["umount", "-l", str(path)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def mountpoint_active(path: pathlib.Path) -> bool:
    return subprocess.run(
        ["mountpoint", "-q", str(path)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    ).returncode == 0


def ensure_group(name: str) -> grp.struct_group:
    try:
        return grp.getgrnam(name)
    except KeyError:
        run_checked(["groupadd", "--system", name])
        return grp.getgrnam(name)


def ensure_user(username: str, home_root: pathlib.Path, group_name: str) -> pwd.struct_passwd:
    try:
        return pwd.getpwnam(username)
    except KeyError:
        user_home = home_root / username
        ensure_dir(user_home)
        run_checked(
            [
                "useradd",
                "--system",
                "--gid",
                group_name,
                "--home-dir",
                str(user_home),
                "--create-home",
                "--shell",
                "/usr/sbin/nologin",
                username,
            ]
        )
        return pwd.getpwnam(username)


def parse_control_plane(paths: dict[str, pathlib.Path]) -> dict[str, Any]:
    payload = read_json(paths["control_plane_file"])
    if not isinstance(payload, dict):
        return {
            "enabled": False,
            "reason": "control-plane JSON not available yet",
            "smb": {},
            "applications": [],
        }
    smb = payload.get("smb_sidecar") or {}
    applications = payload.get("applications") or []
    return {
        "enabled": bool(smb.get("enabled")),
        "reason": "",
        "smb": smb,
        "applications": applications,
    }


def normalized_path(raw: Any, fallback: pathlib.Path) -> pathlib.Path:
    text = str(raw or "").strip()
    return pathlib.Path(text) if text else fallback


def normalize_gateway_endpoint(raw: str) -> str:
    value = str(raw or "").strip() or "127.0.0.1:61080"
    if value.startswith("[") and "]:" in value:
        host, port = value[1:].split("]:", 1)
        if host == "::":
            host = "::1"
        return f"[{host}]:{port}"
    if value.count(":") == 1:
        host, port = value.rsplit(":", 1)
        host = host.strip()
        if host in ("", "0.0.0.0", "*"):
            host = "127.0.0.1"
        elif host == "::":
            host = "::1"
        return f"{host}:{port}"
    return value


def normalized_mount_root(paths: dict[str, pathlib.Path], smb: dict[str, Any]) -> pathlib.Path:
    env_values = read_env_file(paths["env_file"])
    return normalized_path(
        smb.get("mount_root"),
        pathlib.Path(
            str(env_values.get("CCBG_SMB_MOUNT_ROOT") or "").strip()
            or "/mnt/ccbg/smb/mounts"
        ),
    )


def build_share_models(
    paths: dict[str, pathlib.Path], control_plane: dict[str, Any], env_values: dict[str, str]
) -> dict[str, Any]:
    smb = control_plane["smb"] or {}
    applications = {
        str(entry.get("id") or "").strip(): entry
        for entry in (control_plane.get("applications") or [])
        if isinstance(entry, dict) and str(entry.get("id") or "").strip()
    }
    if not applications:
        access_key_id = str(env_values.get("CCBG_S3_ACCESS_KEY_ID") or "ccbg").strip()
        secret_access_key = str(env_values.get("CCBG_S3_SECRET_ACCESS_KEY") or "change-me").strip()
        if access_key_id and secret_access_key:
            applications["default"] = {
                "id": "default",
                "access_key_id": access_key_id,
                "secret_access_key": secret_access_key,
                "enabled": True,
            }
    enabled_users = [entry for entry in (smb.get("users") or []) if entry.get("enabled", True)]
    enabled_shares = [entry for entry in (smb.get("shares") or []) if entry.get("enabled", True)]
    mount_root = normalized_mount_root(paths, smb)
    config_root = normalized_path(
        smb.get("config_root"),
        pathlib.Path(
            str(env_values.get("CCBG_SMB_CONFIG_ROOT") or "").strip()
            or str(paths["state_root"] / "config")
        ),
    )
    data_root = normalized_path(
        smb.get("data_root"),
        pathlib.Path(
            str(env_values.get("CCBG_SMB_DATA_ROOT") or "").strip()
            or str(paths["state_root"] / "data")
        ),
    )
    home_root = data_root / "homes"
    runtime_root = data_root / "runtime"
    smb_conf_path = config_root / "smb" / "smb.conf"
    rclone_conf_path = config_root / "rclone" / "rclone.conf"
    gateway_endpoint = normalize_gateway_endpoint(
        str(env_values.get("CCBG_BIND_ADDR", "127.0.0.1:61080"))
    )
    region = str(env_values.get("CCBG_S3_REGION", "us-east-1")).strip() or "us-east-1"
    bind_addr = str(smb.get("bind_addr") or "0.0.0.0").strip() or "0.0.0.0"
    server_string = str(smb.get("server_string") or "").strip() or "CCBG SMB Sidecar"
    base = {
        "mount_root": mount_root,
        "config_root": config_root,
        "data_root": data_root,
        "home_root": home_root,
        "runtime_root": runtime_root,
        "smb_conf_path": smb_conf_path,
        "rclone_conf_path": rclone_conf_path,
        "smbd_unit_name": managed_unit_name("smbd"),
        "applications": applications,
        "bind_addr": bind_addr,
        "port": int(smb.get("port") or 445),
        "workgroup": str(smb.get("workgroup") or "WORKGROUP").strip() or "WORKGROUP",
        "server_string": server_string,
        "create_mask": str(smb.get("create_mask") or "0660").strip() or "0660",
        "directory_mask": str(smb.get("directory_mask") or "0770").strip() or "0770",
        "disable_splice": bool(smb.get("disable_splice")),
        "vfs_objects": [
            str(value).strip()
            for value in (smb.get("vfs_objects") or [])
            if str(value).strip()
        ],
        "gateway_endpoint": gateway_endpoint,
        "region": region,
    }

    users = []
    users_by_id: dict[str, dict[str, Any]] = {}
    for entry in enabled_users:
        user_id = str(entry.get("id") or "").strip()
        username = str(entry.get("username") or "").strip()
        password = str(entry.get("password") or "")
        if not user_id or not username or not password:
            raise RuntimeError(
                f"SMB user is missing id/username/password: {user_id or username or '<unknown>'}"
            )
        model = {
            "id": user_id,
            "username": username,
            "password": password,
            "allowed_share_ids": [
                str(value).strip()
                for value in (entry.get("allowed_share_ids") or [])
                if str(value).strip()
            ],
        }
        users.append(model)
        users_by_id[user_id] = model

    shares = []
    for entry in enabled_shares:
        share_id = str(entry.get("id") or "").strip()
        application_id = str(entry.get("application_id") or "").strip()
        share_name = str(entry.get("share_name") or "").strip()
        bucket = str(entry.get("bucket") or "").strip()
        if not share_id or not share_name or not application_id or not bucket:
            raise RuntimeError(
                "SMB share is missing id/share_name/application_id/bucket: "
                f"{share_id or share_name or '<unknown>'}"
            )
        application = applications.get(application_id)
        if not application:
            raise RuntimeError(
                f"SMB share {share_id} references unknown application {application_id}"
            )
        access_key_id = str(application.get("access_key_id") or "").strip()
        secret_access_key = str(application.get("secret_access_key") or "").strip()
        if not access_key_id or not secret_access_key:
            raise RuntimeError(
                "SMB share "
                f"{share_id} references application {application_id} without complete S3 credentials"
            )
        prefix = str(entry.get("prefix") or "").strip().strip("/")
        remote_path = bucket if not prefix else f"{bucket}/{prefix}"
        share_usernames = []
        for user_id in (entry.get("valid_user_ids") or []):
            username = users_by_id.get(str(user_id).strip(), {}).get("username")
            if username:
                share_usernames.append(username)
        mount_path = mount_root / share_id
        shares.append(
            {
                "id": share_id,
                "share_name": share_name,
                "application_id": application_id,
                "bucket": bucket,
                "prefix": prefix,
                "remote_path": remote_path,
                "read_only": bool(entry.get("read_only")),
                "browseable": bool(entry.get("browseable", True)),
                "guest_ok": bool(entry.get("guest_ok")),
                "valid_usernames": share_usernames,
                "mount_path": mount_path,
                "unit_name": managed_unit_name("rclone", share_id),
                "create_mask": str(entry.get("create_mask") or smb.get("create_mask") or "0660").strip()
                or "0660",
                "directory_mask": str(
                    entry.get("directory_mask") or smb.get("directory_mask") or "0770"
                ).strip()
                or "0770",
                "access_key_id": access_key_id,
                "secret_access_key": secret_access_key,
            }
        )

    return {
        "enabled": True,
        "reason": "",
        "users": users,
        "shares": shares,
        **base,
    }


def runtime_spec_payload(model: dict[str, Any]) -> dict[str, Any]:
    return {
        "runtime_spec_version": RUNTIME_SPEC_VERSION,
        "bind_addr": model["bind_addr"],
        "config_root": str(model["config_root"]),
        "create_mask": model["create_mask"],
        "data_root": str(model["data_root"]),
        "directory_mask": model["directory_mask"],
        "disable_splice": model["disable_splice"],
        "gateway_endpoint": model["gateway_endpoint"],
        "mount_root": str(model["mount_root"]),
        "port": model["port"],
        "region": model["region"],
        "server_string": model["server_string"],
        "shares": [
            {
                "id": share["id"],
                "share_name": share["share_name"],
                "application_id": share["application_id"],
                "bucket": share["bucket"],
                "prefix": share["prefix"],
                "remote_path": share["remote_path"],
                "read_only": share["read_only"],
                "browseable": share["browseable"],
                "guest_ok": share["guest_ok"],
                "valid_usernames": share["valid_usernames"],
                "mount_path": str(share["mount_path"]),
                "create_mask": share["create_mask"],
                "directory_mask": share["directory_mask"],
                "access_key_id": share["access_key_id"],
                "secret_access_key": share["secret_access_key"],
            }
            for share in model["shares"]
        ],
        "users": [
            {
                "id": user["id"],
                "username": user["username"],
                "password": user["password"],
                "allowed_share_ids": user["allowed_share_ids"],
            }
            for user in model["users"]
        ],
        "vfs_objects": model["vfs_objects"],
        "workgroup": model["workgroup"],
    }


def desired_hash_for_model(model: dict[str, Any]) -> str:
    serialized = json.dumps(runtime_spec_payload(model), sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(serialized.encode("utf-8")).hexdigest()


def generate_rclone_conf(model: dict[str, Any]) -> str:
    sections: list[str] = []
    for share in model["shares"]:
        section = "\n".join(
            [
                f"[ccbg-{share['id']}]",
                "type = s3",
                "provider = Other",
                "env_auth = false",
                f"access_key_id = {share['access_key_id']}",
                f"secret_access_key = {share['secret_access_key']}",
                f"region = {model['region']}",
                f"endpoint = http://{model['gateway_endpoint']}",
                "force_path_style = true",
                "no_check_bucket = true",
                "",
            ]
        )
        sections.append(section)
    return "\n".join(sections)


def generate_smb_conf(model: dict[str, Any]) -> str:
    runtime_root = pathlib.Path(model["runtime_root"])
    run_dir = runtime_root / "run"
    private_dir = runtime_root / "private"
    state_dir = runtime_root / "state"
    cache_dir = runtime_root / "cache"
    lock_dir = runtime_root / "locks"
    log_dir = runtime_root / "logs"
    for directory in (run_dir, private_dir, state_dir, cache_dir, lock_dir, log_dir):
        ensure_dir(directory)

    global_lines = [
        "[global]",
        f"   workgroup = {model['workgroup']}",
        f"   server string = {model['server_string']}",
        "   map to guest = Never",
        "   load printers = no",
        "   printing = bsd",
        "   disable spoolss = yes",
        "   passdb backend = tdbsam",
        "   security = user",
        f"   create mask = {model['create_mask']}",
        f"   directory mask = {model['directory_mask']}",
        "   vfs objects = "
        + (" ".join(model["vfs_objects"]) if model["vfs_objects"] else "streams_xattr catia fruit"),
        "   ea support = yes",
        "   store dos attributes = yes",
        "   fruit:metadata = stream",
        "   fruit:model = MacSamba",
        "   fruit:posix_rename = yes",
        "   fruit:veto_appledouble = no",
        f"   use sendfile = {'no' if model['disable_splice'] else 'yes'}",
        f"   pid directory = {run_dir}",
        f"   lock directory = {lock_dir}",
        f"   state directory = {state_dir}",
        f"   cache directory = {cache_dir}",
        f"   private dir = {private_dir}",
        f"   log file = {log_dir / 'smbd.log'}",
        "   max log size = 10000",
        f"   smb ports = {model['port']}",
    ]
    bind_addr = str(model["bind_addr"]).strip()
    if bind_addr in ("0.0.0.0", "*"):
        global_lines.append(f"   interfaces = {' '.join(local_ipv4_interface_specs())}")
        global_lines.append("   bind interfaces only = yes")
    elif bind_addr not in ("::", ""):
        global_lines.append(f"   interfaces = {bind_addr}")
        global_lines.append("   bind interfaces only = yes")

    body = ["\n".join(global_lines)]
    for share in model["shares"]:
        share_lines = [
            f"[{share['share_name']}]",
            f"   path = {share['mount_path']}",
            "   comment = "
            f"app={share['application_id']} | bucket={share['bucket']}"
            + (f" | prefix={share['prefix']}" if share["prefix"] else ""),
            f"   browseable = {'yes' if share['browseable'] else 'no'}",
            f"   read only = {'yes' if share['read_only'] else 'no'}",
            f"   guest ok = {'yes' if share['guest_ok'] else 'no'}",
            f"   create mask = {share['create_mask']}",
            f"   directory mask = {share['directory_mask']}",
        ]
        if share["valid_usernames"]:
            share_lines.append(f"   valid users = {' '.join(share['valid_usernames'])}")
        body.append("\n".join(share_lines))
    return "\n\n".join(body) + "\n"


def prepare_samba_runtime_tree(model: dict[str, Any]) -> None:
    runtime_root = pathlib.Path(model["runtime_root"])
    volatile_paths = [
        runtime_root / "locks" / "msg.lock",
        runtime_root / "locks" / "msg.sock",
        runtime_root / "logs" / "cores",
    ]
    for path in volatile_paths:
        if path.exists():
            if path.is_dir():
                shutil.rmtree(path, ignore_errors=True)
            else:
                try:
                    path.unlink()
                except OSError:
                    pass

    for root, dirs, files in os.walk(runtime_root):
        root_path = pathlib.Path(root)
        os.chown(root_path, 0, 0)
        os.chmod(root_path, 0o700 if root_path.name == "private" else 0o755)
        for name in dirs:
            path = root_path / name
            os.chown(path, 0, 0)
            os.chmod(path, 0o700 if path.name == "private" else 0o755)
        for name in files:
            path = root_path / name
            os.chown(path, 0, 0)


def ensure_mount_permissions(mount_root: pathlib.Path, group_entry: grp.struct_group) -> None:
    ensure_dir(mount_root)
    os.chown(mount_root, 0, group_entry.gr_gid)
    os.chmod(mount_root, 0o770)


def process_rss_bytes(pid: int | None) -> int:
    if not pid:
        return 0
    status_path = pathlib.Path("/proc") / str(pid) / "status"
    try:
        for line in status_path.read_text(encoding="utf-8").splitlines():
            if line.startswith("VmRSS:"):
                parts = line.split()
                if len(parts) >= 2:
                    return int(parts[1]) * 1024
    except (FileNotFoundError, PermissionError, ValueError):
        return 0
    return 0


def find_process_pid(processes: list[dict[str, Any]], role: str) -> int | None:
    for entry in processes:
        if str(entry.get("role") or "") == role:
            pid = normalize_pid(entry.get("pid"))
            return pid or None
    return None


def find_smbd_pid_for_model(model: dict[str, Any] | None) -> int | None:
    if not model:
        return None
    smb_conf_path = str(model.get("smb_conf_path") or "")
    if not smb_conf_path:
        return None
    for proc_dir in pathlib.Path("/proc").iterdir():
        if not proc_dir.name.isdigit():
            continue
        pid = int(proc_dir.name)
        if pid == os.getpid():
            continue
        args = process_cmdline(pid)
        if not args:
            continue
        executable = pathlib.Path(args[0]).name
        if executable == "smbd" and smb_conf_path in args:
            return pid
    return None


def build_process_payload(role: str, pid: int | None, extra: dict[str, Any] | None = None) -> dict[str, Any]:
    normalized_pid = normalize_pid(pid)
    exists = pid_exists(normalized_pid)
    payload = {
        "role": role,
        "pid": normalized_pid if exists else None,
        "running": exists,
        "rss_bytes": process_rss_bytes(normalized_pid) if exists else 0,
    }
    if extra:
        payload.update(extra)
    return payload


def runtime_metadata(paths: dict[str, pathlib.Path]) -> dict[str, Any]:
    payload = read_json(paths["metadata_file"])
    return payload if isinstance(payload, dict) else {}


def stop_previous_runtime(paths: dict[str, pathlib.Path]) -> None:
    previous = runtime_metadata(paths)
    share_states = previous.get("share_states") or []
    processes = previous.get("processes") or []

    stopped_units = set()
    for process in processes:
        unit_name = str(process.get("unit_name") or "").strip()
        if unit_name and unit_name not in stopped_units:
            stop_systemd_unit(unit_name)
            stopped_units.add(unit_name)
    for share in share_states:
        unit_name = str(share.get("unit_name") or "").strip()
        if unit_name and unit_name not in stopped_units:
            stop_systemd_unit(unit_name)
            stopped_units.add(unit_name)

    for process in processes:
        pid = normalize_pid(process.get("pid"))
        terminate_pid(pid)

    for share in share_states:
        mount_text = str(share.get("mount_path") or "").strip()
        if not mount_text:
            continue
        mount_path = pathlib.Path(mount_text)
        if mount_path.exists():
            unmount_path(mount_path)


def stop_model_runtime(model: dict[str, Any]) -> None:
    smb_conf_path = str(model["smb_conf_path"])
    rclone_remotes = {f"ccbg-{share['id']}:{share['remote_path']}" for share in model["shares"]}
    mount_paths = {str(share["mount_path"]) for share in model["shares"]}
    stop_systemd_unit(str(model.get("smbd_unit_name") or "").strip())
    for share in model["shares"]:
        stop_systemd_unit(str(share.get("unit_name") or "").strip())

    for proc_dir in pathlib.Path("/proc").iterdir():
        if not proc_dir.name.isdigit():
            continue
        pid = int(proc_dir.name)
        if pid == os.getpid():
            continue
        args = process_cmdline(pid)
        if not args:
            continue
        executable = pathlib.Path(args[0]).name
        if executable == "rclone" and len(args) >= 4 and args[1] == "mount":
            if args[2] in rclone_remotes or args[3] in mount_paths:
                terminate_pid(pid)
        elif executable == "smbd" and smb_conf_path in args:
            terminate_pid(pid)

    for mount_path in mount_paths:
        path = pathlib.Path(mount_path)
        if path.exists():
            unmount_path(path)


def write_runtime_files(model: dict[str, Any]) -> None:
    write_text(pathlib.Path(model["rclone_conf_path"]), generate_rclone_conf(model))
    write_text(pathlib.Path(model["smb_conf_path"]), generate_smb_conf(model))


def ensure_samba_users(model: dict[str, Any]) -> None:
    conf_path = pathlib.Path(model["smb_conf_path"])
    home_root = pathlib.Path(model["home_root"])
    ensure_dir(home_root)
    for user in model["users"]:
        ensure_user(user["username"], home_root, GROUP_NAME)
        run_checked(
            ["smbpasswd", "-c", str(conf_path), "-s", "-a", user["username"]],
            input_text=f"{user['password']}\n{user['password']}\n",
        )
        subprocess.run(
            ["smbpasswd", "-c", str(conf_path), "-e", user["username"]],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )


def fuse_unavailable_message(share: dict[str, Any]) -> str:
    return (
        "SMB share mounts need /dev/fuse. The managed smbd listener can "
        "run without shares, but rclone-backed shares such as "
        f"{share['share_name']} cannot mount until this LXC/container "
        "exposes /dev/fuse."
    )


def start_rclone_mounts(model: dict[str, Any], group_entry: grp.struct_group) -> list[dict[str, Any]]:
    log_dir = pathlib.Path(model["runtime_root"]) / "logs"
    ensure_dir(log_dir)

    share_states: list[dict[str, Any]] = []
    for share in model["shares"]:
        mount_path = pathlib.Path(share["mount_path"])
        ensure_dir(mount_path)
        os.chown(mount_path, 0, group_entry.gr_gid)
        os.chmod(mount_path, 0o770)
        log_path = log_dir / f"rclone-{share['id']}.log"
        if not pathlib.Path("/dev/fuse").exists():
            last_error = fuse_unavailable_message(share)
            with log_path.open("ab") as log_handle:
                log_handle.write(f"{last_error}\n".encode("utf-8"))
            share_states.append(
                {
                    "id": share["id"],
                    "share_name": share["share_name"],
                    "mount_path": str(mount_path),
                    "remote_path": share["remote_path"],
                    "read_only": share["read_only"],
                    "unit_name": share["unit_name"],
                    "pid": None,
                    "mounted": False,
                    "running": False,
                    "rss_bytes": 0,
                    "log_path": str(log_path),
                    "last_error": last_error,
                }
            )
            continue

        log_handle = log_path.open("ab")
        cmd = [
            "rclone",
            "mount",
            f"ccbg-{share['id']}:{share['remote_path']}",
            str(mount_path),
            "--config",
            str(model["rclone_conf_path"]),
            "--allow-other",
            "--dir-cache-time",
            "30s",
            "--vfs-cache-mode",
            "minimal",
            "--uid",
            "0",
            "--gid",
            str(group_entry.gr_gid),
            "--dir-perms",
            "0770",
            "--file-perms",
            "0660",
        ]
        if share["read_only"]:
            cmd += ["--read-only"]
        unit_name = str(share["unit_name"])
        if systemd_run_available():
            log_handle.close()
            pid = start_transient_unit(unit_name, cmd, log_path)
        else:
            process = subprocess.Popen(
                cmd,
                stdout=log_handle,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
            time.sleep(1.0)
            if process.poll() is not None:
                log_handle.close()
                raise RuntimeError(
                    f"rclone mount for share {share['id']} exited immediately; see {log_path}"
                )
            pid = process.pid
        share_states.append(
            {
                "id": share["id"],
                "share_name": share["share_name"],
                "mount_path": str(mount_path),
                "remote_path": share["remote_path"],
                "read_only": share["read_only"],
                "pid": pid,
                "unit_name": unit_name,
                "mounted": mountpoint_active(mount_path),
                "running": pid_exists(pid),
                "rss_bytes": process_rss_bytes(pid),
                "log_path": str(log_path),
            }
        )
    return share_states


def start_smbd(model: dict[str, Any]) -> tuple[int, str]:
    runtime_root = pathlib.Path(model["runtime_root"])
    log_dir = runtime_root / "logs"
    ensure_dir(log_dir)
    # Debian's smbd still expects the system runtime pipe root under /run/samba.
    # LXC guests without the distro smbd unit may not create it for us.
    ensure_dir(pathlib.Path("/run/samba"))
    ensure_dir(pathlib.Path("/run/samba/ncalrpc"))
    log_path = log_dir / "smbd-launch.log"
    command = [
        "smbd",
        "--foreground",
        "--no-process-group",
        "-s",
        str(model["smb_conf_path"]),
    ]
    if systemd_run_available():
        pid = start_transient_unit(str(model["smbd_unit_name"]), command, log_path)
        return pid, str(log_path)

    log_handle = log_path.open("ab")
    process = subprocess.Popen(
        command,
        stdout=log_handle,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    time.sleep(1.0)
    if process.poll() is not None:
        log_handle.close()
        raise RuntimeError(f"smbd exited immediately; see {log_path}")
    return process.pid, str(log_path)


def listener_probe_target(bind_addr: str, port: int) -> tuple[socket.AddressFamily, tuple[Any, ...]]:
    host = bind_addr.strip()
    if host in ("", "0.0.0.0", "*"):
        return socket.AF_INET, ("127.0.0.1", port)
    if host == "::":
        return socket.AF_INET6, ("::1", port, 0, 0)
    if ":" in host and not host.count("."):
        return socket.AF_INET6, (host, port, 0, 0)
    return socket.AF_INET, (host, port)


def listener_ready(bind_addr: str, port: int) -> bool:
    family, target = listener_probe_target(bind_addr, port)
    try:
        with socket.socket(family, socket.SOCK_STREAM) as handle:
            handle.settimeout(1.0)
            handle.connect(target)
        return True
    except OSError:
        return False


def wait_for_listener(bind_addr: str, port: int, timeout_seconds: float) -> bool:
    deadline = time.time() + timeout_seconds
    while time.time() < deadline:
        if listener_ready(bind_addr, port):
            return True
        time.sleep(0.25)
    return listener_ready(bind_addr, port)


def refresh_share_states(shares: list[dict[str, Any]]) -> list[dict[str, Any]]:
    refreshed = []
    for share in shares:
        pid = normalize_pid(share.get("pid"))
        mount_path = pathlib.Path(str(share.get("mount_path") or ""))
        refreshed.append(
            {
                **share,
                "pid": pid if pid_exists(pid) else None,
                "running": pid_exists(pid),
                "mounted": mountpoint_active(mount_path) if mount_path.exists() else False,
                "rss_bytes": process_rss_bytes(pid),
            }
        )
    return refreshed


def build_runtime_payload(
    paths: dict[str, pathlib.Path],
    model: dict[str, Any] | None,
    previous: dict[str, Any],
    *,
    state: str,
    last_error: str | None,
) -> dict[str, Any]:
    processes = previous.get("processes") or []
    smbd_pid = find_process_pid(processes, "smbd")
    if not smbd_pid:
        smbd_pid = find_smbd_pid_for_model(model)
    share_states = refresh_share_states(previous.get("share_states") or [])
    refreshed_processes = []
    if smbd_pid:
        smbd_extra = {}
        for process in processes:
            if str(process.get("role") or "") == "smbd":
                smbd_extra = {
                    key: value
                    for key, value in process.items()
                    if key not in {"role", "pid", "running", "rss_bytes"}
                }
                break
        refreshed_processes.append(build_process_payload("smbd", smbd_pid, smbd_extra))
    elif processes:
        refreshed_processes.append(build_process_payload("smbd", None))
    for share in share_states:
        extra = {
            "share_id": share.get("id"),
            "share_name": share.get("share_name"),
            "mount_path": share.get("mount_path"),
            "log_path": share.get("log_path"),
        }
        pid = normalize_pid(share.get("pid"))
        refreshed_processes.append(build_process_payload(f"rclone:{share.get('id')}", pid, extra))

    total_rss_bytes = sum(int(entry.get("rss_bytes") or 0) for entry in refreshed_processes)
    running_process_count = sum(1 for entry in refreshed_processes if entry.get("running"))
    mounted_share_count = sum(1 for share in share_states if share.get("mounted"))
    enabled_share_count = len(model["shares"]) if model else len(share_states)
    listener = None
    listener_is_ready = False
    if model:
        listener = f"{model['bind_addr']}:{model['port']}"
        if smbd_pid and pid_exists(smbd_pid):
            listener_is_ready = listener_ready(model["bind_addr"], model["port"])

    payload = {
        "schema_version": STATUS_SCHEMA_VERSION,
        "state": state,
        "mode": "host_process",
        "auto_managed": True,
        "listener": listener,
        "listener_ready": listener_is_ready,
        "enabled_share_count": enabled_share_count,
        "mounted_share_count": mounted_share_count,
        "process_count": running_process_count,
        "total_rss_bytes": total_rss_bytes,
        "last_error": last_error,
        "share_states": share_states,
        "processes": refreshed_processes,
        "runtime_root": str(model["runtime_root"]) if model else str(paths["state_root"]),
        "mount_root": str(model["mount_root"]) if model else previous.get("mount_root"),
        "config_root": str(model["config_root"]) if model else previous.get("config_root"),
        "data_root": str(model["data_root"]) if model else previous.get("data_root"),
        "desired_hash": previous.get("desired_hash"),
        "last_success_at_unix_ms": previous.get("last_success_at_unix_ms"),
    }
    return payload


def write_status(paths: dict[str, pathlib.Path], payload: dict[str, Any]) -> None:
    payload["schema_version"] = STATUS_SCHEMA_VERSION
    payload["status_updated_at_unix_ms"] = now_ms()
    write_json(paths["status_file"], payload)


def share_state_errors(share_states: list[dict[str, Any]]) -> list[str]:
    errors = []
    seen = set()
    for share in share_states:
        message = str(share.get("last_error") or "").strip()
        if message and message not in seen:
            seen.add(message)
            errors.append(message)
    return errors


def sync_runtime() -> int:
    paths = resolve_paths()
    ensure_dir(paths["state_root"])
    env_values = read_env_file(paths["env_file"])
    control_plane = parse_control_plane(paths)

    if not control_plane["enabled"]:
        stop_previous_runtime(paths)
        payload = {
            "state": "disabled",
            "mode": "host_process",
            "auto_managed": True,
            "listener": None,
            "listener_ready": False,
            "enabled_share_count": 0,
            "mounted_share_count": 0,
            "process_count": 0,
            "total_rss_bytes": 0,
            "last_error": control_plane["reason"] or None,
            "share_states": [],
            "processes": [],
            "runtime_root": str(paths["state_root"]),
        }
        write_status(paths, payload)
        write_json(paths["metadata_file"], payload)
        return 0

    try:
        model = build_share_models(paths, control_plane, env_values)
        desired_hash = desired_hash_for_model(model)
        previous = runtime_metadata(paths)
        if previous.get("desired_hash") == desired_hash:
            payload = build_runtime_payload(paths, model, previous, state="running", last_error=None)
            all_processes_running = payload["process_count"] == len(payload["processes"]) and len(payload["processes"]) > 0
            all_shares_mounted = payload["mounted_share_count"] == len(model["shares"])
            previous_share_errors = share_state_errors(payload.get("share_states") or [])
            if payload.get("listener_ready") and previous_share_errors:
                payload["state"] = "degraded"
                payload["last_error"] = " ".join(previous_share_errors)
                write_status(paths, payload)
                write_json(paths["metadata_file"], payload)
                return 0
            if all_processes_running and all_shares_mounted and payload.get("listener_ready"):
                payload["last_success_at_unix_ms"] = previous.get("last_success_at_unix_ms") or now_ms()
                write_status(paths, payload)
                write_json(paths["metadata_file"], payload)
                return 0

        group_entry = ensure_group(GROUP_NAME)
        ensure_mount_permissions(pathlib.Path(model["mount_root"]), group_entry)
        ensure_dir(pathlib.Path(model["config_root"]))
        ensure_dir(pathlib.Path(model["data_root"]))
        write_runtime_files(model)
        prepare_samba_runtime_tree(model)
        ensure_samba_users(model)
        stop_previous_runtime(paths)
        stop_model_runtime(model)
        share_states = start_rclone_mounts(model, group_entry)
        smbd_pid, smbd_log_path = start_smbd(model)
        if not wait_for_listener(model["bind_addr"], model["port"], 5.0):
            raise RuntimeError(
                f"smbd started but did not accept connections on {model['bind_addr']}:{model['port']}"
            )

        processes = [
            build_process_payload(
                "smbd",
                smbd_pid,
                {
                    "unit_name": model["smbd_unit_name"],
                    "log_path": smbd_log_path,
                },
            )
        ]
        for share in share_states:
            processes.append(
                build_process_payload(
                    f"rclone:{share['id']}",
                    normalize_pid(share.get("pid")),
                    {
                        "share_id": share["id"],
                        "share_name": share["share_name"],
                        "mount_path": share["mount_path"],
                        "unit_name": share["unit_name"],
                        "log_path": share["log_path"],
                    },
                )
            )
        total_rss_bytes = sum(int(entry.get("rss_bytes") or 0) for entry in processes)
        mounted_share_count = sum(1 for entry in share_states if entry.get("mounted"))
        share_errors = share_state_errors(share_states)
        listener = f"{model['bind_addr']}:{model['port']}"
        payload = {
            "state": "degraded" if share_errors else "running",
            "mode": "host_process",
            "auto_managed": True,
            "listener": listener,
            "listener_ready": True,
            "enabled_share_count": len(model["shares"]),
            "mounted_share_count": mounted_share_count,
            "process_count": sum(1 for entry in processes if entry.get("running")),
            "total_rss_bytes": total_rss_bytes,
            "last_error": " ".join(share_errors) if share_errors else None,
            "share_states": share_states,
            "processes": processes,
            "runtime_root": str(model["runtime_root"]),
            "mount_root": str(model["mount_root"]),
            "config_root": str(model["config_root"]),
            "data_root": str(model["data_root"]),
            "desired_hash": desired_hash,
            "last_success_at_unix_ms": None if share_errors else now_ms(),
        }
        write_status(paths, payload)
        write_json(paths["metadata_file"], payload)
        return 0
    except Exception as error:  # noqa: BLE001
        stop_previous_runtime(paths)
        payload = build_runtime_payload(
            paths,
            locals().get("model"),
            runtime_metadata(paths),
            state="error",
            last_error=str(error),
        )
        write_status(paths, payload)
        write_json(paths["metadata_file"], payload)
        return 1


def stop_runtime() -> int:
    paths = resolve_paths()
    ensure_dir(paths["state_root"])
    stop_previous_runtime(paths)
    payload = {
        "state": "stopped",
        "mode": "host_process",
        "auto_managed": True,
        "listener": None,
        "listener_ready": False,
        "enabled_share_count": 0,
        "mounted_share_count": 0,
        "process_count": 0,
        "total_rss_bytes": 0,
        "last_error": None,
        "share_states": [],
        "processes": [],
        "runtime_root": str(paths["state_root"]),
    }
    write_status(paths, payload)
    write_json(paths["metadata_file"], payload)
    return 0


def show_status() -> int:
    paths = resolve_paths()
    payload = read_json(paths["status_file"]) or {
        "state": "unknown",
        "mode": "host_process",
        "auto_managed": True,
        "runtime_root": str(paths["state_root"]),
    }
    json.dump(payload, sys.stdout, indent=2, ensure_ascii=True)
    sys.stdout.write("\n")
    return 0


def main(argv: list[str]) -> int:
    action = argv[1] if len(argv) > 1 else "sync"
    if action == "sync":
        return sync_runtime()
    if action == "stop":
        return stop_runtime()
    if action == "status":
        return show_status()
    sys.stderr.write("usage: ccbg-smb-sidecar.py [sync|stop|status]\n")
    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))

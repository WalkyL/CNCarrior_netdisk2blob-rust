// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use hex::encode as hex_encode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::{Digest as Sha1Digest, Sha1};
use sha2::Sha256;

const DEFAULT_ENV_FILE: &str = "/etc/ccbg/ccbg.env";
const DEFAULT_CONTROL_PLANE_FILE: &str = "/var/lib/ccbg/control-plane.json";
const DEFAULT_UNIT_PREFIX: &str = "ccbg-smb-sidecar";
const DEFAULT_GATEWAY_ENDPOINT: &str = "127.0.0.1:61080";
const DEFAULT_SMB_MOUNT_ROOT: &str = "/mnt/ccbg/smb/mounts";
const DEFAULT_SMB_CONFIG_ROOT: &str = "/var/lib/ccbg/smb-sidecar/config";
const DEFAULT_SMB_DATA_ROOT: &str = "/var/lib/ccbg/smb-sidecar/data";
const DEFAULT_SMB_WORKGROUP: &str = "WORKGROUP";
const DEFAULT_SMB_SERVER_STRING: &str = "CCBG SMB Sidecar";
const DEFAULT_SMB_CREATE_MASK: &str = "0660";
const DEFAULT_SMB_DIRECTORY_MASK: &str = "0770";
const DEFAULT_S3_REGION: &str = "us-east-1";
const SMB_GROUP_NAME: &str = "ccbg-smb";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedPaths {
    env_file: PathBuf,
    control_plane_file: PathBuf,
    state_root: PathBuf,
    status_file: PathBuf,
    metadata_file: PathBuf,
}

impl ResolvedPaths {
    fn resolve() -> Result<(Self, BTreeMap<String, String>)> {
        let env_file = env::var_os("CCBG_ENV_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_ENV_FILE));
        let env_values = read_env_file(&env_file)
            .with_context(|| format!("read SMB sidecar env file {}", env_file.display()))?;
        Ok((Self::from_parts(env_file, &env_values), env_values))
    }

    fn from_parts(env_file: PathBuf, env_values: &BTreeMap<String, String>) -> Self {
        let control_plane_file = env_values
            .get("CCBG_CONTROL_PLANE_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CONTROL_PLANE_FILE));
        let control_plane_dir = control_plane_file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let state_root = control_plane_dir.join("smb-sidecar");
        let status_file = state_root.join("status.json");
        let metadata_file = state_root.join("managed-runtime.json");
        Self {
            env_file,
            control_plane_file,
            state_root,
            status_file,
            metadata_file,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ControlPlaneFile {
    #[serde(default)]
    smb_sidecar: SmbSidecarConfig,
    #[serde(default)]
    applications: Vec<DataPlaneApplication>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct SmbSidecarConfig {
    #[allow(dead_code)]
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    bind_addr: String,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    mount_root: Option<String>,
    #[serde(default)]
    config_root: Option<String>,
    #[serde(default)]
    data_root: Option<String>,
    #[serde(default)]
    workgroup: Option<String>,
    #[serde(default)]
    server_string: Option<String>,
    #[serde(default)]
    create_mask: Option<String>,
    #[serde(default)]
    directory_mask: Option<String>,
    #[serde(default)]
    disable_splice: bool,
    #[serde(default)]
    vfs_objects: Vec<String>,
    #[serde(default)]
    users: Vec<SmbUser>,
    #[serde(default)]
    shares: Vec<SmbShare>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct SmbUser {
    id: String,
    username: String,
    #[serde(default)]
    password: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    allowed_share_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct SmbShare {
    id: String,
    share_name: String,
    application_id: String,
    bucket: String,
    #[serde(default)]
    prefix: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    read_only: bool,
    #[serde(default = "default_true")]
    browseable: bool,
    #[serde(default)]
    guest_ok: bool,
    #[serde(default)]
    valid_user_ids: Vec<String>,
    #[serde(default)]
    create_mask: Option<String>,
    #[serde(default)]
    directory_mask: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct DataPlaneApplication {
    id: String,
    access_key_id: String,
    #[serde(default)]
    secret_access_key: String,
    #[allow(dead_code)]
    #[serde(default = "default_true")]
    enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SidecarModel {
    mount_root: PathBuf,
    config_root: PathBuf,
    data_root: PathBuf,
    home_root: PathBuf,
    runtime_root: PathBuf,
    smb_conf_path: PathBuf,
    rclone_conf_path: PathBuf,
    gateway_endpoint: String,
    region: String,
    bind_addr: String,
    port: u16,
    workgroup: String,
    server_string: String,
    create_mask: String,
    directory_mask: String,
    disable_splice: bool,
    vfs_objects: Vec<String>,
    users: Vec<UserModel>,
    shares: Vec<ShareModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UserModel {
    id: String,
    username: String,
    password: String,
    allowed_share_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShareModel {
    id: String,
    share_name: String,
    application_id: String,
    bucket: String,
    prefix: String,
    remote_path: String,
    read_only: bool,
    browseable: bool,
    guest_ok: bool,
    valid_usernames: Vec<String>,
    mount_path: PathBuf,
    create_mask: String,
    directory_mask: String,
    access_key_id: String,
    secret_access_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShareRuntimeState {
    id: String,
    share_name: String,
    mount_path: PathBuf,
    remote_path: String,
    read_only: bool,
    pid: Option<u32>,
    unit_name: String,
    mounted: bool,
    running: bool,
    rss_bytes: u64,
    log_path: PathBuf,
    last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagedProcessState {
    role: String,
    pid: Option<u32>,
    running: bool,
    rss_bytes: u64,
    unit_name: Option<String>,
    log_path: Option<PathBuf>,
    share_id: Option<String>,
    share_name: Option<String>,
    mount_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct RuntimeSpec<'a> {
    runtime_spec_version: u32,
    bind_addr: &'a str,
    config_root: String,
    create_mask: &'a str,
    data_root: String,
    directory_mask: &'a str,
    disable_splice: bool,
    gateway_endpoint: &'a str,
    mount_root: String,
    port: u16,
    region: &'a str,
    server_string: &'a str,
    shares: Vec<RuntimeSpecShare<'a>>,
    users: Vec<RuntimeSpecUser<'a>>,
    vfs_objects: &'a [String],
    workgroup: &'a str,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct RuntimeSpecShare<'a> {
    id: &'a str,
    share_name: &'a str,
    application_id: &'a str,
    bucket: &'a str,
    prefix: &'a str,
    remote_path: &'a str,
    read_only: bool,
    browseable: bool,
    guest_ok: bool,
    valid_usernames: &'a [String],
    mount_path: String,
    create_mask: &'a str,
    directory_mask: &'a str,
    access_key_id: &'a str,
    secret_access_key: &'a str,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct RuntimeSpecUser<'a> {
    id: &'a str,
    username: &'a str,
    password: &'a str,
    allowed_share_ids: &'a [String],
}

fn default_true() -> bool {
    true
}

fn read_env_file(path: &Path) -> Result<BTreeMap<String, String>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let raw = fs::read_to_string(path)?;
    let mut values = BTreeMap::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        values.insert(key.trim().to_string(), value.trim().to_string());
    }
    Ok(values)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("create directory {}", path.display()))
}

fn atomic_write_text(path: &Path, contents: &str) -> Result<()> {
    ensure_dir(path.parent().unwrap_or_else(|| Path::new(".")))?;
    let temp_path = path.with_file_name(format!(".{}.tmp-{}", path.file_name().and_then(|v| v.to_str()).unwrap_or("tmp"), std::process::id()));
    fs::write(&temp_path, contents)
        .with_context(|| format!("write temporary file {}", temp_path.display()))?;
    fs::rename(&temp_path, path)
        .with_context(|| format!("replace {} with temporary file", path.display()))?;
    Ok(())
}

fn write_json_value(path: &Path, payload: &Value) -> Result<()> {
    let serialized = serde_json::to_string_pretty(payload)
        .with_context(|| format!("serialize JSON for {}", path.display()))?;
    atomic_write_text(path, &(serialized + "\n"))
}

fn normalize_pid(value: Option<&Value>) -> u32 {
    value
        .and_then(Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok())
        .unwrap_or(0)
}

fn pid_exists(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn terminate_pid(pid: u32) {
    if !pid_exists(pid) {
        return;
    }
    let _ = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status();
    for _ in 0..25 {
        if !pid_exists(pid) {
            return;
        }
        sleep(Duration::from_millis(200));
    }
    let _ = Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status();
}

fn stop_systemd_unit(unit_name: &str) {
    if unit_name.trim().is_empty() {
        return;
    }
    let _ = Command::new("systemctl").args(["stop", unit_name]).status();
    let _ = Command::new("systemctl")
        .args(["reset-failed", unit_name])
        .status();
}

fn choose_fusermount() -> Option<&'static str> {
    ["fusermount3", "fusermount"].into_iter().find(|name| {
        Command::new(name)
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    })
}

fn unmount_path(path: &Path) {
    if let Some(fusermount) = choose_fusermount() {
        let _ = Command::new(fusermount).args(["-u", &path.display().to_string()]).status();
        let _ = Command::new(fusermount).args(["-uz", &path.display().to_string()]).status();
    }
    let _ = Command::new("umount").arg(path).status();
    let _ = Command::new("umount").args(["-l", &path.display().to_string()]).status();
}

fn runtime_metadata(paths: &ResolvedPaths) -> Value {
    match fs::read_to_string(&paths.metadata_file) {
        Ok(raw) => serde_json::from_str::<Value>(&raw).unwrap_or_else(|_| json!({})),
        Err(_) => json!({}),
    }
}

fn stop_previous_runtime(paths: &ResolvedPaths) {
    let previous = runtime_metadata(paths);
    let share_states = previous
        .get("share_states")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let processes = previous
        .get("processes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut stopped_units = BTreeSet::new();
    for process in &processes {
        let unit_name = process
            .get("unit_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if !unit_name.is_empty() && stopped_units.insert(unit_name.to_string()) {
            stop_systemd_unit(unit_name);
        }
    }
    for share in &share_states {
        let unit_name = share
            .get("unit_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if !unit_name.is_empty() && stopped_units.insert(unit_name.to_string()) {
            stop_systemd_unit(unit_name);
        }
    }

    for process in &processes {
        terminate_pid(normalize_pid(process.get("pid")));
    }
    for share in &share_states {
        let mount_path = share
            .get("mount_path")
            .and_then(Value::as_str)
            .map(PathBuf::from);
        if let Some(mount_path) = mount_path {
            if mount_path.exists() {
                unmount_path(&mount_path);
            }
        }
    }
}

fn write_status(paths: &ResolvedPaths, payload: &mut Value) -> Result<()> {
    payload["schema_version"] = json!(1);
    payload["status_updated_at_unix_ms"] = json!(now_ms());
    write_json_value(&paths.status_file, payload)
}

fn disabled_payload(paths: &ResolvedPaths, reason: Option<&str>) -> Value {
    json!({
        "state": "disabled",
        "mode": "host_process",
        "auto_managed": true,
        "listener": Value::Null,
        "listener_ready": false,
        "enabled_share_count": 0,
        "mounted_share_count": 0,
        "process_count": 0,
        "total_rss_bytes": 0,
        "last_error": reason,
        "share_states": [],
        "processes": [],
        "runtime_root": paths.state_root.display().to_string(),
    })
}

fn stopped_payload(paths: &ResolvedPaths) -> Value {
    json!({
        "state": "stopped",
        "mode": "host_process",
        "auto_managed": true,
        "listener": Value::Null,
        "listener_ready": false,
        "enabled_share_count": 0,
        "mounted_share_count": 0,
        "process_count": 0,
        "total_rss_bytes": 0,
        "last_error": Value::Null,
        "share_states": [],
        "processes": [],
        "runtime_root": paths.state_root.display().to_string(),
    })
}

fn read_status(paths: &ResolvedPaths) -> Result<Value> {
    if !paths.status_file.exists() {
        return Ok(json!({
            "state": "unknown",
            "mode": "host_process",
            "auto_managed": true,
            "runtime_root": paths.state_root.display().to_string(),
        }));
    }
    let raw = fs::read_to_string(&paths.status_file)
        .with_context(|| format!("read {}", paths.status_file.display()))?;
    let parsed = serde_json::from_str::<Value>(&raw)
        .with_context(|| format!("parse {}", paths.status_file.display()))?;
    Ok(parsed)
}

#[allow(dead_code)]
fn read_control_plane(paths: &ResolvedPaths) -> Result<ControlPlaneFile> {
    if !paths.control_plane_file.exists() {
        return Ok(ControlPlaneFile::default());
    }
    let raw = fs::read_to_string(&paths.control_plane_file)
        .with_context(|| format!("read {}", paths.control_plane_file.display()))?;
    let parsed = serde_json::from_str::<ControlPlaneFile>(&raw)
        .with_context(|| format!("parse {}", paths.control_plane_file.display()))?;
    Ok(parsed)
}

fn normalize_gateway_endpoint(raw: &str) -> String {
    let value = raw.trim();
    let value = if value.is_empty() {
        DEFAULT_GATEWAY_ENDPOINT
    } else {
        value
    };
    if let Some((host, port)) = value
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("]:"))
    {
        let host = if host == "::" { "::1" } else { host };
        return format!("[{host}]:{port}");
    }
    if value.matches(':').count() == 1 {
        let (host, port) = value
            .rsplit_once(':')
            .expect("single colon already checked");
        let host = match host.trim() {
            "" | "0.0.0.0" | "*" => "127.0.0.1",
            "::" => "::1",
            other => other,
        };
        return format!("{host}:{port}");
    }
    value.to_string()
}

fn normalize_path(raw: Option<&str>, fallback: &str) -> PathBuf {
    let text = raw.unwrap_or_default().trim();
    if text.is_empty() {
        PathBuf::from(fallback)
    } else {
        PathBuf::from(text)
    }
}

fn unit_prefix() -> String {
    env::var("CCBG_SMB_SIDECAR_UNIT_PREFIX")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_UNIT_PREFIX.to_string())
}

fn managed_unit_name(role: &str, identifier: Option<&str>) -> String {
    let prefix = unit_prefix();
    let Some(identifier) = identifier.filter(|value| !value.trim().is_empty()) else {
        return format!("{prefix}-smbd.service");
    };
    let slug = identifier
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() { "default" } else { slug.as_str() };
    let mut hasher = Sha1::new();
    hasher.update(identifier.as_bytes());
    let digest = hex_encode(hasher.finalize());
    format!("{prefix}-{role}-{slug}-{}.service", &digest[..8])
}

fn systemd_run_available() -> bool {
    if env::var("CCBG_SMB_SIDECAR_FORCE_NO_SYSTEMD_RUN")
        .map(|value| value == "1")
        .unwrap_or(false)
    {
        return false;
    }
    command_success(Command::new("systemd-run").arg("--version"))
        && command_success(Command::new("systemctl").arg("--version"))
}

fn systemd_unit_main_pid(unit_name: &str) -> Option<u32> {
    if unit_name.trim().is_empty() || !systemd_run_available() {
        return None;
    }
    let output = Command::new("systemctl")
        .args(["show", unit_name, "--property", "MainPID", "--value"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    value.parse::<u32>().ok().filter(|pid| *pid > 0)
}

fn start_transient_unit(unit_name: &str, command: &[String], log_path: &Path) -> Result<u32> {
    ensure_dir(log_path.parent().unwrap_or_else(|| Path::new(".")))?;
    stop_systemd_unit(unit_name);
    let mut args = vec![
        "--collect".to_string(),
        "--quiet".to_string(),
        "--service-type=exec".to_string(),
        format!("--property=StandardOutput=append:{}", log_path.display()),
        format!("--property=StandardError=append:{}", log_path.display()),
        "--unit".to_string(),
        unit_name.to_string(),
    ];
    args.extend(command.iter().cloned());
    let status = Command::new("systemd-run")
        .args(&args)
        .status()
        .with_context(|| format!("start transient unit {unit_name}"))?;
    if !status.success() {
        bail!("systemd-run failed for {unit_name} with status {status}");
    }
    sleep(Duration::from_secs(1));
    let pid = systemd_unit_main_pid(unit_name)
        .with_context(|| format!("resolve main pid for transient unit {unit_name}"))?;
    if !pid_exists(pid) {
        bail!("managed unit {unit_name} exited immediately; see {}", log_path.display());
    }
    Ok(pid)
}

fn mountpoint_active(path: &Path) -> bool {
    Command::new("mountpoint")
        .args(["-q", &path.display().to_string()])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn process_rss_bytes(pid: Option<u32>) -> u64 {
    let Some(pid) = pid else {
        return 0;
    };
    let status_path = Path::new("/proc").join(pid.to_string()).join("status");
    let Ok(raw) = fs::read_to_string(status_path) else {
        return 0;
    };
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let value = rest
                .split_whitespace()
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            return value * 1024;
        }
    }
    0
}

fn process_cmdline(pid: u32) -> Vec<String> {
    let path = Path::new("/proc").join(pid.to_string()).join("cmdline");
    let Ok(raw) = fs::read(path) else {
        return Vec::new();
    };
    raw.split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).to_string())
        .collect()
}

fn find_smbd_pid_for_model(model: &SidecarModel) -> Option<u32> {
    let smb_conf_path = model.smb_conf_path.display().to_string();
    let entries = fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let pid_text = file_name.to_string_lossy();
        let Ok(pid) = pid_text.parse::<u32>() else {
            continue;
        };
        if pid == std::process::id() {
            continue;
        }
        let args = process_cmdline(pid);
        if args.is_empty() {
            continue;
        }
        let executable = Path::new(&args[0])
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if executable == "smbd" && args.iter().any(|value| value == &smb_conf_path) {
            return Some(pid);
        }
    }
    None
}

fn share_state_errors(share_states: &[ShareRuntimeState]) -> Vec<String> {
    let mut errors = Vec::new();
    let mut seen = BTreeSet::new();
    for share in share_states {
        let Some(message) = share.last_error.as_deref().map(str::trim) else {
            continue;
        };
        if !message.is_empty() && seen.insert(message.to_string()) {
            errors.push(message.to_string());
        }
    }
    errors
}

fn desired_hash_for_model(model: &SidecarModel) -> Result<String> {
    let serialized = serde_json::to_string(&runtime_spec_payload(model))
        .context("serialize runtime spec for desired hash")?;
    let mut hasher = Sha256::new();
    hasher.update(serialized.as_bytes());
    Ok(hex_encode(hasher.finalize()))
}

fn supported_vfs_objects(values: &[String]) -> Vec<String> {
    let requested = if values.is_empty() {
        vec!["catia".to_string()]
    } else {
        values.to_vec()
    };
    let builtins = ["streams_xattr", "catia"]
        .into_iter()
        .collect::<BTreeSet<_>>();
    let module_root = Path::new("/usr/lib/x86_64-linux-gnu/samba/vfs");
    let mut supported = Vec::new();
    let mut seen = BTreeSet::new();
    for value in requested {
        let name = value.trim();
        if name.is_empty() || !seen.insert(name.to_string()) {
            continue;
        }
        if builtins.contains(name) || module_root.join(format!("{name}.so")).exists() {
            supported.push(name.to_string());
        }
    }
    supported
}

fn local_ipv4_interface_specs() -> Vec<String> {
    let mut specs = vec!["127.0.0.1/8".to_string()];
    let output = Command::new("ip")
        .args(["-o", "-4", "addr", "show", "scope", "global", "up"])
        .output();
    let Ok(output) = output else {
        return specs;
    };
    if !output.status.success() {
        return specs;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let parts = line.split_whitespace().collect::<Vec<_>>();
        let Some(index) = parts.iter().position(|part| *part == "inet") else {
            continue;
        };
        let Some(cidr) = parts.get(index + 1) else {
            continue;
        };
        if cidr.starts_with("127.") {
            continue;
        }
        if !specs.iter().any(|entry| entry == cidr) {
            specs.push((*cidr).to_string());
        }
    }
    specs
}

fn build_sidecar_model(
    _paths: &ResolvedPaths,
    env_values: &BTreeMap<String, String>,
    control_plane: &ControlPlaneFile,
) -> Result<SidecarModel> {
    let smb = &control_plane.smb_sidecar;
    let mut applications = BTreeMap::new();
    for entry in &control_plane.applications {
        let id = entry.id.trim();
        if id.is_empty() {
            continue;
        }
        applications.insert(id.to_string(), entry.clone());
    }
    if applications.is_empty() {
        let access_key_id = env_values
            .get("CCBG_S3_ACCESS_KEY_ID")
            .map(String::as_str)
            .unwrap_or("ccbg")
            .trim();
        let secret_access_key = env_values
            .get("CCBG_S3_SECRET_ACCESS_KEY")
            .map(String::as_str)
            .unwrap_or("change-me")
            .trim();
        if !access_key_id.is_empty() && !secret_access_key.is_empty() {
            applications.insert(
                "default".to_string(),
                DataPlaneApplication {
                    id: "default".to_string(),
                    access_key_id: access_key_id.to_string(),
                    secret_access_key: secret_access_key.to_string(),
                    enabled: true,
                },
            );
        }
    }

    let mount_root = normalize_path(
        smb.mount_root.as_deref(),
        env_values
            .get("CCBG_SMB_MOUNT_ROOT")
            .map(String::as_str)
            .unwrap_or(DEFAULT_SMB_MOUNT_ROOT),
    );
    let config_root = normalize_path(
        smb.config_root.as_deref(),
        env_values
            .get("CCBG_SMB_CONFIG_ROOT")
            .map(String::as_str)
            .unwrap_or(DEFAULT_SMB_CONFIG_ROOT),
    );
    let data_root = normalize_path(
        smb.data_root.as_deref(),
        env_values
            .get("CCBG_SMB_DATA_ROOT")
            .map(String::as_str)
            .unwrap_or(DEFAULT_SMB_DATA_ROOT),
    );
    let home_root = data_root.join("homes");
    let runtime_root = data_root.join("runtime");

    let mut users = Vec::new();
    let mut users_by_id = BTreeMap::new();
    for entry in &smb.users {
        if !entry.enabled {
            continue;
        }
        let id = entry.id.trim();
        let username = entry.username.trim();
        let password = entry.password.as_deref().unwrap_or_default();
        if id.is_empty() || username.is_empty() || password.trim().is_empty() {
            bail!(
                "SMB user is missing id/username/password: {}",
                if !id.is_empty() { id } else { username }
            );
        }
        let user = UserModel {
            id: id.to_string(),
            username: username.to_string(),
            password: password.to_string(),
            allowed_share_ids: entry
                .allowed_share_ids
                .iter()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect(),
        };
        users_by_id.insert(user.id.clone(), user.username.clone());
        users.push(user);
    }

    let mut shares = Vec::new();
    for entry in &smb.shares {
        if !entry.enabled {
            continue;
        }
        let share_id = entry.id.trim();
        let share_name = entry.share_name.trim();
        let application_id = entry.application_id.trim();
        let bucket = entry.bucket.trim();
        if share_id.is_empty()
            || share_name.is_empty()
            || application_id.is_empty()
            || bucket.is_empty()
        {
            bail!(
                "SMB share is missing id/share_name/application_id/bucket: {}",
                [share_id, share_name, application_id, bucket]
                    .into_iter()
                    .find(|value| !value.is_empty())
                    .unwrap_or("<unknown>")
            );
        }
        let application = applications.get(application_id).with_context(|| {
            format!("SMB share {share_id} references unknown application {application_id}")
        })?;
        if application.access_key_id.trim().is_empty()
            || application.secret_access_key.trim().is_empty()
        {
            bail!(
                "SMB share {share_id} references application {application_id} without complete S3 credentials"
            );
        }
        let prefix = entry.prefix.trim().trim_matches('/').to_string();
        let remote_path = if prefix.is_empty() {
            bucket.to_string()
        } else {
            format!("{bucket}/{prefix}")
        };
        let valid_usernames = entry
            .valid_user_ids
            .iter()
            .filter_map(|value| users_by_id.get(value.trim()).cloned())
            .collect::<Vec<_>>();
        shares.push(ShareModel {
            id: share_id.to_string(),
            share_name: share_name.to_string(),
            application_id: application_id.to_string(),
            bucket: bucket.to_string(),
            prefix,
            remote_path,
            read_only: entry.read_only,
            browseable: entry.browseable,
            guest_ok: entry.guest_ok,
            valid_usernames,
            mount_path: mount_root.join(share_id),
            create_mask: entry
                .create_mask
                .as_deref()
                .unwrap_or(
                    smb.create_mask
                        .as_deref()
                        .unwrap_or(DEFAULT_SMB_CREATE_MASK),
                )
                .trim()
                .to_string(),
            directory_mask: entry
                .directory_mask
                .as_deref()
                .unwrap_or(
                    smb.directory_mask
                        .as_deref()
                        .unwrap_or(DEFAULT_SMB_DIRECTORY_MASK),
                )
                .trim()
                .to_string(),
            access_key_id: application.access_key_id.trim().to_string(),
            secret_access_key: application.secret_access_key.trim().to_string(),
        });
    }

    Ok(SidecarModel {
        mount_root,
        config_root: config_root.clone(),
        data_root: data_root.clone(),
        home_root,
        runtime_root: runtime_root.clone(),
        smb_conf_path: config_root.join("smb").join("smb.conf"),
        rclone_conf_path: config_root.join("rclone").join("rclone.conf"),
        gateway_endpoint: normalize_gateway_endpoint(
            env_values
                .get("CCBG_BIND_ADDR")
                .map(String::as_str)
                .unwrap_or(DEFAULT_GATEWAY_ENDPOINT),
        ),
        region: env_values
            .get("CCBG_S3_REGION")
            .map(String::as_str)
            .unwrap_or(DEFAULT_S3_REGION)
            .trim()
            .to_string(),
        bind_addr: {
            let raw = smb.bind_addr.trim();
            if raw.is_empty() {
                "127.0.0.1".to_string()
            } else {
                raw.to_string()
            }
        },
        port: smb.port.unwrap_or(445),
        workgroup: smb
            .workgroup
            .as_deref()
            .unwrap_or(DEFAULT_SMB_WORKGROUP)
            .trim()
            .to_string(),
        server_string: smb
            .server_string
            .as_deref()
            .unwrap_or(DEFAULT_SMB_SERVER_STRING)
            .trim()
            .to_string(),
        create_mask: smb
            .create_mask
            .as_deref()
            .unwrap_or(DEFAULT_SMB_CREATE_MASK)
            .trim()
            .to_string(),
        directory_mask: smb
            .directory_mask
            .as_deref()
            .unwrap_or(DEFAULT_SMB_DIRECTORY_MASK)
            .trim()
            .to_string(),
        disable_splice: smb.disable_splice,
        vfs_objects: supported_vfs_objects(&smb.vfs_objects),
        users,
        shares,
    })
}

fn runtime_spec_payload(model: &SidecarModel) -> RuntimeSpec<'_> {
    RuntimeSpec {
        runtime_spec_version: 2,
        bind_addr: &model.bind_addr,
        config_root: model.config_root.display().to_string(),
        create_mask: &model.create_mask,
        data_root: model.data_root.display().to_string(),
        directory_mask: &model.directory_mask,
        disable_splice: model.disable_splice,
        gateway_endpoint: &model.gateway_endpoint,
        mount_root: model.mount_root.display().to_string(),
        port: model.port,
        region: &model.region,
        server_string: &model.server_string,
        shares: model
            .shares
            .iter()
            .map(|share| RuntimeSpecShare {
                id: &share.id,
                share_name: &share.share_name,
                application_id: &share.application_id,
                bucket: &share.bucket,
                prefix: &share.prefix,
                remote_path: &share.remote_path,
                read_only: share.read_only,
                browseable: share.browseable,
                guest_ok: share.guest_ok,
                valid_usernames: &share.valid_usernames,
                mount_path: share.mount_path.display().to_string(),
                create_mask: &share.create_mask,
                directory_mask: &share.directory_mask,
                access_key_id: &share.access_key_id,
                secret_access_key: &share.secret_access_key,
            })
            .collect(),
        users: model
            .users
            .iter()
            .map(|user| RuntimeSpecUser {
                id: &user.id,
                username: &user.username,
                password: &user.password,
                allowed_share_ids: &user.allowed_share_ids,
            })
            .collect(),
        vfs_objects: &model.vfs_objects,
        workgroup: &model.workgroup,
    }
}

fn generate_rclone_conf(model: &SidecarModel) -> String {
    model
        .shares
        .iter()
        .map(|share| {
            format!(
                "[ccbg-{id}]\n\
type = s3\n\
provider = Other\n\
env_auth = false\n\
access_key_id = {access_key_id}\n\
secret_access_key = {secret_access_key}\n\
region = {region}\n\
endpoint = http://{endpoint}\n\
force_path_style = true\n\
no_check_bucket = true\n",
                id = share.id,
                access_key_id = share.access_key_id,
                secret_access_key = share.secret_access_key,
                region = model.region,
                endpoint = model.gateway_endpoint,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn generate_smb_conf(model: &SidecarModel) -> String {
    let runtime_root = &model.runtime_root;
    let run_dir = runtime_root.join("run");
    let private_dir = runtime_root.join("private");
    let state_dir = runtime_root.join("state");
    let cache_dir = runtime_root.join("cache");
    let lock_dir = runtime_root.join("locks");
    let log_dir = runtime_root.join("logs");
    let mut global_lines = vec![
        "[global]".to_string(),
        format!("   workgroup = {}", model.workgroup),
        format!("   server string = {}", model.server_string),
        "   map to guest = Never".to_string(),
        "   load printers = no".to_string(),
        "   printing = bsd".to_string(),
        "   disable spoolss = yes".to_string(),
        "   passdb backend = tdbsam".to_string(),
        "   security = user".to_string(),
        format!("   create mask = {}", model.create_mask),
        format!("   directory mask = {}", model.directory_mask),
        format!(
            "   vfs objects = {}",
            if model.vfs_objects.is_empty() {
                "catia".to_string()
            } else {
                model.vfs_objects.join(" ")
            }
        ),
        "   ea support = yes".to_string(),
        "   store dos attributes = yes".to_string(),
        format!(
            "   use sendfile = {}",
            if model.disable_splice { "no" } else { "yes" }
        ),
        format!("   pid directory = {}", run_dir.display()),
        format!("   lock directory = {}", lock_dir.display()),
        format!("   state directory = {}", state_dir.display()),
        format!("   cache directory = {}", cache_dir.display()),
        format!("   private dir = {}", private_dir.display()),
        format!("   log file = {}", log_dir.join("smbd.log").display()),
        "   max log size = 10000".to_string(),
        format!("   smb ports = {}", model.port),
    ];
    if model.vfs_objects.iter().any(|value| value == "fruit") {
        global_lines.extend(
            [
                "   fruit:metadata = stream",
                "   fruit:model = MacSamba",
                "   fruit:posix_rename = yes",
                "   fruit:veto_appledouble = no",
            ]
            .into_iter()
            .map(str::to_string),
        );
    }
    match model.bind_addr.trim() {
        "0.0.0.0" | "*" => {
            global_lines.push(format!(
                "   interfaces = {}",
                local_ipv4_interface_specs().join(" ")
            ));
            global_lines.push("   bind interfaces only = yes".to_string());
        }
        "::" | "" => {}
        value => {
            global_lines.push(format!("   interfaces = {value}"));
            global_lines.push("   bind interfaces only = yes".to_string());
        }
    }

    let mut body = vec![global_lines.join("\n")];
    for share in &model.shares {
        let mut share_lines = vec![
            format!("[{}]", share.share_name),
            format!("   path = {}", share.mount_path.display()),
            format!(
                "   comment = app={} | bucket={}{}",
                share.application_id,
                share.bucket,
                if share.prefix.is_empty() {
                    String::new()
                } else {
                    format!(" | prefix={}", share.prefix)
                }
            ),
            format!(
                "   browseable = {}",
                if share.browseable { "yes" } else { "no" }
            ),
            format!(
                "   read only = {}",
                if share.read_only { "yes" } else { "no" }
            ),
            format!(
                "   guest ok = {}",
                if share.guest_ok { "yes" } else { "no" }
            ),
            format!("   create mask = {}", share.create_mask),
            format!("   directory mask = {}", share.directory_mask),
        ];
        if !share.valid_usernames.is_empty() {
            share_lines.push(format!(
                "   valid users = {}",
                share.valid_usernames.join(" ")
            ));
        }
        body.push(share_lines.join("\n"));
    }
    format!("{}\n", body.join("\n\n"))
}

fn write_runtime_files(model: &SidecarModel) -> Result<()> {
    ensure_dir(model.rclone_conf_path.parent().unwrap_or_else(|| Path::new(".")))?;
    ensure_dir(model.smb_conf_path.parent().unwrap_or_else(|| Path::new(".")))?;
    atomic_write_text(&model.rclone_conf_path, &generate_rclone_conf(model))?;
    atomic_write_text(&model.smb_conf_path, &generate_smb_conf(model))?;
    Ok(())
}

fn remove_path_if_exists(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        fs::remove_dir_all(path).with_context(|| format!("remove directory {}", path.display()))?;
    } else {
        fs::remove_file(path).with_context(|| format!("remove file {}", path.display()))?;
    }
    Ok(())
}

fn run_best_effort(args: &[&str]) {
    if args.is_empty() {
        return;
    }
    let _ = Command::new(args[0]).args(&args[1..]).status();
}

fn capture_output(args: &[&str]) -> Option<String> {
    if args.is_empty() {
        return None;
    }
    let output = Command::new(args[0]).args(&args[1..]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn ensure_mount_permissions(mount_root: &Path) -> Result<()> {
    ensure_group(SMB_GROUP_NAME)?;
    ensure_dir(mount_root)?;
    if let Some(group_info) = capture_output(&["getent", "group", SMB_GROUP_NAME]) {
        let parts = group_info.split(':').collect::<Vec<_>>();
        if let Some(gid) = parts.get(2).copied() {
            run_best_effort(&["chown", &format!("0:{gid}"), &mount_root.display().to_string()]);
        }
    }
    run_best_effort(&["chmod", "770", &mount_root.display().to_string()]);
    Ok(())
}

fn prepare_samba_runtime_tree(model: &SidecarModel) -> Result<()> {
    let volatile_paths = [
        model.runtime_root.join("locks").join("msg.lock"),
        model.runtime_root.join("locks").join("msg.sock"),
        model.runtime_root.join("logs").join("cores"),
    ];
    for path in &volatile_paths {
        remove_path_if_exists(path)?;
    }

    for directory in [
        &model.runtime_root,
        &model.runtime_root.join("run"),
        &model.runtime_root.join("private"),
        &model.runtime_root.join("state"),
        &model.runtime_root.join("cache"),
        &model.runtime_root.join("locks"),
        &model.runtime_root.join("logs"),
    ] {
        ensure_dir(directory)?;
    }

    let runtime_root = model.runtime_root.display().to_string();
    let private_dir = model.runtime_root.join("private").display().to_string();
    run_best_effort(&["chown", "-R", "0:0", &runtime_root]);
    run_best_effort(&["chmod", "755", &runtime_root]);
    run_best_effort(&["chmod", "755", &model.runtime_root.join("run").display().to_string()]);
    run_best_effort(&["chmod", "700", &private_dir]);
    run_best_effort(&["chmod", "755", &model.runtime_root.join("state").display().to_string()]);
    run_best_effort(&["chmod", "755", &model.runtime_root.join("cache").display().to_string()]);
    run_best_effort(&["chmod", "755", &model.runtime_root.join("locks").display().to_string()]);
    run_best_effort(&["chmod", "755", &model.runtime_root.join("logs").display().to_string()]);
    Ok(())
}

fn command_success(command: &mut Command) -> bool {
    command.status().map(|status| status.success()).unwrap_or(false)
}

fn ensure_group(name: &str) -> Result<()> {
    if command_success(Command::new("getent").args(["group", name])) {
        return Ok(());
    }
    let status = Command::new("groupadd")
        .args(["--system", name])
        .status()
        .with_context(|| format!("create group {name}"))?;
    if status.success() {
        Ok(())
    } else {
        bail!("groupadd failed for {name} with status {status}")
    }
}

fn ensure_user(username: &str, home_root: &Path, group_name: &str) -> Result<()> {
    if command_success(Command::new("id").args(["-u", username])) {
        return Ok(());
    }
    let user_home = home_root.join(username);
    ensure_dir(&user_home)?;
    let status = Command::new("useradd")
        .args([
            "--system",
            "--gid",
            group_name,
            "--home-dir",
            &user_home.display().to_string(),
            "--create-home",
            "--shell",
            "/usr/sbin/nologin",
            username,
        ])
        .status()
        .with_context(|| format!("create user {username}"))?;
    if status.success() {
        Ok(())
    } else {
        bail!("useradd failed for {username} with status {status}")
    }
}

fn ensure_samba_users(model: &SidecarModel) -> Result<()> {
    ensure_group(SMB_GROUP_NAME)?;
    ensure_dir(&model.home_root)?;
    for user in &model.users {
        ensure_user(&user.username, &model.home_root, SMB_GROUP_NAME)?;
        let mut child = Command::new("smbpasswd")
            .args([
                "-c",
                &model.smb_conf_path.display().to_string(),
                "-s",
                "-a",
                &user.username,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn smbpasswd for {}", user.username))?;
        {
            let stdin = child
                .stdin
                .as_mut()
                .context("open smbpasswd stdin for password input")?;
            write!(stdin, "{}\n{}\n", user.password, user.password)
                .with_context(|| format!("write password for {}", user.username))?;
        }
        let output = child
            .wait_with_output()
            .with_context(|| format!("wait for smbpasswd {}", user.username))?;
        if !output.status.success() {
            bail!(
                "smbpasswd failed for {}: {}",
                user.username,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }

        let _ = Command::new("smbpasswd")
            .args([
                "-c",
                &model.smb_conf_path.display().to_string(),
                "-e",
                &user.username,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    Ok(())
}

fn stop_model_runtime(model: &SidecarModel) {
    stop_systemd_unit(&managed_unit_name("smbd", None));
    for share in &model.shares {
        stop_systemd_unit(&managed_unit_name("rclone", Some(&share.id)));
    }

    let rclone_remotes = model
        .shares
        .iter()
        .map(|share| format!("ccbg-{}:{}", share.id, share.remote_path))
        .collect::<BTreeSet<_>>();
    let mount_paths = model
        .shares
        .iter()
        .map(|share| share.mount_path.display().to_string())
        .collect::<BTreeSet<_>>();
    let smb_conf_path = model.smb_conf_path.display().to_string();

    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Ok(pid) = file_name.to_string_lossy().parse::<u32>() else {
                continue;
            };
            if pid == std::process::id() {
                continue;
            }
            let args = process_cmdline(pid);
            if args.is_empty() {
                continue;
            }
            let executable = Path::new(&args[0])
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if executable == "rclone" && args.len() >= 4 && args.get(1).map(String::as_str) == Some("mount") {
                if rclone_remotes.contains(&args[2]) || mount_paths.contains(&args[3]) {
                    terminate_pid(pid);
                }
            } else if executable == "smbd" && args.iter().any(|value| value == &smb_conf_path) {
                terminate_pid(pid);
            }
        }
    }

    for mount_path in mount_paths {
        let path = PathBuf::from(mount_path);
        if path.exists() {
            unmount_path(&path);
        }
    }
}

fn fuse_unavailable_message(share: &ShareModel) -> String {
    format!(
        "SMB share mounts need /dev/fuse. The managed smbd listener can run without shares, but rclone-backed shares such as {} cannot mount until this LXC/container exposes /dev/fuse.",
        share.share_name
    )
}

fn append_log_line(path: &Path, line: &str) -> Result<()> {
    ensure_dir(path.parent().unwrap_or_else(|| Path::new(".")))?;
    let mut handle = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open log {}", path.display()))?;
    writeln!(handle, "{line}").with_context(|| format!("append log {}", path.display()))
}

fn spawn_detached(command: &[String], log_path: &Path) -> Result<Child> {
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("open detached log {}", log_path.display()))?;
    let stderr_log = log
        .try_clone()
        .with_context(|| format!("clone detached log {}", log_path.display()))?;
    Command::new(&command[0])
        .args(&command[1..])
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr_log))
        .spawn()
        .with_context(|| format!("spawn detached command {}", command.join(" ")))
}

fn start_rclone_mounts(model: &SidecarModel) -> Result<Vec<ShareRuntimeState>> {
    let log_dir = model.runtime_root.join("logs");
    ensure_dir(&log_dir)?;
    let group_gid = capture_output(&["getent", "group", SMB_GROUP_NAME])
        .and_then(|value| value.split(':').nth(2).map(str::to_string))
        .unwrap_or_else(|| "0".to_string());

    let force_no_fuse = env::var("CCBG_SMB_SIDECAR_FORCE_NO_FUSE")
        .map(|value| value == "1")
        .unwrap_or(false);

    let mut share_states = Vec::new();
    for share in &model.shares {
        ensure_dir(&share.mount_path)?;
        run_best_effort(&["chown", &format!("0:{group_gid}"), &share.mount_path.display().to_string()]);
        run_best_effort(&["chmod", "770", &share.mount_path.display().to_string()]);
        let log_path = log_dir.join(format!("rclone-{}.log", share.id));
        let unit_name = managed_unit_name("rclone", Some(&share.id));
        if force_no_fuse || !Path::new("/dev/fuse").exists() {
            let last_error = fuse_unavailable_message(share);
            append_log_line(&log_path, &last_error)?;
            share_states.push(ShareRuntimeState {
                id: share.id.clone(),
                share_name: share.share_name.clone(),
                mount_path: share.mount_path.clone(),
                remote_path: share.remote_path.clone(),
                read_only: share.read_only,
                pid: None,
                unit_name,
                mounted: false,
                running: false,
                rss_bytes: 0,
                log_path,
                last_error: Some(last_error),
            });
            continue;
        }

        let mut command = vec![
            "rclone".to_string(),
            "mount".to_string(),
            format!("ccbg-{}:{}", share.id, share.remote_path),
            share.mount_path.display().to_string(),
            "--config".to_string(),
            model.rclone_conf_path.display().to_string(),
            "--allow-other".to_string(),
            "--dir-cache-time".to_string(),
            "30s".to_string(),
            "--vfs-cache-mode".to_string(),
            "minimal".to_string(),
            "--uid".to_string(),
            "0".to_string(),
            "--gid".to_string(),
            group_gid.clone(),
            "--umask".to_string(),
            "007".to_string(),
            "--dir-perms".to_string(),
            "0770".to_string(),
            "--file-perms".to_string(),
            "0660".to_string(),
        ];
        if share.read_only {
            command.push("--read-only".to_string());
        }
        let pid = if systemd_run_available() {
            start_transient_unit(&unit_name, &command, &log_path)?
        } else {
            let child = spawn_detached(&command, &log_path)?;
            sleep(Duration::from_secs(1));
            if child.id() == 0 || !pid_exists(child.id()) {
                bail!(
                    "rclone mount for share {} exited immediately; see {}",
                    share.id,
                    log_path.display()
                );
            }
            child.id()
        };
        share_states.push(ShareRuntimeState {
            id: share.id.clone(),
            share_name: share.share_name.clone(),
            mount_path: share.mount_path.clone(),
            remote_path: share.remote_path.clone(),
            read_only: share.read_only,
            pid: Some(pid),
            unit_name,
            mounted: mountpoint_active(&share.mount_path),
            running: pid_exists(pid),
            rss_bytes: process_rss_bytes(Some(pid)),
            log_path,
            last_error: None,
        });
    }
    Ok(share_states)
}

fn start_smbd(model: &SidecarModel) -> Result<(u32, PathBuf)> {
    let log_dir = model.runtime_root.join("logs");
    ensure_dir(&log_dir)?;
    ensure_dir(Path::new("/run/samba"))?;
    ensure_dir(Path::new("/run/samba/ncalrpc"))?;
    let log_path = log_dir.join("smbd-launch.log");
    let command = vec![
        "smbd".to_string(),
        "--foreground".to_string(),
        "--no-process-group".to_string(),
        "-s".to_string(),
        model.smb_conf_path.display().to_string(),
    ];
    let pid = if systemd_run_available() {
        start_transient_unit(&managed_unit_name("smbd", None), &command, &log_path)?
    } else {
        let child = spawn_detached(&command, &log_path)?;
        sleep(Duration::from_secs(1));
        if child.id() == 0 || !pid_exists(child.id()) {
            bail!("smbd exited immediately; see {}", log_path.display());
        }
        child.id()
    };
    Ok((pid, log_path))
}

fn listener_probe_addr(bind_addr: &str, port: u16) -> Result<SocketAddr> {
    let host = bind_addr.trim();
    let target = if matches!(host, "" | "0.0.0.0" | "*") {
        format!("127.0.0.1:{port}")
    } else if host == "::" {
        format!("[::1]:{port}")
    } else if host.contains(':') && !host.contains('.') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    target.parse().with_context(|| format!("parse listener probe target {target}"))
}

fn listener_ready(bind_addr: &str, port: u16) -> bool {
    let Ok(target) = listener_probe_addr(bind_addr, port) else {
        return false;
    };
    TcpStream::connect_timeout(&target, Duration::from_secs(1)).is_ok()
}

fn wait_for_listener(bind_addr: &str, port: u16, timeout_seconds: f64) -> bool {
    let deadline = SystemTime::now()
        .checked_add(Duration::from_secs_f64(timeout_seconds))
        .unwrap_or(SystemTime::now());
    while SystemTime::now() < deadline {
        if listener_ready(bind_addr, port) {
            return true;
        }
        sleep(Duration::from_millis(250));
    }
    listener_ready(bind_addr, port)
}

fn build_process_payload(
    role: String,
    pid: Option<u32>,
    unit_name: Option<String>,
    log_path: Option<PathBuf>,
    share_id: Option<String>,
    share_name: Option<String>,
    mount_path: Option<PathBuf>,
) -> ManagedProcessState {
    let running = pid.map(pid_exists).unwrap_or(false);
    ManagedProcessState {
        role,
        pid: pid.filter(|pid| pid_exists(*pid)),
        running,
        rss_bytes: process_rss_bytes(pid.filter(|pid| pid_exists(*pid))),
        unit_name,
        log_path,
        share_id,
        share_name,
        mount_path,
    }
}

fn process_state_to_value(process: &ManagedProcessState) -> Value {
    let mut payload = json!({
        "role": process.role,
        "pid": process.pid,
        "running": process.running,
        "rss_bytes": process.rss_bytes,
    });
    if let Some(unit_name) = &process.unit_name {
        payload["unit_name"] = json!(unit_name);
    }
    if let Some(log_path) = &process.log_path {
        payload["log_path"] = json!(log_path.display().to_string());
    }
    if let Some(share_id) = &process.share_id {
        payload["share_id"] = json!(share_id);
    }
    if let Some(share_name) = &process.share_name {
        payload["share_name"] = json!(share_name);
    }
    if let Some(mount_path) = &process.mount_path {
        payload["mount_path"] = json!(mount_path.display().to_string());
    }
    payload
}

fn share_state_to_value(share: &ShareRuntimeState) -> Value {
    let mut payload = json!({
        "id": share.id,
        "share_name": share.share_name,
        "mount_path": share.mount_path.display().to_string(),
        "remote_path": share.remote_path,
        "read_only": share.read_only,
        "pid": share.pid,
        "unit_name": share.unit_name,
        "mounted": share.mounted,
        "running": share.running,
        "rss_bytes": share.rss_bytes,
        "log_path": share.log_path.display().to_string(),
    });
    if let Some(last_error) = &share.last_error {
        payload["last_error"] = json!(last_error);
    }
    payload
}

fn build_runtime_payload(
    paths: &ResolvedPaths,
    model: Option<&SidecarModel>,
    desired_hash: Option<&str>,
    state: &str,
    last_error: Option<&str>,
    share_states: &[ShareRuntimeState],
    processes: &[ManagedProcessState],
    last_success_at_unix_ms: Option<u64>,
) -> Value {
    let total_rss_bytes = processes.iter().map(|entry| entry.rss_bytes).sum::<u64>();
    let running_process_count = processes.iter().filter(|entry| entry.running).count();
    let mounted_share_count = share_states.iter().filter(|entry| entry.mounted).count();
    let enabled_share_count = model.map(|value| value.shares.len()).unwrap_or(share_states.len());
    let listener = model.map(|value| format!("{}:{}", value.bind_addr, value.port));
    let listener_ready_value = model
        .and_then(|value| {
            let smbd_running = processes
                .iter()
                .any(|entry| entry.role == "smbd" && entry.running);
            if smbd_running {
                Some(listener_ready(&value.bind_addr, value.port))
            } else {
                Some(false)
            }
        })
        .unwrap_or(false);

    json!({
        "state": state,
        "mode": "host_process",
        "auto_managed": true,
        "listener": listener,
        "listener_ready": listener_ready_value,
        "enabled_share_count": enabled_share_count,
        "mounted_share_count": mounted_share_count,
        "process_count": running_process_count,
        "total_rss_bytes": total_rss_bytes,
        "last_error": last_error,
        "share_states": share_states.iter().map(share_state_to_value).collect::<Vec<_>>(),
        "processes": processes.iter().map(process_state_to_value).collect::<Vec<_>>(),
        "runtime_root": model
            .map(|value| value.runtime_root.display().to_string())
            .unwrap_or_else(|| paths.state_root.display().to_string()),
        "mount_root": model.map(|value| value.mount_root.display().to_string()),
        "config_root": model.map(|value| value.config_root.display().to_string()),
        "data_root": model.map(|value| value.data_root.display().to_string()),
        "desired_hash": desired_hash,
        "last_success_at_unix_ms": last_success_at_unix_ms,
    })
}

fn process_count_matches(processes: &[ManagedProcessState]) -> bool {
    !processes.is_empty() && processes.iter().all(|entry| entry.running)
}

fn run_status() -> Result<()> {
    let (paths, _) = ResolvedPaths::resolve()?;
    let payload = read_status(&paths)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&payload).context("serialize SMB sidecar runtime status")?
    );
    Ok(())
}

fn run_stop_with_paths(paths: &ResolvedPaths) -> Result<()> {
    ensure_dir(&paths.state_root)?;
    stop_previous_runtime(paths);
    let mut payload = stopped_payload(paths);
    write_status(paths, &mut payload)?;
    write_json_value(&paths.metadata_file, &payload)?;
    Ok(())
}

fn run_stop() -> Result<()> {
    let (paths, _) = ResolvedPaths::resolve()?;
    run_stop_with_paths(&paths)
}

fn run_sync_with_paths(paths: &ResolvedPaths) -> Result<()> {
    ensure_dir(&paths.state_root)?;
    let env_values = read_env_file(&paths.env_file)?;
    let control_plane = read_control_plane(paths)?;
    if !control_plane.smb_sidecar.enabled {
        stop_previous_runtime(paths);
        let mut payload = disabled_payload(paths, None);
        write_status(paths, &mut payload)?;
        write_json_value(&paths.metadata_file, &payload)?;
        return Ok(());
    }
    let model = build_sidecar_model(paths, &env_values, &control_plane)?;
    let desired_hash = desired_hash_for_model(&model)?;
    let previous = runtime_metadata(paths);
    let previous_desired_hash = previous
        .get("desired_hash")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if previous_desired_hash == desired_hash {
        let previous_processes = previous
            .get("processes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let previous_share_states = previous
            .get("share_states")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let smbd_pid = previous_processes
            .iter()
            .find(|entry| entry.get("role").and_then(Value::as_str) == Some("smbd"))
            .and_then(|entry| normalize_pid(entry.get("pid")).checked_sub(0))
            .filter(|pid| *pid > 0)
            .or_else(|| find_smbd_pid_for_model(&model));
        let mut processes = Vec::new();
        if let Some(pid) = smbd_pid {
            let log_path = previous_processes
                .iter()
                .find(|entry| entry.get("role").and_then(Value::as_str) == Some("smbd"))
                .and_then(|entry| entry.get("log_path").and_then(Value::as_str))
                .map(PathBuf::from);
            let unit_name = previous_processes
                .iter()
                .find(|entry| entry.get("role").and_then(Value::as_str) == Some("smbd"))
                .and_then(|entry| entry.get("unit_name").and_then(Value::as_str))
                .map(ToString::to_string)
                .or_else(|| Some(managed_unit_name("smbd", None)));
            processes.push(build_process_payload(
                "smbd".to_string(),
                Some(pid),
                unit_name,
                log_path,
                None,
                None,
                None,
            ));
        }
        let share_states = previous_share_states
            .iter()
            .map(|entry| {
                let pid = normalize_pid(entry.get("pid"));
                let mount_path = entry
                    .get("mount_path")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .unwrap_or_default();
                ShareRuntimeState {
                    id: entry.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                    share_name: entry
                        .get("share_name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    mount_path: mount_path.clone(),
                    remote_path: entry
                        .get("remote_path")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    read_only: entry.get("read_only").and_then(Value::as_bool).unwrap_or(false),
                    pid: if pid_exists(pid) { Some(pid) } else { None },
                    unit_name: entry
                        .get("unit_name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    mounted: if mount_path.exists() {
                        mountpoint_active(&mount_path)
                    } else {
                        false
                    },
                    running: pid_exists(pid),
                    rss_bytes: process_rss_bytes(if pid_exists(pid) { Some(pid) } else { None }),
                    log_path: entry
                        .get("log_path")
                        .and_then(Value::as_str)
                        .map(PathBuf::from)
                        .unwrap_or_default(),
                    last_error: entry
                        .get("last_error")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                }
            })
            .collect::<Vec<_>>();
        for share in &share_states {
            processes.push(build_process_payload(
                format!("rclone:{}", share.id),
                share.pid,
                Some(share.unit_name.clone()),
                Some(share.log_path.clone()),
                Some(share.id.clone()),
                Some(share.share_name.clone()),
                Some(share.mount_path.clone()),
            ));
        }
        let share_errors = share_state_errors(&share_states);
        let all_shares_mounted = share_states.len() == model.shares.len()
            && share_states.iter().all(|entry| entry.mounted);
        let payload_state = if listener_ready(&model.bind_addr, model.port) && !share_errors.is_empty() {
            "degraded"
        } else if process_count_matches(&processes)
            && all_shares_mounted
            && listener_ready(&model.bind_addr, model.port)
        {
            "running"
        } else {
            "reconciling"
        };
        if payload_state != "reconciling" {
            let last_error_text = if share_errors.is_empty() {
                None
            } else {
                Some(share_errors.join(" "))
            };
            let last_success = if payload_state == "running" {
                previous
                    .get("last_success_at_unix_ms")
                    .and_then(Value::as_u64)
                    .or_else(|| Some(now_ms()))
            } else {
                previous.get("last_success_at_unix_ms").and_then(Value::as_u64)
            };
            let mut payload = build_runtime_payload(
                paths,
                Some(&model),
                Some(&desired_hash),
                payload_state,
                last_error_text.as_deref(),
                &share_states,
                &processes,
                last_success,
            );
            write_status(paths, &mut payload)?;
            write_json_value(&paths.metadata_file, &payload)?;
            return Ok(());
        }
    }

    ensure_mount_permissions(&model.mount_root)?;
    ensure_dir(&model.config_root)?;
    ensure_dir(&model.data_root)?;
    write_runtime_files(&model)?;
    prepare_samba_runtime_tree(&model)?;
    ensure_samba_users(&model)?;
    stop_previous_runtime(paths);
    stop_model_runtime(&model);
    let share_states = start_rclone_mounts(&model)?;
    let (smbd_pid, smbd_log_path) = start_smbd(&model)?;
    if !wait_for_listener(&model.bind_addr, model.port, 5.0) {
        bail!(
            "smbd started but did not accept connections on {}:{}",
            model.bind_addr,
            model.port
        );
    }

    let mut processes = vec![build_process_payload(
        "smbd".to_string(),
        Some(smbd_pid),
        Some(managed_unit_name("smbd", None)),
        Some(smbd_log_path),
        None,
        None,
        None,
    )];
    for share in &share_states {
        processes.push(build_process_payload(
            format!("rclone:{}", share.id),
            share.pid,
            Some(share.unit_name.clone()),
            Some(share.log_path.clone()),
            Some(share.id.clone()),
            Some(share.share_name.clone()),
            Some(share.mount_path.clone()),
        ));
    }
    let share_errors = share_state_errors(&share_states);
    let last_error_text = if share_errors.is_empty() {
        None
    } else {
        Some(share_errors.join(" "))
    };
    let last_success = if share_errors.is_empty() {
        Some(now_ms())
    } else {
        None
    };
    let mut payload = build_runtime_payload(
        paths,
        Some(&model),
        Some(&desired_hash),
        if share_errors.is_empty() { "running" } else { "degraded" },
        last_error_text.as_deref(),
        &share_states,
        &processes,
        last_success,
    );
    write_status(paths, &mut payload)?;
    write_json_value(&paths.metadata_file, &payload)?;
    Ok(())
}

fn run_sync() -> Result<()> {
    let (paths, _) = ResolvedPaths::resolve()?;
    run_sync_with_paths(&paths)
}

fn main() -> Result<()> {
    let action = env::args().nth(1).unwrap_or_else(|| "sync".to_string());
    match action.as_str() {
        "status" => run_status(),
        "sync" => run_sync(),
        "stop" => run_stop(),
        _ => bail!("usage: smb-sidecar-host [sync|stop|status]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_control_plane() -> ControlPlaneFile {
        serde_json::from_value(json!({
            "smb_sidecar": {
                "enabled": true,
                "bind_addr": "127.0.0.1",
                "port": 445,
                "mount_root": "/mnt/ccbg/smb/mounts",
                "config_root": "/var/lib/ccbg/smb-sidecar/config",
                "data_root": "/var/lib/ccbg/smb-sidecar/data",
                "workgroup": "WORKGROUP",
                "create_mask": "0660",
                "directory_mask": "0770",
                "vfs_objects": ["catia", "fruit"],
                "users": [
                    {
                        "id": "smb-user-1",
                        "username": "smbuser1",
                        "password": "secret",
                        "enabled": true,
                        "allowed_share_ids": []
                    }
                ],
                "shares": [
                    {
                        "id": "root",
                        "share_name": "CCBGRoot",
                        "application_id": "default",
                        "bucket": "root",
                        "prefix": "",
                        "enabled": true,
                        "read_only": false,
                        "browseable": true,
                        "guest_ok": false,
                        "valid_user_ids": ["smb-user-1"],
                        "create_mask": "0660",
                        "directory_mask": "0770"
                    }
                ]
            },
            "applications": [
                {
                    "id": "default",
                    "access_key_id": "test-access",
                    "secret_access_key": "test-secret",
                    "enabled": true
                }
            ]
        }))
        .expect("sample control plane should deserialize")
    }

    fn sample_control_plane_value() -> Value {
        json!({
            "smb_sidecar": {
                "enabled": true,
                "bind_addr": "0.0.0.0",
                "port": 445,
                "mount_root": "/mnt/ccbg/smb/mounts",
                "config_root": "/var/lib/ccbg/smb-sidecar/config",
                "data_root": "/var/lib/ccbg/smb-sidecar/data",
                "workgroup": "WORKGROUP",
                "create_mask": "0660",
                "directory_mask": "0770",
                "vfs_objects": ["catia", "fruit"],
                "users": [
                    {
                        "id": "smb-user-1",
                        "username": "smbuser1",
                        "password": "secret",
                        "enabled": true,
                        "allowed_share_ids": []
                    }
                ],
                "shares": [
                    {
                        "id": "root",
                        "share_name": "CCBGRoot",
                        "application_id": "default",
                        "bucket": "root",
                        "prefix": "",
                        "enabled": true,
                        "read_only": false,
                        "browseable": true,
                        "guest_ok": false,
                        "valid_user_ids": ["smb-user-1"],
                        "create_mask": "0660",
                        "directory_mask": "0770"
                    }
                ]
            },
            "applications": [
                {
                    "id": "default",
                    "access_key_id": "test-access",
                    "secret_access_key": "test-secret",
                    "enabled": true
                }
            ]
        })
    }

    fn sample_share_runtime_state() -> ShareRuntimeState {
        ShareRuntimeState {
            id: "root".to_string(),
            share_name: "CCBGRoot".to_string(),
            mount_path: PathBuf::from("/mnt/ccbg/smb/mounts/root"),
            remote_path: "root".to_string(),
            read_only: false,
            pid: Some(1234),
            unit_name: managed_unit_name("rclone", Some("root")),
            mounted: true,
            running: true,
            rss_bytes: 4096,
            log_path: PathBuf::from("/var/lib/ccbg/smb-sidecar/data/runtime/logs/rclone-root.log"),
            last_error: None,
        }
    }

    fn sample_process_state() -> ManagedProcessState {
        ManagedProcessState {
            role: "smbd".to_string(),
            pid: None,
            running: false,
            rss_bytes: 0,
            unit_name: Some(managed_unit_name("smbd", None)),
            log_path: Some(PathBuf::from(
                "/var/lib/ccbg/smb-sidecar/data/runtime/logs/smbd-launch.log",
            )),
            share_id: None,
            share_name: None,
            mount_path: None,
        }
    }

    #[test]
    fn read_env_file_skips_comments_and_blank_lines() {
        let temp = tempfile::NamedTempFile::new().expect("temp env should create");
        fs::write(
            temp.path(),
            "# comment\n\nCCBG_CONTROL_PLANE_FILE = /tmp/control-plane.json\nOTHER=value\n",
        )
        .expect("temp env should write");
        let values = read_env_file(temp.path()).expect("env file should parse");
        assert_eq!(
            values.get("CCBG_CONTROL_PLANE_FILE"),
            Some(&"/tmp/control-plane.json".to_string())
        );
        assert_eq!(values.get("OTHER"), Some(&"value".to_string()));
    }

    #[test]
    fn normalize_gateway_endpoint_handles_wildcards_and_ipv6() {
        assert_eq!(normalize_gateway_endpoint("0.0.0.0:61080"), "127.0.0.1:61080");
        assert_eq!(normalize_gateway_endpoint("*:61080"), "127.0.0.1:61080");
        assert_eq!(normalize_gateway_endpoint("[::]:61080"), "[::1]:61080");
        assert_eq!(normalize_gateway_endpoint("localhost:61080"), "localhost:61080");
    }

    #[test]
    fn managed_unit_name_is_stable_and_sanitized() {
        let one = managed_unit_name("rclone", Some("Root Share/01"));
        let two = managed_unit_name("rclone", Some("Root Share/01"));
        assert_eq!(one, two);
        assert!(one.starts_with("ccbg-smb-sidecar-rclone-root-share-01-"));
        assert!(one.ends_with(".service"));
        assert_eq!(managed_unit_name("smbd", None), "ccbg-smb-sidecar-smbd.service");
    }

    #[test]
    fn resolve_paths_honors_control_plane_override() {
        let env_file = PathBuf::from("/tmp/ccbg.env");
        let values = BTreeMap::from([(
            "CCBG_CONTROL_PLANE_FILE".to_string(),
            "/srv/ccbg/control-plane.json".to_string(),
        )]);
        let paths = ResolvedPaths::from_parts(env_file.clone(), &values);
        assert_eq!(paths.env_file, env_file);
        assert_eq!(
            paths.control_plane_file,
            PathBuf::from("/srv/ccbg/control-plane.json")
        );
        assert_eq!(paths.state_root, PathBuf::from("/srv/ccbg/smb-sidecar"));
        assert_eq!(
            paths.status_file,
            PathBuf::from("/srv/ccbg/smb-sidecar/status.json")
        );
        assert_eq!(
            paths.metadata_file,
            PathBuf::from("/srv/ccbg/smb-sidecar/managed-runtime.json")
        );
    }

    #[test]
    fn build_sidecar_model_rejects_missing_user_password() {
        let paths = ResolvedPaths::from_parts(PathBuf::from("/tmp/ccbg.env"), &BTreeMap::new());
        let env_values = BTreeMap::new();
        let mut payload = sample_control_plane_value();
        payload["smb_sidecar"]["users"][0]["password"] = Value::Null;
        let control_plane: ControlPlaneFile =
            serde_json::from_value(payload).expect("deserialize mutated control plane");
        let error = build_sidecar_model(&paths, &env_values, &control_plane)
            .expect_err("model should reject missing password");
        assert!(error.to_string().contains("SMB user is missing id/username/password"));
    }

    #[test]
    fn build_sidecar_model_rejects_unknown_application() {
        let paths = ResolvedPaths::from_parts(PathBuf::from("/tmp/ccbg.env"), &BTreeMap::new());
        let env_values = BTreeMap::new();
        let mut payload = sample_control_plane_value();
        payload["smb_sidecar"]["shares"][0]["application_id"] = json!("missing-app");
        let control_plane: ControlPlaneFile =
            serde_json::from_value(payload).expect("deserialize mutated control plane");
        let error = build_sidecar_model(&paths, &env_values, &control_plane)
            .expect_err("model should reject unknown application");
        assert!(error.to_string().contains("references unknown application missing-app"));
    }

    #[test]
    fn read_status_returns_contract_fallback_when_missing() {
        let temp_dir = tempfile::tempdir().expect("tempdir should create");
        let paths = ResolvedPaths {
            env_file: temp_dir.path().join("ccbg.env"),
            control_plane_file: temp_dir.path().join("control-plane.json"),
            state_root: temp_dir.path().join("smb-sidecar"),
            status_file: temp_dir.path().join("smb-sidecar").join("status.json"),
            metadata_file: temp_dir
                .path()
                .join("smb-sidecar")
                .join("managed-runtime.json"),
        };
        let status = read_status(&paths).expect("missing status should fall back");
        assert_eq!(status["state"], "unknown");
        assert_eq!(status["mode"], "host_process");
        assert_eq!(status["auto_managed"], true);
        assert_eq!(
            status["runtime_root"],
            Value::String(paths.state_root.display().to_string())
        );
    }

    #[test]
    fn build_sidecar_model_uses_application_credentials() {
        let paths = ResolvedPaths::from_parts(PathBuf::from("/tmp/ccbg.env"), &BTreeMap::new());
        let env_values = BTreeMap::from([
            ("CCBG_BIND_ADDR".to_string(), "0.0.0.0:61080".to_string()),
            ("CCBG_S3_REGION".to_string(), "us-east-1".to_string()),
        ]);
        let model = build_sidecar_model(&paths, &env_values, &sample_control_plane())
            .expect("model should build");
        assert_eq!(model.gateway_endpoint, "127.0.0.1:61080");
        assert_eq!(model.vfs_objects, vec!["catia".to_string()]);
        assert_eq!(model.users.len(), 1);
        assert_eq!(model.shares.len(), 1);
        assert_eq!(model.shares[0].access_key_id, "test-access");
        assert_eq!(model.shares[0].secret_access_key, "test-secret");
        assert_eq!(
            model.shares[0].valid_usernames,
            vec!["smbuser1".to_string()]
        );
    }

    #[test]
    fn runtime_spec_and_config_generation_follow_contract() {
        let paths = ResolvedPaths::from_parts(PathBuf::from("/tmp/ccbg.env"), &BTreeMap::new());
        let env_values = BTreeMap::from([
            ("CCBG_BIND_ADDR".to_string(), "127.0.0.1:61080".to_string()),
            ("CCBG_S3_REGION".to_string(), "us-east-1".to_string()),
        ]);
        let model = build_sidecar_model(&paths, &env_values, &sample_control_plane())
            .expect("model should build");
        let runtime = runtime_spec_payload(&model);
        assert_eq!(runtime.runtime_spec_version, 2);
        assert_eq!(runtime.shares.len(), 1);
        assert_eq!(runtime.users.len(), 1);

        let rclone_conf = generate_rclone_conf(&model);
        assert!(rclone_conf.contains("[ccbg-root]"));
        assert!(rclone_conf.contains("access_key_id = test-access"));
        assert!(rclone_conf.contains("secret_access_key = test-secret"));

        let smb_conf = generate_smb_conf(&model);
        assert!(smb_conf.contains("vfs objects = catia"));
        assert!(smb_conf.contains("[CCBGRoot]"));
        assert!(smb_conf.contains("valid users = smbuser1"));
        assert!(!smb_conf.contains("fruit:metadata"));
    }

    #[test]
    fn desired_hash_changes_when_runtime_spec_changes() {
        let paths = ResolvedPaths::from_parts(PathBuf::from("/tmp/ccbg.env"), &BTreeMap::new());
        let env_values = BTreeMap::from([
            ("CCBG_BIND_ADDR".to_string(), "127.0.0.1:61080".to_string()),
            ("CCBG_S3_REGION".to_string(), "us-east-1".to_string()),
        ]);
        let model = build_sidecar_model(&paths, &env_values, &sample_control_plane())
            .expect("model should build");
        let mut changed = model.clone();
        changed.port = 1445;
        let first = desired_hash_for_model(&model).expect("hash should build");
        let second = desired_hash_for_model(&changed).expect("hash should build after change");
        assert_ne!(first, second);
    }

    #[test]
    fn share_state_errors_deduplicates_messages() {
        let mut one = sample_share_runtime_state();
        one.last_error = Some("need /dev/fuse".to_string());
        let mut two = sample_share_runtime_state();
        two.id = "family".to_string();
        two.share_name = "Family".to_string();
        two.last_error = Some("need /dev/fuse".to_string());
        let mut three = sample_share_runtime_state();
        three.id = "archive".to_string();
        three.share_name = "Archive".to_string();
        three.last_error = Some("smbd listener failed".to_string());

        let errors = share_state_errors(&[one, two, three]);
        assert_eq!(errors, vec!["need /dev/fuse".to_string(), "smbd listener failed".to_string()]);
    }

    #[test]
    fn build_runtime_payload_without_model_uses_fallback_paths_and_counts() {
        let temp_dir = tempfile::tempdir().expect("tempdir should create");
        let paths = ResolvedPaths {
            env_file: temp_dir.path().join("ccbg.env"),
            control_plane_file: temp_dir.path().join("control-plane.json"),
            state_root: temp_dir.path().join("smb-sidecar"),
            status_file: temp_dir.path().join("smb-sidecar").join("status.json"),
            metadata_file: temp_dir
                .path()
                .join("smb-sidecar")
                .join("managed-runtime.json"),
        };
        let share_states = vec![sample_share_runtime_state()];
        let processes = vec![sample_process_state()];
        let payload = build_runtime_payload(
            &paths,
            None,
            Some("hash-1"),
            "degraded",
            Some("need /dev/fuse"),
            &share_states,
            &processes,
            Some(123456),
        );
        assert_eq!(payload["runtime_root"], paths.state_root.display().to_string());
        assert_eq!(payload["enabled_share_count"], 1);
        assert_eq!(payload["mounted_share_count"], 1);
        assert_eq!(payload["process_count"], 0);
        assert_eq!(payload["listener"], Value::Null);
        assert_eq!(payload["desired_hash"], "hash-1");
    }

    #[test]
    fn build_runtime_payload_with_model_keeps_listener_false_without_running_smbd() {
        let temp_dir = tempfile::tempdir().expect("tempdir should create");
        let env_values = BTreeMap::from([
            (
                "CCBG_SMB_CONFIG_ROOT".to_string(),
                temp_dir.path().join("config").display().to_string(),
            ),
            (
                "CCBG_SMB_DATA_ROOT".to_string(),
                temp_dir.path().join("data").display().to_string(),
            ),
            ("CCBG_BIND_ADDR".to_string(), "127.0.0.1:61080".to_string()),
            ("CCBG_S3_REGION".to_string(), "us-east-1".to_string()),
        ]);
        let paths = ResolvedPaths::from_parts(temp_dir.path().join("ccbg.env"), &env_values);
        let model = build_sidecar_model(&paths, &env_values, &sample_control_plane())
            .expect("model should build");
        let payload = build_runtime_payload(
            &paths,
            Some(&model),
            Some("hash-2"),
            "running",
            None,
            &[],
            &[sample_process_state()],
            None,
        );
        assert_eq!(payload["listener"], format!("{}:{}", model.bind_addr, model.port));
        assert_eq!(payload["listener_ready"], false);
    }

    #[test]
    fn process_count_matches_requires_non_empty_and_all_running() {
        assert!(!process_count_matches(&[]));
        let all_running = vec![ManagedProcessState {
            running: true,
            ..sample_process_state()
        }];
        assert!(process_count_matches(&all_running));
        let mixed = vec![
            ManagedProcessState {
                running: true,
                ..sample_process_state()
            },
            ManagedProcessState {
                role: "rclone:root".to_string(),
                running: false,
                ..sample_process_state()
            },
        ];
        assert!(!process_count_matches(&mixed));
    }

    #[test]
    fn write_runtime_files_and_prepare_tree_create_expected_outputs() {
        let temp_dir = tempfile::tempdir().expect("tempdir should create");
        let env_file = temp_dir.path().join("ccbg.env");
        let control_plane_file = temp_dir.path().join("control-plane.json");
        let env_values = BTreeMap::from([
            (
                "CCBG_SMB_CONFIG_ROOT".to_string(),
                temp_dir.path().join("config").display().to_string(),
            ),
            (
                "CCBG_SMB_DATA_ROOT".to_string(),
                temp_dir.path().join("data").display().to_string(),
            ),
            ("CCBG_BIND_ADDR".to_string(), "127.0.0.1:61080".to_string()),
            ("CCBG_S3_REGION".to_string(), "us-east-1".to_string()),
        ]);
        let paths = ResolvedPaths::from_parts(env_file, &env_values);
        let model = build_sidecar_model(&paths, &env_values, &sample_control_plane())
            .expect("model should build");

        write_runtime_files(&model).expect("runtime files should write");
        prepare_samba_runtime_tree(&model).expect("runtime tree should prepare");

        let rclone_conf = fs::read_to_string(&model.rclone_conf_path)
            .expect("rclone conf should exist");
        let smb_conf = fs::read_to_string(&model.smb_conf_path).expect("smb conf should exist");
        assert!(rclone_conf.contains("[ccbg-root]"));
        assert!(smb_conf.contains("[CCBGRoot]"));
        assert!(model.runtime_root.join("run").exists());
        assert!(model.runtime_root.join("private").exists());
        assert!(model.runtime_root.join("state").exists());
        assert!(control_plane_file.parent().unwrap().exists());
    }

    #[test]
    fn sync_disabled_writes_disabled_status_and_metadata() {
        let temp_dir = tempfile::tempdir().expect("tempdir should create");
        let paths = ResolvedPaths {
            env_file: temp_dir.path().join("ccbg.env"),
            control_plane_file: temp_dir.path().join("control-plane.json"),
            state_root: temp_dir.path().join("smb-sidecar"),
            status_file: temp_dir.path().join("smb-sidecar").join("status.json"),
            metadata_file: temp_dir
                .path()
                .join("smb-sidecar")
                .join("managed-runtime.json"),
        };
        fs::write(&paths.control_plane_file, "{}\n").expect("control plane should write");

        run_sync_with_paths(&paths).expect("disabled sync should succeed");

        let status_raw = fs::read_to_string(&paths.status_file).expect("status should exist");
        let status: Value = serde_json::from_str(&status_raw).expect("status should parse");
        assert_eq!(status["state"], "disabled");
        assert_eq!(status["mode"], "host_process");

        let metadata_raw =
            fs::read_to_string(&paths.metadata_file).expect("metadata should exist");
        let metadata: Value = serde_json::from_str(&metadata_raw).expect("metadata should parse");
        assert_eq!(metadata["state"], "disabled");
    }

    #[test]
    fn stop_writes_stopped_status_and_metadata() {
        let temp_dir = tempfile::tempdir().expect("tempdir should create");
        let paths = ResolvedPaths {
            env_file: temp_dir.path().join("ccbg.env"),
            control_plane_file: temp_dir.path().join("control-plane.json"),
            state_root: temp_dir.path().join("smb-sidecar"),
            status_file: temp_dir.path().join("smb-sidecar").join("status.json"),
            metadata_file: temp_dir
                .path()
                .join("smb-sidecar")
                .join("managed-runtime.json"),
        };

        run_stop_with_paths(&paths).expect("stop should succeed");

        let status_raw = fs::read_to_string(&paths.status_file).expect("status should exist");
        let status: Value = serde_json::from_str(&status_raw).expect("status should parse");
        assert_eq!(status["state"], "stopped");

        let metadata_raw =
            fs::read_to_string(&paths.metadata_file).expect("metadata should exist");
        let metadata: Value = serde_json::from_str(&metadata_raw).expect("metadata should parse");
        assert_eq!(metadata["state"], "stopped");
    }
}

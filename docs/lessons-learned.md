# CCBG Lessons Learned

## Object Delete Convergence

- Delete paths must be idempotent. If the backend object is already gone, `NotFound` should not
  block metadata cleanup.
- Residual object cleanup must remove the full metadata set together: placement, logical object,
  and protection plan.
- Cleanup UI should describe the action as removing residual object metadata, not only placement,
  so operators do not assume a narrower effect than the code actually has.
- Bulk cleanup should classify rows by live backend state and metadata state, not by prefix listing
  alone.
- Missing placement metadata must be treated as a repair condition, not as a signal to fall back
  to the current primary provider.
- Missing placement metadata must also not become an automatic cleanup path. If `logical_objects`
  or `object_protection_plans` still exist, the safer default is to block and require a dedicated
  repair flow that proves whether the backend object is gone or the placement row was lost.
- Cleanup tooling needs an orphan view for `logical_objects` and `object_protection_plans`; a
  placement-only scan can hide real residue.
- Replication-delete persistence and WAL replay are recovery mechanisms, not substitutes for an
  atomic delete guarantee.
- Multipart upload sessions need their own expiry and cleanup cadence, or aborted uploads will
  linger as hidden state even after object deletion looks healthy.
- Backup import must validate cross-table consistency before replacing state, otherwise a restore
  can reintroduce orphan metadata.

## Deployment

- When deploying from Windows to a Linux host, build the Linux ELF inside the local Podman
  build-runner image instead of relying on `cargo build --release` on the host.
- Deploy `gatewayd` and `assets/admin/index.html` together, then verify both remote hashes and
  service health before considering the deploy complete.
- SMB sidecar host-process mode needs its optional Samba VFS modules to match the generated
  `smb.conf`. If `fruit` is enabled in config but `samba-vfs-modules` is missing, the sidecar can
  still look `running` while client login fails at `IPC$`.
- `rclone mount` permission flags alone are not enough for SMB group write access. Without an
  explicit `--umask 007`, `--dir-perms 0770` / `--file-perms 0660` are masked down to
  `0750` / `0640`, which lets SMB users read but blocks create/delete.
- For SMB sidecar host-process mode, do not use `ccbg-smb-sidecar-sync.service` as the liveness
  signal. It is intentionally short-lived; the real checks are `/opt/ccbg/bin/smb-sidecar-host status`
  plus the transient `ccbg-smb-sidecar-*.service` units.
- The documented release host must match the actual active build machine. When the build host moves
  from one LAN node to another, update the release SOP, checklist, host-boundary ADR, and workflow
  entrypoint notes in the same change; stale host labels will send operators down the wrong path.
- A containerized build on the same Windows host cannot use `127.0.0.1:10808` as its proxy; the
  build-runner container must use the host LAN IP (`192.168.1.52:10808` in the current setup).
- Keep release workflow defaults aligned with the image that actually exists. If the current
  build-runner image cannot link Darwin targets, macOS jobs should stay disabled by default rather
  than failing every manual release run.
- GitHub Actions artifact downloads are not guaranteed to keep one fixed directory shape. Release
  helper scripts must tolerate both `dest/<files>` and `dest/<artifact>/<files>` layouts.
- When Bash/WSL launches Windows CLIs such as `gh.exe` or `python.exe`, convert absolute POSIX
  paths before handing them over. Otherwise `/mnt/d/...` can be misread as `D:\mnt\d\...` and
  release helper scripts fail for path reasons instead of real build reasons.
- On mixed Windows environments, full quality gates and final asset merge/upload do not need to use
  the same shell. Use the shell that gives stable test behavior for the gate, and the shell that
  gives correct CLI path semantics for the artifact merge.
- Native Windows/macOS installs cannot rely on `CCBG_CONFIG_DIR` alone when the working directory is
  moved to the data dir. They must also point `CCBG_BROWSER_FLOW_CATALOG_DIR`,
  `CCBG_PROVIDER_BRIDGE_CATALOG_DIR`, and `CCBG_PROVIDER_CAPABILITY_CATALOG_DIR` at the installed
  config tree, or `gatewayd` can fail startup validation on a clean host.
- Native package `--skip-build` paths must not silently fall back to `target/release/gatewayd` for a
  different target triple. If the requested target binary is missing, packaging should fail instead
  of producing a structurally valid archive with the wrong executable inside.
- Release merge helpers must validate externally downloaded artifacts against their `.sha256` sidecars
  before copying them into the final release directory, and should rewrite copied checksum sidecars
  to reference the local release filenames rather than stale build-host paths.
- Release asset download helpers must treat duplicate filename matches as an error. Quietly taking
  the first `find` result is not acceptable for formal release inputs.
- Package smoke tests for critical release paths must restore any temporary `target/<triple>/release`
  overrides and clean their own smoke artifacts. Otherwise the test harness itself can contaminate
  later `--skip-build` release work.
- When launching long-running sidecar processes under `systemd-run`, prefer `StandardOutput=` /
  `StandardError=` properties over shell redirection. It avoids transient-unit escape warnings and
  keeps runtime logs attached to the unit cleanly.
- Sidecar runtime generation should tolerate missing optional VFS plugins and degrade to the
  modules actually present on the host instead of producing a false-healthy SMB listener that
  rejects all sessions.

## Container Memory / OOM

- cgroup v2 counts file page cache against the LXC memory limit. On `.49` (CCBG gateway LXC),
  gatewayd's own anonymous memory is only ~60 MB, but spool I/O (`/var/lib/ccbg/body-spool`)
  balloons page cache to ~115 MB, so the container runs near its ceiling even though the process
  looks small. Raising the LXC `memory` from 128 MiB to 192 MiB left `memory.peak` at ~190 MiB —
  only ~1 MiB of headroom, so OOM still fired.
- OOM history on `.49` is unambiguous in journald: gatewayd was killed by the OOM killer on
  `2026-07-30 11:34:09Z` and `2026-07-31 11:06:32Z` (both `result 'oom-kill'`, restart counter 1).
  The `Main process exited, code=killed, status=9/KILL` line is the OOM signature; a SIGSEGV
  (`status=11/SEGV`) is a different failure class and should not be conflated.
- An LXC guest cannot create its own swap or tune its own memory controller from inside. `swapon`
  fails with `Operation not permitted` (needs host-side device cgroup / CAP_SYS_ADMIN), and writing
  `/sys/fs/cgroup/memory.high` fails with `Permission denied` because the container's root cgroup is
  host-owned. Both must be configured on the PVE host (`pct set <ctid> --swap` / `lxc.cgroup2.memory.high`).
- LXC virtual swap (`/proc/swaps` shows `none virtual`) only helps if the host actually has swap
  backing it; verify with `free -h` on the PVE host before relying on `--swap`.
- The right mitigation for page-cache-driven OOM is a `memory.high` soft limit well below the hard
  `memory.max`, so the kernel reclaims cache proactively instead of waiting until the hard OOM
  cliff. A containerized blob gateway with spool streaming needs explicit headroom between
  `memory.high`, `memory.max`, and the observed `memory.peak`.
- `pct set <ctid> --memory-high` is NOT a valid option; PVE rejects it with `Unknown option:
  memory-high`. `lxc.cgroup2.*` knobs must be written directly into `/etc/pve/lxc/<ctid>.conf`
  (e.g. `lxc.cgroup2.memory.high: 209715200`) and take effect only after the container restarts.
  Verify on the host with `cat /sys/fs/cgroup/lxc/<ctid>/memory.high`, not from inside the guest
  (the guest sees `max` for its own root cgroup even when the host-side value is set).
- Applied 2026-08-04 on `.49` (CT 104): `memory` 256 MiB, `swap` 1 GiB,
  `memory.high` 209715200 (200 MiB), verified host-side. gatewayd healthy
  (`/healthz` 200) after the reboot that applied the settings.

## S3 Compatibility Gaps

- Directory traversal for mounted clients must be driven by local placement metadata, and the
  emitted listing timestamps must stay S3-compatible; provider-native timestamp strings can break
  `rclone mount` even when exact-key reads already work.
- Multipart completion must accept entity-encoded ETags, return response headers promptly, and
  persist the final S3-compatible multipart ETag; otherwise clients can time out or reject the
  upload as corrupted.
- Public `HEAD` / `LIST` responses must prefer the gateway's persisted logical `ETag` over a
  backend-native development `ETag`; otherwise multipart-complete objects can be written
  correctly but later fail client-side verification.
- Stub-provider smoke and real-client interoperability are different layers. `rclone` needs its
  validated compatibility knobs (`use_unsigned_payload`, `use_multipart_etag`,
  `disable_checksum`) in smoke and operator docs, or the harness will report false negatives that
  do not match production S3 application flows.
- Large-object smoke tests should stay explicit about the currently validated size boundary and
  provider capability flags; passing a stress test does not mean the provider has unlocked a larger
  permanent upload policy.
- Cleanup after soak or recovery work must delete both cloud objects and local metadata rows; a
  successful backend delete is not enough if placement, logical object, or protection-plan residue
  remains.
- China Mobile overwrite writes must not delete the old same-name object before a new upload plan is
  actually required by the upstream `exist=true && uploadId missing` branch. If `file/create`
  itself fails, eager deletion turns a recoverable write failure into real data loss.

## Provider Keepalive

- Keepalive verification must run in a dedicated low-frequency loop, not as a side effect of
  webhook delivery.
- The smallest stable keepalive path is each provider's direct `health()` probe; CDP/browser
  stays capture-only and must not become a long-running dependency.
- Separate polling knobs for webhook alerts and provider lease probes so one can be disabled or
  slowed without affecting the other.
- OneDrive stays parked and does not participate in the active keepalive path.
- Lease records need to preserve enough incident history to answer "auth died first, or service
  stalled first" after a reboot. A single `last_verified_at_unix_ms` that is overwritten on every
  poll is not enough; keep at least a first-failure or last-status-change timestamp.
- Dogfood primary-provider auth expiry must be treated as a first-class availability incident, not
  only an Admin warning. On `.49` in July 2026, both `unicom` and `telecom` leases had already lost
  their last successful verification on `2026-07-06`, days before the manual reboot on
  `2026-07-09 15:56:26Z`.
- A clean reboot sequence is different from a crash. For the same `.49` incident, system logs showed
  `systemd-logind: The system will reboot now!` and an orderly shutdown of `ccbg.service`, SMB,
  and SSH, so the box was manually rebooted after the degraded state rather than rebooting because
  of an OOM, kernel panic, or disk failure.
- Current lease probing is only observability plus reauth signaling. It does not auto-refresh
  `unicom` / `telecom`, demote a stale primary provider, or fast-fail request paths before they hit
  slow upstream auth errors. If dogfood responsiveness matters more than preserving the stale
  primary choice, the gateway needs an explicit policy for stale-primary fencing or automatic
  fallback.
- Periodic direct health probes do not extend browser session TTL. Providers like Unicom and Telecom
  expire session cookies based on browser-side activity, not API access. CDP page `Page.reload` is a
  more promising keepalive strategy than direct health checks.
- CDP keepalive must bind to the specific endpoint and target page used during credential capture.
  Guessing the right endpoint across multiple CDP browsers leads to wrong-page reloads or no-ops.
  Store `capture_cdp_endpoint_url` and `capture_cdp_target_selector` in provider credentials.
- When saving captured credentials, the save path must include the session's CDP binding metadata.
  The Provider Credential Input DTO and the front-end `collectProviderCredentialInput` must both
  carry `capture_cdp_endpoint_url` and `capture_cdp_target_selector`, or keepalive cannot bind to
  the correct browser target.
- Operator-selectable CDP endpoint per carrier assistant is essential. Without explicit selection,
  keepalive may target the wrong browser tab (e.g., a temp-mail page instead of the carrier page).
- CDP keepalive using `Page.reload` is an experimental strategy. Its effectiveness varies by
  provider: it works when the carrier's session TTL is extended by browser page activity, but
  does not help if the session is purely time-based or if the CDP target page has navigated away.

## Pluggable Entry Points

- Capability entry scripts should be idempotent so operators can rerun them after host reboots
  without creating duplicate browser or bridge processes.
- Prefer checking the smallest necessary local state before launching anything; do not turn entry
  scripts into permanent background daemons.
- Keep browser/CDP handling capture-only and outside the core runtime so low-memory deployments can
   omit it entirely.

## Stale Primary Fencing

- Alert-only fencing is not enough for production. The routing layer must also deprioritise the
  stale primary so new PUT objects land on a healthy write target instead of queueing behind auth
  failures.
- The right place to apply stale-primary filtering is `eligible_write_candidates_for_object`, not
  `select_home_write_provider` — the former owns the candidate ordering used by all write paths.
- Auto-unfence is implicit: when the lease record transitions from `reauth_required` back to
  `active`, `is_primary_stale` returns `false` and the primary re-enters the candidate pool
  automatically. No separate reset logic is needed.
- A stale primary with no healthy write targets should still accept writes. The fencing code must
  guard against removing the last candidate: if the ordered list becomes empty after removing the
  primary, re-add the primary.

## WAL Commit Failure Alerting

- WAL commit failures must be tracked in runtime state, not only logged as `warn!`. The runtime
  state is consumed by `build_admin_alerts` to produce a visible error alert in the Admin panel.
- Clearing the failure state on the next successful commit is important: stale failure alerts
  erode operator trust. Use `mark_gateway_write_ahead_log_committed_or_warn` to set _and_ clear
  the runtime state in one place.
- The alert detail should include the bucket, key, and error message so operators can diagnose
  without digging through logs.

## CDP Keepalive Monitoring

- With `RUST_LOG=warn`, the keepalive loop produces no visible log output on success. To monitor
  keepalive activity, either lower the log level, write a separate polling script, or expose
  keepalive status through a non-admin endpoint.
- The CDP keepalive runtime state (`provider_cdp_keepalive`) is only visible through the Admin
  status API, which requires a session cookie. For automated monitoring, either use the control
  API key or a separate monitor script that scrapes `journalctl` for keepalive-related entries.

## Remote Ops Scripting

- When deploying shell scripts to remote hosts via SSH from PowerShell, heredocs with embedded
  quotes (`"`, `'`) are fragile. Write the script to a local temp file first, then `scp` it to
  the target host. This avoids PowerShell heredoc parsing issues with Python f-strings, shell
  variable interpolation, and nested quotes.
- `nohup` + `&` over SSH on systemd hosts can cause the SSH session to hang waiting for the
  background process to detach. Use `setsid` or `systemd-run --user` to fully detach long-running
  monitor scripts.

## Security Audit Gaps Closed (2026-08)

- `/v1/containers` and `/v1/objects` JSON listing endpoints previously performed no
  `authorize_s3` check — only data-plane rate limiting. Any "JSON listing endpoint that exposes
  S3 metadata" must authenticate with the same `authorize_s3` + `ensure_s3_application_permission`
  path as the S3 routes; treat no JSON route as implicitly public just because it is a helper API.
- Admin login had no brute-force throttle. A per-username rate limiter (5 attempts / 60 s,
  module-level `OnceLock<Mutex<HashMap>>`, recording before credential verification and cleared on
  success) is enough to stop online guessing without breaking legitimate use. The limiter returns
  the same 401 status as bad credentials, differing only in the message body.
- The provider-limit-probe SSE stream used `mpsc::unbounded_channel()`. A slow HTTP consumer can
  grow the channel without bound (OOM/backlog vector). Use a bounded channel (`channel(32)`) with
  `try_send` for best-effort progress events and guaranteed delivery for terminal events.

## Tokio Async Channel Pitfalls

- `tokio::sync::mpsc::Sender::blocking_send` panics with "Cannot block the current thread from
  within a runtime" when called inside a `tokio::spawn` task. Use `sender.send(event).await`
  instead: it yields to the runtime while waiting for buffer space and still guarantees delivery.
  `blocking_send` is only valid from a non-async context (e.g. a dedicated blocking thread).

## CORS / tower-http 0.6.8

- `CorsLayer::allow_origin(Any)` combined with `allow_credentials(true)` panics at startup in
  `tower-http >= 0.6` ("Cannot combine Access-Control-Allow-Credentials: true with
  Access-Control-Allow-Origin: *"). For an admin API that needs credentialed cross-origin access,
  use `AllowOrigin::mirror_request()` to echo the request origin — valid with credentials and
  verified to emit the correct `access-control-allow-origin` header.

## Secure Cookie Over Plain HTTP

- `CCBG_ADMIN_COOKIE_SECURE` defaults to `true`. On an HTTP-only deployment (no TLS), the browser
  refuses to store the `Secure` session cookie: login reports success but the redirect back to `/`
  carries no session and bounces to `/login` again. HTTP-only deployments MUST set
  `CCBG_ADMIN_COOKIE_SECURE=false`; set it back to `true` only behind a TLS reverse proxy.

## Cross-Compiling the Linux Binary Without the Build Runner

- When the `.52` Podman build runner is unreachable, a Windows host can still produce the Linux
  ELF with `cargo-zigbuild --release --target x86_64-unknown-linux-gnu`. Zig acts as the Linux
  linker; `zig.exe` needs its full bundled directory (not just the binary) or it fails with
  "unable to find zig installation directory". If the build host has no internet, download the
  zig archive on a host that does (e.g. `.49`) and `scp` it back.
- The resulting ELF must be packaged with `build-lxc-package.sh --binary <linux-gatewayd>
  --helper-binary <linux-smb-sidecar-host>` on a Linux host (the script uses `file`/`install`/
  `sha256sum`/`tar`). A stub `cargo` shim satisfies its unconditional `resolve-cargo.sh` when
  `--skip-build` is in effect.

## Related Docs

- [Object Delete Convergence spec](SPEC.md#object-delete-convergence)
- [PVE/LXC deployment guide](pve-lxc-deployment.md)

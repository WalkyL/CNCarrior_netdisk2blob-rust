# CCBG TODO Closeout Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 收口 CCBG 仓库内可由本地源码、脚本和文档证明的 TODO，同时把 .43 人工验收、真实运营商凭据、真机运行、签名/公证和正式发布保留为明确的外部 gate。

**Architecture:** 以当前 Rust 能力、provider/catalog 配置和既有 release 脚本为事实源；先同步过时文档，再运行现有质量门和发布材料 smoke。只在本地 CCBG 专用临时目录生成可审计的包、checksum 和 provenance，验证后删除，不新增第二套发布流程。

**Tech Stack:** Rust workspace/Cargo、Bash release scripts、Python catalog/license/provenance checks、Markdown documentation、Git、CodeGraph local index。

## Global Constraints

- 保留 .codegraph/ 及其本地索引，不修改其他项目、共享 Cargo 缓存、.49 线上配置或远程仓库。
- 不读取、猜测、写入或提交运营商凭据；不把 .43 浏览器人工验收、Windows/macOS 真机结果或 macOS 签名/公证结果伪造为完成。
- 先用当前源码和测试确认能力，再改文档；历史 CDP 观察保留时间上下文，并标注已被 credential lease probe 方案取代。
- 新鲜验证统一使用 CCBG 专用临时路径；只删除本次创建且已核对为 CCBG 所有的目录。
- 任何脚本/源码修复都遵循 RED -> GREEN -> REFACTOR，并在提交前运行 git diff --check。
- 不执行 GitHub Release 上传、Cloudflare 发布、包管理器上架或 git push。

## File Map

- README.md：当前 provider、S3、平台和真实回归边界。
- docs/auth-step-by-step.md：认证和三家 provider 的当前能力说明。
- docs/lessons-learned.md：lease probing、primary fencing、CDP keepalive 的历史与当前实现分界。
- docs/todo-20260711.md：将已取代的 CDP keepalive 监控项从活动 TODO 中移出，并指向当前 lease probe 验证。
- docs/todo.md：保留真实外部 TODO，并明确仓库内可执行项已收口。
- docs/release-checklist.md：记录本地质量门/材料 smoke 证据，保留正式 release 和人工 gate。
- docs/windows-macos-public-site-rollout-todo.md：将已落地的 catalog、打包、模板和自动化状态与真机 gate 分离。
- docs/superpowers/plans/2026-08-21-todo-closeout.md：本计划。
- scripts/ 下已有 package/release/check 脚本：仅在新鲜测试证明存在缺陷时修改，并在对应脚本测试中留下回归断言。

### Task 1: Establish the current capability ledger

**Files:**
- Read: Cargo.toml, crates/gatewayd/src/main.rs, crates/provider-telecom/src/lib.rs, crates/provider-mobile/src/lib.rs, config/provider-capabilities/*.json, config/provider-probes/*.json
- Read: scripts/check-release-ready.sh, scripts/release-local.sh, scripts/build-native-package.sh, scripts/generate-release-provenance.py
- Modify: none

**Interfaces:**
- Consumes: current source implementations, provider capability catalogs, package scripts, and origin/main...HEAD history.
- Produces: an evidence table used by Tasks 2–5; no production behavior change.

- [ ] **Step 1: Record the repository baseline.**

Run from D:\workspaces\ccbg:

~~~
git status --short --branch
git diff --check
git diff --stat origin/main...HEAD
codegraph status
~~~

Expected: no unrelated uncommitted changes, .codegraph remains present, and the only pre-existing local commits are the design/CodeGraph commits identified in the handoff.

- [ ] **Step 2: Confirm each stale documentation claim against source.**

Run:

~~~
rg -n "Presigned|presigned|Multipart|multipart|CopyObject|copy_object|RenameObject|MoveObject|virtual-host|path-style" README.md crates config docs
rg -n "family|rename|copy|move|delete|multipart" crates/provider-telecom crates/provider-mobile docs/auth-step-by-step.md config/provider-capabilities config/provider-probes
rg -n "CREDENTIAL_LEASE|credential lease|CDP_KEEPALIVE|cdp_keepalive|primary_fenced|first_failure_at" crates/gatewayd deploy config docs
~~~

Expected: later documentation edits claim only paths represented by current code/catalogs; live provider success remains described as requiring operator-supplied credentials and real acceptance.

- [ ] **Step 3: Capture the exact current release identity.**

Run:

~~~
grep -nE '^version =|DEFAULT_RELEASE_FINGERPRINT|CCBG_RELEASE_DATE' Cargo.toml crates/gatewayd/src/main.rs PROVENANCE.md
~~~

Use the observed 0.1.12/fingerprint values in local evidence notes only; do not create a new formal release tag or alter release constants as part of TODO closeout.

### Task 2: Synchronize documentation with verified facts

**Files:**
- Modify: README.md
- Modify: docs/auth-step-by-step.md
- Modify: docs/lessons-learned.md
- Modify: docs/todo-20260711.md
- Modify: docs/todo.md

**Interfaces:**
- Consumes: Task 1 evidence ledger.
- Produces: consistent user-facing capability and TODO status text without changing runtime behavior.

- [ ] **Step 1: Correct the README capability summary.**

Update the opening provider summary and the “当前 S3 兼容实现边界” section so they reflect the current implementation confirmed in Task 1: telecom object actions and family/upload paths that are actually implemented, the currently supported S3 features (including presigned/multipart only where source/tests confirm them), and the remaining limitation that provider full real-world regression still needs live credentials. Keep the China Mobile 8 GiB upstream 04010319 and local No space left on device evidence explicitly bounded; do not turn it into a general large-file guarantee.

- [ ] **Step 2: Correct provider status in the authentication guide.**

In the Telecom “当前代码支持到什么程度” section, replace stale “family upload 未启用” and “rename/copy/move 未完成” wording when Task 1 confirms those paths in source/tests. State separately that local implementation/tests do not prove a live operator session, and retain the credential-injection and no-secret-logging rules. Keep the Unicom/Mobile and OneDrive boundaries unchanged unless Task 1 finds a concrete contradiction.

- [ ] **Step 3: Mark the old CDP keepalive record as historical and superseded.**

In docs/todo-20260711.md, retain the historical date, endpoint, and monitoring context, but change the active section title/status to say that the CDP reload keepalive experiment is superseded by direct provider credential lease probing. Point readers to the current lease-probe configuration/verification path and do not mark a 24-hour live .49 observation as complete without a fresh external record.

In docs/lessons-learned.md, preserve the historical CDP observations and add a current-state paragraph under the keepalive material: direct provider health/credential lease probes are the runtime keepalive path, CDP is for login/capture and optional browser workflows, and the probe only reports/requires reauthentication unless a separately documented policy says otherwise.

- [ ] **Step 4: Make the main TODO ledger explicit about the external boundary.**

Near OPS-006 in docs/todo.md, add a dated closeout note that repository-local checks, scripts, package structure checks, and documentation synchronization are handled by this plan, while .43 deployment/manual acceptance remains pending until its health/admin/browser checklist is executed by an operator with the required external access.

- [ ] **Step 5: Run documentation-only checks and commit the fact sync.**

Run:

~~~
git diff --check
python3 scripts/catalog-lint.py
python3 scripts/license-check.py --skip-cargo-metadata
~~~

Expected: Markdown changes have no whitespace errors, catalog/license checks pass, and no credential-like values were introduced. Commit the coherent documentation group:

~~~
git add docs/superpowers/plans/2026-08-21-todo-closeout.md README.md docs/auth-step-by-step.md docs/lessons-learned.md docs/todo-20260711.md docs/todo.md
git commit -m "docs: synchronize CCBG todo status"
~~~

### Task 3: Verify and harden the local quality gates

**Files:**
- Test/inspect: scripts/check-release-ready.sh, scripts/test-native-package-scripts.sh, scripts/check-native-package-smoke.sh, scripts/test-release-asset-merge-scripts.sh, scripts/test-smb-sidecar-host-all.sh
- Modify only if a reproducible failure identifies a script defect: the failing script and its corresponding test script.

**Interfaces:**
- Consumes: Task 2 documentation state and existing release/check interfaces.
- Produces: fresh local pass/fail evidence; any fix has a focused regression test and a separate commit.

- [ ] **Step 1: Establish the RED/GREEN gate for existing script tests.**

Run the existing edge and smoke suites before changing implementation:

~~~
bash -n scripts/check-release-ready.sh scripts/release-local.sh scripts/build-native-package.sh
bash scripts/test-native-package-scripts.sh
bash scripts/check-native-package-smoke.sh
bash scripts/test-release-asset-merge-scripts.sh
env SMB_SIDECAR_SKIP_UNIT=1 bash scripts/test-smb-sidecar-host-all.sh
~~~

Expected: every command exits zero. If a command fails, preserve its output, identify whether the cause is missing host tooling, an external dependency, or a repository defect, and do not paper over an external failure.

- [ ] **Step 2: Add a regression assertion before fixing any repository defect.**

For a repository defect, extend the smallest existing shell test with a deterministic fixture and an expect-fail/expect-pass assertion that reproduces the observed behavior. Run that focused test and confirm it fails for the intended reason before editing the implementation. Do not add a test for an unavailable Windows/macOS host or an operator credential.

- [ ] **Step 3: Implement the smallest local fix and refactor the test harness.**

Change only the script behavior covered by the failing assertion. Re-run the focused test, then the complete affected script suite. Use bash -n on every modified shell script and git diff --check; leave release upload and cloud deployment flags disabled.

- [ ] **Step 4: Commit a script fix only when one was required.**

If no repository defect is found, make no empty “fix” commit. If one is found, stage exactly the two literal paths identified in Step 2 (the failing script and its matching test; do not use a broad glob), then commit them:

~~~
git commit -m "test: cover local release gate edge case"
~~~

### Task 4: Generate and verify CCBG-local release evidence

**Files:**
- Use: scripts/check-release-ready.sh, scripts/release-local.sh, scripts/test-release-asset-merge-scripts.sh, scripts/generate-release-provenance.py, scripts/check-backup-restore-drill.py
- Create temporarily only under: target/ccbg-todo-closeout-* and target/release-local/ccbg-todo-closeout-*
- Modify: none

**Interfaces:**
- Consumes: the current committed tree and the package/release script interfaces.
- Produces: disposable package-structure, checksum, provenance, backup-drill, and release-asset-merge evidence; no public release.

- [ ] **Step 1: Run Rust checks with an isolated CCBG target directory.**

From Git Bash or WSL, set CARGO_TARGET_DIR to a CCBG-only temporary directory such as D:/workspaces/ccbg/target/ccbg-todo-closeout-cargo, then run:

~~~
cargo fmt --all -- --check
cargo check --workspace --locked
cargo test --workspace --locked
~~~

Expected: format, check, and all workspace tests pass. The target directory must be inside CCBG and must not be a shared Cargo cache.

- [ ] **Step 2: Run the complete release-ready gate.**

Run the existing gate with the same isolated CARGO_TARGET_DIR and with native package smoke enabled only when the host has the required package tooling:

~~~
CCBG_CHECK_NATIVE_PACKAGE_SMOKE=true bash scripts/check-release-ready.sh
~~~

Record each command’s exit status. A missing external binary, unavailable provider session, or absent .43 host is an external/environment limitation; do not convert it to a source success claim.

- [ ] **Step 3: Exercise disposable release metadata and asset merge paths.**

Run:

~~~
bash scripts/test-release-asset-merge-scripts.sh
~~~

Verify the generated local release directory contains only the expected artifact pairs plus ccbg-checksums.txt, release-provenance.json, and release-provenance.md; verify every checksum with sha256sum -c and parse provenance JSON with Python. Do not set CCBG_RELEASE_UPLOAD_GITHUB=true, invoke Cloudflare deployment, or use a formal release tag.

- [ ] **Step 4: Validate package and backup drill outputs.**

Use the existing native package smoke and backup drill scripts to verify bin/, assets/admin/, platform install/uninstall scripts, manifests, and checksums. Confirm fake binaries are used only for structure smoke and that the result is not described as Windows/macOS runtime acceptance. Confirm the backup drill uses a CCBG-local scratch path and reports a successful restore.

- [ ] **Step 5: Remove only the exact temporary outputs.**

Before deletion, list the exact CCBG paths created by this task and confirm they are under D:\workspaces\ccbg\target\ccbg-todo-closeout-* or the matching CCBG release-local directory. Remove those paths, then verify they no longer exist. Preserve .codegraph, source files, existing user-owned artifacts, and shared Cargo caches.

### Task 5: Update the evidence ledger and release gates

**Files:**
- Modify: docs/release-checklist.md
- Modify: docs/windows-macos-public-site-rollout-todo.md
- Modify: docs/todo.md only if the evidence note in Task 2 needs the final command/date/commit values
- Modify: docs/lessons-learned.md only if a new local limitation must be recorded; do not rewrite historical entries

**Interfaces:**
- Consumes: fresh command output from Tasks 3–4.
- Produces: an auditable checklist that distinguishes local completion from external gates.

- [ ] **Step 1: Mark only freshly proven local checks.**

In docs/release-checklist.md, check local format/check/test, catalog, license, public-boundary, package-structure, release-asset-merge, backup-drill, and provenance/checksum evidence only when the corresponding fresh command passed. Keep formal release artifact, .43 manual acceptance, Windows/macOS real-host smoke, signatures/notarization, operator credential refresh, and public upload boxes unchecked.

- [ ] **Step 2: Update rollout TODO statuses without overclaiming.**

In docs/windows-macos-public-site-rollout-todo.md, mark catalog/static-site/package-script/template/automation implementation items as locally complete when their existing scripts and fresh smoke prove them. Label PKG-002/PKG-003 and the release gate as “script/package path complete; real Windows/macOS host verification pending” where appropriate. Keep browser installation-page acceptance, .43 smoke, real host background execution, provenance publication, and formal release gates open.

- [ ] **Step 3: Add the final closeout evidence line.**

Record the final local commit(s), test command families, temporary-path cleanup result, and remaining external gates. Use point-in-time wording and include the current CCBG release identity without changing Cargo.toml or release fingerprints.

- [ ] **Step 4: Verify and commit the checklist update.**

Run:

~~~
git diff --check
rg -n "OPS-006|pending|人工|真机|签名|公证|外部|credential|provenance" docs/todo.md docs/release-checklist.md docs/windows-macos-public-site-rollout-todo.md
~~~

Then commit:

~~~
git add docs/release-checklist.md docs/windows-macos-public-site-rollout-todo.md docs/todo.md docs/lessons-learned.md
git commit -m "docs: record CCBG local closeout evidence"
~~~

### Task 6: Final workspace audit and handoff

**Files:**
- Read: all files changed by Tasks 2–5
- Modify: none unless the audit finds a direct inconsistency introduced by this work

**Interfaces:**
- Consumes: committed documentation, optional tested script fix, and cleaned temporary directories.
- Produces: final auditable status report; no remote side effects.

- [ ] **Step 1: Run final verification.**

Run:

~~~
git status --short --branch
git log --oneline --decorate -8
git diff --check origin/main...HEAD
if (Test-Path '.codegraph') { 'CODEGRAPH_PRESENT' }
Get-ChildItem -Force | Select-Object Name,Mode,Length
~~~

Expected: only intended local commits are ahead of origin/main, the working tree is clean, .codegraph is present, and no CCBG temporary closeout directory remains.

- [ ] **Step 2: Confirm no remote or unrelated project mutation.**

Verify no git push, GitHub upload, Cloudflare deployment, operator credential write, or path outside D:\workspaces\ccbg was performed by this plan. Report any pre-existing unrelated state instead of deleting it.

- [ ] **Step 3: Produce the final report.**

Report: local commits, fresh checks and test counts, package/provenance smoke result, exact temporary paths removed, .codegraph preserved, working-tree state, and remaining external gates (OPS-006, .49 reauthentication where needed, .43 browser/manual acceptance, Windows/macOS real-host smoke, signing/notarization, and formal publication).

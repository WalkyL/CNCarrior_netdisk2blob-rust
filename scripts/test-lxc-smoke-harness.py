#!/usr/bin/env python3
"""Regression test for the LXC smoke harness exit semantics (finding #14).

Copies deploy/lxc/_smoke_test.sh into a temporary directory alongside a
deliberately failing smoke.sh, invokes Bash, and asserts that a FAIL line
produces a nonzero exit status. The harness resolves SMOKE relative to its own
location (BASH_SOURCE), so the copied harness picks up the failing double. With
the buggy harness the script prints FAIL but always exits 0, so this assertion
fails until the harness accumulates failures and propagates a nonzero status.

The test uses only Python stdlib + tempfile and never edits the production
smoke.sh.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
HARNESS = REPO_ROOT / "deploy" / "lxc" / "_smoke_test.sh"


def make_failing_smoke(tmp: Path) -> Path:
    """A smoke.sh double that always fails, so every harness case prints FAIL
    and the harness must propagate a nonzero final status."""
    smoke = tmp / "smoke.sh"
    smoke.write_text(
        "#!/usr/bin/env bash\n"
        'echo "FAIL: injected failure"\n'
        "exit 1\n",
        encoding="utf-8",
    )
    smoke.chmod(0o755)
    return smoke


def make_success_smoke(tmp: Path) -> Path:
    """A deterministic success double that behaves like the real smoke.sh.

    Emulates the smoke.sh contract so every harness case matches its expected
    outcome: liveness (/healthz != 200) is a hard failure, and a degraded
    readiness (/readyz != 200) only fails in strict mode
    (CCBG_SMOKE_REQUIRE_READY=1). This lets the harness's healthy/degraded cases
    pass without substituting for the real matrix (which the harness runs
    against its own mock)."""
    smoke = tmp / "smoke.sh"
    smoke.write_text(
        "#!/usr/bin/env bash\n"
        'endpoint="${CCBG_SMOKE_ENDPOINT:-http://127.0.0.1:61080}"\n'
        'metrics="${CCBG_SMOKE_METRICS_ENDPOINT:-${endpoint}}"\n'
        'require_ready="${CCBG_SMOKE_REQUIRE_READY:-0}"\n'
        'healthz="$(curl -sS -o /dev/null -w \'%{http_code}\' --max-time 5 "${endpoint}/healthz" 2>/dev/null || echo "000")"\n'
        'if [ "${healthz}" != "200" ]; then\n'
        '  echo "LIVENESS FAIL: /healthz ${healthz}" >&2\n'
        "  exit 1\n"
        "fi\n"
        'readyz="$(curl -sS -o /dev/null -w \'%{http_code}\' --max-time 5 "${metrics}/readyz" 2>/dev/null || echo "000")"\n'
        'if [ "${readyz}" != "200" ] && [ "${require_ready}" = "1" ]; then\n'
        '  echo "READINESS FAIL: /readyz ${readyz}" >&2\n'
        "  exit 1\n"
        "fi\n"
        'echo "PASS: healthy (readyz=${readyz}, require_ready=${require_ready})"\n'
        "exit 0\n",
        encoding="utf-8",
    )
    smoke.chmod(0o755)
    return smoke


def run_harness(tmp: Path) -> tuple[int, str]:
    harness = tmp / "_smoke_test.sh"
    shutil.copy(HARNESS, harness)
    harness.chmod(0o755)
    out = tmp / "out.txt"
    with out.open("w", encoding="utf-8") as handle:
        proc = subprocess.run(
            ["bash", str(harness)],
            cwd=str(tmp),
            stdout=handle,
            stderr=subprocess.STDOUT,
            text=True,
            env={**os.environ},
        )
    return proc.returncode, out.read_text(encoding="utf-8")


def main() -> int:
    if not HARNESS.exists():
        print(f"harness not found: {HARNESS}", file=sys.stderr)
        return 2

    # Scenario 1: a deliberately failing smoke.sh double must produce a nonzero
    # harness exit (finding #14). With the buggy harness FAIL is printed but the
    # exit status is 0, so this assertion fails until the harness propagates it.
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        make_failing_smoke(tmp)
        rc, output = run_harness(tmp)

    if "FAIL" not in output:
        print("harness did not emit a FAIL line; cannot validate exit semantics", file=sys.stderr)
        print(output, file=sys.stderr)
        return 2

    if rc == 0:
        print("BUG: harness printed FAIL but exited 0 (finding #14)", file=sys.stderr)
        print(output, file=sys.stderr)
        return 1

    print(f"OK: harness propagated FAIL as exit {rc}")

    # Scenario 2: a deterministic success double selected by endpoint/strict
    # flag must not trip a false failure in the harness. The harness runs the
    # success double for its healthy/degraded cases; any harness failure here
    # (nonzero exit) means it falsely failed a healthy double.
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        make_success_smoke(tmp)
        rc, output = run_harness(tmp)

    if "RESULT: PASS" not in output:
        print("harness did not report PASS for a healthy double", file=sys.stderr)
        print(output, file=sys.stderr)
        return 1
    if rc != 0:
        print(f"harness exited {rc} for a healthy double (false positive)", file=sys.stderr)
        print(output, file=sys.stderr)
        return 1

    print("OK: harness passed the deterministic success double")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

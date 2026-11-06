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
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Run local checks and independently validate reports from this test run."""
import json
import os
from pathlib import Path
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]


def verify(root=ROOT):
    artifacts = root / "artifacts"
    artifacts.mkdir(exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix="verify-", dir=artifacts))
    environment = os.environ.copy()
    environment.update(
        VIEW_STORAGE_TEST_REPORTS=str(directory / "api-reports"),
        VIEW_STORAGE_HTTP_REPORTS=str(directory / "http-reports"),
    )
    commands = [
        ["make", "check"],
        ["uv", "run", "--with", "PyYAML==6.0.3", "python", "scripts/test-ci-workflows.py"],
        ["uv", "run", "scripts/validate-contract.py"],
        ["uv", "run", "scripts/validate-produced-reports.py", environment["VIEW_STORAGE_TEST_REPORTS"]],
        ["uv", "run", "scripts/validate-produced-reports.py", "--http", environment["VIEW_STORAGE_HTTP_REPORTS"]],
    ]
    receipt = {"reports": str(directory), "checks": [], "passed": False}
    try:
        for command in commands:
            print("Running: " + " ".join(command), flush=True)
            result = subprocess.run(command, cwd=root, env=environment, check=False)
            receipt["checks"].append({"command": command, "exit_code": result.returncode})
            if result.returncode:
                return result.returncode
        receipt["passed"] = True
        return 0
    finally:
        (directory / "verification.json").write_text(json.dumps(receipt, indent=2) + "\n")
        print(f"Verification evidence: {directory}", flush=True)


if __name__ == "__main__":
    raise SystemExit(verify())

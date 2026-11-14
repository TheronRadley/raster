#!/usr/bin/env python3
"""Exercise the built C# diagnostics companion without external test packages."""
import json
import subprocess
import sys
import tempfile
from pathlib import Path


def main():
    if len(sys.argv) != 3:
        raise SystemExit("usage: test_diagnostics.py <diagnostics.dll> <real-health.json>")
    dll, health_path = sys.argv[1:]
    command = ["dotnet", str(Path(dll).resolve())]
    report = json.loads(Path(health_path).read_text(encoding="utf-8"))
    secret = "private-path-do-not-print"
    report["config_path"] = secret

    def check(data, expected, *, stdin=False):
        content = json.dumps(data).encode("utf-8") if not isinstance(data, bytes) else data
        if stdin:
            result = subprocess.run(command + ["-"], input=content, capture_output=True)
        else:
            path.write_bytes(content)
            result = subprocess.run(command + [str(path)], capture_output=True)
        assert result.returncode == expected, (result.returncode, result.stdout, result.stderr)
        assert secret.encode() not in result.stdout + result.stderr
        return result

    with tempfile.TemporaryDirectory(prefix="raster diagnostics ") as temp:
        path = Path(temp) / "health report.json"
        assert b"Raster health: OK" in check(report, 0).stdout
        check(report, 0, stdin=True)
        check({**report, "config_valid": False}, 1)
        check({**report, "status": "error"}, 1)
        check({**report, "stdout_tty": "false"}, 2)
        check({}, 2)
        check(b"not json", 2)
        check(b" " * (1024 * 1024 + 1), 2, stdin=True)
        result = subprocess.run(command + [str(path.parent / "missing.json")], capture_output=True)
        assert result.returncode == 2 and not result.stdout
        result = subprocess.run(command, capture_output=True)
        assert result.returncode == 2
    print("C# diagnostics checks passed")


if __name__ == "__main__":
    main()

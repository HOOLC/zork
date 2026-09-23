"""Run the unchanged native frame gate on a separate macOS test host."""
import hashlib
import json
from pathlib import Path
import re
import shlex
import subprocess


HOST = re.compile(r"[A-Za-z0-9._@-]+\Z")
REMOTE_ROOT = re.compile(r"/tmp/zork-frame\.[A-Za-z0-9]+\Z")


def command(host, *args, capture=False):
    result = subprocess.run(["ssh", "-o", "BatchMode=yes", host, shlex.join(args)],
                            check=True, text=True, capture_output=capture)
    return result.stdout.strip() if capture else None


def sha256(path):
    value = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def run(host, binary, output, root):
    if not host or host.startswith("-") or not HOST.fullmatch(host):
        raise RuntimeError("Frame host must be an SSH host or user@host without shell syntax")
    remote = command(host, "mktemp", "-d", "/tmp/zork-frame.XXXXXXXX", capture=True)
    if not REMOTE_ROOT.fullmatch(remote):
        raise RuntimeError("Remote frame fixture path is invalid")
    try:
        command(host, "mkdir", "-p", f"{remote}/scripts/lib", f"{remote}/bin", f"{remote}/output")
        files = ((root / "scripts/smoke-critical.py", f"{remote}/scripts/smoke-critical.py"),
                 (root / "scripts/lib/smoke_mesh_fixture.py", f"{remote}/scripts/lib/smoke_mesh_fixture.py"),
                 (root / "scripts/lib/smoke_remote_frame.py", f"{remote}/scripts/lib/smoke_remote_frame.py"),
                 (root / "scripts/test-client-frame-budget.py", f"{remote}/scripts/test-client-frame-budget.py"),
                 (binary, f"{remote}/bin/zork-gui-render-bench"))
        for source, destination in files:
            subprocess.run(["scp", "-q", str(source), f"{host}:{destination}"], check=True)
        expected = sha256(binary)
        actual = command(host, "shasum", "-a", "256", f"{remote}/bin/zork-gui-render-bench",
                         capture=True).split()[0]
        if actual != expected:
            raise RuntimeError("Frame binary changed during transfer to the test host")
        with (output / "runner.log").open("w") as log:
            process = subprocess.run([
                "ssh", "-o", "BatchMode=yes", host,
                shlex.join(["/usr/bin/python3", f"{remote}/scripts/test-client-frame-budget.py",
                             "--binary", f"{remote}/bin/zork-gui-render-bench",
                             "--output", f"{remote}/output"]),
            ], stdout=log, stderr=subprocess.STDOUT, timeout=240)
        subprocess.run(["scp", "-q", "-r", f"{host}:{remote}/output/.", str(output)], check=True)
        result = output / "result.json"
        if not result.is_file():
            raise RuntimeError(f"Remote frame runner exited {process.returncode} without evidence")
        report = json.loads(result.read_text())
        if report.get("binarySha256") is not None and report["binarySha256"] != expected:
            raise RuntimeError("Remote frame report does not match the supplied binary")
        if process.returncode != 0 and report.get("passed"):
            raise RuntimeError("Remote frame exit code disagrees with its report")
        report["executionHost"] = host
        report["rawEvidence"] = str(output / "native-frames.json")
        result.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        return report
    finally:
        try:
            command(host, "/usr/bin/python3", "-c",
                    f"import shutil; shutil.rmtree({remote!r})")
        except (OSError, subprocess.CalledProcessError) as error:
            (output / "cleanup-error.txt").write_text(str(error) + "\n")

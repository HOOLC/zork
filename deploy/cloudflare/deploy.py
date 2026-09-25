#!/usr/bin/env python3
"""Prepare an immutable local deployment candidate, check private inputs, or deploy it."""
import argparse
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import tempfile
import time
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parent
PRIVATE = Path.home() / "zork-deploy"

def read_config(path):
    script = """import fs from 'node:fs'; import {parse} from 'jsonc-parser';
const errors=[]; const value=parse(fs.readFileSync(process.argv[1], 'utf8'),errors,{allowTrailingComma:true});
if(errors.length) process.exit(1); process.stdout.write(JSON.stringify(value));"""
    result = subprocess.run(["node", "--input-type=module", "-e", script, str(path.resolve())],
                            cwd=ROOT, capture_output=True, text=True)
    if result.returncode:
        raise ValueError("Invalid JSONC configuration: " + str(path))
    return json.loads(result.stdout)

def private_json(path):
    if path.is_symlink() or (path.stat().st_mode & 0o077):
        raise ValueError(f"Private input must be a regular file with mode 600: {path}")
    return json.loads(path.read_text())

def write_private(path, data):
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    descriptor = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    with os.fdopen(descriptor, "w") as output:
        json.dump(data, output)
        output.flush()
        os.fsync(output.fileno())

def wrangler_authenticated():
    try:
        result = subprocess.run(["pnpm", "exec", "wrangler", "whoami", "--json"],
                                cwd=ROOT, capture_output=True, timeout=30)
        return result.returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        return False

def configuration(args):
    config = read_config(ROOT / "wrangler.jsonc")
    if args.config and args.config.exists():
        local = read_config(args.config)
        for key in ("name", "account_id", "routes", "workers_dev"):
            if key in local:
                config[key] = local[key]
        for key in ("PUBLIC_ORIGIN", "GOOGLE_CLIENT_ID"):
            if key in local.get("vars", {}):
                config["vars"][key] = local["vars"][key]
    origin = config["vars"]["PUBLIC_ORIGIN"]
    parsed = urlparse(origin)
    if parsed.scheme != "https" or not parsed.netloc or parsed.username or parsed.password or parsed.path or parsed.query or parsed.fragment:
        raise ValueError("PUBLIC_ORIGIN must be one canonical HTTPS origin")
    missing = []
    if not config.get("account_id"):
        missing.append("Cloudflare account_id in the private deployment config")
    routes = config.get("routes", [])
    if not any(r.get("custom_domain") is True and r.get("pattern") == parsed.hostname for r in routes):
        missing.append("Cloudflare custom-domain route matching PUBLIC_ORIGIN")
    values = {}
    callback = origin + "/v1/auth/google/callback"
    if args.google_client.exists():
        web = private_json(args.google_client).get("web", {})
        if not web.get("client_id") or not web.get("client_secret"):
            missing.append("Google Web OAuth client_id and client_secret")
        else:
            config["vars"]["GOOGLE_CLIENT_ID"] = web["client_id"]
            values["GOOGLE_CLIENT_SECRET"] = web["client_secret"]
        if callback not in web.get("redirect_uris", []):
            missing.append("Google authorized redirect URI: " + callback)
    elif config["vars"].get("GOOGLE_CLIENT_ID"):
        missing.append("Google Web OAuth JSON at " + str(args.google_client))
    if not (args.token_file.exists() or os.environ.get("CLOUDFLARE_API_TOKEN") or wrangler_authenticated()):
        missing.append("Cloudflare API token file/environment, or an interactive Wrangler login")
    return config, values, missing

@contextmanager
def wrangler_environment(token_file=None):
    env = os.environ.copy()
    if token_file and token_file.exists():
        if token_file.is_symlink() or token_file.stat().st_mode & 0o077:
            raise ValueError("Cloudflare token file must have mode 600")
        env["CLOUDFLARE_API_TOKEN"] = token_file.read_text().strip()
    yield env

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def prepare(args, config, missing):
    output = args.output.resolve()
    if output.exists():
        raise ValueError("Choose a new output directory; an existing candidate is immutable")
    output.mkdir(parents=True)
    shutil.copy2(ROOT / "README.md", output / "README.md")
    build_config = dict(config)
    build_config["main"] = str(ROOT / "src/index.ts")
    # Public inputs only. Neither Google secret nor signing/admin/CF credentials
    # are part of the candidate, its config, or the build command's arguments.
    private_build_config = ROOT / ".wrangler/relay-candidate.json"
    private_build_config.parent.mkdir(exist_ok=True)
    private_build_config.write_text(json.dumps(build_config))
    try:
        with wrangler_environment() as env:
            with (output / "build.log").open("w") as log:
                subprocess.run(["pnpm", "exec", "wrangler", "deploy", "--dry-run", "--config", str(private_build_config),
                                "--outdir", str(output / "worker")], cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
        worker = output / "worker/index.js"
        if not worker.exists() or "/__test/" in worker.read_text():
            raise ValueError("Production bundle is missing or contains fixture routes")
        deployed = dict(config)
        deployed.pop("$schema", None)
        deployed["main"] = "worker/index.js"
        deployed["no_bundle"] = True
        (output / "wrangler.json").write_text(json.dumps(deployed, indent=2) + "\n")
        repository = ROOT.parents[1]
        tracked = subprocess.check_output(["git", "diff", "--name-only", "-z", "HEAD"], cwd=repository).decode().split("\0")
        untracked = subprocess.check_output(["git", "ls-files", "--others", "--exclude-standard", "-z"], cwd=repository).decode().split("\0")
        changes = {name: digest(repository / name) if (repository / name).is_file() else None
                   for name in sorted(set(tracked + untracked)) if name}
        manifest = {"source_base": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                    "source_changes": changes,
                    "deployed": False,
                    "configuration_gaps": missing,
                    "files": {str(p.relative_to(output)): digest(p) for p in sorted(output.rglob("*")) if p.is_file() and p.name != "build.log"}}
        (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        print("Prepared candidate:", output)
    finally:
        private_build_config.unlink(missing_ok=True)

def deploy(args, values, missing):
    candidate = args.output.resolve()
    manifest = json.loads((candidate / "manifest.json").read_text())
    for name, expected in manifest["files"].items():
        if digest(candidate / name) != expected:
            raise ValueError("Candidate file changed: " + name)
    config = json.loads((candidate / "wrangler.json").read_text())
    current, values, missing = configuration(args)
    for key in ("name", "account_id", "routes", "vars"):
        if config.get(key) != current.get(key):
            raise ValueError("Private/public configuration changed; prepare and validate a new candidate")
    required = [item for item in missing if not item.startswith("Cloudflare API token")]
    if required:
        raise ValueError("Configuration is incomplete; run check")
    keys_path = args.session_keys
    if not keys_path.exists():
        write_private(keys_path, {"AUTH_SIGNING_KEY": secrets.token_urlsafe(32), "ADMIN_TOKEN": secrets.token_urlsafe(32)})
    keys = private_json(keys_path)
    for name in ("AUTH_SIGNING_KEY", "ADMIN_TOKEN"):
        if not isinstance(keys.get(name), str) or len(keys[name]) < 43:
            raise ValueError("Invalid private session key material")
        values[name] = keys[name]
    with wrangler_environment(args.token_file) as env:
        command = ["pnpm", "exec", "wrangler"]
        # Snapshot metadata for an operator-directed rollback. Do not log secrets.
        previous = subprocess.run(command + ["deployments", "list", "--config", str(candidate / "wrangler.json"), "--json"],
                                  cwd=ROOT, env=env, capture_output=True, text=True)
        (candidate / "previous-deployments.json").write_text(previous.stdout)
        with tempfile.TemporaryDirectory(prefix="zork-worker-secrets-") as directory:
            secret_file = Path(directory) / "secrets.json"
            write_private(secret_file, values)
            subprocess.run(command + ["secret", "bulk", str(secret_file), "--config", str(candidate / "wrangler.json")],
                           cwd=ROOT, env=env, check=True)
        subprocess.run(command + ["deploy", "--config", str(candidate / "wrangler.json")],
                       cwd=ROOT, env=env, check=True)
    if args.restart_relay:
        # Credentials enter curl over stdin, never in its command line or logs.
        # The just-activated Worker can take a short time to reach this edge.
        confirmed = False
        for attempt in range(6):
            result = subprocess.run(["curl", "--silent", "--show-error", "--fail", "--max-time", "10",
                                     "--request", "POST", "--config", "-",
                                     config["vars"]["PUBLIC_ORIGIN"] + "/v1/admin/relay/restart"],
                                    input='header = "Authorization: Bearer ' + values["ADMIN_TOKEN"] + '"\n',
                                    capture_output=True, text=True)
            if result.returncode == 0:
                try:
                    confirmed = json.loads(result.stdout).get("restarted") is True
                except (ValueError, AttributeError):
                    pass
            if confirmed:
                break
            if attempt < 5:
                time.sleep(2)
        if not confirmed:
            raise ValueError("Worker deployed, but the relay restart is not confirmed; retry the admin restart")
        print("Relay connections were told to reconnect.")
    print("Worker deployed. Native relay acceptance is required; verify Google login separately if configured.")

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("check", "prepare", "deploy"))
    parser.add_argument("--config", type=Path, default=ROOT / "wrangler.local.json")
    parser.add_argument("--google-client", type=Path, default=PRIVATE / "google-oauth.json")
    parser.add_argument("--token-file", type=Path, default=PRIVATE / "cloudflare-api-token")
    parser.add_argument("--session-keys", type=Path, default=PRIVATE / "session-keys.json")
    parser.add_argument("--output", type=Path, default=ROOT.parents[1] / "artifacts/cloudflare-candidate")
    parser.add_argument("--restart-relay", action="store_true", help="After deployment, send Restarting to every relay connection and close it")
    args = parser.parse_args()
    try:
        config, values, missing = configuration(args)
        if args.command == "check":
            print(json.dumps({"ready": not missing, "missing": missing, "callback": config["vars"]["PUBLIC_ORIGIN"] + "/v1/auth/google/callback"}, indent=2))
            return int(bool(missing))
        if args.command == "prepare":
            prepare(args, config, missing)
        else:
            deploy(args, values, missing)
        return 0
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        # CalledProcessError contains only our credential-free command arguments.
        print(str(error))
        return 1

if __name__ == "__main__":
    raise SystemExit(main())

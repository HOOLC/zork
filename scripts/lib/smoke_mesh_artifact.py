"""Reuse Linux smoke binaries by their backend inputs, independently of UI commits."""
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile

from deployment import atomic_json, digest, exclusive

BINARIES = ("zork-station", "zork-agent")
PACKAGES = ("zork-station", "zork-agent-server")
DEFAULT_IMAGE = "rust:1-bookworm"


def git(repo, *args):
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    return subprocess.check_output(["git", *args], cwd=repo, env=env)


def source_inputs(repo, metadata=None):
    repo = repo.resolve()
    metadata = metadata or json.loads(subprocess.check_output([
        "cargo", "metadata", "--locked", "--offline", "--format-version=1",
        "--filter-platform", "aarch64-unknown-linux-gnu"], cwd=repo))
    packages = {p["id"]: p for p in metadata["packages"]}
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    pending = [p["id"] for p in packages.values() if p["name"] in PACKAGES]
    if {packages[item]["name"] for item in pending} != set(PACKAGES):
        raise RuntimeError("Station and Agent packages are missing from Cargo metadata")
    seen = set()
    while pending:
        item = pending.pop()
        if item in seen:
            continue
        seen.add(item)
        pending.extend(d["pkg"] for d in nodes[item]["deps"]
                       if any(k["kind"] != "dev" for k in d["dep_kinds"]))
    local = [packages[item] for item in seen if packages[item]["source"] is None]
    roots = sorted({str(Path(p["manifest_path"]).resolve().parent.relative_to(repo)) for p in local})
    files = {"Cargo.toml", "Cargo.lock"}
    for name in ("rust-toolchain", "rust-toolchain.toml"):
        if (repo / name).exists():
            files.add(name)
    paths = git(repo, "ls-files", "--cached", "--others", "--exclude-standard", "-z")
    for raw in paths.split(b"\0"):
        name = os.fsdecode(raw)
        if name and any(name.startswith(root + "/") for root in roots):
            if (repo / name).is_file():
                files.add(name)
    records = {}
    for name in sorted(files):
        path = repo / name
        if not path.resolve().is_relative_to(repo):
            raise RuntimeError(f"Build input escapes checkout: {name}")
        records[name] = digest(path)
    fingerprint = hashlib.sha256(json.dumps(records, sort_keys=True).encode()).hexdigest()
    return {"sha256": fingerprint, "files": records, "roots": roots,
            "packages": sorted(f'{p["name"]}@{p["version"]}' for p in local)}


def image_info(reference):
    image = json.loads(subprocess.check_output(["docker", "image", "inspect", reference]))[0]
    if (image["Os"], image["Architecture"]) != ("linux", "arm64"):
        raise RuntimeError("Smoke runtime must be a local Linux arm64 image")
    return image


def binary_digests(directory):
    result = {}
    for name in BINARIES:
        path = directory / name
        with path.open("rb") as source:
            header = source.read(20)
        if (header[:6] != b"\x7fELF\x02\x01" or header[18:20] != b"\xb7\x00"
                or not os.access(path, os.X_OK)):
            raise RuntimeError(f"Expected executable Linux arm64 ELF: {path}")
        result[name] = digest(path)
    return result


def validate(repo, directory, inputs=None):
    record = json.loads((directory / "build.json").read_text())
    if record.get("version") != 1 or record.get("profile") != "release":
        raise RuntimeError("Unsupported Linux smoke build record")
    inputs = inputs or source_inputs(repo)
    if record.get("source_sha256") != inputs["sha256"]:
        raise RuntimeError("Linux smoke backend inputs changed; run scripts/build/critical-mesh.py")
    if record.get("binaries") != binary_digests(directory):
        raise RuntimeError("Linux smoke binary changed after capture")
    return record


def lock_path(directory):
    return directory.with_name(directory.name + ".lock")


def prepare(repo, directory, runtime=DEFAULT_IMAGE, from_image=None):
    repo, directory = repo.resolve(), directory.resolve()
    directory.parent.mkdir(parents=True, exist_ok=True)
    with exclusive(lock_path(directory)):
        inputs = source_inputs(repo)
        image = image_info(runtime)  # Inspect only: never pull or rebuild an image implicitly.
        try:
            previous = validate(repo, directory, inputs)
            if previous["runtime_image"] == image["Id"]:
                print(f"REUSED Linux Station and Agent: {directory}", flush=True)
                return previous
        except (OSError, ValueError, RuntimeError, KeyError):
            pass
        with tempfile.TemporaryDirectory(prefix=".mesh-", dir=directory.parent) as scratch:
            stage = Path(scratch)
            if from_image:
                imported = image_info(from_image)
                revision = git(repo, "rev-parse", "HEAD").decode().strip()
                if (imported["Config"].get("Labels") or {}).get("org.opencontainers.image.revision") != revision:
                    raise RuntimeError("Imported Station image must match checkout HEAD")
                changed = git(repo, "diff", "HEAD", "--name-only", "--",
                              "Cargo.toml", "Cargo.lock", *inputs["roots"])
                untracked = git(repo, "ls-files", "--others", "--exclude-standard", "--",
                                *inputs["roots"])
                if changed.strip() or untracked.strip():
                    raise RuntimeError("Cannot import image binaries with modified backend inputs")
                container = subprocess.check_output(["docker", "create", imported["Id"]], text=True).strip()
                try:
                    for name in BINARIES:
                        subprocess.run(["docker", "cp", f"{container}:/usr/local/bin/{name}",
                                        str(stage / name)], check=True)
                finally:
                    subprocess.run(["docker", "rm", container], check=True, stdout=subprocess.DEVNULL)
                provenance = {"imported_image": imported["Id"], "revision": revision}
            else:
                # One reusable cache per checkout; no cross-worktree mtimes or build artifacts.
                volume = "zork-critical-" + hashlib.sha256(os.fsencode(repo)).hexdigest()[:16]
                subprocess.run(["docker", "volume", "create", "--label", "ing.zork.purpose=critical-smoke",
                                "--label", f"ing.zork.checkout={repo}", volume], check=True,
                               stdout=subprocess.DEVNULL)
                clean = ["cargo", "clean", "--release", "--manifest-path", "/src/Cargo.toml"]
                for package in inputs["packages"]:
                    clean += ["-p", package]
                build = ["cargo", "build", "--locked", "--release", "--manifest-path", "/src/Cargo.toml"]
                for package in PACKAGES:
                    build += ["-p", package]
                script = shlex.join(clean) + " && " + shlex.join(build)
                script += " && cp /cache/target/release/zork-station /cache/target/release/zork-agent /out/"
                print("Backend inputs changed; compiling binaries in the existing image", flush=True)
                command = [
                    "docker", "run", "--rm", "--workdir", "/", "--entrypoint", "sh",
                    "--mount", f"type=bind,source={repo},target=/src,readonly",
                    "--mount", f"type=volume,source={volume},target=/cache",
                    "--mount", f"type=bind,source={stage},target=/out",
                    "-e", "CARGO_HOME=/cache/cargo", "-e", "CARGO_TARGET_DIR=/cache/target",
                    "-e", "CARGO_BUILD_JOBS=4", "-e", "CARGO_INCREMENTAL=0",
                    image["Id"], "-ec", script]
                with exclusive(repo / ".tmp/critical-mesh-build.lock"):
                    subprocess.run(command, check=True)
                provenance = {"compiler_image": image["Id"], "cache_volume": volume}
            if source_inputs(repo)["sha256"] != inputs["sha256"]:
                raise RuntimeError("Backend inputs changed during binary capture")
            record = {"version": 1, "profile": "release", "runtime_image": image["Id"],
                      "source_sha256": inputs["sha256"], "binaries": binary_digests(stage),
                      **provenance}
            directory.mkdir(exist_ok=True)
            for name in BINARIES:
                shutil.move(stage / name, directory / name)
            atomic_json(directory / "build.json", record)
        print(f"READY Linux Station and Agent: {directory}", flush=True)
        return record

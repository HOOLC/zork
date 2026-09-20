#!/usr/bin/env python3
"""Read-only Synch CAS audit. No source retirement, pin removal or GC is performed.

The estimate follows the pinned synch-store content-GC predicate. It is a
reviewable snapshot, never a deletion manifest: the engine must recheck every
reference, pin, age and active writer when actually collecting an object.
"""
import argparse
from collections import Counter, defaultdict
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import sqlite3
import stat
import time

# NodeConfig::default in the pinned vendor/synch-engine; not an override.
RETENTION_NS = 7 * 24 * 3600 * 1_000_000_000
RETIRED = {"files", "zork"}


def regular(path):
    try:
        info = path.lstat()
        return info if stat.S_ISREG(info.st_mode) else None
    except FileNotFoundError:
        return None


def iso(nanos):
    return datetime.fromtimestamp(nanos / 1e9, timezone.utc).isoformat()


def prefix_of(candidate, source):
    """Compare bytes, with stable file identities; never emit history content."""
    before = (regular(candidate), regular(source))
    if None in before or before[0].st_size > before[1].st_size:
        return False
    with candidate.open("rb") as old, source.open("rb") as current:
        while chunk := old.read(1024 * 1024):
            if chunk != current.read(len(chunk)):
                return False
    def identity(info):
        return None if info is None else (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns)
    return tuple(map(identity, before)) == tuple(map(identity, (regular(candidate), regular(source))))


def history_prefixes(data, objects):
    sources = defaultdict(list)
    # Only unchanged, real Session log segments. Compressed/unknown formats are
    # not inferred and an absent source never makes a CAS root disposable.
    for path in (data / "shared-files/sessions").glob("*/segments/*.jsonl"):
        info = regular(path)
        if info and info.st_size >= 4096:
            with path.open("rb") as stream:
                sources[hashlib.sha256(stream.read(4096)).digest()].append(path)
    groups = defaultdict(lambda: {"objects": 0, "logical_bytes": 0, "eligible_now": 0})
    for obj in objects:
        if not obj["complete"] or obj["size"] < 4096:
            continue
        path = data / "mesh/synch/store" / obj["root"][:2] / obj["root"]
        if path.parent.is_symlink() or not regular(path):
            continue
        with path.open("rb") as stream:
            matches = sources.get(hashlib.sha256(stream.read(4096)).digest(), [])
        for source in matches:
            if prefix_of(path, source):
                relative = str(source.relative_to(data))
                obj["history_prefix_of"] = relative
                group = groups[relative]
                group["source_bytes"] = source.stat().st_size
                group["objects"] += 1
                group["logical_bytes"] += obj["size"]
                group["eligible_now"] += obj["status"] == "eligible"
                break
    return dict(sorted(groups.items()))


def audit(data, now=None, compare_history=False):
    data = Path(data).resolve(strict=True)
    mesh = data / "mesh/synch"
    database = mesh / "synchronicity.db"
    captured = time.time_ns()
    simulated_time = now is not None
    now = captured if now is None else now
    before = now - RETENTION_NS
    # SQLite backup gives a consistent in-memory read snapshot, including WAL.
    # Do not use immutable=1: it would silently ignore a live WAL.
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as original:
        original.execute("PRAGMA query_only=ON")
        db = sqlite3.connect(":memory:")
        original.backup(db)
    db.row_factory = sqlite3.Row
    try:
        sources = [dict(row) for row in db.execute("SELECT space,kind,local_path FROM sources ORDER BY space")]
        retiring = {source["space"] for source in sources} & RETIRED
        own = db.execute("SELECT value FROM config WHERE key='self_origin_id'").fetchone()
        origin = own[0] if own else None
        if isinstance(origin, bytes):
            origin = origin.decode("utf-8")
        refs, pins = defaultdict(list), defaultdict(list)
        for row in db.execute("SELECT lower(hex(content)) AS root,origin_id,space,path FROM entries WHERE content IS NOT NULL"):
            refs[row["root"]].append({k: row[k] for k in ("origin_id", "space", "path")})
        for row in db.execute("SELECT lower(hex(root)) AS root,holder,release_after FROM pins"):
            pins[row["root"]].append({"holder": row["holder"], "release_after": row["release_after"]})
        objects = []
        for row in db.execute("SELECT lower(hex(root)) AS root,size,complete,inline IS NOT NULL AS inline,last_access FROM blobs ORDER BY root"):
            obj = dict(row)
            obj["references"], obj["pins"] = refs[obj["root"]], pins[obj["root"]]
            obj["eligible_after"] = iso(obj["last_access"] + RETENTION_NS + 1)
            obj["status"] = ("referenced" if obj["references"] else "pinned" if obj["pins"]
                             else "retained_by_age" if obj["last_access"] >= before else "eligible")
            # Forecast only the local startup retirement, retaining all remote
            # entries and all unrelated/operator/replica pins. This is not SQL
            # to apply: signed publication must precede ending the source role.
            remaining = [r for r in obj["references"] if r["origin_id"] != origin or r["space"] not in retiring]
            remaining_pins = [p for p in obj["pins"] if p["holder"] not in {"source:" + s for s in retiring}
                              or any(r["space"] == p["holder"][7:] for r in remaining)]
            obj["after_local_retirement"] = ("referenced" if remaining else "pinned" if remaining_pins
                                             else "retained_by_age" if obj["last_access"] >= before else "eligible")
            objects.append(obj)
        roots = {obj["root"] for obj in objects}
        cas = []
        for path in sorted((mesh / "store").glob("*/*")):
            info = regular(path)
            if path.parent.is_symlink() or not info or not re.fullmatch(r"[0-9a-f]{64}(\.obao)?", path.name):
                continue
            root = path.name.removesuffix(".obao")
            cas.append({"path": str(path.relative_to(mesh)), "root": root, "bytes": info.st_size,
                        "allocated_bytes": info.st_blocks * 512,
                        "orphan": root not in roots, "old_enough": info.st_mtime_ns < before})
        def totals(field):
            result = {}
            for status in ("referenced", "pinned", "retained_by_age", "eligible"):
                rows = [obj for obj in objects if obj[field] == status]
                selected = {obj["root"] for obj in rows}
                result[status] = {"objects": len(rows), "logical_bytes": sum(obj["size"] for obj in rows),
                                  "cas_file_bytes": sum(f["bytes"] for f in cas if f["root"] in selected)}
            return result
        groups = history_prefixes(data, objects) if compare_history else None
        return {"schema": 1, "dry_run": True, "apply_supported": False, "data": str(data),
                "captured_at": iso(captured), "evaluated_at": iso(now), "simulated_time": simulated_time, "retention_seconds": RETENTION_NS // 1_000_000_000,
                "gc_before": iso(before), "self_origin": origin, "sources": sources,
                "business_source_registered": any(s["space"] in RETIRED for s in sources),
                "current": totals("status"), "after_local_retirement": totals("after_local_retirement"),
                "pins_by_holder": dict(Counter(p["holder"] for group in pins.values() for p in group)),
                "replicas": [dict(row) for row in db.execute("SELECT * FROM replicas")],
                "retained_trie_roots": db.execute("SELECT count(DISTINCT root) FROM head_history").fetchone()[0],
                "history_prefix_groups": groups,
                "orphan_files": {"files": sum(f["orphan"] for f in cas),
                                 "old_enough": sum(f["orphan"] and f["old_enough"] for f in cas)},
                "cas_files": cas, "objects": objects,
                "limits": ["A live filesystem is not an atomic snapshot; rerun before any maintenance.",
                           "Eligible means the stored predicate matches; active writers are checked only by the engine.",
                           "Scheduled pins are retained until the engine actually expires them.",
                           "CAS file lengths/allocated blocks do not measure uniquely reclaimable APFS extents.",
                           "Trie retention is separate from content retention; no execution logs are cleanup candidates."]}
    finally:
        db.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data", type=Path, required=True, help="Station root, or access client's transport root")
    parser.add_argument("--compare-history", action="store_true", help="prove CAS payloads are prefixes of preserved local Session logs")
    parser.add_argument("--at", help="evaluate ages at an explicit ISO-8601 instant (still read-only)")
    parser.add_argument("--output", type=Path, help="write a private report outside the node data directory")
    args = parser.parse_args()
    now = None
    if args.at:
        at = datetime.fromisoformat(args.at)
        if at.tzinfo is None:
            parser.error("--at requires an explicit timezone")
        now = int(at.timestamp() * 1e9)
    if args.output and args.output.resolve().is_relative_to(args.data.resolve()):
        parser.error("--output must be outside the node data directory")
    report = audit(args.data, now, args.compare_history)
    text = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        args.output.write_text(text)
        print(json.dumps({key: report[key] for key in ("dry_run", "data", "business_source_registered", "current", "after_local_retirement")}, ensure_ascii=False))
    else:
        print(text, end="")


if __name__ == "__main__":
    main()

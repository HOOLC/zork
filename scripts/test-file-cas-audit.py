#!/usr/bin/env python3
"""Exercise audit retention/role forecasts without permitting data mutation."""
import hashlib
import importlib.util
from pathlib import Path
import sqlite3
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("audit", Path(__file__).with_name("audit-file-cas.py"))
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


class AuditTest(unittest.TestCase):
    def test_current_remote_pins_and_age_are_all_independent_guards(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            mesh = root / "mesh/synch"
            mesh.mkdir(parents=True)
            database = mesh / "synchronicity.db"
            db = sqlite3.connect(database)
            db.executescript("""
                CREATE TABLE config(key TEXT,value TEXT);
                INSERT INTO config VALUES ('self_origin_id','self');
                CREATE TABLE sources(space TEXT,kind TEXT,local_path TEXT);
                INSERT INTO sources VALUES ('files','filesystem','unchanged-original');
                CREATE TABLE entries(origin_id TEXT,space TEXT,path TEXT,content BLOB);
                CREATE TABLE pins(root BLOB,holder TEXT,release_after INTEGER);
                CREATE TABLE blobs(root BLOB,size INTEGER,complete INTEGER,inline BLOB,last_access INTEGER);
                CREATE TABLE replicas(space TEXT,retention TEXT);
                CREATE TABLE head_history(root BLOB);
            """)
            now = audit.RETENTION_NS * 2
            for i in range(1, 7):
                db.execute("INSERT INTO blobs VALUES (?,?,1,NULL,?)", (bytes([i])*32, 5000, 1 if i != 2 else now))
            # Local retirement cannot erase the other origin's live reference.
            for i, origin in ((3, "self"), (4, "other")):
                db.execute("INSERT INTO entries VALUES (?,'files','history',?)", (origin, bytes([i])*32))
                db.execute("INSERT INTO pins VALUES (?,'source:files',NULL)", (bytes([i])*32,))
            db.execute("INSERT INTO pins VALUES (?,'operator',NULL)", (bytes([5])*32,))
            db.execute("INSERT INTO pins VALUES (?,'replica:archive',1)", (bytes([6])*32,))
            db.commit(); db.close()
            original = database.read_bytes()
            report = audit.audit(root, now=now)
            self.assertEqual(report["current"]["eligible"]["objects"], 1)
            self.assertEqual(report["current"]["retained_by_age"]["objects"], 1)
            self.assertEqual(report["after_local_retirement"]["eligible"]["objects"], 2)
            self.assertEqual(report["after_local_retirement"]["referenced"]["objects"], 1)
            self.assertEqual(report["after_local_retirement"]["pinned"]["objects"], 2)
            self.assertEqual(original, database.read_bytes())
            # A stale pin without a source role cannot be dropped by startup's
            # retirement routine; the audit must not forecast an extra unpin.
            db = sqlite3.connect(database)
            db.execute("DELETE FROM sources"); db.commit(); db.close()
            report = audit.audit(root, now=now)
            self.assertEqual(report["after_local_retirement"], report["current"])

    def test_prefix_evidence_compares_bytes_and_keeps_original_and_cas(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "shared-files/sessions/session/segments/history.jsonl"
            source.parent.mkdir(parents=True)
            data = b'{"event":"retained"}\n' * 400
            source.write_bytes(data)
            digest = "a" * 64
            payload = root / "mesh/synch/store" / digest[:2] / digest
            payload.parent.mkdir(parents=True)
            payload.write_bytes(data[:5000])
            obj = {"root": digest, "size": 5000, "complete": True, "status": "retained_by_age"}
            before = {p: hashlib.sha256(p.read_bytes()).hexdigest() for p in (source, payload)}
            groups = audit.history_prefixes(root, [obj])
            self.assertEqual(next(iter(groups.values()))["objects"], 1)
            self.assertEqual(next(iter(groups.values()))["eligible_now"], 0)
            self.assertEqual(before, {p: hashlib.sha256(p.read_bytes()).hexdigest() for p in (source, payload)})
            payload.write_bytes(data[:4999] + b"!")
            self.assertEqual(audit.history_prefixes(root, [obj]), {})


if __name__ == "__main__":
    unittest.main()

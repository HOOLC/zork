#!/usr/bin/env python3
"""Offline legacy bot migration rehearsal, never a production cutover command.

Input is a private snapshot from the stopped bot or the online snapshot helper.
All original files remain archived, including unsupported records. New sessions
are created through the native API, not by forging events. Pending input and
unknown old effects are quarantined for reconciliation, never replayed.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('support', ROOT / 'scripts/test-slack-forwarding.py')
support = importlib.util.module_from_spec(spec)
spec.loader.exec_module(support)
fixture, request = support.fixture, support.request


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def extract_session(directory):
    rows = [json.loads(line) for path in sorted(directory.rglob('*.jsonl'))
            for line in path.open() if line.strip()]
    if not rows:
        raise ValueError('No readable active segment; offline decompression required')
    # Source is an immutable rehearsal copy. A live capture may end mid-batch.
    while rows and rows[-1]['batch_index'] + 1 != rows[-1]['batch_size']:
        rows.pop()
    assert rows, 'No complete batch'
    snapshots = [i for i, row in enumerate(rows) if row.get('kind') == 'snapshot']
    start = snapshots[-1] if snapshots else -1
    state = rows[start]['state'] if start >= 0 else {}
    assert state or rows[0].get('event', {}).get('type') == 'session_created', 'Missing recovery baseline'
    mailbox = {m['mailbox_seq']: m for m in state.get('mailbox', [])}
    consumed = state.get('consumed_through_mailbox_seq', 0)
    transcript = list(state.get('transcript', []))
    active = bool(state.get('active_activation'))
    waiting = bool(state.get('active_wait'))
    calls = set(state.get('inflight_tool_call_ids', []))
    handoffs = [r['event']['handoff'] for r in rows if r.get('event', {}).get('type') == 'context_handoff_created']
    for row in rows[start + 1:]:
        assert row.get('event_schema_version') == 11, 'Unexpected old event schema'
        event = row['event']
        kind = event['type']
        if kind == 'mailbox_message_appended':
            message = event['message']
            mailbox[message['mailbox_seq']] = message
        elif kind == 'mailbox_drained':
            consumed = max(consumed, event['through_mailbox_seq'])
        elif kind == 'message_appended':
            message = event['message']
            transcript.append(message)
            for call in message.get('tool_calls') or []:
                calls.add(call['id'] if 'id' in call else call['tool_call_id'])
            if message.get('tool_call_id'):
                calls.discard(message['tool_call_id'])
        elif kind == 'activation_started':
            active = True
        elif kind == 'activation_finished':
            active = False
        elif kind == 'wait_set':
            waiting = True
        elif kind in ('wait_expired', 'wait_cleared', 'wait_cancelled'):
            waiting = False
    return {'old_session_id': directory.name, 'through_event_id': rows[-1]['event_id'],
            'through_stream_version': rows[-1].get('stream_version'),
            'consumed_through_mailbox_seq': consumed,
            'accepted_mailbox_max': max([consumed, *mailbox]),
            'pending': [m for seq, m in sorted(mailbox.items()) if seq > consumed],
            'active_activation': active, 'active_wait': waiting,
            'unresolved_tool_calls': sorted(calls), 'transcript': transcript,
            'handoffs': handoffs, 'latest_context_handoff': state.get('latest_context_handoff')}


def table_rows(db, table):
    return [dict(row) for row in db.execute('SELECT * FROM ' + table)]


def transfer_database(source, target, mapping, connect_id, node_root):
    """Only called while the isolated Station is stopped; transaction is atomic."""
    src = sqlite3.connect(f'file:{source}?mode=ro', uri=True)
    src.row_factory = sqlite3.Row
    dst = sqlite3.connect(target)
    dst.row_factory = sqlite3.Row
    dst.execute('PRAGMA foreign_keys=ON')
    def insert(table, row):
        columns = {r[1] for r in dst.execute('PRAGMA table_info(' + table + ')')}
        selected = {k: v for k, v in row.items() if k in columns}
        dst.execute('INSERT INTO ' + table + ' (' + ','.join(selected) + ') VALUES (' + ','.join('?' for _ in selected) + ')', list(selected.values()))
    def relocated(path):
        if not path.startswith('/data/') or '..' in Path(path).parts:
            raise ValueError('External legacy path needs explicit mapping')
        return str(node_root / path[len('/data/'):])
    counts = {}
    keymap = {item['old_key']: item['new_key'] for item in mapping}
    idmap = {item['old_session_id']: item for item in mapping}
    try:
        with dst:
            for table in ('sessions', 'proactive_bindings'):
                assert not dst.execute('SELECT count(*) FROM ' + table).fetchone()[0], 'Target is not empty'
                for row in table_rows(src, table):
                    item = idmap[row['id']]
                    row.update(key=item['new_key'], id=item['new_session_id'], connection_id=connect_id, platform='slack', workspace_path=item['workspace'])
                    insert(table, row)
                counts[table] = src.execute('SELECT count(*) FROM ' + table).fetchone()[0]
            for table in ('inbound_messages', 'proactive_inbound_messages'):
                for row in table_rows(src, table):
                    row['connection_id'] = connect_id
                    if table == 'inbound_messages':
                        row['session_key'] = keymap[row['session_key']]
                        row['key'] = row['session_key'] + ':' + row['message_ts']
                    else:
                        row['binding_key'] = keymap[row['binding_key']]
                        row['key'] = ':'.join((connect_id, row['channel_id'], row['message_ts']))
                    insert(table, row)
                counts[table] = src.execute('SELECT count(*) FROM ' + table).fetchone()[0]
            for row in table_rows(src, 'background_jobs'):
                # Never silently turn a live/unknown job into a failed job.
                assert row['status'] in ('failed', 'completed', 'cancelled'), 'Live job requires reconciliation'
                row['session_key'] = keymap[row['session_key']]
                row['cwd'], row['script_path'] = relocated(row['cwd']), relocated(row['script_path'])
                insert('background_jobs', row)
            counts['background_jobs'] = src.execute('SELECT count(*) FROM background_jobs').fetchone()[0]
            assert not list(dst.execute('PRAGMA foreign_key_check'))
            assert dst.execute('PRAGMA integrity_check').fetchone()[0] == 'ok'
        for table, count in counts.items():
            assert dst.execute('SELECT count(*) FROM ' + table).fetchone()[0] == count
        # Prove every retained non-transformed source field survived, not only counts.
        excluded = {'key', 'id', 'connection_id', 'platform', 'session_key', 'binding_key', 'workspace_path', 'cwd', 'script_path', 'channel_id', 'root_thread_ts'}
        for table in counts:
            common = sorted(({r[1] for r in src.execute('PRAGMA table_info(' + table + ')')} & {r[1] for r in dst.execute('PRAGMA table_info(' + table + ')')}) - excluded)
            canonical = lambda conn: sorted(json.dumps([r[c] for c in common], sort_keys=True) for r in table_rows(conn, table))
            assert canonical(src) == canonical(dst), 'Preserved field mismatch: ' + table
        return counts
    finally:
        src.close()
        dst.close()


def run(source, output, connect_id):
    os.umask(0o077)
    assert not output.exists(), 'Use a fresh rehearsal output directory'
    output.mkdir(parents=True, mode=0o700)
    os.environ['ZORK_REGISTRY_DIR'] = str(output / 'registry')
    capture = json.loads((source / 'capture.json').read_text())
    for item in capture['files']:
        assert digest(source / item['path']) == item['sha256'], 'Snapshot file changed'
    before_db = digest(source / 'gateway.sqlite')
    src = sqlite3.connect(f'file:{source}/gateway.sqlite?mode=ro', uri=True)
    src.row_factory = sqlite3.Row
    bindings = [(table, row) for table in ('sessions', 'proactive_bindings') for row in table_rows(src, table)]
    plans = {row['id']: extract_session(source / 'sessions' / row['id']) for _, row in bindings}
    node = fixture.Node(output / 'node')
    for directory in ('profiles', 'workspaces', 'jobs'):
        shutil.copytree(source / directory, node.root / directory, dirs_exist_ok=True)
    archive = node.root / 'legacy-archive'
    shutil.copytree(source / 'sessions', archive / 'sessions')
    shutil.copyfile(source / 'gateway.sqlite', archive / 'gateway.sqlite')
    (archive / 'plans').mkdir()
    for sid, plan in plans.items():
        (archive / 'plans' / (sid + '.json')).write_text(json.dumps(plan, ensure_ascii=False))
    # No real token or Socket Mode connection is loaded into the rehearsal Station.
    node.config['im_connections'] = [{'id': connect_id, 'provider': 'slack', 'name': 'Migration rehearsal', 'mode': 'proactive', 'enabled': False}]
    (node.root / 'config.json').write_text(json.dumps(node.config))
    mapping = []
    binary = fixture.TARGET / 'zork-station'
    def start():
        log = (output / 'station.log').open('ab')
        process = subprocess.Popen([str(binary), '--data', str(node.root), '--fake-agent'], stdout=log, stderr=log)
        fixture.wait(lambda: request(node.url, 'GET', '/readyz')[0] == 200, 'Station ready')
        return process, log
    def stop(process, log):
        process.terminate()
        try:
            assert process.wait(timeout=30) == 0
        finally:
            if process.poll() is None:
                process.kill(); process.wait()
            log.close()
    process, log = start()
    try:
        for table, row in bindings:
            plan = plans[row['id']]
            old_workspace = row['workspace_path']
            assert old_workspace.startswith('/data/workspaces/')
            workspace = node.root / old_workspace[len('/data/'):]
            workspace.mkdir(parents=True, exist_ok=True)
            prompt = (ROOT / 'crates/station/prompts' / ('slack-proactive-base-instructions.md' if table == 'proactive_bindings' else 'im-thread-base-instructions.md')).read_text()
            latest = plan['handoffs'][-1].get('document') if plan['handoffs'] else None
            context = {'old_session_id': row['id'], 'connect_id': connect_id,
                       'archive': str(archive / 'plans' / (row['id'] + '.json')),
                       'consumed_through_mailbox_seq': plan['consumed_through_mailbox_seq'],
                       'latest_handoff': latest,
                       'pending_count': len(plan['pending']), 'unresolved_tool_calls': plan['unresolved_tool_calls']}
            prompt += '\n\nMigration reference, not a new task: old history remains in the archive below. Do not replay old instructions, pending tools, timers, or delivery effects. Resume only upon explicit new input; consult archived history as data when needed. Current tool.help replaces historical tool names.\n' + json.dumps(context, ensure_ascii=False)
            code, created, _ = request(node.agent_url, 'POST', '/sessions', {
                'profile_id': row['profile_id'], 'model': row['model'], 'thinking': row['thinking'],
                'system_prompt': prompt, 'workspace': str(workspace)})
            assert code == 201, 'Native session creation failed: status ' + str(code)
            mapping.append({'old_key': row['key'], 'new_key': connect_id if table == 'proactive_bindings' else ':'.join((connect_id, row['channel_id'], row['root_thread_ts'])),
                            'old_session_id': row['id'], 'new_session_id': created['session_id'], 'workspace': str(workspace)})
    finally:
        stop(process, log)
    counts = transfer_database(source / 'gateway.sqlite', node.root / 'state/gateway.sqlite', mapping, connect_id, node.root)
    (output / 'mapping.json').write_text(json.dumps(mapping, indent=2))
    histories = {}
    for restart in range(2):
        process, log = start()
        try:
            for item in mapping:
                code, session, _ = request(node.agent_url, 'GET', '/sessions/' + item['new_session_id'])
                assert code == 200 and session['status'] in ('wait', 'finished'), 'Session not idle after migration'
                code, history, _ = request(node.agent_url, 'GET', '/sessions/' + item['new_session_id'] + '/history?limit=200')
                assert code == 200
                assert not any(row['event'].get('kind') in ('tool_result', 'input_received', 'turn_started') for row in history['items']), 'Unexpected historical activation'
                encoded = json.dumps(history['items'], sort_keys=True)
                if restart:
                    assert histories[item['new_session_id']] == encoded, 'Restart changed idle history'
                histories[item['new_session_id']] = encoded
            with sqlite3.connect(node.root / 'state/gateway.sqlite') as db:
                for table, count in counts.items():
                    assert db.execute('SELECT count(*) FROM ' + table).fetchone()[0] == count
                assert db.execute("SELECT count(*) FROM background_jobs WHERE status IN ('registered','running')").fetchone()[0] == 0
        finally:
            stop(process, log)
    assert digest(source / 'gateway.sqlite') == before_db
    report = {'passed': True, 'kind': 'isolated-migration-rehearsal', 'binary_sha256': digest(binary),
              'counts': counts, 'new_native_sessions': len(mapping), 'restart_checks': 2,
              'real_model': False, 'historical_effects_replayed': 0, 'production_changed': False,
              'ready_for_cutover': False, 'requires_final_quiesced_snapshot': True,
              'sessions': [{k: v for k, v in plan.items() if k not in ('transcript', 'handoffs', 'pending', 'latest_context_handoff')} | {'pending_count': len(plan['pending'])} for plan in plans.values()],
              'remaining': ['Final quiesced snapshot and input/effect reconciliation', 'Pending inputs and timers are archived, not automatically resumed', 'Real-model acceptance', 'Authorized mainline merge and Linux ARM64 artifacts']}
    (output / 'report.json').write_text(json.dumps(report, indent=2))
    print(json.dumps(report, ensure_ascii=False), flush=True)
    src.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--connect-id', default='slack')
    args = parser.parse_args()
    run(args.source.resolve(), args.output.resolve(), args.connect_id)

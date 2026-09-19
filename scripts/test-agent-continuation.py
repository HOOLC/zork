#!/usr/bin/env python3
"""Production Station/Agent/Chat regression in an isolated data root.

By default a controlled HTTP provider exercises failure and crash boundaries.
--live-profile explicitly selects a private profile for bounded billable calls;
only the selected enabled model is copied, never an existing Session or Mesh.
"""
import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import queue
import shutil
import signal
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]


def port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def wait(check, description, seconds=30):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            value = check()
            if value:
                return value
        except (URLError, ConnectionError):
            pass
        time.sleep(0.05)
    raise AssertionError('Timed out: ' + description)


class Provider:
    def __init__(self):
        self.requests = queue.Queue()
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                reply = queue.Queue()
                owner.requests.put((body, reply))
                status, payload = reply.get(timeout=60)
                data = json.dumps(payload).encode()
                self.send_response(status)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(data)))
                self.end_headers()
                try:
                    self.wfile.write(data)
                except (BrokenPipeError, ConnectionResetError):
                    pass

            def log_message(self, *_):
                pass

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.url = f'http://127.0.0.1:{self.server.server_port}/v1'

    def next(self):
        return self.requests.get(timeout=30)

    @staticmethod
    def answer(request, text='', tool=None, arguments=None, error=None):
        body, reply = request
        if error:
            reply.put((400, {'error': {'type': 'invalid_request_error', 'message': error}}))
            return
        message = {'role': 'assistant', 'content': text}
        if tool:
            message['tool_calls'] = [{'id': 'call-' + str(time.time_ns()), 'type': 'function',
                'function': {'name': 'call', 'arguments': json.dumps({
                    'tool': tool, 'action': tool, 'arguments': arguments or {}})}}]
        reply.put((200, {'id': 'response-' + str(time.time_ns()), 'object': 'chat.completion',
            'created': 0, 'model': body['model'],
            'choices': [{'index': 0, 'message': message,
                         'finish_reason': 'tool_calls' if tool else 'stop'}],
            'usage': {'prompt_tokens': 100, 'completion_tokens': 10, 'total_tokens': 110}}))

    def end(self):
        self.answer(self.next(), tool='end')

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


class Station:
    def __init__(self, root, binary, profile, model):
        self.root, self.binary, self.model = root, binary, model
        self.bindings = {name: f'127.0.0.1:{port()}' for name in ('runtime', 'station', 'control', 'agent')}
        (root / 'profiles').mkdir()
        profile_path = root / 'profiles/fixture.json'
        profile_path.write_text(json.dumps(profile))
        profile_path.chmod(0o600)
        (root / 'config.json').write_text(json.dumps({
            'bind': self.bindings, 'admin': {'token': 'continuation-fixture'}}))
        self.log = (root / 'process.log').open('ab')
        self.process = None
        self.session = self.home = self.key = None

    def request(self, method, path, data=None, agent=False):
        url = 'http://' + self.bindings['agent' if agent else 'runtime'] + path
        headers = {'Content-Type': 'application/json', 'Authorization': 'Bearer continuation-fixture'}
        req = Request(url, method=method, headers=headers,
                      data=None if data is None else json.dumps(data).encode())
        try:
            response = urlopen(req, timeout=15)
        except HTTPError as error:
            response = error
        with response:
            body = response.read()
            value = json.loads(body) if body else None
            if path == '/readyz' and response.status == 503:
                return False
            assert response.status in (200, 201, 202, 204), (method, path, response.status, value)
            return value

    def start(self):
        self.process = subprocess.Popen([str(self.binary), '--data', str(self.root)],
            stdout=self.log, stderr=self.log, start_new_session=True)
        wait(lambda: self.request('GET', '/readyz'), 'Station ready')

    def stop(self, crash=False):
        if self.process:
            if self.process.poll() is None:
                os.killpg(self.process.pid, signal.SIGKILL if crash else signal.SIGTERM)
            try:
                self.process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                os.killpg(self.process.pid, signal.SIGKILL)
                self.process.wait()
            self.process = None

    def create(self):
        value = self.request('POST', '/v1/node/agents', {
            'id': 'continuation-probe', 'name': 'Continuation probe', 'role': 'leader',
            'profile_id': 'fixture', 'model': self.model['id'],
            'thinking': self.model['default_thinking'],
            'instructions': 'This is an isolated regression Session. Use only this execution workspace. '
                            'Publish intended test results in this Chat and confirm with end. '
                            'When a background job exits, read async-result.txt and report its contents. '
                            'Never access other Sessions, devices or user files.'})
        self.key = value['session_key']
        self.session = value['session_id']
        self.home = self.request('POST', '/v1/node/agents/continuation-probe/open', {})['session_id']

    def send(self, text):
        self.request('POST', f'/v1/im/sessions/{self.home}/messages', {'content': text})

    def events(self, session=None):
        paths = list(self.root.glob(f'**/sessions/{session or self.session}/segments/*.jsonl'))
        result = []
        for path in sorted(paths):
            for line in path.read_text().splitlines():
                try:
                    result.append(json.loads(line))
                except json.JSONDecodeError:
                    pass  # A reader can catch the active writer's torn tail.
        return result

    def finished(self, after=0, outcome='finished', seconds=30):
        def terminal():
            events = self.events()[after:]
            assert sum(e['event']['kind'] == 'step_started' for e in events) <= 32, 'phase exceeded model request bound'
            result = next((e['event'] for e in reversed(events) if e['event']['kind'] == 'turn_finished'), None)
            if result:
                assert result['outcome'] == outcome, result
                return result
            return None
        return wait(terminal, 'durable turn ' + outcome, seconds)

    def messages(self):
        return self.request('GET', f'/v1/im/sessions/{self.home}/messages')['items']

    def public(self):
        return [message for message in self.messages() if message['role'] == 'assistant']

    def sql(self, statement, args=()):
        with sqlite3.connect(self.root / 'state/station.sqlite') as db:
            return db.execute(statement, args).fetchall()

    def workspace(self):
        return Path(self.sql('SELECT workspace_path FROM sessions WHERE id=?', (self.session,))[0][0])

    def activity(self):
        items = self.request('GET', f'/v1/im/sessions/{self.home}/status')['items']
        return next((item['activity'] for item in items if item['id'] == 'continuation-probe'), None)


def controlled(station, provider, passed):
    station.send('Implement the test result and report it')
    provider.answer(provider.next(), 'I will continue now. PRIVATE internal text.')
    reminder = provider.next()
    assert '[runtime.end_confirmation]' in json.dumps(reminder[0])
    assert station.public() == []
    station.send('Also verify the output')
    wait(lambda: any(e['event']['kind'] == 'input_appended' and 'Also verify the output' in e['event']['input']['content']
                     for e in station.events()), 'later Chat input durably reaches the running Session')
    provider.answer(reminder, tool='file.write', arguments={'path': 'probe.txt', 'content': 'verified'})
    next_request = provider.next()
    assert 'Also verify the output' in json.dumps(next_request[0])
    provider.answer(next_request, tool='chat.post_message', arguments={'text': 'Implemented and verified'})
    provider.end()
    station.finished()
    assert station.workspace().joinpath('probe.txt').read_text() == 'verified'
    assert len(station.public()) == 1
    assert len(station.sql('SELECT chat_id FROM chat_channels')) == 1, 'control Session fabricated a second Chat'
    passed('text-only reminder resumes tools; later input is consumed; only the deliberate Chat message is published')

    baseline = len(station.events())
    station.send('Intentional silence')
    provider.answer(provider.next(), 'No public reply needed')
    provider.end()
    station.finished(baseline)
    assert len(station.public()) == 1
    passed('end confirms intentional silence without copying assistant text to Chat')

    baseline = len(station.events())
    station.send('Exercise a provider failure')
    provider.answer(provider.next(), error='fixture provider rejected this request')
    station.finished(baseline, 'failed')
    wait(lambda: 'fixture provider rejected' in json.dumps(station.activity()), 'visible provider failure')
    assert len(station.public()) == 1
    station.stop()
    station.start()
    wait(lambda: 'fixture provider rejected' in json.dumps(station.activity()), 'restored provider failure')
    assert len(station.sql('SELECT chat_id FROM chat_channels')) == 1
    assert provider.requests.empty(), 'failed request restarted automatically'
    passed('HTTP 400 fails once; Chat activity preserves the reason across restart without publishing assistant text')

    baseline = len(station.events())
    station.send('Continue after fixing the request')
    provider.answer(provider.next(), tool='end')
    station.finished(baseline)
    passed('new input resumes the same Session after failure')

    baseline = len(station.events())
    station.send('Ignore end confirmation repeatedly')
    for text in ['Done', 'Done again', '']:
        provider.answer(provider.next(), text)
    station.finished(baseline, 'failed')
    wait(lambda: 'internal assistant text' in json.dumps(station.activity()), 'visible confirmation failure')
    passed('repeated unconfirmed endings stop at a bounded failure with a visible diagnostic')

    baseline = len(station.events())
    station.send('Start a background job')
    script = 'while [ ! -f release-job ]; do sleep 0.1; done\nprintf background-result > async-result.txt'
    provider.answer(provider.next(), tool='job.register', arguments={'kind': 'test', 'script': script})
    provider.end()
    station.finished(baseline)
    baseline = len(station.events())
    station.workspace().joinpath('release-job').touch()
    result_request = provider.next()
    assert 'job_exit' in json.dumps(result_request[0])
    provider.answer(result_request, 'Background completed internally')
    reminder = provider.next()
    assert '[runtime.end_confirmation]' in json.dumps(reminder[0])
    provider.answer(reminder, tool='chat.post_message', arguments={'text': 'Background result reported'})
    provider.end()
    station.finished(baseline)
    assert len(station.public()) == 2
    passed('background completion wakes the original Session and prompts deliberate result publication')

    # Simulate a crash after Agent mailbox fsync but before outbox acknowledgement.
    original = next(e['event']['input'] for e in station.events()
                    if e['event']['kind'] == 'input_appended'
                    and 'job_exit' in e['event']['input']['content'])
    job_id = original['content'].split('job_id: ')[1].split('\n')[0]
    event = {'session_key': station.key, 'job_id': job_id, 'kind': 'test',
             'event_kind': 'job_exit', 'summary': f'Background job {job_id} finished.'}
    station.stop()
    station.sql('INSERT INTO job_mailbox(sequence,session_key,event_json) VALUES(?,?,?)',
                (original['position']['sequence'], station.key, json.dumps(event)))
    before = len(station.events())
    station.start()
    wait(lambda: not station.sql('SELECT 1 FROM job_mailbox'), 'acknowledgement replay')
    assert not [e for e in station.events()[before:] if e['event']['kind'] == 'input_appended']
    assert len(station.public()) == 2 and provider.requests.empty()
    passed('lost background delivery acknowledgement is deduplicated after restart')

    baseline = len(station.events())
    station.send('Start a non-restartable job')
    provider.answer(provider.next(), tool='job.register', arguments={
        'kind': 'test', 'restart_on_boot': False, 'script': 'printf started >> attempts.txt\nsleep 1000'})
    provider.end()
    station.finished(baseline)
    wait(lambda: station.workspace().joinpath('attempts.txt').exists(), 'job side effect')
    restart_baseline = len(station.events())
    station.stop(crash=True)
    station.start()
    interrupted = provider.next()
    assert 'job_interrupted' in json.dumps(interrupted[0]) and 'unknown' in json.dumps(interrupted[0])
    provider.answer(interrupted, tool='end')
    station.finished(restart_baseline)
    wait(lambda: not station.sql('SELECT 1 FROM job_mailbox'), 'interruption delivered')
    assert station.workspace().joinpath('attempts.txt').read_text() == 'started'
    passed('restart reports unknown background effects and never replays a non-restartable command')

    baseline = len(station.events())
    station.send('Start a background job that fails')
    provider.answer(provider.next(), tool='job.register', arguments={
        'kind': 'test', 'restart_on_boot': False,
        'script': 'while [ ! -f release-failure ]; do sleep 0.1; done\nexit 7'})
    provider.end()
    station.finished(baseline)
    baseline = len(station.events())
    station.workspace().joinpath('release-failure').touch()
    failed_job = provider.next()
    assert 'job_failed' in json.dumps(failed_job[0])
    provider.answer(failed_job, tool='chat.post_message', arguments={'text': 'Background failure reported'})
    provider.end()
    station.finished(baseline)
    passed('background failure is delivered to the model and deliberately reported in Chat')

    station.stop()
    before = len(station.events())
    notification = dict(event, job_id='notify', kind='notify', event_kind='notify',
                        summary='Result persisted before mailbox delivery')
    station.sql('INSERT INTO job_mailbox(session_key,event_json) VALUES(?,?)',
                (station.key, json.dumps(notification)))
    station.start()
    resumed = provider.next()
    assert 'Result persisted before mailbox delivery' in json.dumps(resumed[0])
    provider.answer(resumed, tool='end')
    station.finished(before)
    passed('an undelivered durable notification wakes the original Session after restart')

    baseline = len(station.events())
    count = len(station.public())
    station.send('Exercise a rejected Chat publication')
    provider.answer(provider.next(), tool='chat.post_message', arguments={
        'chat_id': 'missing-chat-fixture', 'text': 'This cannot be delivered'})
    failed_send = provider.next()
    failures = [e['event']['result'] for e in station.events()[baseline:]
                if e['event']['kind'] == 'tool_result' and e['event']['result']['tool'] == 'chat.post_message']
    assert len(failures) == 1 and failures[0]['outcome'] == 'failed', failures
    provider.answer(failed_send, 'The publication failed; this is internal text')
    provider.end()
    station.finished(baseline)
    assert len(station.public()) == count
    passed('rejected publication remains a failed tool result; internal text is not forwarded and the send is not retried')

    station.request('POST', '/v1/node/agents', {
        'id': 'continuation-worker', 'name': 'Continuation worker', 'role': 'worker',
        'profile_id': 'fixture', 'model': station.model['id'], 'thinking': station.model['default_thinking'],
        'allowed_leaders': ['continuation-probe']})
    station.send('Assign an isolated Worker')
    provider.answer(provider.next(), tool='agent.assign', arguments={
        'worker_id': 'continuation-worker', 'goal': 'Perform the fixture work and publish its result'})
    pair = [provider.next(), provider.next()]
    child = next(r for r in pair if 'Agent: Continuation worker' in json.dumps(r[0]))
    parent = next(r for r in pair if r is not child)
    provider.answer(parent, tool='end')
    provider.answer(child, 'Worker completed; PRIVATE internal report')
    reminder = provider.next()
    assert '[runtime.end_confirmation]' in json.dumps(reminder[0])
    provider.answer(reminder, tool='chat.post_message', arguments={'text': 'Worker deliberately delivered'})
    # Publication both returns to the Worker and wakes its subscribed creator.
    provider.end()
    provider.end()
    chat, child_session = station.sql("SELECT c.chat_id,s.id FROM worker_tasks w JOIN chat_channels c ON c.session_key=w.session_key JOIN sessions s ON s.key=w.session_key WHERE w.worker_id='continuation-worker'")[0]
    wait(lambda: all(station.request('GET', f'/sessions/{session}', agent=True)['status'] == 'finished'
                     for session in [station.session, child_session]), 'Worker and creator finish their confirmed turns')
    messages = station.request('GET', f'/v1/im/sessions/{chat}/messages')['items']
    assert any('Worker deliberately delivered' in json.dumps(m) for m in messages)
    assert all('PRIVATE internal report' not in json.dumps(m) for m in messages)
    passed('a newly assigned Worker receives the reminder and publishes its result to its own task Chat')


def live(station, passed):
    nonce = 'continuation-' + os.urandom(6).hex()
    baseline = len(station.events())
    station.send('This turn is an end-confirmation probe. Reply with exactly the INTERNAL assistant text '
                 'END_CONFIRMATION_PROBE and do not call any tool in that response, including end. '
                 'There is no work to execute or Chat message to publish for this probe. '
                 'Only after the runtime sends its end-confirmation reminder, call end.')
    station.finished(baseline, seconds=240)
    assert any('[runtime.end_confirmation]' in json.dumps(e['event'].get('notices', [])) for e in station.events()[baseline:])
    assert station.public() == [], 'intentional internal text was publicly posted'
    passed('real model receives the runtime reminder and explicitly confirms intentional silence')

    baseline = len(station.events())
    station.send('Complete this authorized work: use file.write to '
                 f'create probe.txt containing exactly {nonce}, read it with file.read, and publish its '
                 'contents in this Chat with chat.post_message. Use only the isolated workspace. Then end.')
    station.finished(baseline, seconds=240)
    assert station.workspace().joinpath('probe.txt').read_text() == nonce
    wait(lambda: nonce in json.dumps(station.public()), 'real model result publication')
    passed('real model performs file tool round trip and deliberately posts to isolated Chat')

    baseline = len(station.events())
    if station.model['id'] == 'deepseek-flash':
        station.model['streaming'] = False
        station.request('PUT', '/v1/node/profiles/fixture/models', {'models': [station.model]})
    station.send('Read probe.txt again and post its exact contents followed by SECOND to this Chat. '
                 'Then end; keep the file unchanged.')
    station.finished(baseline, seconds=240)
    assert any(nonce in json.dumps(m) and 'SECOND' in json.dumps(m) for m in station.public())
    passed('real model handles subsequent input with existing tool results and provider context'
           + (' after switching the isolated profile to non-streaming' if station.model['id'] == 'deepseek-flash' else ''))

    baseline = len(station.events())
    script = f'while [ ! -f release-job ]; do sleep 0.1; done\nprintf %s {nonce}-BACKGROUND > async-result.txt'
    station.send('Call job.register with kind=test, restart_on_boot=false, and this script: '
                 + json.dumps(script) + '. After registration, end this turn without waiting or polling. '
                 'When its completion arrives later, read async-result.txt, publish its contents in this Chat, then end.')
    station.finished(baseline, seconds=240)
    baseline = len(station.events())
    station.workspace().joinpath('release-job').touch()
    station.finished(baseline, seconds=240)
    assert any(nonce + '-BACKGROUND' in json.dumps(m) for m in station.public())
    passed('real background job result wakes the same Session and is reported to its Chat')

    before = station.public()
    station.stop()
    station.start()
    assert station.public() == before
    baseline = len(station.events())
    station.send('After restart, read probe.txt and post its exact contents followed by RESTART. Then end.')
    station.finished(baseline, seconds=240)
    assert any(nonce in json.dumps(m) and 'RESTART' in json.dumps(m) for m in station.public())
    passed('real model resumes the same Session after Station restart without replaying prior Chat publications')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/debug/zork-station')
    parser.add_argument('--report', type=Path, default=ROOT / 'artifacts/agent-continuation/process')
    parser.add_argument('--live-profile', type=Path)
    parser.add_argument('--model', default='deepseek-flash')
    args = parser.parse_args()
    args.report.mkdir(parents=True, exist_ok=True)
    checks = []
    provider = None
    if args.live_profile:
        profile = json.loads(args.live_profile.read_text())
        model = next(m for m in profile['models'] if m['id'] == args.model and m.get('enabled', True))
        profile['models'] = [dict(model, default=True)]
        model = profile['models'][0]
        model['limits']['max_output_tokens'] = min(model['limits']['max_output_tokens'], 4096)
    else:
        provider = Provider()
        model = {'id': 'fixture-model', 'api': 'openai-completions', 'streaming': False,
                 'thinking': ['off'], 'default_thinking': 'off', 'enabled': True, 'default': True,
                 'capabilities': {'input': ['text']},
                 'limits': {'context_window_tokens': 100000, 'max_output_tokens': 4096}}
        profile = {'provider': 'openai-compatible', 'billing': 'usage', 'base_url': provider.url,
                   'auth': {'type': 'api_key', 'key': 'sk-fixture'}, 'models': [model]}

    def passed(name):
        checks.append(name)
        print('PASS: ' + name, flush=True)

    report = {'checks': checks, 'live': bool(args.live_profile), 'model': model['id'],
              'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest()}
    with tempfile.TemporaryDirectory(prefix='zork-continuation-') as directory:
        station = Station(Path(directory), args.binary.resolve(), profile, model)
        try:
            station.start()
            station.create()
            (live(station, passed) if args.live_profile else controlled(station, provider, passed))
            report['passed'] = True
        except Exception as error:
            report['passed'] = False
            report['failure'] = str(error)
            raise
        finally:
            chats = {}
            try:
                if station.home and station.process and station.process.poll() is None:
                    for (chat_id,) in station.sql('SELECT chat_id FROM chat_channels'):
                        chats[chat_id] = station.request('GET', f'/v1/im/sessions/{chat_id}/messages')['items']
            except (AssertionError, OSError) as error:
                report['chat_capture_error'] = str(error)
            finally:
                station.stop()
            station.log.close()
            if provider:
                provider.close()
            report['session_id'] = station.session
            report['chat_id'] = station.home
            events = station.events()
            sessions = {session: station.events(session) for (session,) in
                        station.sql('SELECT id FROM sessions WHERE id IS NOT NULL')}
            report['events'] = len(events)
            report['requests'] = sum(e['event']['kind'] == 'step_started' for e in events)
            report['usage'] = {key: sum(e['event'].get('usage', {}).get(key, 0) for e in events
                                      if e['event'].get('usage')) for key in ('input_tokens', 'output_tokens')}
            (args.report / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
            (args.report / 'events.jsonl').write_text(''.join(json.dumps(e, ensure_ascii=False) + '\n' for e in events))
            for session, history in sessions.items():
                (args.report / f'{session}.jsonl').write_text(''.join(json.dumps(e, ensure_ascii=False) + '\n' for e in history))
            (args.report / 'chat-messages.json').write_text(json.dumps(chats, ensure_ascii=False, indent=2) + '\n')
            shutil.copy(station.root / 'process.log', args.report / 'process.log')
    print(args.report / 'result.json')


if __name__ == '__main__':
    main()

#!/usr/bin/env python3
"""Real thinking model + real Station, OS-sandboxed, Slack effects loopback-only."""
import argparse
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs
from urllib.request import Request, urlopen
from urllib.error import HTTPError

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('support', ROOT / 'scripts/test-slack-forwarding.py')
support = importlib.util.module_from_spec(spec); spec.loader.exec_module(support)
fixture, request = support.fixture, support.request


class Endpoints:
    def __init__(self, profile):
        self.calls, self.models, self.uploads = [], [], []
        self.profile = profile
        self.requests = 0
        owner = self
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args): pass
            def reply(self, data, status=200):
                raw = json.dumps(data).encode()
                self.send_response(status); self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(raw))); self.end_headers(); self.wfile.write(raw)
            def do_POST(self):
                raw = self.rfile.read(int(self.headers.get('Content-Length', 0)))
                if self.path == '/v1/chat/completions':
                    owner.requests += 1
                    if owner.requests > 48:
                        self.reply({'error': 'acceptance_budget_exhausted'}, 403); return
                    body = json.loads(raw)
                    started = time.monotonic()
                    row = {'thinking': body.get('reasoning_effort'), 'chat_template_kwargs': body.get('chat_template_kwargs'), 'reasoning_chars': 0, 'content_chars': 0}
                    owner.models.append(row)
                    req = Request(profile['base_url'].rstrip('/') + '/chat/completions', data=raw,
                                  headers={'Content-Type': 'application/json', 'User-Agent': 'zork-upgrade-acceptance', 'Authorization': 'Bearer ' + profile['auth']['key']})
                    try:
                        try: response = urlopen(req, timeout=180)
                        except HTTPError as error: response = error
                        with response:
                            row['http_status'] = response.status
                            self.send_response(response.status); self.send_header('Content-Type', response.headers.get('Content-Type', 'text/event-stream'))
                            self.send_header('Connection', 'close'); self.end_headers(); self.close_connection = True
                            for line in response:
                                if 'ttfb_ms' not in row: row['ttfb_ms'] = round((time.monotonic() - started) * 1000)
                                if line.startswith(b'data: ') and line.strip() != b'data: [DONE]':
                                    try:
                                        event = json.loads(line[6:])
                                        for choice in event.get('choices', []):
                                            delta = choice.get('delta', {})
                                            row['reasoning_chars'] += len(delta.get('reasoning_content') or delta.get('reasoning') or '')
                                            row['content_chars'] += len(delta.get('content') or '')
                                        if event.get('usage'): row['usage'] = event['usage']
                                    except ValueError: pass
                                self.wfile.write(line); self.wfile.flush()
                    except (OSError, TimeoutError):
                        row['transport_failed'] = True
                    finally:
                        row['duration_ms'] = round((time.monotonic() - started) * 1000)
                        print(json.dumps({'model_request': owner.requests, **row}), flush=True)
                    return
                if self.path == '/upload/FTEST':
                    owner.uploads.append(raw); self.reply({'ok': True}); return
                if not self.path.startswith('/api/'):
                    self.reply({'error': 'blocked'}, 403); return
                method = self.path[len('/api/'):]
                args = {key: values[0] for key, values in parse_qs(raw.decode(), keep_blank_values=True).items()}
                if method in ('auth.test', 'apps.connections.open'):
                    self.reply({'ok': False, 'error': 'socket_disabled_for_acceptance'}); return
                owner.calls.append({'method': method, 'arguments': args})
                if method == 'conversations.replies':
                    self.reply({'ok': True, 'messages': [{'user': 'UTEST', 'ts': '100.000001', 'text': '请修复工作目录里的 calculator.py：add(2, 3) 应返回 5。先确认收到，再在这个线程报告结果并上传 report.txt。'}], 'has_more': False})
                elif method == 'chat.postMessage':
                    self.reply({'ok': True, 'channel': args.get('channel'), 'ts': '100.' + str(100000 + len(owner.calls)), 'message': {'text': args.get('text')}})
                elif method == 'files.getUploadURLExternal':
                    self.reply({'ok': True, 'file_id': 'FTEST', 'upload_url': owner.url + '/upload/FTEST'})
                elif method == 'files.completeUploadExternal':
                    try: files = json.loads(args.get('files', 'null'))
                    except ValueError: files = None
                    if not isinstance(files, list) or not files or not all(isinstance(file, dict) and file.get('id') == 'FTEST' for file in files):
                        self.reply({'ok': False, 'error': 'invalid_arguments', 'response_metadata': {'messages': ['files must be an array containing uploaded file IDs']}}); return
                    self.reply({'ok': True, 'files': [{'id': 'FTEST'}]})
                else: self.reply({'ok': False, 'error': 'unsupported_fixture_method'})
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.url = 'http://127.0.0.1:' + str(self.server.server_port)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True); self.thread.start()
    def close(self):
        self.server.shutdown(); self.server.server_close(); self.thread.join()


def main(source, output, migrated, profile_id):
    os.umask(0o077)
    assert not output.exists(); output.mkdir(parents=True, mode=0o700)
    assert profile_id and '/' not in profile_id and '\\' not in profile_id
    profile = json.loads((source / 'profiles' / (profile_id + '.json')).read_text())
    endpoint = Endpoints(profile)
    report = {'real_model': True, 'production_changed': False, 'real_slack_writes': 0}
    process = None
    try:
        os.environ['ZORK_REGISTRY_DIR'] = str(output / 'registry')
        node = fixture.Node(output / 'node')
        guarded = copy.deepcopy(profile)
        guarded['base_url'] = endpoint.url + '/v1'; guarded['auth']['key'] = 'fixture-model-proxy'
        # Preserve actual thinking and context settings; cap individual test outputs only.
        guarded['models'][0]['limits']['max_output_tokens'] = 8192
        guarded['models'][0]['single_system_message'] = True
        (node.root / 'profiles/acceptance-model.json').write_text(json.dumps(guarded))
        node.config['im_connections'] = [{'id': 'acceptance', 'name': 'Loopback acceptance', 'provider': 'slack', 'mode': 'proactive', 'enabled': True, 'app_token': 'fixture-no-socket', 'bot_token': 'fixture-no-real-token', 'api_base_url': endpoint.url + '/api'}]
        (node.root / 'config.json').write_text(json.dumps(node.config))
        (node.workspace / 'calculator.py').write_text('def add(a, b):\n    return a - b\n')
        binary = (fixture.TARGET / 'zork-station').resolve()
        with binary.open('rb') as stream:
            report['binary_sha256'] = hashlib.file_digest(stream, 'sha256').hexdigest()
        # Keep the real token solely in this harness, outside the sandbox process.
        ports = [endpoint.server.server_port, *[int(value.rsplit(':', 1)[1]) for value in node.config['bind'].values()], node.udp]
        sandbox = '(version 1)(allow default)\n(deny network-outbound)\n' + ''.join('(allow network-outbound (remote ip "localhost:' + str(port) + '"))\n' for port in ports)
        sandbox += '(deny file-read-data (subpath "/Users") (subpath "/Volumes") (subpath "/private/tmp") (subpath "/private/var/folders"))\n'
        sandbox += '(allow file-read-data (subpath ' + json.dumps(str(node.root.resolve())) + ') (literal ' + json.dumps(str(binary)) + '))\n'
        sandbox += '(deny file-write*)\n(allow file-write* (subpath ' + json.dumps(str(node.root.resolve())) + ') (literal "/dev/null"))\n'
        policy = output / 'sandbox.sb'; policy.write_text(sandbox)
        subprocess.run(['/usr/bin/sandbox-exec', '-f', str(policy), '/bin/cat', str(node.workspace / 'calculator.py')], stdout=subprocess.DEVNULL, check=True)
        # Negative probes verify boundaries before running any autonomous model.
        for command in (['/bin/cat', str(source / 'config.json')], ['/usr/bin/touch', str(output / 'forbidden-write')], ['/usr/bin/curl', '-sS', '--max-time', '3', 'https://slack.com/api/auth.test']):
            probe = subprocess.run(['/usr/bin/sandbox-exec', '-f', str(policy), *command], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            assert probe.returncode != 0, 'Sandbox negative probe unexpectedly succeeded'
        report['sandbox_negative_probes'] = 3
        prompt = (ROOT / 'crates/station/prompts/slack-proactive-base-instructions.md').read_text()
        plans = list((migrated / 'node/legacy-archive/plans').glob('*.json'))
        old = next(json.loads(path.read_text()) for path in plans if json.loads(path.read_text()).get('handoffs'))
        prompt += '\nMigration archive reference only, not a task to execute. Do not repeat earlier actions.\n' + json.dumps({'old_session_id': old['old_session_id'], 'latest_handoff': old['handoffs'][-1]['document']}, ensure_ascii=False)
        started = time.monotonic()
        scratch = node.root / 'tmp'; scratch.mkdir()
        with (output / 'station.log').open('wb') as log:
            process = subprocess.Popen(['/usr/bin/sandbox-exec', '-f', str(policy), str(binary), '--data', str(node.root)], stdout=log, stderr=log, env=dict(os.environ, TMPDIR=str(scratch)))
            def ready():
                assert process.poll() is None, 'Sandboxed Station exited; inspect private log'
                return request(node.url, 'GET', '/readyz')[0] == 200
            fixture.wait(ready, 'Sandboxed Station ready')
            code, created, _ = request(node.agent_url, 'POST', '/sessions', {'profile_id': 'acceptance-model', 'model': profile['models'][0]['id'], 'thinking': 'xhigh', 'system_prompt': prompt, 'workspace': str(node.workspace)})
            assert code == 201, 'Real model session creation failed'
            sid = created['session_id']
            content = '这是新任务，只处理此任务，不继续历史任务。connection=acceptance，channel=CTEST，thread_ts=100.000001。请读取这个 Slack 线程中的请求，按请求修复工作目录里的文件，先回复收到，再报告结果并上传报告文件。所有操作目标均为该线程。不要只在内部回答。'
            code, _, _ = request(node.agent_url, 'POST', '/sessions/' + sid + '/mailbox', {'content': content})
            assert code in (200, 202)
            deadline = time.monotonic() + 480
            active = False
            while time.monotonic() < deadline:
                _, session, _ = request(node.agent_url, 'GET', '/sessions/' + sid)
                active = active or bool(endpoint.models)
                if active and session['status'] in ('wait', 'finished', 'failed'): break
                time.sleep(.5)
            report['status'] = session['status']
            _, history, _ = request(node.agent_url, 'GET', '/sessions/' + sid + '/history?limit=200')
            (output / 'history.json').write_text(json.dumps(history))
            report['tool_results'] = [{'tool': item['event']['result']['tool'], 'outcome': item['event']['result']['outcome']} for item in history['items'] if item['event'].get('kind') == 'tool_result']
            report['duration_ms'] = round((time.monotonic() - started) * 1000)
            report['model_requests'] = endpoint.models
            report['slack_methods'] = [item['method'] for item in endpoint.calls]
            posts = [item['arguments'] for item in endpoint.calls if item['method'] == 'chat.postMessage']
            report['reply_count'] = len(posts)
            report['correct_thread'] = bool(posts) and all(item.get('channel') == 'CTEST' and item.get('thread_ts') == '100.000001' for item in posts)
            report['uploads'] = len(endpoint.uploads)
            completions = [item['arguments'] for item in endpoint.calls if item['method'] == 'files.completeUploadExternal']
            report['upload_correct_thread'] = bool(completions) and all(item.get('channel_id') == 'CTEST' and item.get('thread_ts') == '100.000001' and any(file.get('id') == 'FTEST' for file in json.loads(item.get('files', '[]'))) for item in completions)
            report_file = node.workspace / 'report.txt'
            report['uploaded_bytes_match'] = report_file.is_file() and bool(endpoint.uploads) and endpoint.uploads[-1] == report_file.read_bytes()
            report['thinking_observed'] = sum(row['reasoning_chars'] for row in endpoint.models) > 0
            report['code_changed'] = (node.workspace / 'calculator.py').read_text() != 'def add(a, b):\n    return a - b\n'
            report['passed'] = (report['thinking_observed'] and report['correct_thread'] and len(posts) >= 2 and report['upload_correct_thread'] and report['uploaded_bytes_match'] and report['code_changed'] and session['status'] in ('wait', 'finished'))
    finally:
        if process and process.poll() is None:
            process.terminate()
            try: process.wait(timeout=30)
            except subprocess.TimeoutExpired: process.kill(); process.wait()
        endpoint.close()
        (output / 'report.json').write_text(json.dumps(report, indent=2))
        print(json.dumps(report), flush=True)
    assert report.get('passed'), 'Real-model acceptance incomplete; inspect private history'


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ('source', 'output', 'migrated'): parser.add_argument('--' + arg, type=Path, required=True)
    parser.add_argument('--profile-id', required=True)
    args = parser.parse_args()
    main(args.source.resolve(), args.output.resolve(), args.migrated.resolve(), args.profile_id)

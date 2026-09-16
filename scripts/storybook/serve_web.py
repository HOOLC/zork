#!/usr/bin/env python3
"""Serve a local GPUI Web build, including the precompressed WASM payload."""
import argparse
import functools
import json
import re
import threading
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

class Handler(SimpleHTTPRequestHandler):
    def do_POST(self):
        if urlsplit(self.path).path != '/__client_trace' or not self.server.trace_directory:
            self.send_error(404)
            return
        origin = urlsplit(self.headers.get('Origin', ''))
        if origin.scheme not in ('http', 'https') or origin.netloc != self.headers.get('Host'):
            self.send_error(403)
            return
        try:
            size = int(self.headers.get('Content-Length', '0'))
            if not 0 < size <= 32768:
                raise ValueError('size')
            self.connection.settimeout(5)
            data = json.loads(self.rfile.read(size))
            if data.get('schema') != 1 or not re.fullmatch(r'[a-f0-9-]{36}', data.get('id', '')):
                raise ValueError('schema')
        except (ValueError, TypeError, AttributeError, OSError):
            self.send_error(400)
            return
        with self.server.trace_lock:
            sessions = self.server.trace_sessions
            sessions[data['id']] = data
            while len(sessions) > 16:
                del sessions[next(iter(sessions))]
            target = self.server.trace_directory / 'sessions.json'
            temporary = target.with_suffix('.tmp')
            temporary.write_text(json.dumps(sessions, ensure_ascii=False, indent=2) + '\n')
            temporary.replace(target)
        self.send_response(204)
        self.send_header('Content-Length', '0')
        self.end_headers()

    def end_headers(self):
        self.send_header('Cache-Control', 'no-cache')
        super().end_headers()

    def send_head(self):
        original = Path(self.translate_path(urlsplit(self.path).path))
        compressed = original.with_suffix(original.suffix + '.gz')
        encodings = self.headers.get('Accept-Encoding', '').split(',')
        gzip_ok = any(v.strip().split(';')[0] == 'gzip' and 'q=0' not in v for v in encodings)
        if original.suffix == '.wasm' and gzip_ok and compressed.is_file():
            stream = compressed.open('rb')
            self.send_response(200)
            self.send_header('Content-Type', 'application/wasm')
            self.send_header('Content-Encoding', 'gzip')
            self.send_header('Vary', 'Accept-Encoding')
            self.send_header('Content-Length', str(compressed.stat().st_size))
            self.end_headers()
            return stream
        return super().send_head()

if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--directory', type=Path, required=True)
    p.add_argument('--bind', default='127.0.0.1')
    p.add_argument('--port', type=int, default=8876)
    p.add_argument('--trace-directory', type=Path, help='Opt-in bounded client diagnostics; must be outside the served directory')
    a = p.parse_args()
    if not (a.directory / 'index.html').is_file():
        p.error('directory must contain a built index.html')
    if a.trace_directory:
        a.trace_directory = a.trace_directory.resolve()
        if a.trace_directory.is_relative_to(a.directory.resolve()):
            p.error('trace-directory must be outside the served directory')
        a.trace_directory.mkdir(parents=True, exist_ok=True)
    handler = functools.partial(Handler, directory=str(a.directory.resolve()))
    server = ThreadingHTTPServer((a.bind, a.port), handler)
    server.trace_directory = a.trace_directory
    server.trace_lock = threading.Lock()
    server.trace_sessions = {}
    print(f'Serving {a.directory} on {a.bind}:{a.port}', flush=True)
    server.serve_forever()

#!/usr/bin/env python3
"""Idle SSE subscribers must end cleanly without exhausting HTTP shutdown grace."""
import argparse
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import time
from urllib.request import Request, urlopen


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bin-dir', type=Path, default=Path(__file__).resolve().parents[1] / 'target/debug')
    args = parser.parse_args()
    spec = importlib.util.spec_from_file_location('shutdown_fixture', Path(__file__).with_name('test-mesh.py'))
    fixture = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(fixture)
    fixture.TARGET = args.bin_dir.resolve()
    root = Path(tempfile.mkdtemp(prefix='zork-sse-shutdown-', dir='/tmp'))
    node = fixture.Node(root / 'node')
    with (root / 'station.log').open('wb') as log:
        station = subprocess.Popen([str(fixture.TARGET / 'zork-station'), '--data', str(node.root), '--fake-agent'], stdout=log, stderr=log)
        streams = []
        try:
            fixture.wait(lambda: node.get('/v1/mesh').get('origin') == node.origin, 'Station ready')
            body = json.dumps({'profile_id':'fixture','model':'fixture-model','thinking':'off','workspace':str(node.workspace)}).encode()
            with urlopen(Request(node.agent_url + '/sessions', data=body, headers={'Content-Type':'application/json'}), timeout=3) as response:
                session = json.load(response)['session_id']
            for url in (node.url + '/v1/im/events', node.url + '/internal/realtime/events', node.agent_url + f'/sessions/{session}/events'):
                response = urlopen(url, timeout=3)
                assert response.headers.get('Content-Type', '').startswith('text/event-stream')
                streams.append(response)
            started = time.monotonic()
            station.terminate()
            assert station.wait(timeout=5) == 0
            elapsed = time.monotonic() - started
            assert elapsed < 1, f'Idle event streams delayed shutdown by {elapsed:.3f}s'
            # A properly terminated chunked stream returns EOF, rather than
            # being force-aborted after the grace period with IncompleteRead.
            for stream in streams:
                stream.read()
            print(f'PASS: desktop/admin/Agent SSE ended cleanly; Station shutdown {elapsed:.3f}s; {root}')
        finally:
            for stream in streams:
                stream.close()
            if station.poll() is None:
                station.kill()
                station.wait()


if __name__ == '__main__':
    main()

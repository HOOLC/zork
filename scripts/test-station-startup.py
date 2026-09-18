#!/usr/bin/env python3
"""Local readiness must not wait for persisted, unresponsive Mesh peers."""
import argparse
import importlib.util
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from urllib.request import urlopen


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bin-dir', type=Path, default=Path(__file__).resolve().parents[1] / 'target/debug')
    args = parser.parse_args()
    spec = importlib.util.spec_from_file_location('startup_fixture', Path(__file__).with_name('test-mesh.py'))
    fixture = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(fixture)
    fixture.TARGET = args.bin_dir.resolve()
    root = Path(tempfile.mkdtemp(prefix='zork-startup-', dir='/tmp'))
    node, peer = fixture.Node(root / 'node'), fixture.Node(root / 'peer')
    measurements = []
    with socket.socket(type=socket.SOCK_DGRAM) as blackhole:
        blackhole.bind(('127.0.0.1', 0))
        node.config['mesh']['peers'] = [{
            'origin': peer.origin, 'name': 'unresponsive',
            'addr': f'127.0.0.1:{blackhole.getsockname()[1]}', 'execute': [],
        }]
        (node.root / 'config.json').write_text(json.dumps(node.config))
        for attempt in range(2):
            with (root / f'start-{attempt}.log').open('wb') as log:
                started = time.monotonic()
                station = subprocess.Popen([str(fixture.TARGET / 'zork-station'), '--data', str(node.root), '--fake-agent'], stdout=log, stderr=log)
                try:
                    def ready():
                        assert station.poll() is None, 'Station exited during startup'
                        try:
                            with urlopen(node.url + '/readyz', timeout=.2) as response:
                                return response.status == 200
                        except OSError:
                            return False
                    fixture.wait(ready, 'local Station readiness', timeout=3)
                    elapsed = time.monotonic() - started
                    assert elapsed < 3, elapsed
                    assert node.request('GET', '/v1/tasks')[0] == 200
                    status = node.get('/v1/mesh')
                    if attempt:
                        assert status.get('state') == 'starting', status
                    measurements.append(round(elapsed, 3))
                    # The restored-peer check still runs to completion before
                    # exposing Mesh. Local readiness has not bypassed it.
                    fixture.wait(lambda: node.get('/v1/mesh').get('origin') == node.origin,
                                 'Mesh recovery completes independently', timeout=20)
                finally:
                    station.terminate()
                    try:
                        station.wait(timeout=30)
                    except subprocess.TimeoutExpired:
                        station.kill()
                        station.wait()
                        raise
    print(f'PASS: local startup {measurements}s; persisted offline peer does not block local APIs; Mesh recovery completes; {root}')


if __name__ == '__main__':
    main()

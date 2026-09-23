#!/usr/bin/env python3
"""Run a development node in this checkout's test data directory."""
import os
from pathlib import Path
import sys

repo = Path(__file__).resolve().parents[2]
package = sys.argv[1]
if package not in ('zork', 'zork-station'):
    raise SystemExit('Unsupported development package')
os.environ['ZORK_CHANNEL'] = 'test'
os.environ['ZORK_REGISTRY_DIR'] = str(repo / '.tmp/test-node-registry')
os.execv(sys.executable, [sys.executable, str(repo / 'scripts/lib/build_env.py'),
    '--', 'cargo', 'run', '--locked', '-p', package, '--',
    '--data', str(repo / '.tmp/test-node'), *sys.argv[2:]])

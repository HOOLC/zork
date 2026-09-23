#!/usr/bin/env python3
"""Measure the component dialog and page/directory scrolling on native Metal.

This focused runner uses the fixture owned by the full critical smoke suite.
"""
import argparse
import hashlib
import json
from pathlib import Path
import runpy
import subprocess

ROOT = Path(__file__).resolve().parents[2]
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--binary', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--observe', action='store_true')
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=True)
gate = runpy.run_path(str(ROOT / 'scripts/smoke-critical.py'))['GATES']['client-frame']
fixture = dict(gate['fixture'], components_only=True, gallery=True)
config = a.output / 'fixture.json'
config.write_text(json.dumps(fixture, indent=2) + '\n')
report = {'passed': False, 'renderer': 'native-metal', 'physicalPresentationMeasured': False,
          'binarySha256': hashlib.sha256(a.binary.read_bytes()).hexdigest(), 'cases': []}
try:
    with (a.output / 'native.log').open('w') as log:
        result = subprocess.run(['caffeinate', '-d', '-i', '-u', str(a.binary), '--native-frames', str(config), str(a.output)],
                                cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, timeout=120)
    assert result.returncode == 0, 'Native workload failed; see native.log'
    raw = json.loads((a.output / 'native-frames.json').read_text())
    assert raw['status'] == 'measured', raw.get('error')
    assert raw['fixture'] == fixture, 'Native workload fixture changed'
    assert raw['components']['viewport'] == fixture['component_viewport']
    cases = raw['components']['cases'] + raw['gallery']['cases']
    assert [c['name'] for c in raw['gallery']['cases']] == ['page-scroll', 'directory-scroll']
    def distribution(values):
        ordered = sorted(values)
        assert ordered
        return {k: ordered[min(len(ordered)-1, int(len(ordered)*q))] for k,q in [('p50Ms',.5),('p95Ms',.95),('p99Ms',.99),('maxMs',1)]}
    for case in cases:
        frames = case['frames']
        assert len(frames) >= 2, case['name']
        times = [f['completedAt']-f['at'] for f in frames]
        intervals = [b['completedAt']-a['completedAt'] for a,b in zip(frames,frames[1:]) if a['continuing']]
        assert intervals
        row = {'name':case['name'], 'firstFrameMs':times[0], 'frameCount':len(frames),
               'frameComplete':distribution(times), 'frameCpu':distribution([f['cpuMs'] for f in frames]),
               'completionIntervals':distribution(intervals), 'completedFps':len(intervals)*1000/sum(intervals),
               'overBudgetFrames':sum(t >= gate['budget_ms'] for t in times)}
        row['passed'] = (row['overBudgetFrames'] == 0 and row['completedFps'] > gate['target_fps']
                         and max(intervals) < 1000/gate['target_fps'])
        report['cases'].append(row)
    report['passed'] = all(row['passed'] for row in report['cases'])
finally:
    (a.output / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report))
if not a.observe:
    assert report['passed'], 'Native component performance exceeded the frame budget'

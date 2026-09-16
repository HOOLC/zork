#!/usr/bin/env python3
"""Gate CPU and GPU stages from matched runs; timestamp probing adds CPU work."""
import argparse
import json
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--cpu', type=Path, required=True)
p.add_argument('--gpu', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--fps', type=float, default=240)
a = p.parse_args()
assert a.fps > 0
cpu, gpu = (json.loads(path.read_text()) for path in [a.cpu, a.gpu])
for report in [cpu, gpu]:
    assert report['passed'] and not report['errors']
    assert report['actualBackend'] == 'webgpu'
    assert report['uncapped']
    assert len(report['active']) == 8
assert not cpu['gpuTimestampProbe'] and gpu['gpuTimestampProbe']
assert cpu['servedArtifactSha256'] == gpu['servedArtifactSha256']
assert cpu['viewport'] == gpu['viewport']
budget = 1000 / a.fps
rows = []
for c, g in zip(cpu['active'], gpu['active']):
    for field in ['origin', 'index', 'input', 'payloadSha256']:
        assert c[field] == g[field], field
    assert c['cpuRate'] == g['cpuRate'] == 1
    assert not c['cpuProfiler'] and not g['cpuProfiler']
    assert g['gpuExecution']['count'] == g['frameCpu']['count']
    row = {
        'origin': c['origin'], 'index': c['index'], 'input': c['input'],
        'cpuP95Ms': c['frameCpu']['p95Ms'], 'cpuP99Ms': c['frameCpu']['p99Ms'],
        'gpuRenderP95Ms': g['gpuExecution']['p95Ms'], 'gpuRenderP99Ms': g['gpuExecution']['p99Ms'],
        'submissionP95Ms': c['paintP95Ms'],
    }
    row['passed'] = max(row['cpuP95Ms'], row['gpuRenderP95Ms'], row['submissionP95Ms']) <= budget
    rows.append(row)
result = {
    'targetFps': a.fps, 'budgetMs': budget, 'status': 'met' if all(r['passed'] for r in rows) else 'not_met',
    'artifactSha256': cpu['servedArtifactSha256'], 'viewport': cpu['viewport'],
    'cpuEvidence': str(a.cpu), 'gpuEvidence': str(a.gpu),
    'scope': 'Matched full application CPU frames, GPU render-pass execution and uncapped queue submission cadence. CPU and GPU stages may overlap. Queue feedback latency and physical display presentation are separate.',
    'physicalPresentationMeasured': False, 'cases': rows,
}
a.output.parent.mkdir(parents=True, exist_ok=True)
a.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
print(json.dumps(result, ensure_ascii=False))
assert result['status'] == 'met', 'Frame budget exceeded'

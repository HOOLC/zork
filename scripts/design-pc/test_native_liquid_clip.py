# /// script
# dependencies = ["resvg-py==0.5.0", "pillow==11.3.0", "numpy==2.2.6"]
# ///
"""Compare native Metal readbacks with the exact contours from those frames.

resvg rasterizes the reference SVG masks; it does not render the app.
Run after test-client-frame-budget.py has written liquid-visual.json.
"""
import argparse
import json
from html import escape
from io import BytesIO
from pathlib import Path

import numpy as np
from PIL import Image
import resvg_py

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--input', type=Path, required=True)
a = p.parse_args()
frames = json.loads((a.input / 'liquid-visual.json').read_text())
reference_image = Image.open(a.input / 'liquid-reference.png').convert('RGB')
reference = np.asarray(reference_image).astype(float)
report = {'passed': False, 'frames': [], 'transfers': [], 'renderer': 'native-metal'}
for phase in ('opening', 'closing', 'reversing', 'returning'):
    before = np.asarray(Image.open(a.input / f'liquid-{phase}-before-transfer.png').convert('RGBA'))
    after = np.asarray(Image.open(a.input / f'liquid-{phase}-after-transfer.png').convert('RGBA'))
    assert before.shape == after.shape, ('Transfer changed viewport', phase)
    report['transfers'].append({
        'phase': phase,
        'differencePixels': int(np.count_nonzero(np.any(before != after, axis=2))),
    })
report['maximumTransferDifferencePixels'] = max(t['differencePixels'] for t in report['transfers'])
try:
    for frame in frames:
        width, height = reference_image.size
        svg = (f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
               f'viewBox="0 0 {width} {height}"><path d="{escape(frame["motion"]["path"], quote=True)}" '
               'fill="white" stroke="white" stroke-width="3"/></svg>')
        mask = resvg_py.svg_to_bytes(svg_string=svg)
        outside = np.asarray(Image.open(BytesIO(mask)).convert('RGBA'))[:, :, 3] == 0
        outside[:70] = False
        pixels = np.asarray(Image.open(a.input / frame['file']).convert('RGB')).astype(float)
        flat = outside & (reference.min(axis=2) > 240)
        ratio = float(np.median(pixels[:, :, 0][flat] / reference[:, :, 0][flat]))
        count = int(np.count_nonzero(outside & (np.max(np.abs(pixels-reference*ratio), axis=2) > 20)))
        report['frames'].append({'file': frame['file'], 'outsideDifferencePixels': count})
    report['maximumOutsideDifferencePixels'] = max(f['outsideDifferencePixels'] for f in report['frames'])
    report['passed'] = (len(frames) >= 40
                        and report['maximumOutsideDifferencePixels'] <= 10
                        and report['maximumTransferDifferencePixels'] == 0)
finally:
    (a.input / 'liquid-clip-result.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps({k: v for k, v in report.items() if k != 'frames'}))
assert report['passed'], report

# /// script
# dependencies = ["playwright==1.58.0", "pillow==11.3.0", "numpy==2.2.6"]
# ///
"""Compare native Metal readbacks with the exact contours from those frames.

The browser only rasterizes reference SVG masks; it does not render the app.
Run after test-client-frame-budget.py has written liquid-visual.json.
"""
import argparse
import base64
import json
from io import BytesIO
from pathlib import Path

import numpy as np
from PIL import Image
from playwright.sync_api import sync_playwright

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
with sync_playwright() as pw:
    binary = next((Path.home() / 'Library/Caches/ms-playwright/chromium-1228').glob('**/MacOS/Google Chrome for Testing'))
    browser = pw.chromium.launch(headless=True, executable_path=str(binary))
    page = browser.new_page()
    try:
        for frame in frames:
            encoded = page.evaluate('''({path, size}) => {
                const canvas=document.createElement('canvas');
                [canvas.width, canvas.height]=size;
                const c=canvas.getContext('2d'), shape=new Path2D(path);
                c.fillStyle=c.strokeStyle='white';c.lineWidth=3;c.fill(shape);c.stroke(shape);
                return canvas.toDataURL().split(',')[1];
            }''', {'path': frame['motion']['path'], 'size': reference_image.size})
            outside = np.asarray(Image.open(BytesIO(base64.b64decode(encoded))))[:, :, 3] == 0
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
        browser.close()
print(json.dumps({k: v for k, v in report.items() if k != 'frames'}))
assert report['passed'], report

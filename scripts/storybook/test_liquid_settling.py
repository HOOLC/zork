# /// script
# dependencies = ["playwright==1.58.0"]
# ///
"""Measure visible settling and backdrop timing through actual modal input.

Reference masks are rasterized after playback, so comparisons do not slow the
measured animation. This checks animation duration, not rendering throughput.
"""
import argparse
import hashlib
import json
from pathlib import Path
from playwright.sync_api import sync_playwright
from playground_inputs import PlaygroundInputs

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--url', required=True)
p.add_argument('--wasm-artifact', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--observe', action='store_true')
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=True)
report = {'passed': False, 'errors': [], 'sequences': []}
with sync_playwright() as pw:
    binary = next((Path.home() / 'Library/Caches/ms-playwright/chromium-1228').glob('**/MacOS/Google Chrome for Testing'))
    browser = pw.chromium.launch(headless=True, executable_path=str(binary), args=['--force-device-scale-factor=2'])
    page = browser.new_page(viewport={'width': 798, 'height': 838}, device_scale_factor=2)
    page.on('pageerror', lambda error: report['errors'].append(str(error)))
    def deliver(route):
        response = route.fetch()
        raw = response.body()
        assert raw == a.wasm_artifact.read_bytes()
        report['sha256'] = hashlib.sha256(raw).hexdigest()
        route.fulfill(response=response, body=raw)
    page.route('**/pkg/zork_gui_web_bg.wasm.gz', deliver)
    page.goto(a.url + '?story=liquid-gallery&backend=auto')
    page.wait_for_function('document.documentElement.dataset.ready==="true"', timeout=120000)
    inputs = PlaygroundInputs(page, lambda: page.evaluate('JSON.parse(zorkStory.snapshot())'))
    try:
        page.wait_for_timeout(600)
        for opening in (True, False):
            page.evaluate('''open => {
              const frames = []; window.settlingProbe={frames,done:false};
              const start=performance.now();let started=false;
              function frame() {
                const m=JSON.parse(zorkStory.story_state()).dialog;
                if(m&&m.open===open){
                  started=true;
                  frames.push({at:performance.now()-start,pose:m.pose,path:m.path,moving:m.moving,
                    contentAlpha:m.contentAlpha,backdropAlpha:m.backdropAlpha});
                  if(!m.moving&&m.contentAlpha===(open?1:0)&&m.backdropAlpha===(open?1:0)){
                    window.settlingProbe.done=true;return;
                  }
                }
                if(performance.now()-start>6000){window.settlingProbe.error='Did not settle';window.settlingProbe.done=true;return}
                requestAnimationFrame(frame);
              }requestAnimationFrame(frame);
            }''', opening)
            if opening:
                inputs.click('liquid-library-toggle')
            else:
                page.keyboard.press('Escape')
            page.wait_for_function('window.settlingProbe.done', timeout=8000)
            result = page.evaluate('''() => {
              const probe=window.settlingProbe,frames=probe.frames;
              if(probe.error)return probe;
              const canvas=document.createElement('canvas');canvas.width=innerWidth;canvas.height=innerHeight;
              const c=canvas.getContext('2d',{willReadFrequently:true});
              function mask(path){c.clearRect(0,0,canvas.width,canvas.height);c.fillStyle='white';c.fill(new Path2D(path));return c.getImageData(0,0,canvas.width,canvas.height).data}
              const reference=mask(frames.at(-1).path);
              for(const frame of frames){
                const pixels=mask(frame.path);let difference=0;
                for(let i=3;i<pixels.length;i+=4)difference=Math.max(difference,Math.abs(pixels[i]-reference[i]));
                frame.maximumMaskDifference=difference;
              }
              return probe;
            }''')
            assert not result.get('error'), result.get('error')
            frames = result['frames']
            assert len(frames) >= 3, 'Missing animation frames'
            moving_end = next(i for i, f in enumerate(frames) if not f['moving'])
            visible_end = moving_end
            while visible_end > 0 and frames[visible_end - 1]['maximumMaskDifference'] <= 32:
                visible_end -= 1
            first_veil = next((f for f in frames if f['backdropAlpha'] > 0), None)
            entry = {
                'name': 'opening' if opening else 'closing',
                'materialSettledMs': frames[moving_end]['at'],
                'visibleSettledMs': frames[visible_end]['at'],
                'invisibleTailMs': frames[moving_end]['at'] - frames[visible_end]['at'],
                'allSettledMs': frames[-1]['at'],
                'backdropFirstMs': first_veil['at'] if first_veil else None,
                'backdropStartedWhileMoving': bool(first_veil and first_veil['moving']),
                'frames': frames,
            }
            report['sequences'].append(entry)
        report['passed'] = (not report['errors']
                            and all(s['invisibleTailMs'] <= 150 for s in report['sequences'])
                            and report['sequences'][0]['backdropStartedWhileMoving'])
    finally:
        (a.output / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
        browser.close()
print(json.dumps({**report, 'sequences': [{k:v for k,v in s.items() if k!='frames'} for s in report['sequences']]}))
if not a.observe:
    assert report['passed'], 'Visible settling or backdrop timing did not meet the presentation contract'

# /// script
# dependencies = ["playwright==1.58.0", "pillow==11.3.0"]
# ///
"""Compare the actual screen immediately before and after destination-layer changes.

Only the display clock is held; all open/close/reverse operations use physical
input. The first completed presentation is captured before another animation
frame can run. No fixture state or material geometry is changed by the test.
"""
import argparse
import hashlib
import json
from pathlib import Path

from PIL import Image, ImageChops
from playwright.sync_api import sync_playwright
from playground_inputs import PlaygroundInputs

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--url', required=True)
p.add_argument('--wasm-artifact', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--backend', choices=['auto', 'webgl'], default='auto')
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=True)
report = {'passed': False, 'transfers': [], 'errors': []}

with sync_playwright() as pw:
    binary = next((Path.home() / 'Library/Caches/ms-playwright/chromium-1228').glob('**/MacOS/Google Chrome for Testing'))
    browser = pw.chromium.launch(headless=True, executable_path=str(binary), args=['--force-device-scale-factor=2'])
    context = browser.new_context(viewport={'width': 624, 'height': 900}, device_scale_factor=2)
    context.add_init_script('''
      const raf=window.requestAnimationFrame.bind(window), cancel=window.cancelAnimationFrame.bind(window);
      const held=new Map();window.holdDisplay=false;window.captureNext=false;window.captured=false;
      window.requestAnimationFrame=callback=>{
        const id=raf(time=>{if(window.holdDisplay)held.set(id,callback);else callback(time)});return id;
      };
      window.cancelAnimationFrame=id=>{held.delete(id);cancel(id)};
      window.stepDisplay=()=>{
        const pending=[...held];held.clear();
        for(let i=0;i<pending.length;i++){
          const [id,callback]=pending[i];callback(performance.now());
          if(window.captured){for(const [id,callback] of pending.slice(i+1))held.set(id,callback);break;}
        }
      };
      window.resumeDisplay=()=>{window.holdDisplay=false;for(const callback of held.values())raf(callback);held.clear()};
      const painted=backend=>{window.transferBackend=backend;if(window.captureNext){window.captureNext=false;window.captured=true}};
      if(window.GPUQueue){const submit=GPUQueue.prototype.submit;GPUQueue.prototype.submit=function(...args){const r=submit.apply(this,args);painted('webgpu');return r}};
      for(const name of ['WebGLRenderingContext','WebGL2RenderingContext']){
        const proto=window[name]?.prototype;if(!proto)continue;
        for(const name of ['drawArrays','drawElements','drawArraysInstanced','drawElementsInstanced']){
          const draw=proto[name];if(draw)proto[name]=function(...args){const r=draw.apply(this,args);painted('webgl');return r};
        }
      }
    ''')
    page = context.new_page()
    page.on('pageerror', lambda e: report['errors'].append(str(e)))
    artifact = a.wasm_artifact.read_bytes()

    def artifact_route(route):
        response = route.fetch()
        assert response.body() == artifact
        report['sha256'] = hashlib.sha256(artifact).hexdigest()
        route.fulfill(response=response, body=artifact)

    page.route('**/pkg/zork_gui_web_bg.wasm.gz', artifact_route)
    page.goto(a.url + '?story=liquid-gallery&backend=' + a.backend)
    page.wait_for_function('document.documentElement.dataset.ready==="true"', timeout=120000)
    context.set_offline(True)
    inputs = PlaygroundInputs(page, lambda: page.evaluate('JSON.parse(zorkStory.snapshot())'))

    def dialog():
        return page.evaluate("JSON.parse(zorkStory.story_state()).cards.find(c=>c.kind==='dialog').primitive.dialog")

    def transfer(name, opening):
        page.evaluate('window.holdDisplay=true;window.captured=false')
        page.wait_for_timeout(40)
        before = a.output / f'{name}-before.png'
        after = a.output / f'{name}-after.png'
        page.screenshot(path=str(before))
        page.evaluate('window.captureNext=true')
        if opening:
            center = inputs.element('liquid-dialog-trigger')['center']
            page.mouse.click(center['x'], center['y'])
        else:
            page.keyboard.press('Escape')
        for _ in range(60):
            page.evaluate('window.stepDisplay()')
            if page.evaluate('window.captured'):
                break
            page.wait_for_timeout(16)
        assert page.evaluate('window.captured'), 'No presentation after input'
        page.screenshot(path=str(after))
        state = dialog()
        difference = ImageChops.difference(Image.open(before).convert('RGB'), Image.open(after).convert('RGB'))
        changed = sum(max(pixel) > 0 for pixel in difference.getdata())
        difference.save(a.output / f'{name}-difference.png')
        report['transfers'].append({'name': name, 'changedPixels': changed, 'state': state})
        assert state['destinationLayer'] == ('modal' if opening else 'source'), state
        assert changed == 0, (name, changed, difference.getbbox())
        page.evaluate('window.resumeDisplay()')

    try:
        inputs.click('liquid-library-toggle')
        page.wait_for_timeout(2200)
        inputs.click('liquid-tab-7')
        page.wait_for_timeout(1800)
        inputs.locate('liquid-dialog-trigger')
        page.wait_for_timeout(900)
        bounds = inputs.element('liquid-dialog-trigger')['bounds']
        page.mouse.move(616, 720)
        page.mouse.wheel(0, bounds['y'] - 434)
        page.wait_for_timeout(900)
        center = inputs.element('liquid-dialog-trigger')['center']
        page.mouse.move(center['x'], center['y'])
        page.wait_for_timeout(700)
        transfer('opening', True)
        page.wait_for_function("JSON.parse(zorkStory.story_state()).cards.find(c=>c.kind==='dialog').primitive.dialog.backdropAlpha===1")
        transfer('closing', False)
        page.wait_for_timeout(85)
        transfer('reversing', True)
        page.wait_for_timeout(85)
        transfer('returning', False)
        page.wait_for_timeout(2500)
        report['backend'] = page.evaluate('window.transferBackend')
        report['passed'] = not report['errors']
    finally:
        page.evaluate('window.resumeDisplay()')
        if not report['passed']:
            page.screenshot(path=str(a.output / 'failure.png'))
            (a.output / 'failure-state.json').write_text(page.evaluate('zorkStory.story_state()'))
        (a.output / 'result.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
        browser.close()

print(json.dumps({'passed': report['passed'], 'transfers': [(r['name'], r['changedPixels']) for r in report['transfers']]}))

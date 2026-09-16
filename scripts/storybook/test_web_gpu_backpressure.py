# /// script
# dependencies = ["playwright==1.58.0"]
# ///
"""Delay GPU completion to check bounded submission and input/resize recovery.

The GPU still draws the real app. Only completion delivery is held or rejected;
this is a lifecycle regression, not performance evidence or a critical gate.
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
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=True)
report = {'passed': False, 'errors': [], 'cases': []}

with sync_playwright() as pw:
    binary = next((Path.home() / 'Library/Caches/ms-playwright/chromium-1228').glob('**/MacOS/Google Chrome for Testing'))
    browser = pw.chromium.launch(headless=True, executable_path=str(binary), args=['--force-device-scale-factor=2', '--disable-frame-rate-limit', '--disable-gpu-vsync'])
    page = browser.new_page(viewport={'width': 798, 'height': 838}, device_scale_factor=2)
    page.add_init_script('''
      window.draws=0;window.holdGpuCompletion=false;window.heldGpuCompletions=[];
      if(window.GPUQueue){
        const submit=GPUQueue.prototype.submit,done=GPUQueue.prototype.onSubmittedWorkDone;
        GPUQueue.prototype.submit=function(...args){window.draws++;return submit.apply(this,args)};
        GPUQueue.prototype.onSubmittedWorkDone=function(){
          const work=done.call(this);
          if(!window.holdGpuCompletion)return work;
          return new Promise((resolve,reject)=>window.heldGpuCompletions.push(fail=>{
            work.then(()=>fail?reject(Error('Injected completion failure')):resolve(),reject);
          }));
        };
      }
      window.releaseGpuCompletions=fail=>{
        window.holdGpuCompletion=false;
        for(const release of window.heldGpuCompletions.splice(0))release(fail);
      };
    ''')
    page.on('pageerror', lambda error: report['errors'].append(str(error)))

    def deliver(route):
        response = route.fetch()
        raw = response.body()
        assert raw == a.wasm_artifact.read_bytes()
        report['sha256'] = hashlib.sha256(raw).hexdigest()
        route.fulfill(response=response, body=raw)

    page.route('**/pkg/zork_gui_web_bg.wasm.gz', deliver)
    try:
        page.goto(a.url + '?story=liquid-gallery&backend=auto')
        page.wait_for_function('document.documentElement.dataset.ready==="true"', timeout=120000)
        page.context.set_offline(True)
        inputs = PlaygroundInputs(page, lambda: page.evaluate('JSON.parse(zorkStory.snapshot())'))
        for fail in (False, True):
            page.wait_for_timeout(1000)
            trigger = inputs.locate('liquid-library-toggle')['center']
            page.mouse.move(trigger['x'], trigger['y'])
            page.wait_for_timeout(700)
            start = page.evaluate('window.holdGpuCompletion=true;window.draws')
            page.mouse.click(trigger['x'], trigger['y'])
            page.wait_for_function('window.heldGpuCompletions.length===2', timeout=10000)
            page.wait_for_timeout(150)
            paused = page.evaluate('window.draws') - start
            assert paused == 2, ('Unbounded submissions while GPU completion is held', paused)

            page.keyboard.press('Escape')
            page.set_viewport_size({'width': 720 if not fail else 798, 'height': 860 if not fail else 838})
            page.wait_for_timeout(150)
            assert page.evaluate('window.draws') - start == 2
            assert page.evaluate('JSON.parse(zorkStory.story_state()).panel') is None

            page.evaluate('window.releaseGpuCompletions', fail)
            page.wait_for_function('''()=>{
              const s=JSON.parse(zorkStory.story_state());
              return s.panel===null&&s.dialog&&!s.dialog.open&&!s.dialog.moving&&s.dialog.backdropAlpha===0;
            }''', timeout=10000)
            assert page.evaluate('window.draws') > start + 2
            assert inputs.element('liquid-library-toggle')['visible']
            page.wait_for_timeout(1000)
            idle = page.evaluate('window.draws')
            page.wait_for_timeout(250)
            assert page.evaluate('window.draws') == idle, 'Idle recovery kept scheduling frames'
            name = 'failed-completion' if fail else 'completed'
            page.screenshot(path=str(a.output / (name + '.png')))
            report['cases'].append({'name': name, 'pausedSubmissions': paused, 'inputAndResizeRecovered': True, 'idleStopped': True})
        report['passed'] = not report['errors']
    finally:
        (a.output / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
        browser.close()
assert report['passed'], report
print(json.dumps(report))

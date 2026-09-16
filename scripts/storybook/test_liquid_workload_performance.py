# /// script
# dependencies = ["playwright==1.58.0"]
# ///
"""Measure modal opening/closing and both scroll paths in the actual Web app.

RAF CPU and optional GPU execution are stage measurements. GPU queue feedback,
input-to-first-submission latency and submission cadence are reported separately;
they do not replace the native complete-frame gate or measure physical FPS.
"""
import argparse
import hashlib
import json
from pathlib import Path
import runpy
from playwright.sync_api import sync_playwright
from playground_inputs import PlaygroundInputs

ROOT = Path(__file__).resolve().parents[2]
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--url', required=True)
p.add_argument('--wasm-artifact', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--gpu-timestamps', action='store_true')
p.add_argument('--backend', choices=['auto', 'webgl'], default='auto')
p.add_argument('--default-browser-cadence', action='store_true', help='Keep browser frame pacing for a separate user-flow measurement')
p.add_argument('--observe', action='store_true')
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=True)
gate = runpy.run_path(str(ROOT / 'scripts/smoke-critical.py'))['GATES']['client-frame']
report = {'passed': False, 'errors': [], 'cases': [], 'physicalPresentationMeasured': False,
          'gpuTimestampProbe': a.gpu_timestamps, 'viewport': [798,838,2],
          'browserCadence': 'default' if a.default_browser_cadence else 'unlimited',
          'scope': 'Web RAF CPU and optional GPU stages; native complete-frame acceptance is separate.'}

def distribution(values):
    values = sorted(values)
    assert values
    return {key: values[min(len(values)-1, int(len(values)*q))] for key,q in [('p50Ms',.5),('p95Ms',.95),('p99Ms',.99),('maxMs',1)]}

with sync_playwright() as pw:
    binary = next((Path.home() / 'Library/Caches/ms-playwright/chromium-1228').glob('**/MacOS/Google Chrome for Testing'))
    flags = ['--force-device-scale-factor=2']
    if not a.default_browser_cadence:
        flags += ['--disable-frame-rate-limit','--disable-gpu-vsync']
    browser = pw.chromium.launch(headless=True, executable_path=str(binary), args=flags)
    report['browserVersion'] = browser.version
    page = browser.new_page(viewport={'width':798,'height':838}, device_scale_factor=2)
    timestamp = (ROOT / 'scripts/storybook/webgpu_timing.js').read_text() if a.gpu_timestamps else ''
    page.add_init_script(timestamp + '''
      window.liquidDrawCalls=0;window.liquidRafSerial=0;window.liquidGpuQueues=new Set();
      window.perfActive=false;window.perfRows=[];window.perfSubmissions=[];
      for(const kind of ['pointerdown','pointerup','keydown','wheel'])addEventListener(kind,()=>{
        if(!window.perfActive)return;
        const now=performance.now();
        if(window.liquidInputAt===undefined)window.liquidInputAt=now;
        if(kind!=='pointerdown'&&window.perfActivationAt===undefined)window.perfActivationAt=now;
      },true);
      if(window.GPUQueue){const submit=GPUQueue.prototype.submit;GPUQueue.prototype.submit=function(...a){
        window.liquidBackend='webgpu';
        window.liquidDrawCalls++;window.liquidGpuQueues.add(this);
        if(window.perfActive)window.perfSubmissions.push(performance.now());
        return submit.apply(this,a);
      }}
      for(const name of ['WebGLRenderingContext','WebGL2RenderingContext']){
        const prototype=window[name]?.prototype;if(!prototype)continue;
        for(const method of ['drawArrays','drawElements','drawArraysInstanced','drawElementsInstanced']){
          const original=prototype[method];if(!original)continue;
          prototype[method]=function(...args){window.liquidBackend='webgl';window.liquidDrawCalls++;return original.apply(this,args)};
        }
      }
      const raf=window.requestAnimationFrame.bind(window);
      window.requestAnimationFrame=callback=>raf(t=>{
        const start=performance.now(),before=window.liquidDrawCalls;
        const id=++window.liquidRafSerial;
        window.liquidCurrentRaf={id,at:start-window.liquidInputAt};
        callback(t);window.liquidCurrentRaf=undefined;
        if(window.perfActive&&window.liquidDrawCalls!==before){
          if(window.liquidBackend==='webgl')window.perfSubmissions.push(performance.now());
          const row={frame:id,start,at:start-window.perfStart,cpuMs:performance.now()-start,feedbackMs:null};
          window.perfRows.push(row);
          if(window.liquidGpuQueues.size)Promise.all([...window.liquidGpuQueues].map(q=>q.onSubmittedWorkDone())).then(()=>row.feedbackMs=performance.now()-start);
        }
      });
    ''')
    page.on('pageerror', lambda error: report['errors'].append(str(error)))
    def deliver(route):
        response = route.fetch()
        raw = response.body()
        assert raw == a.wasm_artifact.read_bytes()
        report['sha256'] = hashlib.sha256(raw).hexdigest()
        route.fulfill(response=response, body=raw)
    page.route('**/pkg/zork_gui_web_bg.wasm.gz', deliver)
    page.goto(a.url + '?story=liquid-gallery&backend=' + a.backend)
    page.wait_for_function('document.documentElement.dataset.ready==="true"', timeout=120000)
    page.context.set_offline(True)
    report['backend'] = page.evaluate('window.liquidBackend')
    report['secureContext'] = page.evaluate('window.isSecureContext')
    report['submissionMeasurement'] = ('WebGPU queue.submit' if report['backend'] == 'webgpu'
                                       else 'WebGL frame command issue completion; GPU completion not measured')
    assert report['backend'] in ('webgpu', 'webgl'), 'No rendered backend observed'
    if a.backend == 'webgl':
        assert report['backend'] == 'webgl', 'Requested WebGL backend was not used'
    if a.gpu_timestamps:
        assert report['backend'] == 'webgpu', 'GPU timestamp probe requires WebGPU'
    snapshot = lambda: page.evaluate('JSON.parse(zorkStory.snapshot())')
    state = lambda: page.evaluate('JSON.parse(zorkStory.story_state())')
    inputs = PlaygroundInputs(page, snapshot)
    cdp = page.context.new_cdp_session(page)

    def arm():
        page.evaluate('''()=>{
          window.perfRows=[];window.perfSubmissions=[];window.perfStart=performance.now();
          window.liquidInputAt=undefined;window.perfActivationAt=undefined;window.perfActive=true;
          window.liquidTimestampRows=[];window.liquidTimestampErrors=[];window.liquidObserve=true;
          window.liquidTimestampPhase=(window.liquidTimestampPhase||0)+1;
        }''')

    def finish(name, details=None):
        page.evaluate('window.perfActive=false;window.liquidObserve=false')
        page.evaluate('async()=>await Promise.all([...window.liquidGpuQueues].map(q=>q.onSubmittedWorkDone()))')
        if a.gpu_timestamps:
            page.evaluate('window.liquidTimestampFlush()')
        data = page.evaluate('({rows:window.perfRows,submissions:window.perfSubmissions,inputAt:window.liquidInputAt,activationAt:window.perfActivationAt,timestamps:window.liquidTimestampRows,timestampErrors:window.liquidTimestampErrors})')
        assert isinstance(data.get('inputAt'), (int, float)), 'No real input event observed'
        data['rows'] = [r for r in data['rows'] if r['start'] >= data['inputAt']]
        data['submissions'] = [t for t in data['submissions'] if t >= data['inputAt']]
        assert len(data['rows']) >= 2, (name, data)
        row = {'name':name, 'frameCount':len(data['rows']),
               'firstFrameCpuMs':data['rows'][0]['cpuMs'], 'frameCpu':distribution([r['cpuMs'] for r in data['rows']]),
               'gpuQueueFeedback':distribution([r['feedbackMs'] for r in data['rows']]) if report['backend'] == 'webgpu' else None,
               'inputToFirstSubmissionMs':data['submissions'][0]-data['inputAt'],
               'submissionIntervals':distribution([b-c for c,b in zip(data['submissions'],data['submissions'][1:])])}
        assert isinstance(data.get('activationAt'), (int, float)), 'No activation or wheel input observed'
        row['activationToFirstSubmissionMs'] = next(t for t in data['submissions'] if t >= data['activationAt']) - data['activationAt']
        row['firstActivationFrameCpuMs'] = next(r['cpuMs'] for r in data['rows'] if r['start'] >= data['activationAt'])
        if details:
            row.update(details)
        if a.gpu_timestamps:
            assert not data['timestampErrors'], data['timestampErrors']
            gpu = {}
            for sample in data['timestamps']:
                old = gpu.setdefault(sample['frame'], [int(sample['startNs']),int(sample['endNs'])])
                old[0]=min(old[0],int(sample['startNs']));old[1]=max(old[1],int(sample['endNs']))
            assert {r['frame'] for r in data['rows']} <= gpu.keys(), 'Missing GPU execution timestamps'
            row['gpuExecution'] = distribution([(gpu[r['frame']][1]-gpu[r['frame']][0])/1e6 for r in data['rows']])
        row['overBudgetCpuFrames'] = sum(r['cpuMs'] >= gate['budget_ms'] for r in data['rows'])
        row['passed'] = row['overBudgetCpuFrames'] == 0 and (not a.gpu_timestamps or row['gpuExecution']['maxMs'] < gate['budget_ms'])
        (a.output / f'{len(report["cases"]):02}-{name}.json').write_text(json.dumps(data,indent=2)+'\n')
        report['cases'].append(row)
        print(name,'CPU p95/max',row['frameCpu']['p95Ms'],row['frameCpu']['maxMs'],flush=True)

    def modal(name, trigger, read):
        for index in range(2):
            control = inputs.locate(trigger)
            page.mouse.move(control['center']['x'],control['center']['y'])
            page.wait_for_timeout(200)
            arm()
            page.mouse.click(control['center']['x'],control['center']['y'])
            page.wait_for_timeout(1400)
            current = read()
            assert current['open'] and not current['moving'] and current['backdropAlpha'] == 1, (name,current)
            finish(name+'-open',{'index':index})
            arm()
            page.keyboard.press('Escape')
            page.wait_for_timeout(1200)
            current = read()
            assert not current['open'] and not current['moving'], (name,current)
            finish(name+'-close',{'index':index})

    def scroll(name, marker, position):
        # Use a control near the viewport centre, so the displacement remains
        # observable after the top heading has scrolled out of the snapshot.
        candidates = [e for e in snapshot()['elements']
                      if e['visible'] and 220 < e['bounds']['y'] < 610
                      and e['bounds']['height'] <= 64
                      and (name == 'page-scroll' or e['id'].startswith(('liquid-tab-', 'liquid-inspect-')))]
        assert candidates, ('No visible scroll marker', name)
        marker = min(candidates, key=lambda e: abs(e['center']['y']-430))['id']
        before = inputs.element(marker)['bounds']
        page.mouse.move(*position)
        page.wait_for_timeout(200)
        arm()
        for distance in (-240,240,-240,120):
            cdp.send('Input.synthesizeScrollGesture',{'x':position[0],'y':position[1],'yDistance':distance,'speed':1200,'gestureSourceType':'mouse','preventFling':True})
        after = inputs.element(marker)['bounds']
        assert abs(after['y']-before['y']) > 20, (name,before,after)
        finish(name,{'marker':marker,'before':before,'after':after,'scrollSpeed':1200})
        page.screenshot(path=str(a.output/(name+'.png')))

    try:
        page.wait_for_timeout(700)
        modal('directory','liquid-library-toggle',lambda:state()['dialog'])
        scroll('page-scroll','liquid-demo',(748,460))
        inputs.click('liquid-library-toggle')
        page.wait_for_timeout(1200)
        panel=state()['dialog']['pose']
        scroll('directory-scroll','liquid-section-components',(panel['cx']+panel['w']/2-36,panel['cy']))
        inputs.click('liquid-tab-7')
        page.wait_for_timeout(1300)
        inputs.locate('liquid-dialog-trigger')
        page.wait_for_timeout(400)
        read_dialog=lambda:next(c for c in state()['cards'] if c['kind']=='dialog')['primitive']['dialog']
        modal('dialog','liquid-dialog-trigger',read_dialog)
        report['passed'] = not report['errors'] and all(case['passed'] for case in report['cases'])
    finally:
        (a.output/'result.json').write_text(json.dumps(report,indent=2)+'\n')
        browser.close()
if not a.observe:
    assert report['passed'], 'Web playground stage measurement exceeded the frame budget'

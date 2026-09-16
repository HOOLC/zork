# /// script
# dependencies = ["playwright==1.58.0"]
# ///
"""Send stability and browser cadence; run independently of builds/load tests."""
import argparse
import hashlib
import json
from email.utils import parsedate_to_datetime
from pathlib import Path
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit
from playwright.sync_api import sync_playwright
from playground_inputs import PlaygroundInputs

ROOT = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--url', required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--backend', choices=['auto', 'webgl'], default='webgl')
parser.add_argument('--require-backend', choices=['webgpu', 'webgl'])
parser.add_argument('--profile-only', action='store_true')
parser.add_argument('--stress-only', action='store_true')
parser.add_argument('--active-benchmark', action='store_true')
parser.add_argument('--cpu-rate', type=float, default=1)
parser.add_argument('--wasm-artifact', type=Path)
parser.add_argument('--gpu-sync', action='store_true')
parser.add_argument('--gpu-timestamps', action='store_true', help='Measure WebGPU render execution separately from asynchronous completion feedback')
parser.add_argument('--no-cpu-profile', action='store_true')
parser.add_argument('--unlimited-frames', action='store_true', help='Measure uncapped headless rendering throughput, not physical presentation')
parser.add_argument('--headed', action='store_true', help='Run the same workload in a real browser window on an available desktop')
parser.add_argument('--target-fps', type=float, help='Require the full frame P95 CPU and GPU-completion budget; also require painted cadence when uncapped')
args = parser.parse_args()
if args.target_fps is not None and (args.target_fps <= 0 or not args.gpu_sync or not args.no_cpu_profile or not args.active_benchmark):
    parser.error('--target-fps requires a positive value and --active-benchmark --gpu-sync --no-cpu-profile')
args.output.mkdir(parents=True, exist_ok=True)
report = {'passed': False, 'errors': [], 'console': [], 'cadence': {}, 'stress': [], 'uncapped': args.unlimited_frames, 'headless': not args.headed, 'gpuTimestampProbe':args.gpu_timestamps, 'viewport':{'width':798,'height':837,'deviceScaleFactor':2}}
with sync_playwright() as pw:
    binary = Path.home() / 'Library/Caches/ms-playwright/chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing'
    flags = ['--force-device-scale-factor=2']
    if args.unlimited_frames:
        flags += ['--disable-frame-rate-limit', '--disable-gpu-vsync']
    browser = pw.chromium.launch(headless=not args.headed, executable_path=str(binary) if binary.exists() else None, args=flags)
    context = browser.new_context(viewport={'width': 798, 'height': 837}, device_scale_factor=2)
    timestamp_probe = (ROOT / 'scripts/storybook/webgpu_timing.js').read_text() if args.gpu_timestamps else ''
    context.add_init_script(timestamp_probe + '''
        window.liquidDrawCalls = 0;
        window.liquidContexts = new Set();
        window.liquidGpuQueues = new Set();
        window.liquidGpuErrors = [];
        window.liquidGpuAllocations = [];
        if(window.GPUDevice)for(const method of ['createTexture','createBuffer']){
            const original=GPUDevice.prototype[method];
            GPUDevice.prototype[method]=function(descriptor){
                const start=performance.now(),result=original.call(this,descriptor);
                if(Number.isFinite(window.liquidInputAt))window.liquidGpuAllocations.push({method,label:descriptor.label,at:start-window.liquidInputAt,ms:performance.now()-start,size:descriptor.size});
                return result;
            };
        }
        window.liquidRafSerial = 0;
        const requestFrame = window.requestAnimationFrame.bind(window);
        window.requestAnimationFrame = callback => requestFrame(t => {
            const before = window.liquidDrawCalls, start = performance.now();
            const frame = {id:++window.liquidRafSerial, at:start-window.liquidInputAt};
            window.liquidCurrentRaf = frame;
            callback(t);
            window.liquidCurrentRaf = undefined;
            const cpu = performance.now()-start;
            if(window.liquidRafCosts && before !== window.liquidDrawCalls){
                const cost={frame:frame.id,at:frame.at,ms:cpu,completeMs:null};
                if(window.liquidGpuSync&&window.liquidGpuQueues.size){
                    Promise.all([...window.liquidGpuQueues].map(queue=>queue.onSubmittedWorkDone())).then(()=>cost.completeMs=performance.now()-start).catch(e=>window.liquidGpuErrors.push(String(e)));
                }else{
                    if(window.liquidGpuSync)for(const gl of window.liquidContexts)gl.finish();
                    cost.completeMs=performance.now()-start;
                }
                window.liquidRafCosts.push(cost);
            }
        });
        window.addEventListener('pointerup',()=>{if(window.liquidArmed){window.liquidInputAt=performance.now();window.liquidFirstDrawAt=undefined;}},true);
        window.addEventListener('keydown',e=>{if(window.liquidArmed&&e.key==='Enter'){window.liquidInputAt=performance.now();window.liquidFirstDrawAt=undefined;}},true);
        if(window.GPUQueue){
            const submit=GPUQueue.prototype.submit;
            GPUQueue.prototype.submit=function(...args){
                window.liquidDrawCalls++;window.liquidGpuQueues.add(this);
                if(window.liquidInputAt!==undefined&&window.liquidFirstDrawAt===undefined)window.liquidFirstDrawAt=performance.now();
                return submit.apply(this,args);
            };
        }
        for (const name of ['WebGLRenderingContext', 'WebGL2RenderingContext']) {
            const p = window[name]?.prototype;
            if (!p) continue;
            for (const method of ['drawArrays', 'drawElements', 'drawArraysInstanced', 'drawElementsInstanced']) {
                const original = p[method];
                if (typeof original !== 'function') continue;
                p[method] = function(...args) {
                    window.liquidDrawCalls++;
                    window.liquidContexts.add(this);
                    if (window.liquidInputAt !== undefined && window.liquidFirstDrawAt === undefined) window.liquidFirstDrawAt = performance.now();
                    return original.apply(this, args);
                };
            }
        }
    ''')
    page = context.new_page()
    wasm_responses = []
    page.on('response', lambda response: wasm_responses.append(response) if response.url.endswith('/zork_gui_web_bg.wasm.gz') else None)
    page.on('pageerror', lambda e: report['errors'].append(str(e)))
    page.on('console', lambda m: report['console'].append(m.text) if m.type in ['error', 'warning'] else None)

    def state():
        return page.evaluate('JSON.parse(zorkStory.story_state())')

    def snapshot():
        return page.evaluate('JSON.parse(zorkStory.snapshot())')

    def card():
        return next(c for c in state()['cards'] if c['kind'] == 'composer')

    def frame():
        page.evaluate('()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))')

    inputs = PlaygroundInputs(page, snapshot)

    def locate(id):
        return inputs.locate(id)

    def click(id):
        inputs.click(id)

    def choose():
        page.evaluate('zorkStory.select_story("liquid-gallery")')
        frame()
        page.wait_for_timeout(100)
        click('liquid-tab-2')
        page.wait_for_timeout(100)

    def type_text(text):
        click('liquid-composer-editor')
        page.keyboard.press('ControlOrMeta+a')
        ime = context.new_cdp_session(page)
        end = len(text.encode('utf-16-le')) // 2
        ime.send('Input.imeSetComposition', {'text': text, 'selectionStart': end, 'selectionEnd': end})
        ime.send('Input.insertText', {'text': text})
        ime.detach()
        frame()

    def healthy():
        assert not report['errors'], report['errors']
        c = card()
        assert c['departures']['renderErrors'] == 0, c['departures']
        assert all(s['error'] is None and not s['paintError'] for s in c['surfaces']), c['surfaces']
        return c

    try:
        url = urlsplit(args.url)
        query = dict(parse_qsl(url.query)) | {'story': 'liquid-gallery', 'backend': args.backend}
        page.goto(urlunsplit(url._replace(query=urlencode(query))))
        page.wait_for_function('document.documentElement.dataset.ready==="true"', timeout=120000)
        report['webgl'] = page.evaluate('''()=>[...window.liquidContexts].map(gl=>{
            const info=gl.getExtension('WEBGL_debug_renderer_info');
            return {version:gl.getParameter(gl.VERSION),vendor:info?gl.getParameter(info.UNMASKED_VENDOR_WEBGL):null,renderer:info?gl.getParameter(info.UNMASKED_RENDERER_WEBGL):null};
        })''')
        print('renderer', report['webgl'], flush=True)
        report['actualBackend'] = page.evaluate('window.liquidGpuQueues.size ? "webgpu" : "webgl"')
        report['gpuCompletionScope'] = 'GPUQueue.onSubmittedWorkDone feedback latency' if report['actualBackend'] == 'webgpu' else 'WebGL finish at the end of the application RAF callback'
        print('backend', report['actualBackend'], flush=True)
        if args.require_backend:
            assert report['actualBackend'] == args.require_backend, report['actualBackend']
        if args.gpu_timestamps:
            assert report['actualBackend'] == 'webgpu', 'GPU timestamps require WebGPU'
        if args.target_fps and report['actualBackend'] == 'webgpu':
            assert args.gpu_timestamps, 'WebGPU frame budgets require --gpu-timestamps; completion feedback latency is a different metric'
        assert wasm_responses, 'No WASM response was observed'
        report['wasmLastModified'] = wasm_responses[-1].headers.get('last-modified')
        report['wasmBytes'] = int(wasm_responses[-1].headers['content-length'])
        if args.wasm_artifact:
            assert int(args.wasm_artifact.stat().st_mtime) == int(parsedate_to_datetime(report['wasmLastModified']).timestamp())
            assert args.wasm_artifact.stat().st_size == report['wasmBytes']
            report['servedArtifactSha256'] = hashlib.sha256(args.wasm_artifact.read_bytes()).hexdigest()
        if query.get('autoprobe') == '1':
            page.wait_for_function('document.querySelector("#client-trace")?.textContent.includes("最终数据已回传")', timeout=45000)
            report['clientTrace'] = json.loads(page.locator('#client-trace').get_attribute('data-report'))
            probe = report['clientTrace']
            assert probe['complete'] and not probe['failure'], probe
            assert not probe['gpuErrors'], probe['gpuErrors']
            if args.wasm_artifact:
                assert probe['artifact']['encoding'] == 'gzip'
                assert probe['artifact']['sha256'] == report['servedArtifactSha256']
            prefixes = ['baseline-a', 'discard', 'baseline-b'] if query.get('msaa') == 'compare' else ['']
            assert all(probe['phases'][prefix + 'section-' + str(i)]['gpuRenderMs']['count'] > 0 for prefix in prefixes for i in range(10)), probe['phases']
            if query.get('msaa') == 'compare':
                assert probe['discardedPathPasses'] > 0
            report['passed'] = True
            (args.output / 'result.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
            print('Automatic client GPU probe passed', flush=True)
            raise SystemExit(0)
        context.set_offline(True)
        assert snapshot()['viewport'] == {'width': 798, 'height': 837}
        if args.active_benchmark:
            page.evaluate('(enabled)=>window.liquidGpuSync=enabled', args.gpu_sync)
            cdp = context.new_cdp_session(page)
            cdp.send('Emulation.setCPUThrottlingRate', {'rate': args.cpu_rate})
            page.evaluate('''()=>{
                window.liquidInputs=[];
                new PerformanceObserver(list=>{for(const e of list.getEntries())window.liquidInputs.push({name:e.name,duration:e.duration,processingMs:e.processingEnd-e.processingStart,start:e.startTime})}).observe({type:'event',durationThreshold:16});
            }''')
            report['active'] = []
            for mode in [0, 1]:
                choose()
                click('liquid-composer-departure-mode-' + str(mode))
                for index in range(4):
                    payload = '首帧与连续绘制 ' + str(index) if index < 2 else '长消息发送，包含多行中文与 English。' * 16
                    type_text(payload)
                    page.wait_for_timeout(350)
                    target = locate('liquid-composer-send')
                    page.mouse.move(target['center']['x'], target['center']['y'])
                    if not args.no_cpu_profile:
                        cdp.send('Profiler.enable')
                        cdp.send('Profiler.setSamplingInterval', {'interval': 1000})
                        cdp.send('Profiler.start')
                    page.evaluate('''()=>{
                        window.liquidInputAt=undefined;window.liquidFirstDrawAt=undefined;window.liquidArmed=true;
                        window.liquidActive=[];window.liquidRafCosts=[];window.liquidObserve=true;
                        window.liquidGpuAllocations=[];
                        window.liquidTimestampRows=[];window.liquidTimestampPhase=(window.liquidTimestampPhase||0)+1;
                        let previous=performance.now(),draws=window.liquidDrawCalls;
                        const frame=t=>{
                            const input=window.liquidInputAt;
                            if(input!==undefined&&t>=input)window.liquidActive.push({at:t-input,gap:t-previous,painted:draws!==window.liquidDrawCalls});
                            previous=t;draws=window.liquidDrawCalls;
                            if(window.liquidObserve)requestAnimationFrame(frame);
                        };
                        requestAnimationFrame(frame);
                    }''')
                    # Pointer-down is separately captured by EventTiming; mouseup
                    # starts the accepted-send animation window.
                    if index % 2:
                        page.keyboard.press('Enter')
                    else:
                        page.mouse.down()
                        page.mouse.up()
                    page.wait_for_timeout(1350)
                    page.evaluate('window.liquidObserve=false;window.liquidArmed=false')
                    if args.gpu_timestamps:
                        page.evaluate('window.liquidTimestampFlush()')
                    profile = None if args.no_cpu_profile else cdp.send('Profiler.stop')['profile']
                    data = page.evaluate('({samples:window.liquidActive,cpu:window.liquidRafCosts,gpuErrors:window.liquidGpuErrors,gpuAllocations:window.liquidGpuAllocations,gpuTimestamps:window.liquidTimestampRows,timestampErrors:window.liquidTimestampErrors,firstDrawMs:window.liquidFirstDrawAt-window.liquidInputAt,events:window.liquidInputs.filter(e=>e.start>=window.liquidInputAt-100)})')
                    if profile is not None:
                        (args.output / f'active-{mode}-{index}.cpuprofile').write_text(json.dumps(profile))
                    (args.output / f'active-{mode}-{index}.json').write_text(json.dumps(data))
                    values = sorted(s['gap'] for s in data['samples'] if s['at'] < 850)
                    assert values, 'No pointer/animation samples were captured'
                    painted = [s['at'] for s in data['samples'] if s['painted'] and s['at'] < 850]
                    paint_gaps = sorted(b-a for a,b in zip(painted,painted[1:]))
                    cpu = sorted(r['ms'] for r in data['cpu'] if 0 <= r['at'] < 850)
                    complete = sorted(r['completeMs'] for r in data['cpu'] if 0 <= r['at'] < 850 and r['completeMs'] is not None)
                    assert cpu, 'No full application frame CPU samples'
                    assert not data['gpuErrors'], data['gpuErrors']
                    if args.gpu_sync:
                        assert len(complete) == len(cpu), 'GPU completion feedback missing for active frames'
                    assert paint_gaps, 'No actual painting in the active interval'
                    row = {'origin': mode, 'index': index, 'input': 'Enter' if index % 2 else 'pointer', 'cpuRate': args.cpu_rate, 'cpuProfiler': not args.no_cpu_profile, 'frames': len(values), 'firstDrawMs': data['firstDrawMs'], 'p95Ms': values[int(len(values)*.95)], 'maxMs': max(values), 'paintP95Ms': paint_gaps[int(len(paint_gaps)*.95)], 'paintMaxMs': max(paint_gaps), 'events': data['events']}
                    row['payloadSha256'] = hashlib.sha256(payload.encode()).hexdigest()
                    row['frameCpu'] = {'count':len(cpu),'p50Ms':cpu[int(len(cpu)*.5)],'p95Ms':cpu[int(len(cpu)*.95)],'p99Ms':cpu[int(len(cpu)*.99)],'maxMs':max(cpu)}
                    if args.gpu_sync:
                        row['gpuCompletedFrame'] = {'p50Ms':complete[int(len(complete)*.5)],'p95Ms':complete[int(len(complete)*.95)],'p99Ms':complete[int(len(complete)*.99)],'maxMs':max(complete)}
                    if args.gpu_timestamps:
                        assert not data['timestampErrors'], data['timestampErrors']
                        frames = {}
                        for sample in data['gpuTimestamps']:
                            if 0 <= sample['at'] < 850:
                                value = frames.setdefault(sample['frame'], [int(sample['startNs']), int(sample['endNs']), 0])
                                value[0] = min(value[0], int(sample['startNs']))
                                value[1] = max(value[1], int(sample['endNs']))
                                value[2] += sample['passes']
                        expected = {r['frame'] for r in data['cpu'] if 0 <= r['at'] < 850}
                        assert expected == set(frames), 'GPU timestamps must cover every measured application frame'
                        gpu = sorted((end-start)/1e6 for start,end,_ in frames.values())
                        row['gpuExecution'] = {'count':len(gpu),'p50Ms':gpu[int(len(gpu)*.5)],'p95Ms':gpu[int(len(gpu)*.95)],'p99Ms':gpu[int(len(gpu)*.99)],'maxMs':max(gpu),'scope':'First render pass start through last render pass end, including intervening render work; asynchronous notification and presentation excluded','minPasses':min(v[2] for v in frames.values())}
                    report['active'].append(row)
                    print('active', row, flush=True)
                    current = healthy()
                    assert current['departures']['emitted'] == index + 1 and current['input'] == '', current
            if args.target_fps is not None:
                budget = 1000 / args.target_fps
                gpu_metric = 'gpuExecution' if args.gpu_timestamps else 'gpuCompletedFrame'
                failures = [{'origin': r['origin'], 'index': r['index'], 'frameCpuP95Ms': r['frameCpu']['p95Ms'], 'gpuP95Ms': r[gpu_metric]['p95Ms'], 'paintP95Ms': r['paintP95Ms']} for r in report['active'] if r['frameCpu']['p95Ms'] > budget or r[gpu_metric]['p95Ms'] > budget or (args.unlimited_frames and r['paintP95Ms'] > budget)]
                report['frameBudget'] = {'targetFps': args.target_fps, 'budgetMs': budget, 'gpuMetric':gpu_metric,'scope':'CPU and GPU stage budgets plus uncapped submission cadence; CPU and GPU may overlap','status': 'not_met' if failures else 'met', 'physicalPresentationMeasured': False, 'failures': failures}
                (args.output / 'result.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
                assert not failures, report['frameBudget']
            if args.cpu_rate == 1:
                assert all(r['firstDrawMs'] < 35 for r in report['active']), report['active']
                assert all(r['paintP95Ms'] < 20 and r['maxMs'] < 35 for r in report['active']), report['active']
            cdp.detach()
        if not args.stress_only and not args.active_benchmark:
            # Fixed input/waits match the saved before/after CPU recordings.
            # RAF cadence measures browser scheduling, not monitor presentation.
            for adjusted in [False, True]:
                choose()
                if adjusted:
                    click('liquid-flow-more')
                click('liquid-composer-variant-1')
                page.wait_for_timeout(500)
                cdp = context.new_cdp_session(page)
                cdp.send('Profiler.enable')
                cdp.send('Profiler.setSamplingInterval', {'interval': 1000})
                cdp.send('Profiler.start')
                page.evaluate('''()=>{
                    window.frameTimes=[];window.paintFrameTimes=[];
                    let prev=performance.now(), lastPaint, draws=window.liquidDrawCalls;
                    window.collectFrames=true;
                    const frame=t=>{
                        window.frameTimes.push(t-prev);prev=t;
                        if(draws!==window.liquidDrawCalls){
                            if(lastPaint!==undefined)window.paintFrameTimes.push(t-lastPaint);
                            lastPaint=t;draws=window.liquidDrawCalls;
                        }
                        if(window.collectFrames)requestAnimationFrame(frame);
                    };
                    requestAnimationFrame(frame);
                }''')
                for _ in range(4):
                    type_text('发送性能，检查输入和气泡。')
                    click('liquid-composer-send')
                    page.wait_for_timeout(550)
                page.wait_for_timeout(1800)
                page.evaluate('window.collectFrames=false')
                profile = cdp.send('Profiler.stop')['profile']
                cdp.detach()
                label = 'adjusted' if adjusted else 'default'
                values = sorted(page.evaluate('window.frameTimes'))
                (args.output / (label + '.cpuprofile')).write_text(json.dumps(profile))
                (args.output / (label + '-raf.json')).write_text(json.dumps(values))
                row = {'frames': len(values), 'p50Ms': values[int(len(values) * .5)], 'p95Ms': values[int(len(values) * .95)], 'p99Ms': values[int(len(values) * .99)]}
                paint = sorted(page.evaluate('window.paintFrameTimes'))
                assert len(paint) > 60, 'No real WebGL drawing samples'
                row['webglPaint'] = {'frames': len(paint), 'p50Ms': paint[int(len(paint) * .5)], 'p95Ms': paint[int(len(paint) * .95)], 'p99Ms': paint[int(len(paint) * .99)]}
                (args.output / (label + '-paint-raf.json')).write_text(json.dumps(paint))
                report['cadence'][label] = row
                print(label, row, flush=True)
                assert row['p95Ms'] < 35, row
                assert row['webglPaint']['p95Ms'] < 35, row
                assert not healthy()['departures']['flights']
        if not args.profile_only and not args.active_benchmark:
            for mode, extreme in [(0, False), (1, False), (0, True), (1, True)]:
                page.set_viewport_size({'width': 798, 'height': 837})
                choose()
                if extreme:
                    for knob, count in [('budget-more', 2), ('flow-more', 8), ('smoothing-more', 4), ('adhesion-more', 4), ('damping-less', 6)]:
                        for _ in range(count):
                            click('liquid-' + knob)
                click('liquid-composer-departure-mode-' + str(mode))
                click('liquid-slow')
                peak = 0
                for i in range(16):
                    if i in [5, 10]:
                        page.set_viewport_size({'width': 600 if i == 5 else 320, 'height': 837})
                        # Let the 0.3x resized material reach its layout before
                        # targeting a 24px moving button from a prior snapshot.
                        page.wait_for_timeout(1400)
                        frame()
                    type_text('连续中文与 English。' * 16 if i == 2 else '连续发送 ' + str(i))
                    if i % 2:
                        page.keyboard.press('Enter')
                        frame()
                    else:
                        click('liquid-composer-send')
                    current = healthy()
                    assert current['departures']['emitted'] == i + 1, {'index': i, 'card': current}
                    if i == 2:
                        page.screenshot(path=str(args.output / f'long-bubble-{mode}-{extreme}.png'))
                    count = len(current['departures']['flights'])
                    peak = max(peak, count)
                    assert count <= 4
                # The toolbar may be outside the retained automation snapshot
                # after a narrow-window scroll. Return to its actual location.
                page.set_viewport_size({'width': 798, 'height': 837})
                page.mouse.move(400, 200)
                page.mouse.wheel(0, -6000)
                page.wait_for_timeout(150)
                click('liquid-slow')
                locate('liquid-composer-editor')
                page.wait_for_timeout(2200)
                current = healthy()
                assert current['departures']['emitted'] == 16, current['departures']
                assert not current['departures']['flights'], current['departures']
                assert current['surfaces'][0]['particles'] == (64 if extreme else 12) * 4
                assert peak == 4, peak
                row = {'origin': mode, 'extreme': extreme, 'accepted': 16, 'peakFlights': peak, 'restoredParticles': current['surfaces'][0]['particles']}
                report['stress'].append(row)
                page.screenshot(path=str(args.output / f'burst-{mode}-{extreme}.png'))
                print('burst', row, flush=True)
        if query.get('trace') == 'client':
            # The interaction checks above remain offline. Reconnect only for
            # the opt-in diagnostic export after the test workload has ended.
            context.set_offline(False)
            page.wait_for_function('document.querySelector("#client-trace")?.textContent.includes("最终数据已回传")', timeout=40000)
            trace = page.locator('#client-trace').get_attribute('data-report')
            report['clientTrace'] = json.loads(trace)
            assert report['clientTrace']['backend'] == report['actualBackend']
            assert report['clientTrace']['frameCpuMs']['count'] > 0
        report['passed'] = True
        (args.output / 'result.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    finally:
        (args.output / 'diagnostics.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
        try:
            page.screenshot(path=str(args.output / 'last.png'))
            (args.output / 'last-state.json').write_text(json.dumps({'state': state(), 'snapshot': snapshot()}, ensure_ascii=False, indent=2))
        except Exception:
            pass
        browser.close()

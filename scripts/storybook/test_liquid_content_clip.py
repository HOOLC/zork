# /// script
# dependencies = ["playwright==1.58.0", "pillow==11.3.0", "numpy==2.2.6"]
# ///
"""Verify live liquid clipping, source visibility and reversal with physical input.

Holds complete rendered frames for pixel comparisons; this is not a timing benchmark.
"""
from pathlib import Path
from io import BytesIO
import argparse, base64, hashlib, json, sys
import numpy as np
from PIL import Image
from playwright.sync_api import sync_playwright

root = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(root / 'scripts/storybook'))
from playground_inputs import PlaygroundInputs
p = argparse.ArgumentParser()
p.add_argument('--url', required=True)
p.add_argument('--artifact', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--width', type=int, default=798)
p.add_argument('--height', type=int, default=837)
p.add_argument('--dpr', type=float, default=2)
p.add_argument('--backend', choices=['auto', 'webgl'], default='auto')
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=True)
report = {'passed': False, 'frames': [], 'errors': [], 'viewport': [a.width,a.height,a.dpr]}
with sync_playwright() as pw:
    binary = next((Path.home() / 'Library/Caches/ms-playwright/chromium-1228').glob('**/MacOS/Google Chrome for Testing'))
    browser = pw.chromium.launch(headless=True, executable_path=str(binary), args=['--force-device-scale-factor='+str(a.dpr)])
    page = browser.new_page(viewport={'width':a.width,'height':a.height},device_scale_factor=a.dpr)
    page.on('pageerror', lambda error: report['errors'].append(str(error)))
    page.on('console', lambda msg: report['errors'].append(msg.text) if msg.type=='error' and any(w in msg.text for w in ['GPU error','Validation','wgpu','panicked']) else None)
    page.add_init_script('''(() => {
      const raf=window.requestAnimationFrame.bind(window); let n=0; const pending=new Map();
      window.clipPaused=false;
      window.requestAnimationFrame=f=>{const id=++n;pending.set(id,f);raf(t=>{if(!window.clipPaused&&pending.has(id)){pending.delete(id);f(t)}});return id};
      window.cancelAnimationFrame=id=>pending.delete(id);
      window.clipStep=()=>new Promise(resolve=>raf(t=>{const batch=[...pending.values()];pending.clear();batch.forEach(f=>f(t));resolve()}));
      window.clipResume=()=>{window.clipPaused=false;const batch=[...pending.values()];pending.clear();batch.forEach(f=>window.requestAnimationFrame(f))};
    })()''')
    page.add_init_script('''window.clipBackend='none';if(window.GPUQueue){const f=GPUQueue.prototype.submit;GPUQueue.prototype.submit=function(...a){window.clipBackend='webgpu';return f.apply(this,a)}}for(const n of ['WebGLRenderingContext','WebGL2RenderingContext']){const p=window[n]?.prototype;if(p&&typeof p.drawArraysInstanced==='function'){const f=p.drawArraysInstanced;p.drawArraysInstanced=function(...a){window.clipBackend='webgl';return f.apply(this,a)}}}''')
    def deliver(route):
        response=route.fetch(); raw=response.body()
        assert raw==a.artifact.read_bytes()
        report['sha256']=hashlib.sha256(raw).hexdigest()
        route.fulfill(response=response,body=raw)
    page.route('**/pkg/zork_gui_web_bg.wasm.gz',deliver)
    page.goto(a.url+'?story=liquid-gallery&backend='+a.backend)
    page.wait_for_function('document.documentElement.dataset.ready==="true"',timeout=120000)
    snapshot=lambda:page.evaluate('JSON.parse(zorkStory.snapshot())')
    state=lambda:page.evaluate('JSON.parse(zorkStory.story_state())')
    inputs=PlaygroundInputs(page,snapshot)
    report['actualBackend']=page.evaluate('window.clipBackend')
    if a.backend == 'webgl': assert report['actualBackend'] == 'webgl'
    cdp=page.context.new_cdp_session(page)
    try:
        page.mouse.move(a.width-2,a.height-2)
        page.wait_for_timeout(1300)
        control=inputs.locate('liquid-library-toggle')
        source=control['center']
        b=control['bounds']
        caption=tuple(round(x*a.dpr) for x in [b['x']+30,b['y']+5,b['x']+b['width']-6,b['y']+b['height']-5])
        before=Image.open(BytesIO(page.screenshot(path=str(a.output/'before.png')))).convert('RGB')
        reference=np.asarray(before).astype(float)
        ink=np.asarray(before.crop(caption)).max(axis=2)<120
        assert np.count_nonzero(ink)>20
        page.evaluate('window.clipPaused=true')
        phases = [
            ('open', True, 22, True), ('close', False, 22, True),
            ('reopen', True, 22, True), ('reverse-close', False, 3, False),
            ('reverse-open', True, 4, False), ('reverse-close-again', False, 4, False),
            ('reverse-finish-open', True, 22, True),
        ]
        for phase, opening, steps, settle in phases:
            if opening: page.mouse.click(source['x'],source['y'])
            else: page.keyboard.press('Escape')
            for index in range(steps):
                report['active']={'phase':phase,'index':index}
                page.wait_for_timeout(25)
                page.evaluate('window.clipStep()')
                frame=state()['dialog']
                assert frame['open']==opening, ('wrong reversal target',phase,frame['open'])
                mask=page.evaluate('''path => {
                  const canvas=document.createElement('canvas');canvas.width=innerWidth*devicePixelRatio;canvas.height=innerHeight*devicePixelRatio;
                  const c=canvas.getContext('2d');c.scale(devicePixelRatio,devicePixelRatio);
                  const shape=new Path2D(path);c.fillStyle='white';c.strokeStyle='white';c.lineWidth=3;c.fill(shape);c.stroke(shape);
                  return canvas.toDataURL().split(',')[1];
                }''', frame['path'])
                raw=base64.b64decode(cdp.send('Page.captureScreenshot',{'format':'png','fromSurface':True,'captureBeyondViewport':False})['data'])
                assert state()['dialog']['revision']==frame['revision'],'capture advanced a Rust frame'
                picture=Image.open(BytesIO(raw)).convert('RGB')
                pixels=np.asarray(picture).astype(float)
                caption_pixels=np.asarray(picture.crop(caption))
                retention=float(np.count_nonzero((caption_pixels.max(axis=2)<120)&ink)/np.count_nonzero(ink))
                assert len([e for e in snapshot()['elements'] if e['id']=='liquid-library-toggle'])==1
                if mask:
                    outside=np.asarray(Image.open(BytesIO(base64.b64decode(mask))).convert('RGBA'))[:,:,3]==0
                    outside[:round(70*a.dpr)]=False
                    # Recover scrim opacity from the median of unchanged canvas pixels.
                    flat=outside & (reference.min(axis=2)>240)
                    ratio=float(np.median(pixels[:,:,0][flat]/reference[:,:,0][flat]))
                    difference=np.max(np.abs(pixels-reference*ratio),axis=2)
                    escaped=outside & (difference>20)
                    count=int(np.count_nonzero(escaped))
                    if count>10 or index in [2,5,10,21]:
                        picture.save(a.output/f'{phase}-{index:02}.png')
                    report['frames'].append({'phase':phase,'index':index,'outsideDifferencePixels':count,'sourceInkRetention':retention,'scrimRatio':ratio,'motion':frame})
            if settle:
                page.evaluate('window.clipResume()')
                page.wait_for_timeout(2800)
                page.evaluate('window.clipPaused=true')
        # Keep the input tree from an early, small contour, then grow only the
        # rendered material. The same real row must reject a clipped click and
        # accept a click as soon as its point enters the current contour.
        page.keyboard.press('Escape')
        page.evaluate('window.clipResume()')
        page.wait_for_timeout(1800)
        page.evaluate('window.clipPaused=true')
        page.mouse.click(source['x'],source['y'])
        page.evaluate('window.clipStep()')
        row=next(e for e in snapshot()['elements'] if e['id']=='liquid-inspect-fields')
        rb=row['bounds'];target={'x':rb['x']+rb['width']/2,'y':rb['y']+rb['height']/2}
        def target_inside():
            return page.evaluate('''({path,p})=>{
                const c=document.createElement('canvas').getContext('2d');
                return c.isPointInPath(new Path2D(path),p.x,p.y);
            }''', {'path':state()['dialog']['path'],'p':target})
        assert not target_inside(), 'early contour already reached test point'
        page.mouse.click(target['x'],target['y'])
        assert state()['selectedKind'] is None, 'clipped row accepted a click'
        for _ in range(16):
            page.wait_for_timeout(20)
            page.evaluate('window.clipStep()')
            if target_inside(): break
        assert target_inside() and state()['dialog']['moving'], 'test missed moving contour'
        page.mouse.click(target['x'],target['y'])
        assert state()['selectedKind']=='fields', 'growing contour kept a stale input mask'
        report['liveHitMask']=True
        page.evaluate('window.clipResume()')
        page.wait_for_timeout(1800)
        report['maximumOutsideDifferencePixels']=max(f['outsideDifferencePixels'] for f in report['frames'])
        report['minimumSourceInkRetention']=min(f['sourceInkRetention'] for f in report['frames'])
        report['passed']=not report['errors'] and report['maximumOutsideDifferencePixels']<=10 and report['minimumSourceInkRetention']>=.98
    finally:
        (a.output/'result.json').write_text(json.dumps(report,ensure_ascii=False,indent=2))
        browser.close()
print(json.dumps({k:v for k,v in report.items() if k not in ['frames','trace']},ensure_ascii=False))
assert report['passed'],report['maximumOutsideDifferencePixels']

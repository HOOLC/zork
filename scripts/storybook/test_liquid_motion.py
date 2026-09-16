# /// script
# dependencies = ["playwright==1.58.0"]
# ///
"""Physical browser inputs against the deduplicated Rust liquid gallery."""
import argparse, json, time, struct, zlib
from pathlib import Path
from playwright.sync_api import sync_playwright
from playground_inputs import PlaygroundInputs

ROOT=Path(__file__).resolve().parents[2]
p=argparse.ArgumentParser();p.add_argument('--url',required=True);p.add_argument('--output',type=Path,default=ROOT/'artifacts/liquid-rust/web-checks');p.add_argument('--backend',choices=['auto','webgl'],default='auto');args=p.parse_args();args.output.mkdir(parents=True,exist_ok=True)
report={'backend':args.backend,'errors':[],'console':[],'checks':{},'screenshots':[]}
with sync_playwright() as pw:
 binary=Path.home()/'Library/Caches/ms-playwright/chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing'
 browser=pw.chromium.launch(headless=True,executable_path=str(binary),args=["--force-device-scale-factor=2"]);context=browser.new_context(viewport={'width':1100,'height':1500},device_scale_factor=2)
 context.add_init_script(r'''(() => {
  let draws=0,context;
  for(const name of ['WebGLRenderingContext','WebGL2RenderingContext']){
   const p=window[name]?.prototype;if(!p)continue;
   for(const name of ['drawArrays','drawElements','drawArraysInstanced','drawElementsInstanced']){
    const original=p[name];if(typeof original!=='function')continue;
    p[name]=function(...args){draws++;context=this;return original.apply(this,args);};
   }
  }
  window.addEventListener('pointerdown',()=>{if(window.liquidPixelProbe)window.liquidPixelProbe.started=performance.now();},true);
  const frame=window.requestAnimationFrame.bind(window);
  window.requestAnimationFrame=callback=>frame(t=>{
   const before=draws;callback(t);
   const probe=window.liquidPixelProbe,gl=context;
   if(!probe||probe.started===null||!gl||draws===before||performance.now()-probe.started>450)return;
   const rect=gl.canvas.getBoundingClientRect();
   const x=Math.floor((probe.x-rect.left)*gl.drawingBufferWidth/rect.width);
   const y=gl.drawingBufferHeight-1-Math.floor((probe.y-rect.top)*gl.drawingBufferHeight/rect.height);
   const target=gl.READ_FRAMEBUFFER??gl.FRAMEBUFFER;
   const saved=gl.getParameter(gl.READ_FRAMEBUFFER_BINDING??gl.FRAMEBUFFER_BINDING);
   const pack=gl.PIXEL_PACK_BUFFER===undefined?undefined:gl.getParameter(gl.PIXEL_PACK_BUFFER_BINDING);
   const pixel=new Uint8Array(4);
   gl.bindFramebuffer(target,null);
   if(pack!==undefined)gl.bindBuffer(gl.PIXEL_PACK_BUFFER,null);
   gl.readPixels(x,y,1,1,gl.RGBA,gl.UNSIGNED_BYTE,pixel);
   if(pack!==undefined)gl.bindBuffer(gl.PIXEL_PACK_BUFFER,pack);
   gl.bindFramebuffer(target,saved);
   probe.samples.push({timeMs:performance.now()-probe.started,rgb:Array.from(pixel.slice(0,3)),alpha:pixel[3]});
  });
 })();''')
 page=context.new_page();page.on('pageerror',lambda e:report['errors'].append(str(e)));page.on('console',lambda m:report['console'].append(m.text) if m.type in ['error','warning'] else None)
 try:
  page.goto(args.url+'?story=liquid-gallery&backend='+args.backend)
  page.wait_for_function('document.documentElement.dataset.ready==="true"',timeout=120000)
  context.set_offline(True)
  def state():return page.evaluate('JSON.parse(zorkStory.story_state())')
  def snap():return page.evaluate('JSON.parse(zorkStory.snapshot())')
  def card(kind):return next(c for c in state()['cards'] if c['kind']==kind)
  def frame():page.evaluate('()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))')
  def element(id):return next((e for e in snap()['elements'] if e['id']==id),None)
  inputs=PlaygroundInputs(page,snap)
  def locate(id):return inputs.locate(id)
  def click(id):
   print('click',id,flush=True);inputs.click(id)
  def wait(expression):page.wait_for_function(expression,timeout=8000)
  def choose(group):
   page.evaluate('zorkStory.select_story("liquid-gallery")');frame();page.wait_for_timeout(100)
   if group:click('liquid-tab-'+str(group))
   page.wait_for_timeout(100)
  def shot(name):page.screenshot(path=str(args.output/(name+'.png')));report['screenshots'].append(name)
  def type_text(id,text):
   click(id);page.keyboard.press('ControlOrMeta+a');ime=context.new_cdp_session(page);end=len(text.encode('utf-16-le'))//2;ime.send('Input.imeSetComposition',{'text':text,'selectionStart':end,'selectionEnd':end});ime.send('Input.insertText',{'text':text});frame()
  def pixel(x,y):
   raw=page.screenshot(clip={'x':x,'y':y,'width':1,'height':1});offset=8;compressed=b''
   while offset<len(raw):
    length=struct.unpack('>I',raw[offset:offset+4])[0];kind=raw[offset+4:offset+8];data=raw[offset+8:offset+8+length]
    if kind==b'IHDR':assert data[8]==8 and data[9] in [2,6]
    if kind==b'IDAT':compressed+=data
    offset+=12+length
   # First pixel predictors are zero for every PNG row-filter type.
   return list(zlib.decompress(compressed)[1:4])
  choose(0);e=locate('liquid-actions-action-0');page.mouse.move(e['center']['x'],e['center']['y']);page.wait_for_timeout(350)
  edge=(e['center']['x'],e['bounds']['y'])
  normal=pixel(*edge);before=card('actions')['actions'];page.mouse.down();page.wait_for_timeout(140);held=pixel(*edge);shot('button-liquid-held')
  assert max(normal)<190 and min(held)>230,{'normal':normal,'held':held}
  page.mouse.up();page.wait_for_timeout(900);assert card('actions')['actions']==before+1
  restored=pixel(*edge);assert max(abs(a-b) for a,b in zip(normal,restored))<4,restored
  page.keyboard.down('Space');page.wait_for_timeout(150);key_held=pixel(*edge);shot('button-keyboard-held');page.keyboard.up('Space');page.wait_for_timeout(800)
  assert min(key_held)>200,key_held
  assert card('actions')['actions']==before+2
  # Drag-out cancels activation, and moving back into a held control continues pressure.
  page.mouse.move(e['center']['x'],e['center']['y']);page.mouse.down();page.wait_for_timeout(80);page.mouse.move(e['center']['x']+e['bounds']['width']+90,e['center']['y']+80);page.wait_for_timeout(180);page.mouse.up();page.wait_for_timeout(850)
  assert card('actions')['actions']==before+2
  page.mouse.move(e['center']['x'],e['center']['y']);taps=[]
  if args.backend=='webgl':
   page.evaluate('(point)=>window.liquidPixelProbe={x:point[0],y:point[1],started:null,samples:[]}',edge)
   page.mouse.click(e['center']['x'],e['center']['y']);page.wait_for_timeout(550)
   taps=page.evaluate('window.liquidPixelProbe.samples');page.evaluate('window.liquidPixelProbe=null')
   assert len(taps)>2 and all(p['alpha']==255 for p in taps),taps
  else:
   page.mouse.click(e['center']['x'],e['center']['y']);begin=time.perf_counter()
   for target in [20,45,75,110,160,230]:
    page.wait_for_timeout(max(0,target-(time.perf_counter()-begin)*1000));taps.append({'timeMs':(time.perf_counter()-begin)*1000,'rgb':pixel(*edge)})
    if min(taps[-1]['rgb'])>170:shot('button-quick-tap')
  tap=max((p['rgb'] for p in taps),key=min);page.wait_for_timeout(900)
  assert card('actions')['actions']==before+3
  report['checks']['quickTapVisible']=min(tap)>170;report['checks']['tapSamples']=taps
  report['checks']['liquidPress']={'rest':normal,'held':held,'keyboardHeld':key_held,'quickTap':tap,'dragOutCancels':True}
  # The workbench action must show the same actual material pressure as the
  # specimen, including a down/up pair before the next visible frame.
  toolbar=locate('liquid-demo');toolbar_edge=(toolbar['center']['x'],toolbar['bounds']['y'])
  page.mouse.move(toolbar['center']['x'],toolbar['center']['y']);page.wait_for_timeout(350)
  toolbar_rest=pixel(*toolbar_edge)
  page.mouse.down();page.wait_for_timeout(140);toolbar_held=pixel(*toolbar_edge);page.mouse.up();page.wait_for_timeout(900)
  assert max(toolbar_rest)<190 and min(toolbar_held)>230,{'rest':toolbar_rest,'held':toolbar_held}
  if args.backend=='webgl':
   page.evaluate('(point)=>window.liquidPixelProbe={x:point[0],y:point[1],started:null,samples:[]}',toolbar_edge)
   page.mouse.click(toolbar['center']['x'],toolbar['center']['y']);page.wait_for_timeout(550)
   toolbar_taps=page.evaluate('window.liquidPixelProbe.samples');page.evaluate('window.liquidPixelProbe=null')
   assert len(toolbar_taps)>2 and max(min(sample['rgb']) for sample in toolbar_taps)>170,toolbar_taps
  else:
   toolbar_taps=[]
  report['checks']['workbenchUsesSpecimenPressure']={'rest':toolbar_rest,'held':toolbar_held,'quickTapSamples':toolbar_taps}
  page.mouse.move(1090,1490);page.wait_for_timeout(1200);before_frames=card('actions')['frames'];page.wait_for_timeout(500);assert card('actions')['frames']-before_frames<=1
  choose(1);selector_widths=[]
  for variant in [0,1,0]:
   click('liquid-popover-variant-'+str(variant));page.wait_for_timeout(750);e=locate('liquid-popover-trigger');selector_widths.append(e['bounds']['width']);assert 80<e['bounds']['width']<110,e;shot('selector-content-width-'+str(variant))
  assert abs(selector_widths[0]-selector_widths[1])>0.5,selector_widths
  assert abs(selector_widths[0]-selector_widths[2])<0.5,selector_widths
  report['checks']['selectorContentWidths']=selector_widths
  click('liquid-popover-variant-2');page.wait_for_timeout(650);e=locate('liquid-popover-trigger');assert e['bounds']['width']==24 and e['bounds']['height']==24,e
  def menu_pixels():
   e=locate('liquid-popover-trigger');return {'fill':pixel(e['center']['x'],e['bounds']['y']+4),'border':pixel(e['center']['x'],e['bounds']['y'])}
  page.mouse.move(1090,1490);page.wait_for_timeout(550);quiet=menu_pixels();assert quiet['fill']==[246,245,241],quiet
  page.mouse.move(e['center']['x'],e['center']['y']);page.wait_for_timeout(350);assert max(pixel(e['center']['x'],e['center']['y']))<130,'missing three-dot icon';hovered=menu_pixels();assert hovered['fill']==quiet['fill'] and max(hovered['border'])<210,hovered;shot('menu-trigger-hover')
  page.mouse.down();page.wait_for_timeout(140);shot('menu-trigger-held');page.mouse.up();page.wait_for_timeout(700);assert card('popover')['open'];page.mouse.move(1090,1490);page.wait_for_timeout(350);opened=menu_pixels();assert not card('popover')['menuHover'] and min(opened['fill'])>252,opened;locate('liquid-popover-option-2');shot('menu-open')
  click('liquid-popover-trigger');page.wait_for_timeout(900);closed_hovered=menu_pixels();assert not card('popover')['open'] and card('popover')['menuHover'] and closed_hovered['fill']==quiet['fill'] and max(closed_hovered['border'])<210,closed_hovered;shot('menu-closed-hovered')
  page.mouse.move(1090,1490);page.wait_for_timeout(650);closed_quiet=menu_pixels();assert not card('popover')['menuHover'] and closed_quiet['fill']==[246,245,241],closed_quiet;shot('menu-closed-quiet')
  report['checks']['menuRevealStates']={'quiet':quiet,'hovered':hovered,'openedAway':opened,'closedHovered':closed_hovered,'closedAway':closed_quiet}
  click('liquid-popover-trigger');page.wait_for_timeout(500);page.keyboard.press('Escape');page.wait_for_timeout(800);assert not card('popover')['open']
  report['checks']['compactMenu']=True
  choose(2);flights=[]
  for mode in [0,1]:
   click('liquid-composer-variant-0');click('liquid-composer-departure-mode-'+str(mode));type_text('liquid-composer-editor','从'+('发送按钮' if mode==0 else '输入区')+'分离，向上飞起')
   before=card('composer')['departures']['emitted'];click('liquid-composer-send');samples=[]
   for index in range(18):
    page.wait_for_timeout(50);v=card('composer');samples.append(v['departures'])
    if index in [0,2,5,9]:shot('send-'+str(mode)+'-'+str(index))
   assert any(x['flights'] for x in samples),samples
   seen=[f for x in samples for f in x['flights']];assert seen[0]['origin']==('Button' if mode==0 else 'Composer'),seen[0]
   assert min(f['pose']['cy'] for f in seen)<130,seen
   page.wait_for_timeout(1500);v=card('composer');assert v['departures']['emitted']==before+1 and not v['departures']['flights'],v['departures']
   assert v['surfaces'][0]['particles']==48,v['surfaces'][0]
   assert v['surfaces'][0]['error'] is None and not v['surfaces'][0]['paintError']
   flights.append(samples)
  report['checks']['sendOrigins']=flights
  click('liquid-composer-variant-2');before=card('composer')['departures']['emitted'];click('liquid-composer-send');assert card('composer')['departures']['emitted']==before
  click('liquid-composer-variant-0');page.emulate_media(reduced_motion='reduce');type_text('liquid-composer-editor','静态提交');before=card('composer')['departures']['emitted'];click('liquid-composer-send');frame();assert card('composer')['departures']['emitted']==before;assert not card('composer')['departures']['flights']
  report['checks']['stopAndReducedMotionDoNotFly']=True
  assert report['checks']['quickTapVisible'],report['checks']
  assert not report['errors'],report['errors']
  (args.output/'result.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n');print(json.dumps({'checks':list(report['checks']),'errors':report['errors']},ensure_ascii=False))
 finally:
  if report['errors']:(args.output/'errors.json').write_text(json.dumps(report['errors'],ensure_ascii=False,indent=2))
  if page and not page.is_closed():
   page.screenshot(path=str(args.output/'last.png'))
   try:
    if page.evaluate('!!window.zorkStory'):(args.output/'last-state.json').write_text(json.dumps({'state':state(),'snapshot':snap()},ensure_ascii=False,indent=2))
   except Exception:pass
  (args.output/'diagnostics.json').write_text(json.dumps({'console':report['console'],'checks':report['checks'],'errors':report['errors']},ensure_ascii=False,indent=2))
  browser.close()

# /// script
# dependencies = ["playwright==1.58.0"]
# ///
"""Exercise the production business components through Playground's physical inputs."""
import argparse, hashlib, json
from pathlib import Path
from playwright.sync_api import sync_playwright
from playground_inputs import PlaygroundInputs
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--url',required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--wasm-artifact',type=Path,required=True)
a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True)
r={'passed':False,'errors':[],'checks':{}}
with sync_playwright() as pw:
 binary=Path.home()/'Library/Caches/ms-playwright/chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing'
 browser=pw.chromium.launch(headless=True,executable_path=str(binary),args=["--force-device-scale-factor=2"]);context=browser.new_context(viewport={'width':1000,'height':900},device_scale_factor=2);page=context.new_page()
 page.on('pageerror',lambda e:r['errors'].append(str(e)))
 def deliver(route):
  response=route.fetch();data=response.body();assert data==a.wasm_artifact.read_bytes();r['sha256']=hashlib.sha256(data).hexdigest();route.fulfill(response=response,body=data)
 page.route('**/pkg/zork_gui_web_bg.wasm.gz',deliver)
 try:
  page.goto(a.url+'?story=liquid-gallery&backend=webgl');page.wait_for_function('document.documentElement.dataset.ready === "true"',timeout=120000);context.set_offline(True)
  snapshot=lambda:page.evaluate('JSON.parse(zorkStory.snapshot())')
  state=lambda:page.evaluate('JSON.parse(zorkStory.story_state())')
  r['actualViewport']=snapshot()['viewport'];assert r['actualViewport']=={'width':1000,'height':900},r['actualViewport']
  inp=PlaygroundInputs(page,snapshot)
  value=lambda:state()['businessExample']['value']
  def wait_value(expression):page.wait_for_function('(expr)=>{const v=JSON.parse(zorkStory.story_state()).businessExample?.value;return v && Function("v", "return "+expr)(v)}',arg=expression)
  def dismiss():
   for _ in range(4):
    page.keyboard.press('Escape');page.wait_for_timeout(300)
    if not any(e['visible'] and (e['id'].endswith(('-dialog','-modal')) or e['id']=='drive-preview') for e in snapshot()['elements']):break
   page.wait_for_timeout(800)
  def choose(family):
   dismiss();inp.click('liquid-business-'+family);page.wait_for_timeout(1000)
  def shot(name):page.screenshot(path=str(a.output/(name+'.png')))
  def fill(id,text):
   inp.click(id);page.keyboard.press('ControlOrMeta+a');page.keyboard.press('Backspace')
   cdp=context.new_cdp_session(page)
   if text:
    count=len(text.encode('utf-16-le'))//2;cdp.send('Input.imeSetComposition',{'text':text,'selectionStart':count,'selectionEnd':count});cdp.send('Input.insertText',{'text':text})
   cdp.detach();inp.frame()
  choose('composer')
  assert not value()['composer']['capabilities']['enabled']
  fill('liquid-composer-editor','共享组件，中文输入与发送。');wait_value('v.composer.text === "共享组件，中文输入与发送。"')
  assert inp.locate('liquid-composer-send')['enabled']
  page.keyboard.press('Enter');wait_value('v.composer.text === "" && v.composer.events.some(e=>e.startsWith("send:"))')
  assert not inp.locate('liquid-composer-send')['enabled'];shot('composer-send');r['checks']['coreEditAndAcceptedSend']=True
  inp.click('liquid-composer-variant-2');wait_value('v.composer.capabilities.stop')
  fill('liquid-composer-editor','运行中的追加消息');page.keyboard.press('Enter');wait_value('v.composer.text === "" && v.composer.capabilities.stop')
  inp.click('liquid-composer-send');wait_value('!v.composer.capabilities.stop && v.composer.events.includes("stop")');r['checks']['enterAndStopRemainDistinct']=True
  idle_bounds=inp.locate('liquid-composer-send')['bounds'];inp.click('liquid-composer-variant-3');wait_value('v.variant === 3')
  busy=inp.locate('liquid-composer-send');assert not busy['enabled'];assert busy['bounds']['width']==idle_bounds['width'] and busy['bounds']['height']==idle_bounds['height'];shot('composer-busy')
  inp.click('liquid-composer-variant-4');wait_value('!v.composer.capabilities.editable');prior=value()['composer']['text'];inp.click('liquid-composer-editor');page.keyboard.type('blocked');assert value()['composer']['text']==prior;r['checks']['busyAndReadonly']=True
  inp.click('liquid-composer-variant-0');inp.click('liquid-composer-attach');wait_value('v.composer.files.length === 1');file_id=value()['composer']['files'][0]['id']
  inp.click('liquid-composer-fan-toggle');wait_value('v.fan > .98');shot('composer-fan')
  inp.click('liquid-composer-file-'+str(file_id));page.wait_for_function('JSON.parse(zorkStory.snapshot()).elements.some(e=>e.id==="drive-preview"&&e.visible)');shot('composer-file-viewer')
  dismiss();inp.click('liquid-composer-remove-'+str(file_id));wait_value('v.composer.files.length === 0');r['checks']['sharedFanAndFullViewer']=True
  inp.click('liquid-composer-variant-6');wait_value('v.composer.members.length === 3');page.wait_for_timeout(900)
  member=inp.locate('liquid-composer-member-fox');page.mouse.move(member['center']['x'],member['center']['y']);page.wait_for_function('JSON.parse(zorkStory.snapshot()).elements.some(e=>e.id==="detail-tooltip-presence-fox"&&e.visible)');shot('composer-member');page.mouse.move(980,60);page.wait_for_timeout(1000);r['checks']['sharedMemberPreview']=True
  for variant in [1,5,7]:
   inp.click('liquid-composer-variant-'+str(variant));wait_value('v.variant === '+str(variant));page.wait_for_timeout(700)
   assert inp.locate('liquid-composer-editor')['visible'];assert inp.locate('liquid-composer-send')['visible'];shot('composer-variant-'+str(variant))
  r['checks']['allEightComposerVariants']=True
  choose('device');inp.click('device-rename');fill('device-name-input','');inp.click('device-name-save');page.wait_for_timeout(500);assert inp.element('device-rename-dialog')
  fill('device-name-input','组件验证设备');inp.click('device-name-save');inp.wait_for_dismissal('device-rename-dialog');r['checks']['coreDeviceNameValidation']=True;shot('device-renamed')
  choose('mesh');inp.click('mesh-new-peer');fill('mesh-peer-name','测试设备');fill('mesh-peer-origin','');inp.click('mesh-add-peer');page.wait_for_timeout(500);assert inp.element('mesh-peer-dialog')
  fill('mesh-peer-origin','key:'+'y'*52);inp.click('mesh-add-peer');inp.wait_for_dismissal('mesh-peer-dialog');r['checks']['corePeerValidation']=True;shot('peer-added')
  choose('resources');inp.click('business-state-resources-error');page.wait_for_timeout(600)
  refresh=next(e for e in snapshot()['elements'] if e['visible'] and e['role']=='button' and e['label']=='刷新');page.mouse.click(refresh['center']['x'],refresh['center']['y']);page.wait_for_function('JSON.parse(zorkStory.snapshot()).elements.some(e=>e.id==="resource-row-0"&&e.visible)');r['checks']['mockRefreshFeedsSameComponent']=True
  choose('history');row=inp.locate('history-record-0');page.mouse.click(row['center']['x'],row['center']['y']);page.wait_for_timeout(1000);shot('history-row-action');r['checks']['historyActionUsesSharedView']=True
  assert not r['errors'],r['errors'];r['passed']=True;print(json.dumps(r['checks'],ensure_ascii=False),flush=True)
 finally:
  page.screenshot(path=str(a.output/'last.png'));(a.output/'last-state.json').write_text(json.dumps(state(),ensure_ascii=False,indent=2));(a.output/'report.json').write_text(json.dumps(r,ensure_ascii=False,indent=2));browser.close()

# /// script
# dependencies = ["playwright==1.58.0"]
# ///
"""Real browser input against GPUI/WASM. After loading, all checks run offline."""
import argparse,json,time
from pathlib import Path
from playwright.sync_api import sync_playwright
ROOT=Path(__file__).resolve().parents[2]

def main():
 p=argparse.ArgumentParser();p.add_argument('--url',default='http://127.0.0.1:49186/design/components/web/index.html');p.add_argument('--output',type=Path,default=ROOT/'artifacts/storybook/web-checks');p.add_argument('--profiles-only',action='store_true');p.add_argument('--input-only',action='store_true');p.add_argument('--timezone',default='Asia/Shanghai');p.add_argument('--backend',choices=['auto','webgl'],default='auto');args=p.parse_args();args.output.mkdir(parents=True,exist_ok=True)
 with sync_playwright() as pw:
  binary=Path.home()/'Library/Caches/ms-playwright/chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing'
  b=pw.chromium.launch(headless=True,executable_path=str(binary));context=b.new_context(viewport={'width':560,'height':360},timezone_id=args.timezone);page=context.new_page();errors=[];requests=[]
  page.on('pageerror',lambda e:errors.append(str(e)));page.on('request',lambda r:requests.append(r.url))
  page.goto(args.url+'?story=dropdown-closed&backend='+args.backend)
  page.wait_for_function('document.documentElement.dataset.ready==="true"',timeout=60000)
  assert any('.wasm.gz' in u for u in requests),'No WASM module was loaded'
  context.set_offline(True)
  def snapshot():return page.evaluate('JSON.parse(window.zorkStory.snapshot())')
  def state():return page.evaluate('JSON.parse(window.zorkStory.story_state())')
  def element(id):return next(e for e in snapshot()['elements'] if e['id']==id and e['visible'])
  def wait_element(id):return page.wait_for_function('(id)=>JSON.parse(window.zorkStory.snapshot()).elements.find(e=>e.id===id&&e.visible)||null',arg=id,timeout=10000).json_value()
  def click(id):
   e=wait_element(id);page.mouse.click(e['center']['x'],e['center']['y']);page.evaluate('()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))')
  ime=context.new_cdp_session(page)
  def type_ime(text):
   end=len(text.encode('utf-16-le'))//2
   ime.send('Input.imeSetComposition',{'text':text,'selectionStart':end,'selectionEnd':end})
   ime.send('Input.insertText',{'text':text})
  def choose(id,w=560,h=360):
   page.mouse.move(0,0);page.set_viewport_size({'width':w,'height':h});revision=snapshot()['revision'];page.evaluate('(id)=>window.zorkStory.select_story(id)',id)
   page.wait_for_function('([id,rev])=>{const s=JSON.parse(window.zorkStory.story_state());return s.id===id&&s.pending_actions===0&&JSON.parse(window.zorkStory.snapshot()).revision>rev}',arg=[id,revision])
   assert state()['action_error'] is None,state()
  click('story-select');click('story-option-1');assert state()['selected']==1 and not state()['open'];page.wait_for_function('JSON.parse(window.zorkStory.snapshot()).elements.some(e=>e.id==="story-select"&&e.label==="Anthropic")')
  choose('field-empty');click('story-field');type_ime('产品连接 ✓');page.wait_for_function('JSON.parse(window.zorkStory.story_state()).text==="产品连接 ✓"');page.screenshot(path=str(args.output/'input.png'))
  mac=page.evaluate('/Mac|iPhone|iPad/.test(navigator.platform)');primary='Meta' if mac else 'Control'
  document_start='Meta+ArrowUp' if mac else 'Control+Home'
  document_end='Meta+ArrowDown' if mac else 'Control+End'
  line_start='Meta+Shift+ArrowLeft' if mac else 'Shift+Home'
  line_end='Meta+Shift+ArrowRight' if mac else 'Shift+End'
  value='first\nsecond line\nlast';keyboard_checks=[]
  def expect_text(expected):
   try:page.wait_for_function('(value)=>JSON.parse(window.zorkStory.story_state()).text===value',arg=expected,timeout=5000)
   except Exception:
    page.screenshot(path=str(args.output/'keyboard-failure.png'));actual=state()
    (args.output/'keyboard-failure.json').write_text(json.dumps({'expected':expected,'actual':actual},ensure_ascii=False,indent=2))
    raise AssertionError({'expected':expected,'actual':actual})
  for shortcut,expected in [(line_start,'first\nXYond line\nlast'),(line_end,'first\nsecXY\nlast'),('Shift+'+document_start,'XYond line\nlast'),('Shift+'+document_end,'first\nsecXY')]:
   page.keyboard.press('ControlOrMeta+a');type_ime(value)
   expect_text(value)
   for key in [document_start,'ArrowDown','ArrowRight','ArrowRight','ArrowRight',shortcut]:page.keyboard.press(key)
   page.screenshot(path=str(args.output/f'keyboard-selection-{len(keyboard_checks)}.png'))
   page.keyboard.type('XY');expect_text(expected)
   page.keyboard.press('ControlOrMeta+z');expect_text(value)
   page.keyboard.press('ControlOrMeta+Shift+z');expect_text(expected)
   keyboard_checks.append(shortcut+' / undo / redo')
  context.grant_permissions(['clipboard-read','clipboard-write'])
  for paste_key in [primary+'+v',primary+'+Shift+v']:
   page.evaluate('()=>navigator.clipboard.writeText("粘贴 👩‍💻")')
   page.keyboard.press('ControlOrMeta+a');page.keyboard.press(paste_key)
   page.wait_for_function('JSON.parse(window.zorkStory.story_state()).text==="粘贴 👩‍💻"')
   keyboard_checks.append(paste_key)
  page.screenshot(path=str(args.output/'keyboard.png'))
  if args.input_only:
   assert not errors,errors
   (args.output/'result.json').write_text(json.dumps({'renderer':'GPUI WASM / '+args.backend,'platform':'Mac' if mac else 'PC','keyboard_checks':keyboard_checks,'ime':'Chinese commit','errors':errors},ensure_ascii=False,indent=2)+'\n')
   b.close();print('PASS Web keyboard navigation, selection, undo/redo, clipboard shortcuts and IME');return
  choose('markdown-table');table_cells=[e for e in snapshot()['elements'] if e['id'].endswith('-selection') and e['visible']];assert len(table_cells)>=6
  for e in table_cells:page.mouse.click(e['center']['x'],e['center']['y'])
  target=next(e for e in table_cells if e['label']=='已检查');rect=target['bounds'];page.mouse.move(rect['x']+1,target['center']['y']);page.mouse.down();page.mouse.move(rect['x']+rect['width']-1,target['center']['y'],steps=8);page.mouse.up();page.wait_for_function('JSON.parse(window.zorkStory.story_state()).quote?.includes("已检查")');page.screenshot(path=str(args.output/'table-selection.png'))
  choose('connection-create-compact',900,600);wait_element('profile-create-dialog');click('profile-id');type_ime('浏览器组件');page.wait_for_function('JSON.parse(window.zorkStory.story_state()).connection_name==="浏览器组件"');page.keyboard.press('Escape');page.wait_for_function('!JSON.parse(window.zorkStory.snapshot()).elements.some(e=>e.id==="profile-create-dialog")')
  choose('connection-list-compact',900,600);assert '剩余 72%' in wait_element('profile-quota-summary-fixture')['label']
  row=wait_element('profile-detail-fixture');avatar=wait_element('profile-avatar-fixture')
  assert avatar['bounds']['x']-row['bounds']['x']>=12,'hover background touches avatar'
  page.mouse.move(row['center']['x'],row['center']['y']);page.wait_for_timeout(180);page.screenshot(path=str(args.output/'connection-hover-compact.png'))
  choose('model-detail-compact',900,600);assert '剩余 72%' in wait_element('profile-quota')['label']
  assert abs(wait_element('profile-detail-dialog')['center']['x']-450)<1
  assert abs(wait_element('profile-rename')['center']['y']-wait_element('profile-detail-dialog-close')['center']['y'])<1
  assert wait_element('profile-model-discover')['label']=='更新模型'
  quota_label=wait_element('profile-quota-updated')['label'];reset_label=wait_element('profile-quota-window-0')['label']
  assert quota_label.startswith('额度更新于 ') and 'UTC' not in quota_label
  assert ('后重置' in reset_label or '即将重置' in reset_label) and 'UTC' not in reset_label
  assert abs(element('profile-quota-updated')['center']['y']-element('profile-quota-refresh')['center']['y'])<12
  assert '上下文 32K / 输出 4.096K' in wait_element('model-limits-fixture-model')['label']
  click('profile-model-add');wait_element('model-editor-dialog');click('profile-model');type_ime('copied-model')
  click('model-copy-select');click('model-copy-0')
  page.wait_for_function('!JSON.parse(window.zorkStory.snapshot()).elements.some(e=>e.id==="model-copy-select-menu")')
  copied=state();assert copied['model_id']=='copied-model' and copied['context_window']=='32K' and copied['max_output_tokens']=='4.096K',copied
  page.screenshot(path=str(args.output/'model-copy-menu-closed.png'))
  click('model-editor-dialog-close');wait_element('profile-detail-dialog')
  click('profile-quota-refresh');page.wait_for_function('JSON.parse(window.zorkStory.snapshot()).elements.some(e=>e.id==="profile-quota"&&e.label.includes("71%"))')
  assert '刚刚' in wait_element('profile-quota-updated')['label']
  assert wait_element('profile-quota-refresh')['bounds']['width']==32
  click('profile-rename');wait_element('profile-name')
  assert abs(element('profile-name')['center']['y']-element('profile-detail-dialog-close')['center']['y'])<1
  assert abs(element('profile-name-save')['center']['y']-element('profile-detail-dialog-close')['center']['y'])<1
  assert not any(e['id']=='profile-rename' for e in snapshot()['elements'])
  page.screenshot(path=str(args.output/'inline-name-edit.png'))
  click('profile-name');page.keyboard.press('ControlOrMeta+a');type_ime('我的主力模型');click('profile-name-save')
  try:page.wait_for_function('JSON.parse(window.zorkStory.snapshot()).elements.some(e=>e.id==="profile-detail-dialog"&&e.label==="我的主力模型")',timeout=5000)
  except Exception:
   page.screenshot(path=str(args.output/'rename-failure.png'));(args.output/'rename-failure.json').write_text(json.dumps(snapshot(),ensure_ascii=False,indent=2));raise
  page.screenshot(path=str(args.output/'renamed-detail.png'));(args.output/'renamed-detail.json').write_text(json.dumps(snapshot(),ensure_ascii=False,indent=2))
  click('profile-rename');click('profile-name');page.keyboard.press('ControlOrMeta+a');type_ime('未保存的名字');page.keyboard.press('Escape')
  wait_element('profile-rename');assert element('profile-detail-dialog')['label']=='我的主力模型'
  click('profile-rename');click('profile-name');page.keyboard.press('ControlOrMeta+a');type_ime('我的主力模型');page.keyboard.press('Enter')
  wait_element('profile-rename');assert element('profile-detail-dialog')['label']=='我的主力模型'
  page.keyboard.press('Escape');assert wait_element('profile-detail-fixture')['label']=='我的主力模型'
  # An old snapshot refreshes once on opening; re-opening the fresh result does not.
  choose('connection-list-compact',900,600);click('profile-detail-fixture')
  page.wait_for_function('JSON.parse(window.zorkStory.snapshot()).elements.some(e=>e.id==="profile-quota"&&e.label.includes("71%"))')
  page.keyboard.press('Escape');click('profile-detail-fixture');wait_element('profile-detail-dialog');page.wait_for_timeout(180)
  assert '71%' in wait_element('profile-quota')['label']
  click('model-enabled-fixture-model');page.wait_for_function('JSON.parse(window.zorkStory.snapshot()).elements.some(e=>e.id==="model-enabled-fixture-model"&&e.label.includes("关闭"))')
  assert not any(e['id']=='model-editor-dialog' for e in snapshot()['elements'])
  click('profile-model-discover');wait_element('model-edit-updated-model')
  assert '新增 1' in wait_element('profile-model-update-result')['label']
  assert '关闭' in wait_element('model-enabled-fixture-model')['label']
  click('profile-model-discover');page.wait_for_function('JSON.parse(window.zorkStory.snapshot()).elements.some(e=>e.id==="profile-model-update-result"&&e.label==="已是最新")')
  assert len([e for e in snapshot()['elements'] if e['id']=='model-edit-updated-model'])==1
  toggle=wait_element('model-enabled-fixture-model')['bounds'];models=wait_element('profile-models')['bounds']
  assert abs(toggle['x']+toggle['width']-models['x']-models['width'])<2,'model switch must align with the list right edge'
  page.screenshot(path=str(args.output/'models-updated-disabled.png'))
  page.keyboard.press('Escape');click('profile-detail-fixture');wait_element('model-enabled-fixture-model')
  assert '关闭' in element('model-enabled-fixture-model')['label']
  click('model-enabled-fixture-model');page.wait_for_function('JSON.parse(window.zorkStory.snapshot()).elements.some(e=>e.id==="model-enabled-fixture-model"&&e.label.includes("开启"))')
  catalog=page.evaluate('JSON.parse(window.zorkStory.catalog())');checked=[]
  for story in catalog:
   if story['family']=='conversation':continue
   if args.profiles_only and story['family'] not in ['connection','model']:continue
   choose(story['id'],int(story['width']),int(story['height']));wait_element(story['target'])
   page.wait_for_timeout(180) # Decoding embedded SVGs and GPU uploads finish asynchronously.
   current=snapshot();assert not errors,errors
   for e in current['elements']:
    if e['id'].endswith('-footer'):
     assert e['bounds']==e['visible_bounds'],(story['id'],'clipped modal actions')
    if e['id'].endswith('-menu'):
     trigger=next((x for x in current['elements'] if x['id']==e['id'][:-5]),None)
     if trigger:assert abs(e['bounds']['width']-trigger['bounds']['width']-8)<1,(story['id'],'menu width mismatch')
   page.screenshot(path=str(args.output/(story['id']+'.png')))
   (args.output/(story['id']+'.json')).write_text(json.dumps(current,ensure_ascii=False,indent=2)+'\n')
   checked.append(story['id']);print('PASS Web',story['id'],flush=True)
  result={'scope':'profiles' if args.profiles_only else 'gallery','renderer':'GPUI WASM / '+args.backend,'component_package':'zork-ui','offline_after_module_load':True,'real_browser_input':['dropdown selection','Chinese text input','table clicks and drag selection','modal input and Escape','quota display and refresh'],'stories':checked,'errors':errors,'requests_before_offline':requests}
  (args.output/'result.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n');b.close()
if __name__=='__main__':main()

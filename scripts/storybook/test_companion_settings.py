# /// script
# dependencies = ["playwright==1.58.0"]
# ///
"""Device-name and companion editing through real GPUI/WASM browser input."""
from pathlib import Path
import json
from playwright.sync_api import sync_playwright
ROOT=Path(__file__).resolve().parents[2]
out=ROOT/'artifacts/storybook/companion-settings';out.mkdir(parents=True,exist_ok=True)
with sync_playwright() as p:
 b=p.chromium.launch(headless=True,executable_path=str(Path.home()/'Library/Caches/ms-playwright/chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing'))
 page=b.new_page();errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
 for width,height,size in [(900,600,'compact'),(1280,800,'wide')]:
  page.set_viewport_size({'width':width,'height':height})
  page.goto('http://127.0.0.1:49186/design/components/web/index.html?story=device-running-'+size)
  page.wait_for_function('document.documentElement.dataset.ready==="true"',timeout=60000)
  def elements():return page.evaluate('JSON.parse(zorkStory.snapshot()).elements')
  def element(id):return next(e for e in elements() if e['id']==id and e['visible'])
  def wait(id):page.wait_for_function('(id)=>JSON.parse(zorkStory.snapshot()).elements.some(e=>e.id===id&&e.visible)',arg=id);return element(id)
  def click(id):
   e=wait(id);page.mouse.click(e['center']['x'],e['center']['y']);page.wait_for_timeout(100)
  ime=page.context.new_cdp_session(page)
  def fill(id,text):
   click(id);page.keyboard.press('ControlOrMeta+A');page.keyboard.press('Backspace')
   if text:
    end=len(text.encode('utf-16-le'))//2
    ime.send('Input.imeSetComposition',{'text':text,'selectionStart':end,'selectionEnd':end})
    ime.send('Input.insertText',{'text':text})
   page.wait_for_timeout(100)
  def select(story):
   page.mouse.move(0,0);page.evaluate('(id)=>zorkStory.select_story(id)',story+'-'+size);page.wait_for_timeout(250)
  click('device-rename');wait('device-rename-dialog');fill('device-name-input','');click('device-name-save');assert element('device-rename-dialog')
  fill('device-name-input','Example-Laptop');click('device-name-save');assert not any(e['id']=='device-rename-dialog' for e in elements())
  page.mouse.move(width-10,height-10);page.screenshot(path=str(out/('device-'+size+'-gpui.png')))
  click('device-rename');fill('device-name-input','未保存');page.keyboard.press('Escape');page.wait_for_timeout(150);assert not any(e['id']=='device-rename-dialog' for e in elements())
  click('device-rename');page.screenshot(path=str(out/('rename-reopened-'+size+'-gpui.png')));page.keyboard.press('Escape');page.wait_for_timeout(150)
  select('agent-list');click('agent-settings-leader');wait('agent-editor-dialog');assert not any(e['id']=='agent-model-save' for e in elements())
  click('agent-avatar-bear');click('agent-avatar-save');page.wait_for_function('!JSON.parse(zorkStory.snapshot()).elements.some(e=>e.id==="agent-editor-dialog")')
  page.screenshot(path=str(out/('agents-'+size+'-gpui.png')))
  click('agent-settings-leader');page.screenshot(path=str(out/('agent-editor-'+size+'-gpui.png')));page.keyboard.press('Escape')
  select('connection-list');page.screenshot(path=str(out/('connections-'+size+'-gpui.png')))
 assert not errors,errors
 (out/'result.json').write_text(json.dumps({'viewports':[[900,600],[1280,800]],'device_name_validation':True,'rename_save_cancel':True,'single_agent_save':True,'errors':errors},indent=2))
 print('PASS device rename validation/save/cancel and unified companion save in compact and wide GPUI Web');b.close()

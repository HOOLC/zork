# /// script
# dependencies = ["playwright==1.58.0", "pillow==11.3.0"]
# ///
"""Physical browser input and rendered geometry for the corrected controls."""
import argparse,io,json
from pathlib import Path
from PIL import Image
from playwright.sync_api import sync_playwright
parser=argparse.ArgumentParser();parser.add_argument('--base',default='http://127.0.0.1:49186/design/');args=parser.parse_args()
ROOT=Path(__file__).resolve().parents[2];out=ROOT/'artifacts/storybook/react-control-checks';out.mkdir(parents=True,exist_ok=True)
with sync_playwright() as pw:
 binary=Path.home()/'Library/Caches/ms-playwright/chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing'
 browser=pw.chromium.launch(headless=True,executable_path=str(binary));page=browser.new_page(viewport={'width':560,'height':360});errors=[];page.on('pageerror',lambda e:errors.append(str(e).splitlines()[0]))
 page.goto(args.base+'components/web/index.html?story=button-disabled');page.wait_for_function('document.documentElement.dataset.ready==="true"',timeout=60000)
 def state():return page.evaluate('JSON.parse(zorkStory.story_state())')
 def snapshot():return page.evaluate('JSON.parse(zorkStory.snapshot())')
 def el(id):return next(e for e in snapshot()['elements'] if e['id']==id and e['visible'])
 def click(id):
  e=el(id);page.mouse.click(e['center']['x'],e['center']['y']);page.wait_for_timeout(100)
 def choose(id):
  page.mouse.move(0,0);page.evaluate('(id)=>zorkStory.select_story(id)',id);page.wait_for_function('(id)=>JSON.parse(zorkStory.story_state()).id===id&&!JSON.parse(zorkStory.story_state()).pending_actions',arg=id);page.wait_for_timeout(180)
 disabled=el('story-button');assert disabled['bounds']['height']==32;assert not disabled['enabled'];click('story-button');assert state()['clicks']==0
 im=Image.open(io.BytesIO(page.screenshot()));r=disabled['bounds'];rgb=im.getpixel((int(r['x']+8),int(r['y']+8)))[:3];assert rgb==(167,169,170),rgb
 heights={}
 for story,id in [('button-primary','story-button'),('field-value','story-field'),('dropdown-closed','story-select'),('choice-selected','story-choice-0'),('avatar-picker-selected','story-avatar-cat'),('navigation-default','story-nav'),('switch-off','story-switch')]:
  choose(story);r=el(id)['bounds'];heights[story]=r['height'];assert abs(r['height']-32)<.01,(story,r);page.screenshot(path=str(out/(story+'-32px.png')))
 choose('field-error');assert any(e['label']=='填写连接名称。' and e['visible'] for e in snapshot()['elements']);click('story-field');page.keyboard.type('valid-name');page.wait_for_timeout(150);assert not any(e['label']=='填写连接名称。' and e['visible'] for e in snapshot()['elements'])
 choose('button-primary');r=el('story-button')['bounds'];page.mouse.move(r['x']+r['width']/2,r['y']+r['height']/2);page.wait_for_timeout(120);hover_image=Image.open(io.BytesIO(page.screenshot()));hover_rgb=hover_image.getpixel((int(r['x']+8),int(r['y']+16)))[:3];assert hover_rgb==(65,70,76),hover_rgb
 for switch_state in ['off','on','disabled-off','disabled-on','focus']:
  choose('switch-'+switch_state);assert el('story-switch')['bounds']['height']==32;before=state()['checked'];click('story-switch')
  if switch_state.startswith('disabled'):assert state()['checked']==before
  else:
   assert state()['checked']!=before
   page.keyboard.press('Space');page.wait_for_timeout(120);assert state()['checked']==before
   page.keyboard.press('Enter');page.wait_for_timeout(120);assert state()['checked']!=before
  page.screenshot(path=str(out/('switch-'+switch_state+'.png')))
 choose('dropdown-open');trigger=el('story-select')['bounds'];menu=el('story-select-menu')['bounds'];row0=el('story-option-0')['bounds'];row1=el('story-option-1')['bounds'];assert abs(menu['y']-trigger['y']-trigger['height']-6)<.1;assert abs(row1['y']-row0['y']-row0['height']-2)<.1
 assert abs(menu['width']-trigger['width']-8)<.1;assert abs(menu['x']-trigger['x']+4)<.1
 click('story-option-1');assert state()['selected']==1 and not state()['open']
 choose('field-secret');click('story-field');page.keyboard.press('ControlOrMeta+a');ime=page.context.new_cdp_session(page);value='a密👩‍💻é';end=len(value.encode('utf-16-le'))//2;ime.send('Input.imeSetComposition',{'text':value,'selectionStart':end,'selectionEnd':end});ime.send('Input.insertText',{'text':value});page.wait_for_timeout(150);page.screenshot(path=str(out/'password-unicode.png'));assert state()['text']=='[redacted]'
 page.keyboard.press('End');page.keyboard.press('Backspace');page.wait_for_timeout(100);page.screenshot(path=str(out/'password-after-backspace.png'))
 page.keyboard.press('ControlOrMeta+a');page.keyboard.press('Backspace');page.wait_for_timeout(100);page.screenshot(path=str(out/'password-cleared.png'))
 assert not errors,errors
 (out/'result.json').write_text(json.dumps({'control_heights':heights,'disabled_rgb':rgb,'disabled_clicks':0,'menu_gap':6,'row_gap':2,'password_input':'Latin + CJK + ZWJ emoji + combining mark; backspace and clear','errors':errors},indent=2)+'\n');browser.close();print('PASS disabled physical click/color, dropdown gaps and selection, masked Unicode input/backspace/clear')

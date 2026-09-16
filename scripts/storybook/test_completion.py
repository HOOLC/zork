# /// script
# dependencies = ["playwright==1.58.0"]
# ///
"""Exercise completed design flows with physical keyboard/pointer input."""
import argparse,json
from pathlib import Path
from playwright.sync_api import sync_playwright,expect
ROOT=Path(__file__).resolve().parents[2]
p=argparse.ArgumentParser();p.add_argument('--base',default='http://127.0.0.1:49186/design/');p.add_argument('--reference-only',action='store_true');a=p.parse_args()
out=ROOT/'artifacts/storybook/design-completion/interactions';out.mkdir(parents=True,exist_ok=True)
with sync_playwright() as pw:
 binary=Path.home()/'Library/Caches/ms-playwright/chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing'
 browser=pw.chromium.launch(headless=True,executable_path=str(binary));context=browser.new_context(viewport={'width':900,'height':600},permissions=['clipboard-read','clipboard-write']);page=context.new_page();errors=[];checks=[];page.on('pageerror',lambda e:errors.append(str(e)))
 def mark(name):checks.append(name);print('PASS',name,flush=True)
 def ref(story):
  page.goto(a.base+'reference.html?story='+story);page.wait_for_function('(id)=>document.documentElement.dataset.referenceReady===id',arg=story)
 for size in ['compact','wide']:
  page.set_viewport_size({'width':900 if size=='compact' else 1280,'height':600 if size=='compact' else 800})
  ref('connection-provider-'+size);expect(page.get_by_role('option').first).to_be_visible();assert page.get_by_role('option').count()>=3
  for option in page.get_by_role('option').all():assert option.bounding_box()['width']>100
  page.keyboard.press('Escape');expect(page.locator('dialog')).to_be_visible();expect(page.get_by_role('option')).to_have_count(0)
  page.keyboard.press('Escape');expect(page.locator('dialog')).to_have_count(0);mark('reference-menu-escape-'+size)
  ref('model-create-'+size);page.get_by_role('button',name='保存模型',exact=True).click();expect(page.get_by_role('alert')).to_be_visible()
  page.get_by_label('模型 ID',exact=True).fill('design-check-model');page.get_by_label('上下文 token 上限').fill('100');page.get_by_label('输出 token 上限').fill('200');page.get_by_role('button',name='保存模型',exact=True).click();expect(page.get_by_role('alert')).to_be_visible()
  page.get_by_label('输出 token 上限').fill('50');page.get_by_role('button',name='保存模型',exact=True).click();expect(page.get_by_text('design-check-model',exact=True)).to_be_visible();mark('reference-model-validation-save-'+size)
 ref('client-signed-out-compact');page.get_by_role('button',name='登录账号').click();page.get_by_role('button',name='取消',exact=True).click();page.wait_for_timeout(500);expect(page.get_by_role('button',name='登录账号')).to_be_visible()
 page.get_by_role('button',name='登录账号').click();expect(page.get_by_role('button',name='退出账号')).to_be_visible();page.get_by_role('button',name='退出账号').click();expect(page.get_by_role('button',name='登录账号')).to_be_visible();mark('reference-account-cancel-login-logout')
 ref('device-running-compact');foreground=page.get_by_role('button',name='随客户端',exact=True);background=page.get_by_role('button',name='后台运行',exact=True);login=page.get_by_role('switch',name='登录系统后自动启动');foreground.click();expect(login).to_have_count(0);foreground.press('ArrowRight');expect(login).to_be_enabled();login.click();expect(login).to_have_attribute('aria-checked','true');foreground.click();expect(login).to_have_count(0);background.click();expect(login).to_have_attribute('aria-checked','false');expect(page.get_by_role('button',name='设备连接',exact=True)).to_have_count(0);mark('reference-device-mode-dependency-no-duplicate-entry')
 ref('mesh-connected-compact');page.get_by_role('button',name='手动连接',exact=True).click();page.get_by_role('button',name='保存配对').click();expect(page.get_by_role('alert')).to_be_visible();page.get_by_label('设备名称',exact=True).fill('mini3');page.get_by_label('设备身份',exact=True).fill('key:design-test');page.get_by_role('switch',name='客户端权限').click();page.get_by_role('button',name='保存配对').click();expect(page.get_by_text('mini3',exact=True)).to_be_visible();page.get_by_role('button',name='移除',exact=True).last.click();expect(page.get_by_text('mini3',exact=True)).to_have_count(0);mark('reference-mesh-pair-remove')
 ref('enrollment-command-compact');page.get_by_role('button',name='复制命令').click();assert 'zork mesh join' in page.evaluate('navigator.clipboard.readText()');page.get_by_role('button',name='撤销命令').click();expect(page.get_by_text('加入命令已撤销。')).to_be_visible();page.get_by_role('button',name='重新生成').click();expect(page.get_by_label('加入命令',exact=True)).to_be_visible();mark('reference-enrollment-copy-revoke-regenerate')
 ref('conversation-history-wide');page.locator('.ref-history-row').nth(1).click();expect(page.locator('.ref-history-details')).to_be_visible();before=page.locator('.ref-history-axis').inner_text();page.get_by_role('button',name='放大',exact=True).click();assert before!=page.locator('.ref-history-axis').inner_text();page.get_by_role('button',name='全部',exact=True).click();assert before==page.locator('.ref-history-axis').inner_text();page.get_by_role('button',name='关闭执行历史').click();expect(page.locator('.ref-history')).to_have_count(0);mark('reference-history-expand-zoom-fit-close')
 page.goto(a.base+'reference.html?mode=product&story=conversation-messages-wide');page.get_by_role('button',name='设置',exact=True).click();expect(page.get_by_role('heading',name='客户端设置')).to_be_visible();page.get_by_role('button',name='返回对话').click();expect(page.locator('.ref-conversation-root')).to_be_visible();mark('reference-product-navigation')
 if not a.reference_only:
  page.set_viewport_size({'width':700,'height':700});page.goto(a.base+'components/web/index.html?story=comments-compose');page.wait_for_function('document.documentElement.dataset.ready==="true"',timeout=60000)
  def state():return page.evaluate('JSON.parse(zorkStory.story_state())')
  def elements():return page.evaluate('JSON.parse(zorkStory.snapshot()).elements')
  def el(suffix):return next(e for e in elements() if e['id'].endswith(suffix) and e['visible'])
  def has(suffix):return any(e['id'].endswith(suffix) and e['visible'] for e in elements())
  def click(suffix):
   e=el(suffix);page.mouse.click(e['center']['x'],e['center']['y']);page.wait_for_timeout(150)
  def choose(story):
   page.mouse.move(0,0);page.evaluate('(id)=>zorkStory.select_story(id)',story);page.wait_for_function('(id)=>JSON.parse(zorkStory.story_state()).id===id&&!JSON.parse(zorkStory.story_state()).pending_actions',arg=story);page.wait_for_timeout(200)
  click('comment-input');page.keyboard.type('Review comment');page.keyboard.press('Enter');page.wait_for_timeout(150);assert has('comment-edit-demo-1') and not has('selection-comment-popover');click('comment-edit-demo-1');page.keyboard.press('ControlOrMeta+a');page.keyboard.type('Updated comment');click('comment-queue-add');assert any('Updated comment' in e.get('label','') for e in elements()) or not has('selection-comment-popover');click('comment-remove-demo-1');assert not has('comment-edit-demo-1');mark('gpui-comments-enter-edit-remove')
  choose('history-collapsed');click('history-collapsed-row-1');page.screenshot(path=str(out/'history-expanded.png'));choose('history-error');click('history-error-retry');assert has('history-error-row-0');mark('gpui-history-expand-retry')
  choose('avatar-picker-selected');before=state()['selected'];targets=[e for e in elements() if 'story-avatar' in e['id'] and e.get('role')=='option'];assert targets,elements();e=targets[0];page.mouse.click(e['center']['x'],e['center']['y']);page.wait_for_timeout(120);assert state()['selected']!=before;mark('gpui-avatar-selection')
  choose('attachment-file');click('story-attachment');assert state()['open'];click('attachment-close');assert not state()['open'];mark('gpui-attachment-preview')
  choose('modal-standard');page.keyboard.press('Tab');page.keyboard.press('Escape');page.wait_for_timeout(100);assert not state()['open'];click('story-modal-open');assert state()['open'];click('story-modal-cancel');assert not state()['open'];mark('gpui-modal-tab-escape-reopen-cancel')
  choose('dropdown-closed');click('story-select');page.keyboard.press('Escape');page.wait_for_timeout(100);assert not state()['open'];page.keyboard.press('ArrowDown');page.wait_for_timeout(200);assert state()['open'];page.keyboard.press('ArrowDown');page.keyboard.press('Enter');page.wait_for_timeout(150);assert state()['selected']==1 and not state()['open'];mark('gpui-dropdown-keyboard')
  choose('model-create-compact');click('profile-model-save');assert any(e.get('label')=='填写供应商提供的模型 ID。' and e['visible'] for e in elements());click('profile-model');page.keyboard.type('component-style-model');page.wait_for_timeout(150);assert not any(e.get('label')=='填写供应商提供的模型 ID。' and e['visible'] for e in elements());click('profile-context-limit');page.keyboard.press('ControlOrMeta+a');page.keyboard.type('0');click('profile-model-save');assert any(e.get('label')=='请输入大于 0 的整数。' and e['visible'] for e in elements());click('profile-context-limit');page.keyboard.press('ControlOrMeta+a');page.keyboard.type('32000');click('profile-model-save');assert not has('model-editor-dialog');mark('gpui-model-inline-errors-correct-save')
  choose('connection-provider-compact');page.keyboard.press('Escape');page.wait_for_timeout(100);assert has('profile-create-dialog');page.keyboard.press('Escape');page.wait_for_timeout(100);assert not has('profile-create-dialog');mark('gpui-menu-escape-before-modal')
 assert not errors,errors
 (out/('reference-result.json' if a.reference_only else 'result.json')).write_text(json.dumps({'checks':checks,'errors':errors},indent=2)+'\n');browser.close()

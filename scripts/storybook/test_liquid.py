# /// script
# dependencies = ["playwright==1.58.0"]
# ///
"""Physical browser inputs against the deduplicated Rust liquid gallery."""
import argparse, json, time, struct, zlib
from pathlib import Path
from playwright.sync_api import sync_playwright
from playground_inputs import PlaygroundInputs

ROOT=Path(__file__).resolve().parents[2]
p=argparse.ArgumentParser();p.add_argument('--url',required=True);p.add_argument('--output',type=Path,default=ROOT/'artifacts/liquid-rust/web-checks');p.add_argument('--backend',choices=['auto','webgl'],default='auto');p.add_argument('--width',type=int,default=1440);p.add_argument('--height',type=int,default=1800);args=p.parse_args();args.output.mkdir(parents=True,exist_ok=True)
report={'backend':args.backend,'errors':[],'console':[],'checks':{},'screenshots':[]}
with sync_playwright() as pw:
 binary=Path.home()/'Library/Caches/ms-playwright/chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing'
 browser=pw.chromium.launch(headless=True,executable_path=str(binary),args=["--force-device-scale-factor=2"]);context=browser.new_context(viewport={'width':args.width,'height':args.height},device_scale_factor=2);page=context.new_page();page.on('pageerror',lambda e:report['errors'].append(str(e)));page.on('console',lambda m:report['console'].append(m.text) if m.type in ['error','warning'] else None)
 context.add_init_script('''if(window.GPUQueue){const submit=GPUQueue.prototype.submit;GPUQueue.prototype.submit=function(...args){window.playgroundUsedWebGPU=true;return submit.apply(this,args);};}''')
 try:
  page.goto(args.url+'?story=liquid-gallery&backend='+args.backend)
  page.wait_for_function('document.documentElement.dataset.ready==="true"',timeout=120000)
  report['actualBackend']=page.evaluate('window.playgroundUsedWebGPU?"webgpu":"webgl"')
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
  preview_padding={}
  def check_preview_padding(kind,scenario='initial'):
   last={'actions':'hint','fields':'reveal','choices':'hint','switch':'disable'}.get(kind)
   if last:
    locate('liquid-'+kind+'-'+last)
   else:
    locate('liquid-'+kind+'-preview-frame')
   box=element('liquid-'+kind+'-preview-frame')['bounds']
   if last:
    end=element('liquid-'+kind+'-'+last)['bounds']
    bottom=box['y']+box['height']-end['y']-end['height']
    side=end['x']-box['x']
   elif kind in ['navigation','rows']:
    first=element('liquid-'+kind+'-row-0')['bounds'];end=element('liquid-'+kind+'-row-4')['bounds']
    bottom=box['y']+box['height']-end['y']-end['height']
    side=first['x']-box['x']
   else:
    material=card(kind)['surfaces'][0]
    poses=[material['pose']]
    if 'source' in material:poses.append(material['source'])
    bottom=box['height']-max(p['cy']+p['h']/2 for p in poses)
    side=18
   expected=34 if kind in ['navigation','rows'] else 18
   assert abs(bottom-expected)<1 and abs(side-bottom)<1,{'kind':kind,'bottom':bottom,'side':side}
   preview_padding[scenario+':'+kind]={'bottom':round(bottom,3),'side':round(side,3)}
  assert snap()['viewport']=={'width':args.width,'height':args.height},snap()['viewport']
  kinds=state()['canonicalKinds'];assert len(kinds)==47 and len(set(kinds))==47,kinds
  assert len(state()['cards'])==47
  for group in range(4):
   choose(group);click('liquid-demo');page.wait_for_timeout(850);shot('group-'+str(group));s=state();assert all(x['error'] is None and not x['paintError'] for c in s['cards'] for x in c['surfaces']),s
   for kind in [['actions','fields','choices','switch'],['navigation','rows','popover','details','notice'],['composer','attachments','comments'],['modal','disclosure']][group]:check_preview_padding(kind)
  report['checks']['canonicalKinds']=kinds
  report['checks']['previewPadding']=preview_padding
  choose(0);e=locate('liquid-actions-action-1');page.mouse.move(e['center']['x'],e['center']['y']);page.wait_for_timeout(350)
  clip={'x':e['bounds']['x']-3,'y':e['bounds']['y']-3,'width':e['bounds']['width']+6,'height':e['bounds']['height']+6}
  hovered=pixel(e['bounds']['x']+8,e['center']['y']);hovered_contour=page.screenshot(clip=clip)
  page.mouse.down();frame();page.wait_for_timeout(60);held=pixel(e['bounds']['x']+8,e['center']['y']);held_contour=page.screenshot(clip=clip)
  shot('button-held');page.mouse.up();frame()
  assert max(abs(a-b) for a,b in zip(hovered,held))<3,{'hovered':hovered,'held':held}
  assert hovered_contour!=held_contour,'Outlined button lost its physical press feedback'
  report['checks']['buttonPressFeedback']={'hoveredRgb':hovered,'heldRgb':held,'outlineMoves':True}
  icon=locate('liquid-actions-icon-24');page.mouse.move(icon['center']['x'],icon['bounds']['y']+50);page.wait_for_timeout(400)
  plain_icon=pixel(icon['bounds']['x']+4,icon['center']['y'])
  assert max(abs(a-b) for a,b in zip(plain_icon,[255,255,255]))<3,{'defaultIconBackground':plain_icon}
  page.mouse.move(icon['center']['x'],icon['center']['y']);page.wait_for_timeout(400)
  hover_icon=pixel(icon['bounds']['x']+4,icon['center']['y'])
  assert sum(plain_icon)-sum(hover_icon)>15,{'plain':plain_icon,'hover':hover_icon}
  shot('icon-action-hover');report['checks']['directIconHoverFill']={'plainRgb':plain_icon,'hoverRgb':hover_icon}
  choose(0);click('liquid-actions-action-0');assert card('actions')['actions']==1
  click('liquid-actions-variant-2');e=locate('liquid-actions-action-0');page.mouse.click(e['center']['x'],e['center']['y']);frame();assert card('actions')['actions']==1
  click('liquid-actions-variant-1');assert element('liquid-actions-action-0')['enabled']==False
  report['checks']['actions']={'disabledBlocks':True,'busyBlocks':True}
  type_text('liquid-fields-name','Rust 输入测试 ✓');assert card('fields')['input']=='Rust 输入测试 ✓',card('fields')
  click('liquid-fields-variant-1');assert card('fields')['invalid'];click('liquid-fields-variant-2');assert not locate('liquid-fields-name')['enabled']
  click('liquid-choices-choice-2-hit');assert card('choices')['selected']==2
  click('liquid-choices-variant-1');click('liquid-choices-choice-1-hit');assert card('choices')['selected']==1
  radio=locate('liquid-choices-choice-1-hit')
  page.mouse.move(radio['center']['x'],radio['bounds']['y']+radio['bounds']['height']+40);page.wait_for_timeout(500)
  radio_fill=pixel(radio['bounds']['x']+4,radio['center']['y'])
  assert max(abs(a-b) for a,b in zip(radio_fill,[255,255,255]))<3,{'radioBackground':radio_fill}
  shot('borderless-radio')
  for key,selected in [('ArrowRight',2),('ArrowRight',0),('End',2),('Home',0),('ArrowDown',1)]:
   page.keyboard.press(key);frame();assert card('choices')['selected']==selected,{'key':key,'selected':card('choices')['selected']}
  report['checks']['borderlessRadio']={'selectedBackground':radio_fill,'keyboardNavigation':True}
  click('liquid-choices-variant-2');click('liquid-choices-choice-1-hit');assert card('choices')['selected']==1
  click('liquid-switch-switch-hit');assert card('switch')['selected']==1
  click('liquid-switch-disable');e=locate('liquid-switch-switch-hit');page.mouse.click(e['center']['x'],e['center']['y']);frame();assert card('switch')['selected']==1
  report['checks']['fieldsChoicesSwitch']=True
  choose(1);click('liquid-navigation-row-4');assert card('navigation')['selected']==4
  click('liquid-navigation-variant-1');click('liquid-navigation-row-1');assert card('navigation')['selected']==1
  click('liquid-rows-variant-1');click('liquid-rows-row-2');assert card('rows')['selected']==2
  click('liquid-popover-trigger');page.wait_for_timeout(650)
  checked=locate('liquid-popover-option-0');unchecked=locate('liquid-popover-option-1')
  selected_pixel=pixel(checked['bounds']['x']+7,checked['center']['y']);plain_pixel=pixel(unchecked['bounds']['x']+7,unchecked['center']['y'])
  assert max(abs(a-b) for a,b in zip(plain_pixel,selected_pixel))<6,{'selected':selected_pixel,'plain':plain_pixel}
  page.mouse.move(unchecked['center']['x'],unchecked['center']['y']);page.wait_for_timeout(350)
  hover_pixel=pixel(unchecked['bounds']['x']+7,unchecked['center']['y'])
  assert sum(plain_pixel)-sum(hover_pixel)>30,{'plain':plain_pixel,'hover':hover_pixel}
  click('liquid-popover-option-1');assert card('popover')['selected']==1 and not card('popover')['open']
  click('liquid-popover-variant-1');click('liquid-popover-trigger');page.wait_for_timeout(650);click('liquid-popover-option-2');assert card('popover')['members'][2]
  page.keyboard.press('Escape');page.wait_for_timeout(650);assert not card('popover')['open']
  click('liquid-popover-variant-2');page.wait_for_timeout(650)
  icon=locate('liquid-popover-trigger');page.mouse.move(icon['center']['x'],icon['bounds']['y']+60);page.wait_for_timeout(500)
  plain_edge=pixel(icon['bounds']['x'],icon['center']['y']);plain_inside=pixel(icon['bounds']['x']+5,icon['center']['y'])
  page.mouse.move(icon['center']['x'],icon['center']['y']);page.wait_for_timeout(500)
  hover_edge=pixel(icon['bounds']['x'],icon['center']['y']);hover_inside=pixel(icon['bounds']['x']+5,icon['center']['y'])
  assert max(abs(a-b) for a,b in zip(plain_inside,hover_inside))<3,{'plain':plain_inside,'hover':hover_inside}
  assert sum(plain_edge)-sum(hover_edge)>20,{'plainEdge':plain_edge,'hoverEdge':hover_edge}
  shot('icon-panel-hover');report['checks']['panelIconHoverOutline']={'plainEdge':plain_edge,'hoverEdge':hover_edge,'interior':hover_inside}
  click('liquid-popover-trigger');page.wait_for_timeout(650);click('liquid-popover-option-1');assert not card('popover')['open']
  click('liquid-details-anchor-1');page.wait_for_timeout(450);assert card('details')['selected']==1
  click('liquid-details-variant-1');page.wait_for_timeout(500);assert card('details')['variant']==1
  click('liquid-inspect-disclosure');click('liquid-disclosure-trigger');page.wait_for_timeout(80);click('liquid-disclosure-trigger');page.wait_for_timeout(700);assert not card('disclosure')['open']
  click('liquid-disclosure-variant-1');click('liquid-disclosure-trigger');page.wait_for_timeout(650);assert card('disclosure')['open']
  report['checks']['navigationRowsPopoversDisclosure']=True
  choose(2)
  assert not locate('liquid-composer-send')['enabled']
  assert card('composer')['composer']['events']==[]
  type_text('liquid-composer-editor','第一行\n第二行\n第三行\n第四行');page.wait_for_timeout(650)
  assert card('composer')['inputHeight']==60 and abs(card('composer')['surfaces'][0]['pose']['h']-112)<.1,card('composer')
  assert locate('liquid-composer-send')['bounds']['width']==24 and locate('liquid-composer-attach')['bounds']['height']==24
  assert element('liquid-composer-grow') is None
  click('liquid-composer-send');page.wait_for_timeout(650);assert card('composer')['actions']==1 and card('composer')['input']==''
  assert not locate('liquid-composer-send')['enabled'];shot('composer-empty')
  click('liquid-composer-variant-2');page.wait_for_timeout(650)
  assert card('composer')['composer']['capabilities']['stop']
  type_text('liquid-composer-editor','运行时追加内容');page.keyboard.press('Enter');frame()
  assert card('composer')['composer']['capabilities']['stop'] and card('composer')['composer']['events']==['send:运行时追加内容:0']
  click('liquid-composer-send');assert card('composer')['composer']['events'][-1]=='stop'
  for variant in [3,4,5]:
   click('liquid-composer-variant-'+str(variant));assert not locate('liquid-composer-send')['enabled']
  assert card('composer')['composer']['capabilities']['editable']
  click('liquid-composer-variant-7');assert not card('composer')['composer']['capabilities']['stop'] and locate('liquid-composer-send')['enabled']
  click('liquid-composer-variant-6');page.wait_for_timeout(800);shot('composer-members')
  e=locate('liquid-composer-member-panda');page.mouse.move(e['center']['x'],e['center']['y']);page.wait_for_timeout(150);assert card('composer')['hoveredMember']=='panda'
  click('liquid-composer-member-panda');assert card('composer')['memberPreview']=='panda';page.keyboard.press('Escape');frame();assert card('composer')['memberPreview'] is None
  click('liquid-composer-variant-0');page.wait_for_timeout(600)
  with page.expect_file_chooser() as picker:click('liquid-composer-attach')
  picker.value.set_files([{'name':name,'mimeType':'text/plain','buffer':b'local fixture'} for name in ['规范.md','设计.txt','验证.md']])
  wait('JSON.parse(zorkStory.story_state()).cards.find(c=>c.kind==="composer").composer.files.length===3')
  assert locate('liquid-composer-send')['enabled']
  e=locate('liquid-composer-fan');page.mouse.move(e['center']['x'],e['center']['y']);page.wait_for_timeout(650);click('liquid-composer-fan');page.wait_for_timeout(650)
  assert card('composer')['fanPinned'];shot('composer-attachments')
  click('liquid-composer-remove-1');page.wait_for_timeout(650);assert [f['id'] for f in card('composer')['composer']['files']]==[2,3]
  click('liquid-composer-file-2');assert card('composer')['filePreview']==2;page.keyboard.press('Escape');frame();assert card('composer')['filePreview'] is None
  click('liquid-composer-send');assert card('composer')['composer']['events']==['send::2']
  report['checks']['realComposer']={'coreCapabilities':True,'enterWhileRunning':True,'taskComments':True,'threeLineLimit':True,'nativeEditorIME':True,'24pxActions':True,'browserFilePicker':True,'stableFileRemoval':True}
  click('liquid-attachments-trigger');page.wait_for_timeout(650);page.keyboard.press('Escape');page.wait_for_timeout(700);assert not card('attachments')['open']
  page.keyboard.press('Enter');page.wait_for_timeout(700);assert card('attachments')['open']
  file_row=locate('liquid-attachments-file-1');remove=locate('liquid-attachments-remove-1')
  page.mouse.move(page.viewport_size['width']-2,20);page.wait_for_timeout(500)
  quiet=min(sum(pixel(remove['center']['x']+dx,remove['center']['y']+dy)) for dx in [-1,0,1] for dy in [-1,0,1])
  assert quiet>750,{'hiddenRemoveInk':quiet}
  plain=pixel(file_row['bounds']['x']+1,file_row['center']['y']);assert min(plain)>=254,{'nestedRowBorder':plain}
  page.mouse.move(file_row['center']['x'],file_row['center']['y']);page.wait_for_timeout(500)
  hot=min(sum(pixel(remove['center']['x']+dx,remove['center']['y']+dy)) for dx in [-1,0,1] for dy in [-1,0,1])
  assert hot<600,{'revealedRemoveInk':hot}
  behind=pixel(remove['bounds']['x']+2,remove['bounds']['y']+2);row_fill=pixel(file_row['bounds']['x']+7,file_row['center']['y'])
  assert max(abs(a-b) for a,b in zip(behind,row_fill))<3,{'quietActionBackground':behind,'rowBackground':row_fill}
  last=locate('liquid-attachments-file-2')['bounds'];panel=locate('liquid-attachments-surface-content')['bounds']
  bottom=panel['y']+panel['height']-last['y']-last['height'];assert 15.5<=bottom<=16.5,{'attachmentBottomInset':bottom}
  shot('attachments-flat-hover')
  click('liquid-attachments-heading');page.mouse.move(page.viewport_size['width']-2,20);page.wait_for_timeout(400)
  page.keyboard.press('Tab');page.wait_for_timeout(300)
  close=locate('liquid-attachments-close')
  focused_ink=min(sum(pixel(close['center']['x']+dx,close['center']['y']+dy)) for dx in [-1,0,1] for dy in [-1,0,1])
  assert focused_ink<600,{'keyboardActionInk':focused_ink}
  click('liquid-attachments-remove-1');page.wait_for_timeout(500);assert len(card('attachments')['files'])==2
  check_preview_padding('attachments','after-remove')
  assert not any(e['id']=='liquid-attachments-back' and e['visible'] for e in snap()['elements']),'Removing a file also activated preview'
  report['checks']['flatAttachmentRowsAndHoverActions']={'hiddenInk':quiet,'hoverInk':hot,'keyboardInk':focused_ink,'bottomInset':bottom}
  click('liquid-attachments-file-0');page.wait_for_timeout(550);click('liquid-attachments-back');page.wait_for_timeout(450);click('liquid-attachments-close');page.wait_for_timeout(700);assert not card('attachments')['open']
  check_preview_padding('attachments','closed')
  click('liquid-comments-trigger');page.wait_for_timeout(700)
  add=locate('liquid-comments-comment-queue-add')['bounds'];panel=locate('liquid-comments-selection-flyout-material-content')['bounds']
  bottom=panel['y']+panel['height']-add['y']-add['height'];assert 15.5<=bottom<=16.5,{'commentBottomInset':bottom}
  shot('comments-content-fit');report['checks']['commentContentFit']={'bottomInset':bottom}
  check_preview_padding('comments','editor')
  type_text('liquid-comments-comment-input','保留真实输入与裁切');click('liquid-comments-comment-queue-add');page.wait_for_timeout(650);assert [draft['comment'] for draft in card('comments')['drafts']]==['保留真实输入与裁切']
  comment_id=card('comments')['drafts'][0]['id'];click(f'liquid-comments-comment-edit-{comment_id}');page.wait_for_timeout(700);assert card('comments')['open'];page.keyboard.press('Escape');page.wait_for_timeout(700)
  report['checks']['composerAttachmentsComments']=True
  choose(3);click('liquid-modal-trigger');page.wait_for_timeout(700);type_text('liquid-modal-name','共享 Rust 表单');click('liquid-modal-error');page.wait_for_timeout(500);assert card('modal')['invalid'];click('liquid-modal-save');page.wait_for_timeout(850);assert card('modal')['actions']==1 and not card('modal')['busy']
  for _ in range(10):page.keyboard.press('Tab')
  page.keyboard.press('Escape');page.wait_for_timeout(700);assert not card('modal')['open']
  click('liquid-inspect-notice');click('liquid-notice-state-4');page.wait_for_timeout(600);click('liquid-notice-retry');page.wait_for_timeout(900);assert card('notice')['variant']==2
  report['checks']['modalNotice']=True
  # Scan each canonical family at narrow sizes, with real scrolling where needed.
  for width in [320,600]:
   page.set_viewport_size({'width':width,'height':1400})
   for group in range(4):
    choose(group);click('liquid-demo');page.wait_for_timeout(750);shot(f'width-{width}-group-{group}')
    for kind in [['actions','fields','choices','switch'],['navigation','rows','popover','details','notice'],['composer','attachments','comments'],['modal','disclosure']][group]:check_preview_padding(kind,f'width-{width}')
    assert all(x['error'] is None and not x['paintError'] for c in state()['cards'] for x in c['surfaces'])
    assert not [e['id'] for e in snap()['elements'] if e['visible'] and (e['bounds']['x']<-2 or e['bounds']['x']+e['bounds']['width']>width+2)],[(e['id'],e['bounds']) for e in snap()['elements'] if e['visible'] and e['bounds']['x']+e['bounds']['width']>width+2]
  # Reproduce the annotated review at the user's actual logical viewport.
  page.set_viewport_size({'width':798,'height':837})
  choose(1);click('liquid-navigation-variant-1');click('liquid-navigation-row-1');page.wait_for_timeout(750)
  # A freed contour address must not reuse a narrower cached outer frame.
  for width in [798,600,798]:
   page.set_viewport_size({'width':width,'height':837});frame()
   # Resizing can move a different tab under the stationary pointer. Keep
   # this contour-cache pixel check in its intended non-hovered state.
   page.mouse.move(width-2,20);page.wait_for_timeout(500)
   e=locate('liquid-navigation-row-2');color=pixel(e['bounds']['x']+e['bounds']['width']-2,e['center']['y']);assert min(color)>=254,{'width':width,'outerFrameRgb':color}
  shot('review-navigation-centered');report['checks']['navigationResizeFrame']=True
  click('liquid-rows-variant-1');click('liquid-rows-row-2');page.wait_for_timeout(750);shot('review-rows-trailing-check')
  click('liquid-popover-variant-2');page.wait_for_timeout(550);click('liquid-popover-trigger')
  assert card('popover')['open'], 'first click after an offscreen source layout change must open the menu'
  page.wait_for_timeout(750);locate('liquid-popover-option-2');shot('review-menu-left-aligned')
  choose(2);click('liquid-attachments-trigger');page.wait_for_timeout(750);shot('review-attachment-quiet-close')
  choose(0);click('liquid-switch-switch-hit');page.wait_for_timeout(750);shot('review-switch-ink');click('liquid-switch-switch-hit');page.wait_for_timeout(750);shot('review-switch-warm-off')
  page.set_viewport_size({'width':1440,'height':1800});choose(3);click('liquid-modal-variant-1');page.emulate_media(reduced_motion='reduce');click('liquid-modal-trigger');frame();assert all(not s['moving'] for s in card('modal')['surfaces']);shot('reduced-modal');page.emulate_media(reduced_motion='no-preference')
  # Read-only details have no blinking caret; material animation must stop.
  page.mouse.move(1430,1780)
  page.wait_for_timeout(1500);before=card('modal')['frames'];page.wait_for_timeout(800);after=card('modal')['frames'];assert after-before<=1,(before,after)
  report['checks']['reducedMotionAndIdle']=True
  # Give recording its own fixed viewport, with all four surfaces visible.
  # The taller screenshot cases above exercise clipping; the independent
  # CPU/GPU frame-budget checks own the performance requirement.
  page.set_viewport_size({'width':1440,'height':1400});frame();page.wait_for_timeout(500)
  choose(4);click('liquid-record');page.wait_for_timeout(5300);record=state()['lastRecord'];assert record['count']==4 and len(record['controls'])==4,record;assert all(c['visible'] and len(c['frames'])>100 for c in record['controls']),[(c['kind'],c['visible'],len(c['frames'])) for c in record['controls']];assert locate('liquid-copy-record')['enabled'];report['checks']['recording']={'count':record['count'],'viewport':record['viewport'],'frames':[len(c['frames']) for c in record['controls']]}
  assert not report['errors'],report['errors']
  report['state']=state();(args.output/'result.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n');print(json.dumps({'checks':report['checks'],'errors':report['errors']},ensure_ascii=False))
 finally:
  if report['errors']:(args.output/'errors.json').write_text(json.dumps(report['errors'],ensure_ascii=False,indent=2))
  if page and not page.is_closed():
   page.screenshot(path=str(args.output/'last.png'))
   try:
    if page.evaluate('!!window.zorkStory'):(args.output/'last-state.json').write_text(json.dumps({'state':state(),'snapshot':snap()},ensure_ascii=False,indent=2))
   except Exception:pass
  (args.output/'diagnostics.json').write_text(json.dumps({'console':report['console'],'checks':report['checks'],'errors':report['errors']},ensure_ascii=False,indent=2))
  browser.close()

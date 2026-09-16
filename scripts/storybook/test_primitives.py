# /// script
# dependencies = ["playwright==1.58.0", "pillow==11.3.0"]
# ///
"""Physical-input regression for the shared component contracts and compact layout."""
import argparse
import hashlib
import json
from io import BytesIO
from pathlib import Path
from PIL import Image
from playwright.sync_api import sync_playwright
from playground_inputs import PlaygroundInputs

PRIMITIVES = 'checkbox checkboxgroup checkboxcards radiocards togglebutton togglegroup slider progress toast badge skeleton spinner accordion collapsible dialog alertdialog contextmenu menubar popovercontent tooltip hovercard tabs toolbar navigationmenu avatar datalist table scrollarea layout otp password form textarea'.split()

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--wasm-artifact', type=Path, required=True)
    parser.add_argument('--backend', choices=['auto', 'webgl'], default='auto')
    parser.add_argument('--case', action='append')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    report = {'passed': False, 'errors': [], 'checks': {}, 'console': []}
    with sync_playwright() as pw:
        binary = Path.home() / 'Library/Caches/ms-playwright/chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing'
        browser = pw.chromium.launch(headless=True, executable_path=str(binary), args=['--force-device-scale-factor=2'])
        context = browser.new_context(viewport={'width': 1000, 'height': 900}, device_scale_factor=2, permissions=['clipboard-read', 'clipboard-write'])
        context.add_init_script('''window.componentPaints=0;
            if(window.GPUQueue){const f=GPUQueue.prototype.submit;GPUQueue.prototype.submit=function(...a){window.componentBackend='webgpu';window.componentPaints++;return f.apply(this,a);};}
            for(const n of ['WebGLRenderingContext','WebGL2RenderingContext']){const p=window[n]?.prototype;if(!p)continue;
              for(const m of ['drawArrays','drawElements','drawArraysInstanced','drawElementsInstanced']){const f=p[m];if(f)p[m]=function(...a){window.componentBackend='webgl';window.componentPaints++;return f.apply(this,a);};}}
        ''')
        page = context.new_page()
        page.on('pageerror', lambda e: report['errors'].append(str(e)))
        page.on('console', lambda m: report['console'].append(m.text) if m.type == 'error' else None)
        def deliver(route):
            response = route.fetch()
            payload = response.body()
            assert payload == args.wasm_artifact.read_bytes(), 'Browser loaded another WASM artifact'
            report['artifactSha256'] = hashlib.sha256(payload).hexdigest()
            route.fulfill(response=response, body=payload)
        page.route('**/pkg/zork_gui_web_bg.wasm.gz', deliver)
        snapshot = lambda: page.evaluate('JSON.parse(zorkStory.snapshot())')
        state = lambda: page.evaluate('JSON.parse(zorkStory.story_state())')
        card = lambda kind: next(c for c in state()['cards'] if c['kind'] == kind)
        primitive = lambda kind: card(kind)['primitive']
        inputs = PlaygroundInputs(page, snapshot)
        def click(control): inputs.click(control)
        def key(text):
            page.keyboard.press(text)
            inputs.frame()
        def select(kind):
            key('Escape')
            page.wait_for_timeout(220)
            click('liquid-inspect-' + kind)
            inputs.reveal('liquid-' + kind + '-preview-frame')
            page.wait_for_timeout(300)
        def shot(name): page.screenshot(path=str(args.output / (name + '.png')))
        def check(condition, detail):
            assert condition, detail
        def value(kind, key_name): return primitive(kind)[key_name]
        def paste(text):
            page.evaluate('(text)=>navigator.clipboard.writeText(text)', text)
            key('ControlOrMeta+v')
            page.wait_for_timeout(120)
        def type_text(text):
            if text.isascii():
                page.keyboard.type(text)
            else:
                # GPUI Web receives non-Latin text through the browser's IME
                # composition events; insertText alone is not a key gesture.
                ime=context.new_cdp_session(page)
                ime.send('Input.imeSetComposition',{'text':text,'selectionStart':len(text),'selectionEnd':len(text)})
                ime.send('Input.insertText',{'text':text})
                ime.detach()
            inputs.frame()

        def selection():
            select('checkbox')
            check(value('checkbox', 'checked') == 'Mixed', primitive('checkbox'))
            click('liquid-checkbox-control');check(value('checkbox', 'checked') == 'On', primitive('checkbox'))
            key('Space');check(value('checkbox', 'checked') == 'Off', primitive('checkbox'))
            click('liquid-checkbox-disabled');click('liquid-checkbox-control')
            check(value('checkbox', 'checked') == 'Off', 'Disabled checkbox changed')
            click('liquid-checkbox-disabled')
            select('checkboxgroup');click('liquid-checkboxgroup-all');check(value('checkboxgroup', 'selected') == [0, 1], primitive('checkboxgroup'))
            click('liquid-checkboxgroup-item-0');check(value('checkboxgroup', 'selected') == [1], primitive('checkboxgroup'))
            click('liquid-checkboxgroup-item-2');check(value('checkboxgroup', 'selected') == [1], 'Disabled option changed')
            select('radiocards');click('liquid-radiocards-item-0');key('ArrowDown');check(value('radiocards', 'selected') == [1], primitive('radiocards'))
            key('ArrowDown');check(value('radiocards', 'selected') == [0], 'Radio did not skip disabled item')
            select('togglegroup');click('liquid-togglegroup-item-0');key('ArrowRight')
            check(value('togglegroup', 'selected') == [0], 'Arrow activated a manual toggle')
            key('Space');check(value('togglegroup', 'selected') == [1], primitive('togglegroup'))
            key('Space');check(value('togglegroup', 'selected') == [], primitive('togglegroup'))
            click('liquid-togglegroup-variant-1');click('liquid-togglegroup-item-0');click('liquid-togglegroup-item-1')
            check(value('togglegroup', 'selected') == [0, 1], primitive('togglegroup'))
            select('togglebutton');click('liquid-togglebutton-control');check(value('togglebutton', 'open'), primitive('togglebutton'))
            key('Space');check(not value('togglebutton', 'open'), primitive('togglebutton'))
            shot('selection')
            return {'mixedCheckbox': True, 'disabled': True, 'rovingRadio': True, 'singleAndMultipleToggle': True}

        def slider():
            select('slider');click('liquid-slider-control-thumb-0');key('Home');check(value('slider', 'values') == [0], primitive('slider'))
            key('ArrowRight');check(value('slider', 'values') == [5], primitive('slider'))
            key('PageUp');check(value('slider', 'values') == [55], primitive('slider'))
            key('End');check(value('slider', 'values') == [100], primitive('slider'))
            click('liquid-slider-variant-1');click('liquid-slider-control-thumb-0');key('End')
            check(value('slider', 'values') == [65, 75], primitive('slider'))
            click('liquid-slider-control-thumb-1');key('Home');check(value('slider', 'values') == [65, 75], primitive('slider'))
            click('liquid-slider-variant-0')
            thumb=inputs.locate('liquid-slider-control-thumb-0')['center'];track=inputs.locate('liquid-slider-control')['bounds']
            before=value('slider','commits');page.mouse.move(thumb['x'],thumb['y']);page.mouse.down();page.mouse.move(track['x']+track['width']+40,thumb['y']+30,steps=8);page.mouse.up();inputs.frame()
            check(value('slider','values') == [100] and value('slider','commits') > before, primitive('slider'))
            click('liquid-slider-variant-2');click('liquid-slider-control-thumb-0');key('ArrowUp');check(value('slider','values') == [40], primitive('slider'))
            click('liquid-slider-disabled');click('liquid-slider-control-thumb-0');key('End');check(value('slider','values') == [40], 'Disabled slider changed')
            shot('slider-vertical')
            return {'keyboardAndGap': True, 'dragOutsideBoundsCommits': True, 'verticalAndDisabled': True}

        def inputs_case():
            select('otp');click('otp-cell-0');paste('１２a３４５６７')
            check(value('otp','otp')['value'] == '123456', primitive('otp'))
            click('otp-cell-2');type_text('9');inputs.frame()
            check(value('otp','otp')['value'] == '129456', primitive('otp'))
            key('ControlOrMeta+z');check(value('otp','otp')['value'] == '123456', primitive('otp'))
            key('ControlOrMeta+a');key('Backspace');check(value('otp','otp')['value'] == '', primitive('otp'))
            paste('654321');click('liquid-otp-variant-1');shot('otp-masked')
            click('liquid-otp-disabled');click('otp-cell-0');type_text('2');inputs.frame();check(value('otp','otp')['value'] == '654321', 'Disabled OTP changed')
            select('password');click('liquid-password-control-input');key('ControlOrMeta+a');type_text('secret-value');key('ArrowLeft');key('Shift+ArrowLeft')
            before=value('password','selection');click('liquid-password-control-toggle')
            check(value('password','input') == 'secret-value' and value('password','selection') == before and value('password','open'), primitive('password'))
            type_text('X');inputs.frame();check('X' in value('password','input'), primitive('password'))
            key('ControlOrMeta+z');check(value('password','input') == 'secret-value', primitive('password'))
            select('textarea');click('liquid-textarea-control-input');type_text('第一行');key('Enter');type_text('第二行');inputs.frame()
            check(value('textarea','input') == '第一行\n第二行', primitive('textarea'))
            click('liquid-textarea-variant-1');click('liquid-textarea-control-input');key('ControlOrMeta+a');type_text('不能写入');inputs.frame()
            paste('不能通过粘贴替换')
            check(value('textarea','input') == '第一行\n第二行', 'Readonly textarea changed')
            select('form');click('liquid-form-name-label');type_text('表单名称');check(value('form','input') == '表单名称', 'Label did not focus its editor');click('liquid-form-submit');check(value('form','actions') == 1, primitive('form'))
            click('liquid-form-variant-1');shot('form-error')
            return {'otpPasteSelectionUndoDisabled': True, 'passwordSelectionUndo': True, 'textareaNewlineReadonly': True, 'formIntent': True}

        def disclosure():
            select('accordion');click('liquid-accordion-item-0-trigger');check(value('accordion','selected') == [0], primitive('accordion'))
            key('ArrowDown');key('Space');check(value('accordion','selected') == [1], primitive('accordion'))
            page.wait_for_timeout(700);click('liquid-accordion-body-field-input');type_text('保留备注');inputs.frame()
            click('liquid-accordion-item-1-trigger');page.wait_for_timeout(700);check(value('accordion','selected') == [] and value('accordion','input') == '保留备注', primitive('accordion'))
            click('liquid-accordion-variant-1');click('liquid-accordion-item-0-trigger');click('liquid-accordion-item-1-trigger');check(value('accordion','selected') == [0,1], primitive('accordion'))
            click('liquid-accordion-variant-2');click('liquid-accordion-item-0-trigger');check(value('accordion','selected') == [0], primitive('accordion'))
            select('collapsible');click('liquid-collapsible-control-item-trigger');page.wait_for_timeout(700);click('liquid-collapsible-body-field-input');type_text('展开内容');inputs.frame()
            click('liquid-collapsible-control-item-trigger');page.wait_for_timeout(650);check(not value('collapsible','open'), primitive('collapsible'))
            select('tabs');click('liquid-tabs-item-0');key('ArrowRight');check(value('tabs','selected') == [1], primitive('tabs'))
            key('ArrowRight');check(value('tabs','selected') == [0], primitive('tabs'))
            click('liquid-tabs-variant-1');click('liquid-tabs-item-0');key('ArrowRight');check(value('tabs','selected') == [0], 'Manual tab activated on arrow')
            key('Space');check(value('tabs','selected') == [1], primitive('tabs'))
            shot('tabs-manual')
            return {'accordionModes': True, 'editableDisclosure': True, 'automaticManualTabs': True}

        def dialogs():
            select('dialog');click('liquid-dialog-trigger');page.wait_for_timeout(800);click('liquid-dialog-dialog-field-input');type_text('对话框内容');inputs.frame()
            for _ in range(8): key('Tab')
            key('Escape');page.wait_for_timeout(650);check(not value('dialog','open') and value('dialog','input') == '对话框内容', primitive('dialog'))
            key('Space');page.wait_for_timeout(700);check(value('dialog','open'), 'Dialog did not restore trigger focus')
            key('Escape');page.wait_for_timeout(650)
            select('alertdialog');click('liquid-alertdialog-trigger');page.wait_for_timeout(800);key('Enter');page.wait_for_timeout(600)
            check(not value('alertdialog','open') and value('alertdialog','actions') == 0, 'Alert default focus was destructive')
            click('liquid-alertdialog-trigger');page.wait_for_timeout(800);page.mouse.click(10,60);inputs.frame();check(value('alertdialog','open'), 'Alert closed on outside click')
            click('liquid-alertdialog-panel-confirm');page.wait_for_timeout(650);check(value('alertdialog','input') == '' and value('alertdialog','actions') == 1, primitive('alertdialog'))
            select('popovercontent');click('liquid-popovercontent-trigger');page.wait_for_timeout(250);click('liquid-popovercontent-popover-field-input');type_text('浮层内容');inputs.frame();key('Escape')
            check(not value('popovercontent','flyout') and value('popovercontent','input') == '浮层内容', primitive('popovercontent'))
            key('Space');page.wait_for_timeout(200);check(value('popovercontent','flyout'), 'Popover did not restore focus')
            click('liquid-popovercontent-close');check(not value('popovercontent','flyout'), primitive('popovercontent'))
            click('liquid-popovercontent-trigger');page.wait_for_timeout(200);click('liquid-popovercontent-popover-field-input')
            for _ in range(5): key('Tab')
            check(not value('popovercontent','flyout'), 'Editor Tab did not dismiss the non-modal popover')
            return {'dialogTrapReturn': True, 'alertCancelDefaultOutsideConfirm': True, 'popoverEditorEscapeReturn': True}

        def menus():
            select('contextmenu');trigger=inputs.locate('liquid-contextmenu-trigger')['center'];page.mouse.click(trigger['x'],trigger['y'],button='right');inputs.frame()
            check(value('contextmenu','menu')['open'], primitive('contextmenu'))
            key('Home');key('ArrowDown');key('Enter');check(value('contextmenu','actions') == 1 and not value('contextmenu','menu')['open'], primitive('contextmenu'))
            key('Shift+F10');inputs.frame();key('s');key('ArrowRight');inputs.frame();check(value('contextmenu','menu')['path'] == [6], primitive('contextmenu'))
            key('Enter');check(value('contextmenu','actions') == 2 and not value('contextmenu','menu')['open'], primitive('contextmenu'))
            page.mouse.move(trigger['x'],trigger['y']);page.mouse.down();page.wait_for_timeout(650);page.mouse.up();inputs.frame();check(value('contextmenu','menu')['open'], 'Long press did not open menu')
            key('Escape');check(not value('contextmenu','menu')['open'], primitive('contextmenu'))
            select('menubar');click('liquid-menubar-file');key('ArrowRight');inputs.frame();check(value('menubar','menubar')[1]['open'], primitive('menubar'))
            key('Escape');key('ArrowRight');key('ArrowDown');check(value('menubar','menubar')[2]['open'], primitive('menubar'))
            shot('menubar-keyboard');key('Escape')
            select('navigationmenu');click('liquid-navigationmenu-products');page.wait_for_timeout(200);click('liquid-navigationmenu-control-link-updates');check(value('navigationmenu','actions') == 1 and '最新更新' in value('navigationmenu','status'), primitive('navigationmenu'))
            return {'rightClickLongPressShortcut': True, 'typeaheadSubmenu': True, 'menubarRoving': True, 'navigationLinks': True}

        def toasts():
            select('toast');click('liquid-toast-queue');check(len(value('toast','toasts')['items']) == 4, primitive('toast'))
            panel=inputs.locate('toast-6')['center'];page.mouse.move(panel['x'],panel['y']);page.wait_for_timeout(4400)
            check(len(value('toast','toasts')['items']) == 4 and value('toast','toasts')['paused'], 'Hover did not pause notification timers')
            click('toast-6-action');page.wait_for_timeout(200);check(value('toast','actions') == 1, primitive('toast'))
            click('liquid-toast-add');key('F8');page.mouse.move(10,60);page.wait_for_timeout(4300)
            check(value('toast','toasts')['paused'], 'Keyboard focus did not pause notification timers')
            key('Escape');page.wait_for_timeout(200);check(not any(t['id'] == 7 for t in value('toast','toasts')['items']), primitive('toast'))
            click('liquid-toast-add');page.mouse.move(10,60);page.wait_for_timeout(4500)
            check(value('toast','toasts')['items'] == [], primitive('toast'))
            before=page.evaluate('window.componentPaints');page.wait_for_timeout(1000);idle=page.evaluate('window.componentPaints')-before
            check(idle == 0, ('Empty notification queue paints',idle))
            return {'boundedQueue':4,'hoverAndFocusPause':True,'actionEscapeTimeout':True,'emptyIdleDraws':idle}

        def layout():
            measurements=[]
            for width in [1000,624,320]:
                page.set_viewport_size({'width':width,'height':837})
                for kind in PRIMITIVES:
                    select(kind)
                    preview=inputs.locate('liquid-'+kind+'-preview-frame')
                    page.wait_for_timeout(200)
                    controls=[e for e in snapshot()['elements'] if e['visible'] and e['role'] in ['button','option','text_input'] and (e['id'].startswith('liquid-'+kind+'-') or e['id'].startswith('otp-cell-'))]
                    bad=[e for e in controls if e['bounds']['x']<-.5 or e['bounds']['x']+e['bounds']['width']>width+.5]
                    check(not bad,(width,kind,bad))
                    check(preview['bounds']['width']<=width,(kind,preview))
                    measurements.append({'width':width,'kind':kind,'preview':preview['bounds'],'controls':len(controls)})
                    shot(f'{width}-{kind}')
            return measurements

        def review():
            page.set_viewport_size({'width':624,'height':837})
            select('actions')
            save=inputs.locate('liquid-actions-action-0')
            page.mouse.move(622,20);page.wait_for_timeout(450)
            pixels=Image.open(BytesIO(page.screenshot())).convert('RGB')
            color=pixels.getpixel((round((save['bounds']['x']+8)*2),round(save['center']['y']*2)))
            check(max(abs(a-b) for a,b in zip(color,(233,100,59)))<3,('Primary fill',color))
            check(inputs.element('liquid-actions-variant-select') is None,'Three choices retained a Select')
            for i in range(3):check(inputs.locate(f'liquid-actions-variant-{i}')['visible'],'Direct variant missing')
            shot('review-primary-and-direct-variants')
            select('choices');click('liquid-choices-variant-2');page.mouse.move(622,20);page.wait_for_timeout(500)
            avatar=inputs.locate('liquid-choices-choice-0-hit')['bounds']
            pixels=Image.open(BytesIO(page.screenshot())).convert('RGB')
            corner=pixels.getpixel((round((avatar['x']+3)*2),round((avatar['y']+3)*2)))
            check(min(corner)>=250,('Avatar background frame remains',corner))
            shot('review-avatar-no-frame')
            select('details');click('liquid-details-variant-1');click('liquid-details-anchor-1');page.mouse.move(622,20);page.wait_for_timeout(900)
            panel=card('details')['surfaces'][0]['pose']
            check(panel['w']<100 and panel['h']<45,('Compact text hint',panel))
            shot('review-compact-text-tooltip')
            select('notice');click('liquid-notice-state-4');page.mouse.move(622,20);page.wait_for_timeout(900);shot('review-error-alignment')
            click('liquid-notice-state-1');page.mouse.move(622,20);page.wait_for_timeout(600);shot('review-loading-alignment')
            return {'primaryRgb':color,'directVariants':3,'avatarCornerRgb':corner,'compactTooltip':panel,'noticeAlignmentScreenshots':True}

        def content_contracts():
            select('tooltip');click('liquid-tooltip-trigger');page.mouse.move(10,60);key('Escape')
            # Dismissal keeps the shared material only for its bounded exit.
            page.wait_for_function("!JSON.parse(zorkStory.snapshot()).elements.some(e => e.id === 'control-hint-liquid-tooltip-trigger')", timeout=2000)
            check(inputs.element('control-hint-liquid-tooltip-trigger') is None,'Escape retained tooltip')
            key('Shift+Tab');key('Tab');page.wait_for_timeout(300)
            hint=inputs.element('control-hint-liquid-tooltip-trigger')
            check(hint and hint['visible'],'Keyboard focus did not show tooltip')
            check(hint['bounds']['width']<120,'Short tooltip is too wide')
            shot('tooltip-keyboard');key('Escape')
            select('toolbar');click('liquid-toolbar-bold');key('ArrowRight');key('Space')
            check(value('toolbar','selected') == [0,1],primitive('toolbar'))
            key('ArrowRight');key('Enter');check(value('toolbar','actions')==1,'Toolbar did not skip disabled link')
            select('tabs');click('liquid-tabs-variant-2');click('liquid-tabs-link-0');key('Tab');key('Enter')
            check(value('tabs','selected')==[1],'Tab navigation link did not activate on Enter')
            click('liquid-tabs-link-2');check(value('tabs','selected')==[1],'Disabled navigation link activated')
            select('progress')
            for _ in range(8):click('liquid-progress-increase')
            check(value('progress','values')==[100],primitive('progress'))
            click('liquid-progress-variant-1');check(inputs.element('liquid-progress-increase') is None,'Indeterminate progress kept numeric controls')
            select('scrollarea');viewport=inputs.locate('liquid-scrollarea-control-viewport');click('liquid-scrollarea-control-viewport')
            key('PageDown');check(value('scrollarea','scroll')['y']<0,'Keyboard did not scroll vertically')
            key('ArrowRight');check(value('scrollarea','scroll')['x']<0,'Keyboard did not scroll horizontally')
            key('Home');check(value('scrollarea','scroll')=={'x':0,'y':0},primitive('scrollarea'))
            page.mouse.move(viewport['center']['x'],viewport['center']['y']);page.mouse.wheel(0,240);page.wait_for_timeout(200)
            check(value('scrollarea','scroll')['y']<0,'Wheel did not scroll the viewport')
            key('Home')
            x=viewport['bounds']['x']+viewport['bounds']['width']-5;y=viewport['bounds']['y']+12
            page.mouse.move(x,y);page.mouse.down();page.mouse.move(x,y+80,steps=8);page.mouse.up();inputs.frame()
            check(value('scrollarea','scroll')['y']<0,'Scrollbar thumb could not be dragged')
            shot('scrollarea-drag')
            return {'tooltipKeyboardEscape':True,'toolbarRovingDisabled':True,'tabNavigationLinks':True,'progressClampsAndIndeterminate':True,'scrollKeyboardWheelThumb':True}

        tests={'selection':selection,'slider':slider,'inputs':inputs_case,'disclosure':disclosure,'dialogs':dialogs,'menus':menus,'toasts':toasts,'layout':layout,'review':review,'content':content_contracts}
        try:
            page.goto(args.url+'?story=liquid-gallery&backend='+args.backend)
            page.wait_for_function('document.documentElement.dataset.ready==="true"',timeout=120000)
            report['actualBackend']=page.evaluate('window.componentBackend')
            context.set_offline(True)
            check(len(state()['canonicalKinds']) == 47,state()['canonicalKinds'])
            for name,test in tests.items():
                if args.case and name not in args.case: continue
                report['activeCase']=name
                report['checks'][name]=test()
                print('PASS',name,flush=True)
            check(not report['errors'],report['errors'])
            report['passed']=True
        except Exception as error:
            report['errors'].append(str(error));shot('failure')
            (args.output/'failure-snapshot.json').write_text(json.dumps(snapshot(),ensure_ascii=False,indent=2))
            (args.output/'failure-state.json').write_text(json.dumps(state(),ensure_ascii=False,indent=2))
            raise
        finally:
            (args.output/'result.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
            browser.close()
    print(json.dumps({'passed':report['passed'],'checks':list(report['checks'])},ensure_ascii=False))

if __name__=='__main__':main()

# /// script
# dependencies = ["playwright==1.58.0"]
# ///
"""Real-input checks for playground navigation, parameter bounds and compact UI."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path
from playwright.sync_api import sync_playwright
from playground_inputs import PlaygroundInputs

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--url', required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--backend', choices=['auto', 'webgl'], default='webgl')
parser.add_argument('--wasm-artifact', type=Path)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
report = {'errors': [], 'console': [], 'warnings': [], 'checks': {}, 'screenshots': [], 'backend': args.backend, 'passed': False}

with sync_playwright() as pw:
    binary = Path.home() / 'Library/Caches/ms-playwright/chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing'
    browser = pw.chromium.launch(headless=True, executable_path=str(binary), args=['--force-device-scale-factor=2'])
    context = browser.new_context(viewport={'width': 1440, 'height': 900}, device_scale_factor=2)
    context.add_init_script('''
        window.playgroundDraws = 0;
        for (const name of ['WebGLRenderingContext', 'WebGL2RenderingContext']) {
            const prototype = window[name]?.prototype;
            if (!prototype) continue;
            for (const method of ['drawArrays', 'drawElements', 'drawArraysInstanced', 'drawElementsInstanced']) {
                const original = prototype[method];
                if (typeof original !== 'function') continue;
                prototype[method] = function(...args) { window.playgroundDraws++; return original.apply(this, args); };
            }
        }
        if (window.GPUQueue) {
            const submit = GPUQueue.prototype.submit;
            GPUQueue.prototype.submit = function(...args) { window.playgroundDraws++; return submit.apply(this, args); };
        }
        window.playgroundPaintedFrames = 0;
        const requestFrame = window.requestAnimationFrame.bind(window);
        window.requestAnimationFrame = callback => requestFrame(timestamp => {
            const before = window.playgroundDraws;
            callback(timestamp);
            if (window.playgroundDraws !== before) window.playgroundPaintedFrames++;
        });
    ''')
    page = context.new_page()
    def deliver_wasm(route):
        # Verify the exact bytes delivered to the page. Large WASM responses
        # can be evicted from Chromium's inspector cache after initialization.
        response = route.fetch()
        payload = response.body()
        report['artifact'] = {'sha256': hashlib.sha256(payload).hexdigest(), 'bytes': len(payload)}
        if args.wasm_artifact:
            expected = args.wasm_artifact.read_bytes()
            if expected[:2] == b'\x1f\x8b' and payload[:2] != b'\x1f\x8b': expected = gzip.decompress(expected)
            assert payload == expected, 'Browser received a different WASM artifact'
        route.fulfill(response=response, body=payload)
    page.route('**/pkg/zork_gui_web_bg.wasm*', deliver_wasm)
    page.on('pageerror', lambda error: report['errors'].append(str(error)))
    page.on('console', lambda message: report['console'].append(message.text) if message.type == 'error' else report['warnings'].append(message.text) if message.type == 'warning' else None)
    def snapshot(): return page.evaluate('JSON.parse(zorkStory.snapshot())')
    def state(): return page.evaluate('JSON.parse(zorkStory.story_state())')
    def card(kind): return next(c for c in state()['cards'] if c['kind'] == kind)
    inputs = PlaygroundInputs(page, snapshot)
    def directory_kinds():
        prefix = "liquid-business-" if state()["section"] == "scenarios" else "liquid-inspect-"
        def rows():
            return [e for e in snapshot()['elements'] if e['id'].startswith(prefix)]
        visible = [e for e in rows() if e['visible']]
        assert visible, 'Directory has no visible component entries'
        anchor = min(visible, key=lambda e: abs(e['center']['y'] - page.viewport_size['height'] / 2))
        page.mouse.move(anchor['center']['x'], anchor['center']['y'])
        page.mouse.wheel(0, -5000)
        page.wait_for_timeout(120)
        seen, previous = set(), None
        for _ in range(20):
            current = {e['id'].removeprefix(prefix) for e in rows()}
            seen.update(current)
            if current == previous:
                break
            previous = current
            page.mouse.wheel(0, 400)
            page.wait_for_timeout(120)
        page.mouse.wheel(0, -5000)
        page.wait_for_timeout(120)
        return seen
    components = {'actions', 'fields', 'choices', 'switch', 'navigation', 'rows', 'popover', 'details', 'notice', 'checkbox', 'checkboxgroup', 'checkboxcards', 'radiocards', 'togglebutton', 'togglegroup', 'slider', 'progress', 'toast', 'badge', 'skeleton', 'spinner', 'accordion', 'collapsible', 'dialog', 'alertdialog', 'contextmenu', 'menubar', 'popovercontent', 'tooltip', 'hovercard', 'tabs', 'toolbar', 'navigationmenu', 'avatar', 'datalist', 'table', 'scrollarea', 'layout', 'otp', 'password', 'form', 'textarea'}
    scenarios = set()
    def shot(name):
        page.screenshot(path=str(args.output / (name + '.png')))
        report['screenshots'].append(name)
    def visible_controls_fit():
        width = page.viewport_size['width']
        controls = [e for e in snapshot()['elements'] if e['visible'] and e['role'] in ['button', 'option', 'text_input']]
        bad = [e['id'] for e in controls if e['bounds']['x'] < -.5 or e['bounds']['x'] + e['bounds']['width'] > width + .5]
        assert not bad, bad
        return len(controls)
    try:
        page.goto(args.url + '?story=liquid-gallery&backend=' + args.backend)
        page.wait_for_function('document.documentElement.dataset.ready==="true"', timeout=120000)
        assert 'artifact' in report, 'WASM load was not observed'
        scenarios = {family for family, _ in state()['businessCatalog']}
        context.set_offline(True)
        page.wait_for_timeout(450)
        shot('desktop-overview')
        assert len(state()['canonicalKinds']) == 47
        assert state()['section'] == 'components' and directory_kinds() == components
        inputs.click('liquid-section-scenarios')
        assert state()['section'] == 'scenarios' and directory_kinds() == scenarios
        shot('desktop-scenarios')
        inputs.click('liquid-section-components')
        page.keyboard.press('ArrowRight')
        inputs.frame()
        assert state()['section'] == 'scenarios' and directory_kinds() == scenarios
        page.keyboard.press('Home')
        inputs.frame()
        assert state()['section'] == 'components' and directory_kinds() == components
        report['checks']['separateComponentAndScenarioDirectories'] = True
        report['checks']['sectionKeyboardNavigation'] = True
        inputs.click('liquid-tab-4')
        assert state()['group'] == 4 and state()['selectedKind'] is None
        report['checks']['independentBenchmarkEntry'] = True
        inputs.click('liquid-section-components')
        assert state()['group'] == 0
        assert inputs.element('liquid-library-toggle') is None
        assert inputs.element('liquid-parameters-toggle') is None
        assert inputs.locate('liquid-budget-more')['bounds']['height'] == 32
        for _ in range(4): inputs.click('liquid-budget-more')
        assert state()['parameters']['budget'] == 64
        assert not inputs.locate('liquid-budget-more')['enabled']
        for _ in range(4): inputs.click('liquid-budget-less')
        assert state()['parameters']['budget'] == 6
        assert not inputs.locate('liquid-budget-less')['enabled']
        inputs.click('liquid-slow')
        assert state()['slow']
        inputs.click('liquid-reset')
        assert state()['parameters']['budget'] == 12 and not state()['slow']
        report['checks']['boundedParametersAndReset'] = True

        inputs.click('liquid-actions-action-0')
        actions = card('actions')['actions']
        inputs.click('liquid-business-composer')
        assert state()['businessFamily'] == 'composer' and state()['section'] == 'scenarios'
        assert inputs.element('liquid-actions-action-0') is None
        inputs.click('liquid-composer-variant-2')
        assert inputs.locate('liquid-composer-send')['label'] == '停止任务'
        shot('desktop-composer')
        inputs.click('liquid-inspect-actions')
        assert state()['section'] == 'components' and card('actions')['actions'] == actions
        inputs.click('liquid-actions-variant-0')
        page.keyboard.press('ArrowRight')
        inputs.frame()
        assert card('actions')['variant'] == 1 and not inputs.locate('liquid-actions-action-0')['enabled']
        inputs.click('liquid-actions-variant-2')
        assert not inputs.locate('liquid-actions-action-0')['enabled']
        inputs.click('liquid-actions-variant-0')
        report['checks']['componentSelectionPreservesState'] = True
        report['checks']['stateSelectorUsesRealInputs'] = True
        report['checks']['stateSelectorKeyboardNavigation'] = True

        inputs.click('liquid-inspect-dialog')
        inputs.click('liquid-dialog-trigger')
        page.wait_for_timeout(700)
        inputs.click('liquid-dialog-dialog-field-input')
        page.keyboard.press('ControlOrMeta+a')
        ime = context.new_cdp_session(page)
        ime.send('Input.imeSetComposition', {'text': 'zuhe', 'selectionStart': 4, 'selectionEnd': 4})
        page.keyboard.press('Tab')
        ime.send('Input.insertText', {'text': '组合输入'})
        ime.detach()
        inputs.frame()
        assert card('dialog')['primitive']['input'] == '组合输入', 'Tab moved focus away during IME composition'
        for _ in range(10): page.keyboard.press('Tab')
        page.keyboard.press('Escape')
        page.wait_for_timeout(600)
        assert not card('dialog')['primitive']['open']
        report['checks']['editorTabRespectsModalAndIME'] = True
        inputs.click('liquid-business-composer')
        inputs.click('liquid-inspect-dialog')
        assert card('dialog')['primitive']['input'] == '组合输入'
        report['checks']['scenarioInputPreservedAcrossSections'] = True

        widths = {}
        for width in [320, 375, 414, 738, 768, 1000]:
            page.set_viewport_size({'width': width, 'height': 837})
            context.set_offline(False)
            page.goto(args.url + '?story=liquid-gallery&backend=' + args.backend)
            page.wait_for_function('document.documentElement.dataset.ready === "true"', timeout=120000)
            context.set_offline(True)
            inputs.frame()
            page.wait_for_timeout(250)
            shot(f'overview-{width}')
            widths[str(width)] = {'visibleControls': visible_controls_fit()}
            if width < 1000:
                inputs.click('liquid-library-toggle')
                page.keyboard.press('Escape')
                inputs.frame()
                page.keyboard.press('Tab')
                inputs.frame()
                page.keyboard.press('Enter')
                inputs.frame()
                assert state()['panel'] == 'parameters', 'Toolbar Tab did not reach the parameter action'
                page.keyboard.press('Escape')
                inputs.frame()
                inputs.click('liquid-library-toggle')
                assert state()['panel'] == 'library'
                shot(f'library-{width}')
                inputs.click('liquid-section-scenarios')
                assert state()['panel'] == 'library' and state()['section'] == 'scenarios'
                assert directory_kinds() == scenarios
                shot(f'scenario-library-{width}')
                visible_controls_fit()
                inputs.click('liquid-business-composer')
                assert state()['panel'] is None and state()['businessFamily'] == 'composer'
                inputs.click('liquid-composer-variant-7')
                assert inputs.locate('liquid-composer-variant-select')['label'] == '任务评论'
                inputs.click('liquid-composer-variant-6')
                page.wait_for_timeout(650)
                shot(f'composer-{width}')
                visible_controls_fit()
            inputs.click('liquid-inspect-actions')
            inputs.click('liquid-parameters-toggle')
            assert state()['panel'] == 'parameters'
            shot(f'parameters-{width}')
            inputs.click('liquid-flow-more')
            assert abs(state()['parameters']['flow'] - .12) < .001
            for key in ['Tab', 'Shift+Tab']:
                for _ in range(12): page.keyboard.press(key)
            page.keyboard.press('Escape')
            inputs.frame()
            assert state()['panel'] is None
            # Escape returns focus to the opening toolbar action. Enter then
            # reopens that same dialog, proving focus restoration is usable.
            page.keyboard.press('Enter')
            inputs.frame()
            assert state()['panel'] == 'parameters', 'Parameter trigger focus was not restored'
            inputs.click('liquid-rules-open')
            assert state()['panel'] == 'rules'
            shot(f'rules-{width}')
            page.keyboard.press('Escape')
            inputs.frame()
            assert state()['panel'] is None
        report['checks']['responsiveWidths'] = widths
        report['checks']['dialogEscapeAndFocusRestoration'] = True
        report['checks']['dialogForwardAndReverseTabCycles'] = True
        report['checks']['toolbarTabNavigation'] = True
        # The shell now has a real material exit, too. Start the idle sample
        # after visible physics and hover feedback stop, within the engine's
        # existing six-second settling bound; retain the idle-frame limit.
        settle_started = page.evaluate('performance.now()')
        page.wait_for_function('''() => {
            const s = JSON.parse(zorkStory.story_state());
            const elements = JSON.parse(zorkStory.snapshot()).elements;
            const visible = new Set(elements.filter(e => e.visible).map(e => e.id));
            const moving = s.dialog?.moving || s.cards.some(c =>
                c.surfaces.some(surface => surface && surface.visible && surface.moving) ||
                (visible.has(c.id + '-variant-select') && c.selector?.moving));
            const count = window.playgroundPaintedFrames, now = performance.now();
            if (moving || window.playgroundIdleCount !== count) {
                window.playgroundIdleCount = count;
                window.playgroundQuietSince = now;
                return false;
            }
            return now - window.playgroundQuietSince >= 200;
        }''', timeout=6000)
        report['settleMs'] = page.evaluate('performance.now()') - settle_started
        before = page.evaluate('window.playgroundPaintedFrames')
        card_frames = {c['kind']: c['frames'] for c in state()['cards']}
        page.wait_for_timeout(2000)
        after = page.evaluate('window.playgroundPaintedFrames')
        # Text caret invalidation also exists in the previous playground (five
        # painted frames in two seconds). Check that control physics stops and
        # the page stays below 3 Hz, rather than mistaking draw calls for frames.
        report['idle'] = {'paintedFrames': after - before, 'durationMs': 2000}
        assert {c['kind']: c['frames'] for c in state()['cards']} == card_frames, 'Settled specimen physics kept advancing'
        assert after - before <= 6, f'Settled playground kept painting continuously: {after - before} frames in two seconds'
        report['checks']['sharedControlsSettleToIdle'] = True
        assert not report['errors'], report['errors']
        assert not report['console'], report['console']
        report['passed'] = True
        print(json.dumps(report['checks'], ensure_ascii=False), flush=True)
    finally:
        try:
            (args.output / 'last-state.json').write_text(json.dumps(state(), ensure_ascii=False, indent=2) + '\n')
            page.screenshot(path=str(args.output / 'last.png'))
        except Exception:
            pass
        (args.output / 'result.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
        browser.close()

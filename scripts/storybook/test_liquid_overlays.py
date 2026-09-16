# /// script
# dependencies = ["playwright==1.58.0"]
# ///
"""Verify live material motion in both the playground chrome and its specimens."""
import argparse
import hashlib
import json
import struct
import time
import zlib
from pathlib import Path
from playwright.sync_api import sync_playwright
from playground_inputs import PlaygroundInputs

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--url', required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--wasm-artifact', type=Path, required=True)
parser.add_argument('--backend', choices=['auto', 'webgl'], default='webgl')
parser.add_argument('--height', type=int, default=837)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
report = {'passed': False, 'errors': [], 'checks': {}, 'sequences': {}}

with sync_playwright() as pw:
    binary = Path.home() / 'Library/Caches/ms-playwright/chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing'
    browser = pw.chromium.launch(headless=True, executable_path=str(binary), args=['--force-device-scale-factor=2'])
    context = browser.new_context(viewport={'width': 768, 'height': args.height}, device_scale_factor=2,
                                  record_video_dir=str(args.output / 'video'),
                                  record_video_size={'width': 768, 'height': args.height})
    context.add_init_script('''
      window.overlaySubmissions = 0;
      if (window.GPUQueue) {
        const submit = GPUQueue.prototype.submit;
        GPUQueue.prototype.submit = function(...args) {
          window.overlayUsesWebGPU = true; window.overlaySubmissions++;
          return submit.apply(this, args);
        };
      }
      for (const name of ['WebGLRenderingContext','WebGL2RenderingContext']) {
        const prototype = window[name]?.prototype;
        if (!prototype) continue;
        for (const name of ['drawArrays','drawElements','drawArraysInstanced','drawElementsInstanced']) {
          const original = prototype[name];
          if (original) prototype[name] = function(...args) {
            window.overlaySubmissions++; return original.apply(this,args);
          };
        }
      }
    ''')
    page = context.new_page()
    def deliver(route):
        response = route.fetch()
        raw = response.body()
        assert raw == args.wasm_artifact.read_bytes(), 'Served WASM differs from the tested artifact'
        report['sha256'] = hashlib.sha256(raw).hexdigest()
        route.fulfill(response=response, body=raw)
    page.route('**/pkg/zork_gui_web_bg.wasm.gz', deliver)
    page.on('pageerror', lambda error: report['errors'].append(str(error)))
    page.on('console', lambda message: report['errors'].append(message.text) if message.type == 'error' else None)
    def state(): return page.evaluate('JSON.parse(zorkStory.story_state())')
    def snapshot(): return page.evaluate('JSON.parse(zorkStory.snapshot())')
    inputs = PlaygroundInputs(page, snapshot)
    def card(kind):
        current = state()
        if kind == 'composer' and current.get('businessFamily') == 'composer':
            return current['businessExample']['value']
        return next(c for c in current['cards'] if c['kind'] == kind)
    def shot(name): page.screenshot(path=str(args.output / f'{name}.png'))
    def pixel(x, y):
        raw = page.screenshot(clip={'x': x, 'y': y, 'width': 1, 'height': 1})
        offset, compressed = 8, b''
        while offset < len(raw):
            length = struct.unpack('>I', raw[offset:offset + 4])[0]
            if raw[offset + 4:offset + 8] == b'IDAT':
                compressed += raw[offset + 8:offset + 8 + length]
            offset += 12 + length
        return list(zlib.decompress(compressed)[1:4])
    def trace(name, read, milliseconds=1300):
        frames, started = [], time.monotonic()
        captured = False
        while (time.monotonic() - started) * 1000 < milliseconds:
            frame = read()
            if frame:
                assert frame['error'] is None and not frame['paintError'], frame
                frame = dict(frame, milliseconds=round((time.monotonic() - started) * 1000, 2))
                frame['contourLoops'] = frame['path'].count('M')
                # Keep the actual contour identity without megabytes of repeated SVG.
                frame['pathHash'] = hashlib.sha256(frame.pop('path').encode()).hexdigest()
                frames.append(frame)
                if name == 'specimen-popover-open' and not captured and .25 < frame['progress'] < .85:
                    shot('specimen-popover-middle')
                    captured = True
                if name == 'chrome-dialog-open' and not captured and .25 < frame['progress'] < .85:
                    shot('chrome-dialog-middle')
                    captured = True
                if name == 'chrome-dialog-open' and frame['progress'] > .9:
                    control = inputs.element('liquid-parameters-close')
                    # Fixed-layout content is cropped while the panel unfolds.
                    # Visible actions must stay on screen during the rebound.
                    if control:
                        bounds = control['bounds']
                        assert 0 <= bounds['x'] <= page.viewport_size['width'] - bounds['width'], bounds
                        assert 0 <= bounds['y'] <= page.viewport_size['height'] - bounds['height'], bounds
            page.evaluate('() => new Promise(resolve => requestAnimationFrame(resolve))')
        # The engine's deterministic settling contract allows 1440 fixed steps
        # (six seconds). Keep the visible-motion trace and also verify that its
        # particle tail reaches that same bounded resting state.
        while frames[-1]['moving'] and time.monotonic() - started < 6:
            inputs.frame()
            frame = read()
            if frame and not frame['moving']:
                frame = dict(frame, milliseconds=round((time.monotonic() - started) * 1000, 2))
                frame['contourLoops'] = frame['path'].count('M')
                frame['pathHash'] = hashlib.sha256(frame.pop('path').encode()).hexdigest()
                frames.append(frame)
        report['sequences'][name] = frames
        return frames
    def material_changes(name, frames, dimension):
        values = [frame['pose'][dimension] for frame in frames]
        assert len({f['pathHash'] for f in frames}) >= 4, f'{name}: no changing liquid contour'
        assert max(values) - min(values) > 12, f'{name}: only content/opacity changed'
        assert any(0.01 < f['progress'] < 0.99 for f in frames), f'{name}: no intermediate surface'
        assert not frames[-1]['moving'], f'{name}: surface did not settle'
        report['checks'][name] = {'intermediateContours': len({f['pathHash'] for f in frames}),
                                  'dimensionRange': [min(values), max(values)]}
        if name == 'chrome-dialog-open':
            separation = [((f['pose']['cx'] - f['source']['cx']) ** 2 +
                           (f['pose']['cy'] - f['source']['cy']) ** 2) ** .5 for f in frames]
            assert max(separation) > 32, 'Modal has lost its source/body material pair'
            assert frames[-1]['contourLoops'] >= 2, 'Separated modal parcels lost their distinct contours'
            assert any(f['contourLoops'] == 1 for f in frames), 'No connected material was observed during separation'
            report['checks']['modalMaterialSeparation'] = {
                'maximumDistance': max(separation), 'loopCounts': sorted({f['contourLoops'] for f in frames})}
    try:
        page.goto(args.url + '?story=liquid-gallery&backend=' + args.backend)
        page.wait_for_function('document.documentElement.dataset.ready === "true" || document.documentElement.dataset.error', timeout=120000)
        assert page.evaluate('document.documentElement.dataset.ready === "true"'), report['errors']
        context.set_offline(True)
        page.wait_for_timeout(400)
        assert snapshot()['viewport'] == {'width': 768, 'height': args.height}, snapshot()['viewport']
        report['backend'] = page.evaluate('window.overlayUsesWebGPU ? "webgpu" : "webgl"')

        inputs.click('liquid-business-composer')
        selector = inputs.locate('liquid-composer-variant-select')
        assert selector['bounds']['width'] < 100, 'A short selector label retained a fixed wide field'
        report['checks']['selectorFitsCurrentLabel'] = True
        trigger = selector['center']
        page.mouse.move(trigger['x'], trigger['y'])
        source_clip = dict(selector['bounds'])
        resting_source = page.screenshot(clip=source_clip)
        page.mouse.down()
        pressed_images = []
        for _ in range(6):
            pressed_images.append(hashlib.sha256(page.screenshot(clip=source_clip)).hexdigest())
            inputs.frame()
        held = trace('chrome-select-held', lambda: card('composer')['selector'], milliseconds=250)
        assert len(set(pressed_images)) >= 3, 'The page button has no continuous pressure feedback'
        assert hashlib.sha256(resting_source).hexdigest() != pressed_images[-1]
        assert all(not frame['open'] and frame['progress'] == 0 for frame in held), 'Button pressure activated the floating material'
        report['checks']['sourcePressVisualFrames'] = len(set(pressed_images))
        page.mouse.move(744, min(600, args.height - 40))
        page.mouse.up()
        trace('chrome-select-release-outside', lambda: card('composer')['selector'])
        assert not card('composer')['selector']['open'], 'Dragging out activated the selector'
        report['checks']['sourcePressUsesContinuousMaterial'] = True

        before = card('composer')['selector']
        old_y = inputs.element('liquid-composer-variant-select')['bounds']['y']
        page.mouse.wheel(0, 100)
        page.wait_for_timeout(300)
        after = card('composer')['selector']
        new_y = inputs.element('liquid-composer-variant-select')['bounds']['y']
        assert abs(new_y - old_y) > 40, 'Scroll did not move the source'
        assert after['revision'] == before['revision'] and after['path'] == before['path'], 'Anchor-only movement retraced the material'
        report['checks']['scrollReusesContour'] = True
        page.mouse.wheel(0, -100)
        page.wait_for_timeout(300)

        inputs.click('liquid-composer-variant-select')
        material_changes('chrome-select-open', trace('chrome-select-open', lambda: card('composer')['selector']), 'h')
        shot('chrome-select-open')
        menu_bounds = inputs.element('liquid-composer-variant-select-menu')['bounds']
        menu_fill = pixel(menu_bounds['x'] + 3, menu_bounds['y'] + menu_bounds['height'] / 2)
        assert min(menu_fill) >= 254, {'floatingMenuFill': menu_fill}
        report['checks']['opaqueFloatingMenu'] = menu_fill
        source = inputs.element('liquid-composer-variant-select')
        source_border = pixel(source['bounds']['x'], source['center']['y'])
        panel_border = pixel(menu_bounds['x'], menu_bounds['y'] + menu_bounds['height'] / 2)
        assert sum(source_border) < 720 and sum(panel_border) < 720, {'source': source_border, 'panel': panel_border}
        report['checks']['openMenuAndTriggerKeepOutline'] = {'source': source_border, 'panel': panel_border}
        first = inputs.locate('liquid-composer-variant-0')
        assert abs(first['bounds']['x'] - menu_bounds['x'] - 6) < .2
        assert abs(first['bounds']['y'] - menu_bounds['y'] - 6) < .2
        assert abs(menu_bounds['height'] - 238) < .2, menu_bounds
        assert menu_bounds['width'] >= 119.8, menu_bounds
        selector_state = card('composer')['selector']
        assert abs(selector_state['hover']['pose']['r'] - (selector_state['pose']['r'] - 6)) < .2
        page.mouse.move(first['center']['x'], first['center']['y'])
        trace('menu-hover-first', lambda: card('composer')['selector']['hover'])
        third = inputs.locate('liquid-composer-variant-2')
        page.mouse.move(third['center']['x'], third['center']['y'])
        sliding = trace('menu-hover-slide', lambda: card('composer')['selector']['hover'])
        ys = [f['pose']['cy'] for f in sliding]
        assert max(ys) - min(ys) > 32 and len({round(y, 1) for y in ys}) >= 4, ys
        shot('menu-hover-third')
        last_y = ys[-1]
        page.mouse.move(744, args.height - 40)
        leaving = trace('menu-hover-leave', lambda: card('composer')['selector']['hover'], milliseconds=350)
        assert all(abs(f['pose']['cy'] - last_y) < .1 for f in leaving)
        assert card('composer')['selector']['hoveredOption'] is None
        report['checks']['menuSlidingHoverAndCompactInsets'] = True
        trigger = inputs.locate('liquid-composer-variant-select')['center']
        page.mouse.click(trigger['x'], trigger['y'])
        page.wait_for_function('''(() => { const s = JSON.parse(zorkStory.story_state()).businessExample.value.selector; return !s.open && s.progress > 0 && s.progress < .95; })()''')
        closing = card('composer')['selector']
        assert 0 < closing['progress'] < 1 and closing['moving']
        page.mouse.click(trigger['x'], trigger['y'])
        inputs.frame()
        reopened = card('composer')['selector']
        assert reopened['open'], 'Reopen click was blocked by retiring menu content'
        assert reopened['progress'] > 0.05, 'Reopen reset the material to its closed source'
        trace('chrome-select-reversed', lambda: card('composer')['selector'])
        page.keyboard.press('Escape')
        material_changes('chrome-select-close', trace('chrome-select-close', lambda: card('composer')['selector']), 'h')
        assert not inputs.element('liquid-composer-variant-select-menu'), 'Closed menu retained interactive content'
        report['checks']['chromeSelectReversal'] = True

        inputs.click('liquid-inspect-actions')
        inputs.click('liquid-parameters-toggle')
        material_changes('chrome-dialog-open', trace('chrome-dialog-open', lambda: state()['dialog']), 'w')
        dialog = state()['dialog']
        assert inputs.element('liquid-parameters-close'), 'The settled modal lost its close action'
        assert abs(dialog['anchor']['w'] - 64) < .1 and abs(dialog['anchor']['h'] - 32) < .1, dialog
        assert abs(dialog['pose']['cx'] - dialog['anchor']['cx']) > 200, dialog
        report['checks']['modalPreservesSeparateTrigger'] = True
        source = dialog['anchor']
        source_border = pixel(source['cx'] - source['w'] / 2, source['cy'])
        panel = dialog['pose']
        panel_border = pixel(panel['cx'] - panel['w'] / 2, panel['cy'])
        assert sum(source_border) < 720 and sum(panel_border) < 720, {'source': source_border, 'panel': panel_border}
        report['checks']['openModalAndTriggerKeepOutline'] = {'source': source_border, 'panel': panel_border}
        for control_id in ['liquid-budget-more', 'liquid-flow-more', 'liquid-damping-more']:
            control = inputs.element(control_id)
            assert control and control['visible'], control_id
            assert abs(control['visible_bounds']['width'] - control['bounds']['width']) < .2, control
        report['checks']['modalBodyKeepsFullControlWidth'] = True
        shot('chrome-dialog-open')
        page.keyboard.press('Escape')
        page.wait_for_function('''(() => { const s = JSON.parse(zorkStory.story_state()).dialog; return !s.open && s.progress > 0 && s.progress < .95; })()''')
        closing = state()['dialog']
        assert closing['moving'] and 0 < closing['progress'] < 1
        page.keyboard.press('Enter')
        inputs.frame()
        assert state()['panel'] == 'parameters', 'Closing modal did not restore its trigger focus'
        assert state()['dialog']['progress'] > 0.05, 'Modal reopen discarded the current material'
        trace('chrome-dialog-reversed', lambda: state()['dialog'])
        inputs.click('liquid-rules-open')
        assert state()['panel'] == 'rules'
        assert state()['dialog']['progress'] > .9, 'Changing modal content rebuilt the surface'
        shot('chrome-dialog-rules')
        page.keyboard.press('Escape')
        material_changes('chrome-dialog-close', trace('chrome-dialog-close', lambda: state()['dialog']), 'w')
        assert not inputs.element('liquid-rules'), 'Closing dialog did not remove its content'
        report['checks']['chromeDialogReversalAndContentChange'] = True
        page.mouse.move(744, args.height - 40)
        page.wait_for_timeout(2400)
        trigger = inputs.locate('liquid-parameters-toggle')
        border = pixel(trigger['bounds']['x'], trigger['center']['y'])
        assert sum(border) < 720, {'restoredTriggerBorder': border}
        shot('chrome-dialog-trigger-restored')
        report['checks']['modalTriggerKeepsBorderAfterRetirement'] = border

        # Different triggers must not inherit a retiring source. Use captured
        # coordinates, bypassing the driver's deliberate close-animation wait.
        triggers = {name: inputs.locate(f'liquid-{name}-toggle') for name in ['parameters', 'library']}
        for first, second in [('parameters', 'library'), ('library', 'parameters')]:
            center = triggers[first]['center']
            page.mouse.click(center['x'], center['y'])
            trace(f'cross-source-{first}-open', lambda: state()['dialog'], milliseconds=300)
            page.keyboard.press('Escape')
            center = triggers[second]['center']
            page.mouse.click(center['x'], center['y'])
            inputs.frame()
            assert state()['panel'] == second, state()['panel']
            source = state()['dialog']['anchor']
            assert abs(source['cx'] - center['x']) <= 4 and abs(source['cy'] - center['y']) <= 4, source
            trace(f'cross-source-{second}-open', lambda: state()['dialog'])
            page.keyboard.press('Escape')
            trace(f'cross-source-{second}-close', lambda: state()['dialog'])
            page.keyboard.press('Enter')
            inputs.frame()
            assert state()['panel'] == second, 'Return focus retained the previous trigger'
            page.keyboard.press('Escape')
            trace(f'cross-source-{second}-finish', lambda: state()['dialog'])
        page.mouse.move(744, args.height - 40)
        page.wait_for_timeout(2400)
        for name, trigger in triggers.items():
            border = pixel(trigger['bounds']['x'], trigger['center']['y'])
            assert sum(border) < 720, {'trigger': name, 'border': border}
        report['checks']['distinctModalSourcesAndReturnFocus'] = True

        inputs.click('liquid-library-toggle')
        material_changes('chrome-library-open', trace('chrome-library-open', lambda: state()['dialog']), 'w')
        inputs.click('liquid-inspect-popover')
        page.wait_for_timeout(700)
        inputs.click('liquid-popover-trigger')
        material_changes('specimen-popover-open', trace('specimen-popover-open', lambda: card('popover')['overlay']), 'h')
        shot('specimen-popover-open')
        page.keyboard.press('Escape')
        trace('specimen-popover-close', lambda: card('popover')['overlay'])

        inputs.click('liquid-inspect-dialog')
        page.wait_for_timeout(700)
        inputs.click('liquid-dialog-trigger')
        material_changes('specimen-modal-open', trace('specimen-modal-open', lambda: card('dialog')['primitive']['dialog']), 'w')
        save = inputs.locate('liquid-dialog-done')['bounds']
        panel = inputs.element('liquid-dialog-panel')['bounds']
        bottom_gap = panel['y'] + panel['height'] - save['y'] - save['height']
        assert 22 <= bottom_gap <= 26, {'bottomGap': bottom_gap, 'panel': panel, 'save': save}
        report['checks']['modalFitsContentHeight'] = {'bottomInset': bottom_gap}
        shot('specimen-modal-open')
        page.keyboard.press('Escape')
        material_changes('specimen-modal-close', trace('specimen-modal-close', lambda: card('dialog')['primitive']['dialog']), 'w')

        page.emulate_media(reduced_motion='reduce')
        inputs.click('liquid-parameters-toggle')
        inputs.frame()
        assert state()['dialog']['progress'] == 1 and not state()['dialog']['moving']
        page.keyboard.press('Escape')
        inputs.frame()
        assert state()['dialog']['progress'] == 0 and not state()['dialog']['moving']
        page.emulate_media(reduced_motion='no-preference')
        report['checks']['reducedMotion'] = True

        page.set_viewport_size({'width': 1440, 'height': 900})
        trace('navigation-fit', lambda: state()['navigation'][0])
        row = inputs.locate('liquid-inspect-fields')['center']
        page.mouse.move(row['x'], row['y'])
        hover = trace('chrome-navigation-hover', lambda: state()['navigation'][0])
        assert len({f['pathHash'] for f in hover}) >= 4
        assert max(f['pose']['cy'] for f in hover) - min(f['pose']['cy'] for f in hover) > 32
        last_y = hover[-1]['pose']['cy']
        page.mouse.move(1430, 880)
        leaving = trace('chrome-navigation-leave', lambda: state()['navigation'][0], milliseconds=350)
        assert all(abs(f['pose']['cy'] - last_y) < .1 for f in leaving), 'Hover moved back to selection on pointer leave'
        shot('chrome-navigation-leave')
        report['checks']['hoverFadesAtLastPosition'] = True
        inputs.click('liquid-inspect-fields')
        selected = trace('chrome-navigation-selection', lambda: state()['navigation'][1])
        assert len({f['pathHash'] for f in selected}) >= 4
        assert state()['selectedKind'] == 'fields'
        inputs.click('liquid-inspect-navigation')
        first_row = inputs.locate('liquid-navigation-row-0')['center']
        page.mouse.move(first_row['x'], first_row['y'])
        trace('specimen-navigation-hover-first', lambda: card('navigation')['surfaces'][0])
        row = inputs.locate('liquid-navigation-row-4')['center']
        page.mouse.move(row['x'], row['y'])
        sample = trace('specimen-navigation-hover', lambda: card('navigation')['surfaces'][0])
        assert len({f['pathHash'] for f in sample}) >= 4
        assert max(f['pose']['cy'] for f in sample) - min(f['pose']['cy'] for f in sample) > 32
        shot('shared-navigation')
        report['checks']['navigationSharesHoverAndSelectionMotion'] = True
        trips = []
        for index in [1, 2, 4]:
            # Sample each browser frame, independently of driver round trips.
            # Inputs remain physical; this observer only reads rendered state.
            first = inputs.locate('liquid-navigation-row-0')['center']
            target = inputs.locate(f'liquid-navigation-row-{index}')['center']
            page.mouse.move(first['x'], first['y'])
            page.wait_for_timeout(700)
            initial = card('navigation')['surfaces'][0]['pose']['cy']
            page.evaluate('''() => {
                window.liquidTravelFrames = []; window.liquidTravelDone = false;
                const start = performance.now();
                function sample(now) {
                    const state = JSON.parse(zorkStory.story_state());
                    const pose = state.cards.find(c => c.kind === 'navigation').surfaces[0].pose;
                    window.liquidTravelFrames.push({...pose, atMs: now - start});
                    if (now - start < 650) requestAnimationFrame(sample);
                    else window.liquidTravelDone = true;
                }
                requestAnimationFrame(sample);
            }''')
            page.mouse.move(target['x'], target['y'])
            page.wait_for_function('window.liquidTravelDone')
            frames = page.evaluate('window.liquidTravelFrames')
            final = frames[-1]['cy']
            departing = next(f['atMs'] for f in frames if abs(f['cy'] - initial) > .25)
            arriving = next(f['atMs'] for f in frames if f['atMs'] >= departing and abs(f['cy'] - final) < .5)
            assert len({round(f['cy'], 2) for f in frames}) >= 3
            trips.append({'index': index, 'distance': abs(final - initial),
                          'durationMs': arriving - departing,
                          'peakSpeed': max(abs(f['vy']) for f in frames)})
            report['sequences'][f'hover-distance-{index}'] = frames
        assert trips[0]['durationMs'] < trips[1]['durationMs'] < trips[2]['durationMs'], trips
        assert trips[2]['durationMs'] < 400, trips
        assert max(t['peakSpeed'] for t in trips) / min(t['peakSpeed'] for t in trips) < 1.5, trips
        report['checks']['hoverDistanceDeterminesDuration'] = trips
        page.wait_for_timeout(1200)
        before = page.evaluate('window.overlaySubmissions')
        page.wait_for_timeout(1200)
        after = page.evaluate('window.overlaySubmissions')
        assert before == after, f'Idle overlays kept submitting draws: {after - before}'
        report['checks']['idleStopsDrawing'] = True
        assert not report['errors'], report['errors']
        report['passed'] = True
        print(json.dumps(report['checks'], ensure_ascii=False))
    finally:
        try:
            shot('last')
            (args.output / 'last-state.json').write_text(json.dumps(state(), ensure_ascii=False, indent=2))
        except Exception as error:
            report['diagnosticError'] = str(error)
        finally:
            (args.output / 'result.json').write_text(json.dumps(report, ensure_ascii=False, indent=2))
            context.close()
            browser.close()

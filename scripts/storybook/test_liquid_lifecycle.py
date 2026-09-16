# /// script
# dependencies = ["playwright==1.58.0", "pillow==11.3.0"]
# ///
"""Verify shared overlay placement, retirement, occlusion, transfer and nested material edges through physical input on grouped pages."""
from pathlib import Path
from io import BytesIO
import argparse, json, hashlib, sys, time
from PIL import Image
from playwright.sync_api import sync_playwright
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/storybook'))
from playground_inputs import PlaygroundInputs
p = argparse.ArgumentParser()
p.add_argument('--output', type=Path, required=True)
p.add_argument('--wasm-artifact', type=Path, required=True)
p.add_argument('--url', required=True)
p.add_argument('--width', type=int, default=624)
p.add_argument('--reduced-motion', action='store_true')
p.add_argument('--backend', choices=['auto', 'webgl'], default='auto')
args = p.parse_args()
OUT = args.output
OUT.mkdir(parents=True, exist_ok=True)
report = {'passed': False, 'errors': [], 'checks': {}, 'sequences': {}, 'width': args.width, 'reducedMotion': args.reduced_motion}
with sync_playwright() as pw:
    binary = Path.home() / 'Library/Caches/ms-playwright/chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing'
    browser = pw.chromium.launch(headless=True, executable_path=str(binary), args=['--force-device-scale-factor=2'])
    context = browser.new_context(viewport={'width': args.width, 'height': 900}, device_scale_factor=2, reduced_motion='reduce' if args.reduced_motion else 'no-preference')
    context.add_init_script('''window.draws=0;
      if(window.GPUQueue){const f=GPUQueue.prototype.submit;GPUQueue.prototype.submit=function(...a){window.lifecycleBackend='webgpu';window.draws++;return f.apply(this,a)}}
      for(const n of ['WebGLRenderingContext','WebGL2RenderingContext']){const p=window[n]?.prototype;if(!p)continue;
        for(const m of ['drawArrays','drawElements','drawArraysInstanced','drawElementsInstanced']){const f=p[m];if(f)p[m]=function(...a){window.lifecycleBackend='webgl';window.draws++;return f.apply(this,a)}}}
    ''')
    page = context.new_page()
    page.on('pageerror', lambda e: report['errors'].append(str(e)))

    def deliver(route):
        response = route.fetch()
        raw = response.body()
        assert raw == args.wasm_artifact.read_bytes()
        report['sha256'] = hashlib.sha256(raw).hexdigest()
        route.fulfill(response=response, body=raw)
    page.route('**/pkg/zork_gui_web_bg.wasm.gz', deliver)
    page.goto(args.url + '?story=liquid-gallery&backend=' + args.backend)
    page.wait_for_function('document.documentElement.dataset.ready==="true"', timeout=120000)
    context.set_offline(True)
    report['backend'] = page.evaluate('window.lifecycleBackend')
    assert report['backend'] in ['webgpu', 'webgl']
    snapshot = lambda: page.evaluate('JSON.parse(zorkStory.snapshot())')
    state = lambda: page.evaluate('JSON.parse(zorkStory.story_state())')
    inputs = PlaygroundInputs(page, snapshot)
    primitive = lambda kind: next((c['primitive'] for c in state()['cards'] if c['kind'] == kind))

    def shot(name):
        return Image.open(BytesIO(page.screenshot(path=str(OUT / (name + '.png'))))).convert('RGB')

    def wait(ms=1000):
        page.wait_for_timeout(ms)

    def select(id):
        page.keyboard.press('Escape')
        wait(1600)
        inputs.click(id)
        wait(600)

    def wheel(delta):
        page.mouse.move(args.width - 8, min(720, page.viewport_size['height'] - 40))
        page.mouse.wheel(0, delta)
        inputs.frame()

    def bounds(id):
        return inputs.element(id)['bounds']

    def opaque_dialog_rim(image, panel, name):
        left, top = panel['cx'] - panel['w'] / 2, panel['cy'] - panel['h'] / 2
        right, bottom = left + panel['w'], top + panel['h']
        inset = panel['r'] + 10
        strips = [
            (left + 4, top + inset, left + 16, bottom - inset),
            (right - 16, top + inset, right - 4, bottom - inset),
            (left + inset, top + 4, right - inset, top + 14),
            (left + inset, bottom - 14, right - inset, bottom - 4),
        ]
        counts = []
        for strip in strips:
            pixels = list(image.crop(tuple(round(v * 2) for v in strip)).getdata())
            counts.append(sum(min(color) < 250 for color in pixels))
        assert counts == [0, 0, 0, 0], ('Dialog has an unfilled interior edge', name, strips, counts)
        report['checks'][name] = counts

    def dialog_title_is_visible(image, panel):
        left, top = panel['cx'] - panel['w'] / 2, panel['cy'] - panel['h'] / 2
        rectangle = (left + 24, top + 24, left + 170, top + 62)
        pixels = list(image.crop(tuple(round(v * 2) for v in rectangle)).getdata())
        ink = sum(max(color) < 150 for color in pixels)
        assert ink > 300, ('Dialog title was covered by its own material', rectangle, ink)
        report['checks']['dialogTitleInkPixels'] = ink

    def settling(kind):
        deadline = time.monotonic() + 6
        entry = []
        while time.monotonic() < deadline:
            m = primitive(kind)['dialog'] if kind == 'dialog' else primitive(kind)['menubar'][0]['materials'][0]
            if kind == 'dialog':
                entry.append({key:m[key] for key in ('open','moving','expansion','contentAlpha','backdropAlpha')})
            if not m.get('moving', False) and (kind != 'dialog' or
                    m.get('backdropAlpha') == (1 if m.get('open') else 0)):
                if kind == 'dialog' and m['open'] and 'midpointBackdrop' not in report['sequences']:
                    if not args.reduced_motion:
                        visible = [f for f in entry if f['backdropAlpha'] > 0]
                        assert visible and visible[0]['moving'] and .5 <= visible[0]['expansion'] < .95, entry
                        assert all(f['backdropAlpha'] == 0 for f in entry if f['expansion'] < .5), entry
                    report['sequences']['midpointBackdrop'] = entry
                    report['checks']['backdropBeginsDuringExpansion'] = True
                return m
            inputs.frame()
        raise AssertionError(('Did not settle', kind, m))
    try:
        select('liquid-tab-7')
        inputs.locate('liquid-dialog-trigger')
        wait(900)
        wheel(bounds('liquid-dialog-trigger')['y'] - 484)
        wait(900)
        # The whole opening animation already belongs to the modal layer.
        # Its source drawing stays below the backdrop as it fades in midway.
        wheel(bounds('liquid-dialog-trigger')['y'] - 720)
        wait(900)
        original = bounds('liquid-dialog-trigger')
        original_image = shot('dialog-source-in-page')
        inputs.click('liquid-dialog-trigger')
        assert primitive('dialog')['dialog']['destinationLayer'] == 'modal'
        if not args.reduced_motion:
            early = primitive('dialog')['dialog']
            if early['expansion'] < .5:
                assert early['backdropAlpha'] == 0, 'Page darkened before the expansion midpoint'
        settling('dialog')
        inputs.frame()
        layered = shot('dialog-source-under-backdrop')
        modal = primitive('dialog')['dialog']
        panel = modal['pose']
        opaque_dialog_rim(layered, panel, 'dialogSeparatedOpaqueRim')
        dialog_title_is_visible(layered, panel)
        assert abs(modal['anchor']['cy'] - original['y'] - original['height'] / 2) < .1, 'Material deformation moved the source layout anchor'
        assert modal['path'].count('M') == 2, 'Modal and source did not separate as one paired material'
        assert original['y'] > panel['cy'] + panel['h'] / 2
        point = (round((original['x'] + original['width'] / 2) * 2), round((original['y'] + 6) * 2))
        before_pixel = original_image.getpixel(point)
        after_pixel = layered.getpixel(point)
        assert min(before_pixel) >= 245 and all(180 <= c <= 225 for c in after_pixel), ('Source escaped the page layer', before_pixel, after_pixel)
        report['checks']['dialogSourceBelowBackdrop'] = {'before': before_pixel, 'after': after_pixel}
        page.keyboard.press('Escape')
        wait(1700)
        wheel(bounds('liquid-dialog-trigger')['y'] - 484)
        wait(900)
        report['dialogTriggerBefore'] = bounds('liquid-dialog-trigger')
        inputs.click('liquid-dialog-trigger')
        wait(1200)
        im = shot('dialog-group-open')
        report['dialogOpen'] = primitive('dialog')['dialog']
        panel = report['dialogOpen']['pose']
        opaque_dialog_rim(im, panel, 'dialogOverlappingOpaqueRim')
        dialog_title_is_visible(im, panel)
        source = report['dialogOpen']['anchor']
        left = panel['cx'] - panel['w'] / 2
        region = (left + 6, source['cy'] - 7, left + 22, source['cy'] + 7)
        pixels = list(im.crop(tuple((round(v * 2) for v in region))).getdata())
        assert pixels and all((min(c) >= 250 for c in pixels)), ('Source ink crossed the modal foreground', region)
        report['checks']['modalSourceOccluded'] = {'region': region, 'nonWhitePixels': sum((min(c) < 250 for c in pixels))}
        page.keyboard.press('Escape')
        inputs.frame()
        destination = primitive('dialog')['dialog']
        assert destination['destinationLayer'] == 'source', destination
        assert not inputs.element('liquid-dialog-panel'), 'Closing retained a live modal input tree'
        reference_y = destination['anchor']['cy'] - destination['paintOffset'][1]
        wheel(180)
        trace = []
        for i in range(24):
            m = primitive('dialog')['dialog']
            trace.append({k: v for k, v in m.items() if k != 'path'})
            inputs.frame()
        settling('dialog')
        if args.reduced_motion:
            assert trace and all(not m['moving'] and m['contentAlpha'] == 0 and m['backdropAlpha'] == 0 for m in trace), ('Reduced motion retained a closing animation', trace)
            assert abs(trace[-1]['anchor']['cy'] - destination['anchor']['cy']) > 30, 'Closed source did not scroll'
            report['checks']['reducedMotionClosesImmediately'] = True
        else:
            assert trace and max(abs(m['anchor']['cy'] - m['paintOffset'][1] - reference_y) for m in trace) < .1, ('Closing paint lags its destination layout', trace)
            assert max(abs(m['paintOffset'][1]) for m in trace) > 30, ('Closing destination did not scroll', trace)
            report['checks']['closingFollowsDestinationLayer'] = True
        shot('dialog-close-scroll')
        assert not inputs.element('liquid-dialog-panel'), 'Retired modal is still mounted'
        wheel(210)
        wait(900)
        shot('dialog-closed-scroll')
        assert not inputs.element('liquid-dialog-panel'), 'Scrolling resurrected the modal'
        report['sequences']['closeScroll'] = trace
        report['checks']['dialogRetiredAfterScroll'] = True
        # Put the return beside the window center so a stale ownership
        # bisector would cover the right half of the source's Chinese caption.
        wheel(bounds('liquid-dialog-trigger')['y'] - 434)
        wait(900)
        b = bounds('liquid-dialog-trigger')
        caption = tuple(round(v * 2) for v in (
            b['x'] + b['width'] / 2 - 31, b['y'] + 5,
            b['x'] + b['width'] / 2 + 31, b['y'] + b['height'] - 5))
        reference = list(shot('dialog-caption-before').crop(caption).getdata())
        ink = [i for i, color in enumerate(reference) if max(color) < 120]
        assert len(ink) > 300, 'Missing caption reference'
        inputs.click('liquid-dialog-trigger')
        wait(1100)
        m = primitive('dialog')['dialog']
        b = bounds('liquid-dialog-trigger')
        assert abs(m['anchor']['cy'] - (b['y'] + b['height'] / 2)) < 1, ('Reopened from stale source', m, b)
        page.keyboard.press('Escape')
        fusion = []
        for i in range(22):
            wait(60)
            m = primitive('dialog')['dialog']
            if m['progress'] > .001 or any(abs(m['pose'][key] - m['source'][key]) > 3 for key in ['cx', 'cy', 'w', 'h']):
                continue
            image = shot(f'dialog-caption-fusion-{i:02}')
            pixels = list(image.crop(caption).getdata())
            retained = sum(max(pixels[j]) < 120 for j in ink) / len(ink)
            fusion.append({'moving': m['moving'], 'inkRetained': retained})
            assert retained >= .98, ('Returning material covered the source caption', retained, m)
        assert len(fusion) >= 3, 'Missing fused source caption frames'
        report['checks']['sourceCaptionSurvivesFusion'] = fusion
        settling('dialog')
        inputs.locate('liquid-popovercontent-trigger')
        wait(800)
        wheel(bounds('liquid-popovercontent-trigger')['y'] - 340)
        wait(900)
        inputs.click('liquid-popovercontent-trigger')
        wait(1300)
        shot('flyout-group-open')
        before = (primitive('popovercontent')['flyoutMotion']['anchor'], bounds('liquid-popovercontent-panel-material-content'))
        wheel(-90)
        frames = []
        for i in range(12):
            if inputs.element('liquid-popovercontent-panel-material-content'):
                a, b = (primitive('popovercontent')['flyoutMotion']['anchor'], bounds('liquid-popovercontent-panel-material-content'))
                frames.append({'source': a, 'panel': b, 'material': primitive('popovercontent')['flyoutMotion']['material']})
            inputs.frame()
        shot('flyout-group-scrolled')
        after = frames[-1]
        delta = after['source'][1] - before[0][1]
        panel_delta = after['panel']['y'] - before[1]['y']
        assert abs(delta) > 30, ('Page did not scroll', before, after)
        assert abs(delta - panel_delta) < 1, ('Anchor did not follow', delta, panel_delta, frames)
        offsets = [f['panel']['y'] - f['source'][1] for f in frames]
        assert max(offsets) - min(offsets) < 1, ('Placement lags scrolling', offsets)
        report['checks']['flyoutScroll'] = {'delta': delta, 'panelDelta': panel_delta, 'offsetRange': [min(offsets), max(offsets)]}
        report['sequences']['flyoutScroll'] = frames
        wheel(-320)
        wait(1800)
        edge = primitive('popovercontent')['flyoutMotion']
        b = bounds('liquid-popovercontent-panel-material-content')
        shot('flyout-edge-flipped')
        assert b['y'] >= 12 and b['y'] + b['height'] <= 888 and (b['y'] < edge['anchor'][1]), ('Unfitted edge placement', b, edge)
        report['checks']['flyoutEdgeFit'] = b
        source_before = edge['anchor'][1]
        wheel(-1100)
        wait(650)
        hidden = primitive('popovercontent')['flyoutMotion']
        assert not hidden['sourceVisible'] and (not inputs.element('liquid-popovercontent-panel-material-content')), ('Offscreen popup stayed visible', hidden)
        count = page.evaluate('draws')
        wait(900)
        idle = page.evaluate('draws') - count
        assert idle == 0, ('Offscreen popup continued painting', idle)
        report['checks']['offscreenIdleDraws'] = idle
        wheel(-(source_before - hidden['anchor'][1]))
        wait(1300)
        assert primitive('popovercontent')['flyoutMotion']['sourceVisible']
        # The source is partly clipped under the fixed page header. The popup
        # may escape the scrolling viewport; the button must not escape it.
        page.set_viewport_size({'width': args.width, 'height': 480})
        wait(1000)
        wheel(primitive('popovercontent')['flyoutMotion']['anchor'][1] - 44)
        wait(1700)
        b = bounds('liquid-popovercontent-trigger')
        assert abs(b['y'] - 44) < 1, ('Source did not reach the clip edge', b)
        panel = bounds('liquid-popovercontent-panel-material-content')
        assert panel['y'] >= 76, panel
        open_header = shot('flyout-source-clipped-open')
        visible_button = inputs.element('liquid-popovercontent-trigger')['visible_bounds']
        assert visible_button['height'] < b['height'], visible_button
        page.mouse.click(visible_button['x'] + visible_button['width'] / 2, visible_button['y'] + visible_button['height'] / 2)
        wait(1700)
        assert not primitive('popovercontent')['flyout'], 'The original source lost its click handler'
        closed_header = shot('flyout-source-clipped-closed')
        region = tuple(round(v * 2) for v in (b['x'] - 2, 42, b['x'] + b['width'] + 2, 51))
        assert open_header.crop(region).tobytes() == closed_header.crop(region).tobytes(), 'Source paint escaped its page clipping layer'
        report['checks']['sourceClipsBelowHeader'] = True
        report['checks']['originalSourceTogglesAfterScroll'] = True
        page.set_viewport_size({'width': args.width, 'height': 900})
        wait(900)
        inputs.locate('liquid-menubar-file')
        wait(900)
        inputs.click('liquid-menubar-file')
        wait(1500)
        first = settling('menubar')
        shot('menu-file')

        def menu_frame():
            m = primitive('menubar')['menubar'][0]['materials'][0]
            return {k: v for k, v in m.items() if k != 'path'} | {'pathHash': hashlib.sha256(m['path'].encode()).hexdigest()}
        frames = [menu_frame()]
        e = inputs.locate('liquid-menubar-edit')['center']
        page.evaluate('''() => {
            window.menuTransferFrames = []; window.menuTransferDone = false;
            const start = performance.now();
            function sample(now) {
                const state = JSON.parse(zorkStory.story_state());
                const frame = state.cards.find(c => c.kind === 'menubar').primitive.menubar[0].materials[0];
                window.menuTransferFrames.push({...frame, atMs: now - start});
                if (now - start < 650) requestAnimationFrame(sample);
                else window.menuTransferDone = true;
            }
            requestAnimationFrame(sample);
        }''')
        page.mouse.move(e['x'], e['y'])
        wait(35)
        shot('menu-transfer-middle')
        page.wait_for_function('window.menuTransferDone')
        for frame in page.evaluate('window.menuTransferFrames'):
            path = frame.pop('path')
            frames.append(frame | {'pathHash': hashlib.sha256(path.encode()).hexdigest()})
        last = settling('menubar')
        shot('menu-edit')
        xs = [f['pose']['cx'] for f in frames]
        assert max(xs) - min(xs) > 20 and len(set((round(x, 2) for x in xs))) >= (2 if args.reduced_motion else 5), ('No continuous menu transfer', xs)
        assert all((f['progress'] > 0.99 for f in frames)), ('Menu was restarted', frames)
        report['checks']['menuTransfer'] = {'positions': len(set((round(x, 2) for x in xs))), 'range': [min(xs), max(xs)]}
        report['sequences']['menuTransfer'] = frames
        e = inputs.locate('liquid-menubar-file')['center']
        page.mouse.move(e['x'], e['y'])
        wait(30)
        middle = menu_frame()
        e = inputs.locate('liquid-menubar-edit')['center']
        page.mouse.move(e['x'], e['y'])
        inputs.frame()
        rev = menu_frame()
        assert rev['progress'] > 0.99
        settling('menubar')
        page.keyboard.press('Escape')
        wait(1700)
        shot('menu-closed')
        report['checks']['menuReverse'] = True
        # A read-only anchored card uses the same content presentation as the
        # modal, with its own hover lifetime and no modal backdrop.
        trigger = inputs.locate('liquid-hovercard-trigger')['center']
        page.mouse.move(trigger['x'], trigger['y'])
        wait(750)
        details = primitive('hovercard')['details']
        assert details['active'] and not details['closing'], details
        if not args.reduced_motion:
            assert details['material']['paintOnlyFrames'] > 0, 'Details card did not use shared presentation'
        panel = inputs.locate('detail-tooltip-shared-content')['center']
        page.mouse.move(panel['x'], panel['y'])
        wait(250)
        assert not primitive('hovercard')['details']['closing'], 'Crossing into the card closed it'
        shot('shared-floating-card')
        opened_frames = details['material']['paintOnlyFrames']
        page.mouse.move(args.width - 8, 895)
        deadline = time.monotonic() + 6
        while primitive('hovercard')['details']['active'] and time.monotonic() < deadline:
            inputs.frame()
        closed_details = primitive('hovercard')['details']
        assert not closed_details['active'], ('Details card did not retire', closed_details)
        if not args.reduced_motion:
            assert closed_details['material']['paintOnlyFrames'] > opened_frames
        assert not inputs.element('detail-tooltip-shared-content'), 'Closed card retained input'
        before = page.evaluate('draws')
        wait(900)
        assert page.evaluate('draws') == before, 'Closed details card kept painting'
        report['checks']['sharedFloatingPresentation'] = {'openedFrames': opened_frames,
            'totalFrames': closed_details['material']['paintOnlyFrames'], 'closedIdleDraws': 0}
        select('liquid-tab-8')
        wheel(-10000)
        wait(900)
        inputs.locate('liquid-toolbar-bold')
        if 0 in primitive('toolbar')['selected']:
            inputs.click('liquid-toolbar-bold')
        wait(900)
        bold = bounds('liquid-toolbar-bold')
        points = [(bold['x'] + 1, bold['y'] + 1), (bold['x'] + 1, bold['y'] + bold['height'] - 1)]
        baseline = shot('toolbar-unselected')
        base_pixels = [baseline.getpixel((round(x * 2), round(y * 2))) for x, y in points]
        inputs.click('liquid-toolbar-bold')
        wait(900)
        im = shot('toolbar-selected')
        pixels = [im.getpixel((round(x * 2), round(y * 2))) for x, y in points]
        assert all((min(c) >= 250 for c in pixels)), ('Opaque exterior leaked outside toolbar', pixels, bold, base_pixels)
        center = inputs.element('liquid-toolbar-bold')['center']
        page.mouse.move(center['x'], center['y'])
        page.mouse.down()
        wait(90)
        im = shot('toolbar-held')
        pixels_held = [im.getpixel((round(x * 2), round(y * 2))) for x, y in points]
        page.mouse.up()
        page.mouse.move(args.width - 8, 895)
        wait(1700)
        assert all((min(c) >= 250 for c in pixels_held)), ('Held exterior leaked', pixels_held)
        report['checks']['toolbarExterior'] = {'selected': pixels, 'held': pixels_held}
        before = page.evaluate('draws')
        wait(900)
        idle = page.evaluate('draws') - before
        assert idle == 0, ('Idle paints', idle)
        report['checks']['idleDraws'] = idle
        report['passed'] = not report['errors']
    finally:
        (OUT / 'result.json').write_text(json.dumps(report, ensure_ascii=False, indent=2))
        shot('last-frame')
        (OUT / 'last-snapshot.json').write_text(json.dumps(snapshot(), ensure_ascii=False, indent=2))
        browser.close()
print(json.dumps({'passed': report['passed'], 'checks': report['checks'], 'errors': report['errors']}, ensure_ascii=False))

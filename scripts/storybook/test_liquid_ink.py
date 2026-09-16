# /// script
# dependencies = ["playwright==1.58.0", "pillow==11.3.0"]
# ///
"""Real-input and pixel checks for liquid split ink and the primary send action."""
import argparse
from io import BytesIO
import hashlib
import json
import math
from pathlib import Path

from PIL import Image
from playwright.sync_api import sync_playwright
from playground_inputs import PlaygroundInputs


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--backend', choices=['auto', 'webgl'], default='auto')
    parser.add_argument('--dpr', type=float, default=2)
    parser.add_argument('--wasm-artifact', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    report = {'passed': False, 'errors': [], 'checks': {}, 'dpr': args.dpr}
    with sync_playwright() as pw:
        binary = (Path.home() / 'Library/Caches/ms-playwright/chromium-1228/'
                  'chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/'
                  'Google Chrome for Testing')
        browser = pw.chromium.launch(headless=True, executable_path=str(binary),
                                    args=[f'--force-device-scale-factor={args.dpr}'])
        context = browser.new_context(viewport={'width': 1440, 'height': 900}, device_scale_factor=args.dpr)
        context.add_init_script('''window.inkDraws=0;window.inkFrames=0;
            const requestFrame=window.requestAnimationFrame.bind(window);
            window.requestAnimationFrame=callback=>requestFrame(t=>{
                const before=window.inkDraws;callback(t);
                if(window.inkDraws!==before)window.inkFrames++;});
            if(window.GPUQueue){const f=GPUQueue.prototype.submit;
                GPUQueue.prototype.submit=function(...a){window.inkBackend='webgpu';window.inkDraws++;return f.apply(this,a);};}
            for(const name of ['WebGLRenderingContext','WebGL2RenderingContext']){
                const p=window[name]?.prototype;if(!p)continue;
                for(const name of ['drawArrays','drawElements','drawArraysInstanced','drawElementsInstanced']){
                    const f=p[name];if(!f)continue;p[name]=function(...a){window.inkBackend='webgl';window.inkDraws++;return f.apply(this,a);};}}
        ''')
        page = context.new_page()
        page.on('pageerror', lambda e: report['errors'].append(str(e)))

        def deliver(route):
            response = route.fetch()
            body = response.body()
            assert body == args.wasm_artifact.read_bytes(), 'Wrong WASM artifact'
            report['artifactSha256'] = hashlib.sha256(body).hexdigest()
            route.fulfill(response=response, body=body)

        page.route('**/pkg/zork_gui_web_bg.wasm.gz', deliver)
        snapshot = lambda: page.evaluate('JSON.parse(zorkStory.snapshot())')
        state = lambda: page.evaluate('JSON.parse(zorkStory.story_state())')
        inputs = PlaygroundInputs(page, snapshot)
        composer = lambda: next(c for c in state()['cards'] if c['kind'] == 'composer')

        def capture(name, clip=None):
            raw = page.screenshot(clip=clip)
            (args.output / f'{name}.png').write_bytes(raw)
            return Image.open(BytesIO(raw)).convert('RGB')

        try:
            page.goto(args.url + '?story=liquid-gallery&backend=' + args.backend)
            page.wait_for_function('document.documentElement.dataset.ready==="true"', timeout=120000)
            report['actualBackend'] = page.evaluate('window.inkBackend')
            assert report['actualBackend'] == ('webgpu' if args.backend == 'auto' else 'webgl')
            context.set_offline(True)
            page.wait_for_timeout(400)
            capture('initial')
            inputs.click('liquid-inspect-composer')
            inputs.click('liquid-composer-variant-1')
            inputs.click('liquid-composer-departure-mode-0')
            page.mouse.move(1400, 850)
            page.wait_for_timeout(1400)
            options = [inputs.locate(f'liquid-composer-departure-mode-{i}') for i in range(2)]
            assert all(e['bounds']['height'] == 32 for e in options)
            box = options[0]['bounds']
            clip = {'x': math.floor(box['x'] - 4), 'y': math.floor(box['y']), 'width': 256, 'height': 32}
            first = capture('selected-first', clip)
            inputs.click('liquid-composer-departure-mode-1')
            page.mouse.move(1400, 850)
            page.wait_for_timeout(1400)
            second = capture('selected-second', clip)
            # Use contrasting glyph cores in both settled reference images,
            # including antialiased one-device-pixel strokes at 1x density.
            # A mixed frame must split a single Chinese glyph, not just change
            # the selected label or its overall opacity.
            glyphs = []
            for index, text in enumerate(['从发送按钮', '从输入区']):
                center = options[index]['center']['x'] - clip['x']
                for char in range(len(text)):
                    left = round((center - len(text) * 6 + char * 12) * args.dpr)
                    right = round((center - len(text) * 6 + (char + 1) * 12) * args.dpr)
                    outside, inside = (second, first) if index == 0 else (first, second)
                    pixels = [(x, y) for x in range(left, right) for y in range(first.height)
                              if min(inside.getpixel((x, y))) - max(outside.getpixel((x, y))) > 60]
                    assert len(pixels) >= 4, (index, char, 'missing contrasting glyph')
                    glyphs.append((index, char, pixels))
            report['checks']['settledGlyphs'] = len(glyphs)

            inputs.click('liquid-slow')
            inputs.click('liquid-composer-departure-mode-0')
            page.wait_for_timeout(3000)
            target = inputs.locate('liquid-composer-departure-mode-1')
            page.mouse.click(target['center']['x'], target['center']['y'])
            mixed = []
            for frame in range(36):
                image = capture(f'move-{frame:02}', clip)
                for index, char, pixels in glyphs:
                    outside, inside = (second, first) if index == 0 else (first, second)
                    matches = lambda a, b: max(abs(x-y) for x, y in zip(a, b)) < 20
                    white = sum(matches(image.getpixel(p), inside.getpixel(p)) for p in pixels)
                    black = sum(matches(image.getpixel(p), outside.getpixel(p)) for p in pixels)
                    if min(white, black) >= max(3, len(pixels) * .15):
                        mixed.append({'frame': frame, 'label': index, 'character': char, 'white': white, 'black': black})
                if frame % 6 == 5 and frame < 30:
                    other = options[(frame // 6) % 2]
                    page.mouse.click(other['center']['x'], other['center']['y'])
                page.wait_for_timeout(25)
            assert mixed, 'No frame split the ink inside a single glyph'
            report['checks']['splitGlyphsDuringMoveAndReverse'] = mixed
            inputs.click('liquid-slow')
            inputs.click('liquid-composer-departure-mode-0')
            for key, expected in [('End', 'Composer'), ('Home', 'Button'), ('ArrowRight', 'Composer'), ('ArrowLeft', 'Button')]:
                page.keyboard.press(key)
                page.wait_for_timeout(350)
                assert composer()['departureOrigin'] == expected, (key, composer()['departureOrigin'])
            report['checks']['keyboard'] = True

            send = inputs.locate('liquid-composer-send')
            page.mouse.move(1400, 850)
            page.wait_for_timeout(700)
            image = capture('composer-restored')
            x, y = send['center']['x'], send['center']['y']
            sample = image.getpixel((round((x - 7) * args.dpr), round(y * args.dpr)))
            assert max(abs(a-b) for a,b in zip(sample, (233, 100, 59))) <= 4, ('send theme', sample)
            button = image.crop(tuple(round(v * args.dpr) for v in (x-10, y-10, x+10, y+10)))
            assert sum(min(p) > 220 for p in button.getdata()) > 8, 'No white send arrow'
            report['checks']['sendThemeRgb'] = sample
            page.mouse.move(x, y)
            page.wait_for_timeout(500)
            hover = capture('send-hover')
            hover_color = hover.getpixel((round((x - 7) * args.dpr), round(y * args.dpr)))
            assert max(abs(a-b) for a,b in zip(hover_color, (219, 87, 47))) <= 4, ('send hover', hover_color)
            inputs.click('liquid-composer-variant-0')
            assert not inputs.locate('liquid-composer-send')['enabled']
            capture('send-disabled')
            inputs.click('liquid-composer-variant-1')

            page.set_viewport_size({'width': 320, 'height': 820})
            inputs.locate('liquid-composer-departure-mode-1')
            page.wait_for_timeout(1000)
            for i in range(2):
                e = inputs.locate(f'liquid-composer-departure-mode-{i}')
                assert e['bounds']['x'] >= 0 and e['bounds']['x'] + e['bounds']['width'] <= 320
            capture('narrow')
            page.mouse.move(318, 818)
            page.wait_for_timeout(3200)
            before = page.evaluate('window.inkFrames')
            page.wait_for_timeout(2000)
            idle = page.evaluate('window.inkFrames') - before
            assert idle <= 4, ('idle painted frames', idle)
            report['checks']['narrowAndIdleFrames'] = idle
            assert not report['errors'], report['errors']
            report['passed'] = True
        except Exception as error:
            report['errors'].append(str(error))
            capture('failure')
            (args.output / 'failure-snapshot.json').write_text(json.dumps(snapshot(), ensure_ascii=False, indent=2))
            raise
        finally:
            (args.output / 'result.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
            browser.close()
    print(json.dumps(report, ensure_ascii=False))


if __name__ == '__main__':
    main()

# /// script
# dependencies = ["playwright==1.58.0", "pillow==11.3.0"]
# ///
"""Check real hairline pixels, including the closed perimeter and focus state."""
import argparse
from collections import deque
from io import BytesIO
import json
from pathlib import Path

from PIL import Image
from playwright.sync_api import sync_playwright

from playground_inputs import PlaygroundInputs


def check_outline(image, bounds, dpr):
    left, top, right, bottom = [round(v * dpr) for v in (
        bounds['x'], bounds['y'], bounds['x'] + bounds['width'],
        bounds['y'] + bounds['height'],
    )]
    cx, cy = (left + right) // 2, (top + bottom) // 2
    background = image.getpixel((cx, top - 2))
    contrast = lambda p: max(abs(a - b) for a, b in zip(p, background))
    sides = {
        'top': (cx, top, 0, 1), 'bottom': (cx, bottom - 1, 0, -1),
        'left': (left, cy, 1, 0), 'right': (right - 1, cy, -1, 0),
    }
    colors = {}
    for side, (x, y, dx, dy) in sides.items():
        colors[side] = image.getpixel((x, y))
        assert contrast(colors[side]) >= 20, (side, colors)
        # The hairline belongs wholly to the component. At 2x it occupies one
        # physical pixel; the neighboring outside and inside pixels are clear.
        for offset in [-1, 1]:
            neighbor = image.getpixel((x + offset * dx, y + offset * dy))
            assert contrast(neighbor) <= 3, (side, offset, neighbor, colors)
    for color in colors.values():
        assert max(abs(a - b) for a, b in zip(color, colors['top'])) <= 3, colors

    # Flood the background from outside the whole control. A complete 8-connected
    # antialiased border blocks this 4-connected flood, including at curved joins.
    # This catches small corner gaps which four midpoint samples would miss.
    crop = image.crop((left - 2, top - 2, right + 2, bottom + 2))
    width, height = crop.size
    reached = {(0, 0)}
    pending = deque(reached)
    while pending:
        x, y = pending.popleft()
        for p in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]:
            if (p not in reached and 0 <= p[0] < width and 0 <= p[1] < height
                    and contrast(crop.getpixel(p)) < 8):
                reached.add(p)
                pending.append(p)
    interior = (width // 2, 2 + round(4 * dpr))
    assert contrast(crop.getpixel(interior)) <= 3, ('interior', crop.getpixel(interior))
    assert interior not in reached, 'The border has an open gap'
    return {'sides': colors, 'closedPerimeter': True, 'insideOnly': True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--backend', choices=['auto', 'webgl'], default='auto')
    parser.add_argument('--dpr', type=int, choices=[1, 2], default=2)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    report = {'backend': args.backend, 'dpr': args.dpr, 'checks': {}, 'errors': []}
    with sync_playwright() as pw:
        binary = (Path.home() / 'Library/Caches/ms-playwright/chromium-1228/'
                  'chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/'
                  'Google Chrome for Testing')
        browser = pw.chromium.launch(headless=True, executable_path=str(binary),
                                    args=[f'--force-device-scale-factor={args.dpr}'])
        context = browser.new_context(viewport={'width': 1440, 'height': 900},
                                      device_scale_factor=args.dpr, reduced_motion='reduce')
        context.add_init_script('''if(window.GPUQueue){const submit=GPUQueue.prototype.submit;
            GPUQueue.prototype.submit=function(...args){window.playgroundUsedWebGPU=true;
            return submit.apply(this,args);};}''')
        page = context.new_page()
        page.on('pageerror', lambda error: report['errors'].append(str(error)))
        try:
            page.goto(args.url + '?story=liquid-gallery&backend=' + args.backend)
            page.wait_for_function('document.documentElement.dataset.ready==="true"', timeout=120000)
            report['actualBackend'] = page.evaluate('window.playgroundUsedWebGPU?"webgpu":"webgl"')
            context.set_offline(True)
            snapshot = lambda: page.evaluate('JSON.parse(zorkStory.snapshot())')
            inputs = PlaygroundInputs(page, snapshot)

            def capture(id, name, move_pointer=True):
                element = inputs.locate(id)
                if move_pointer:
                    page.mouse.move(1438, 898)
                page.wait_for_timeout(350)
                raw = page.screenshot()
                (args.output / f'{name}.png').write_bytes(raw)
                image = Image.open(BytesIO(raw)).convert('RGB')
                report['checks'][name] = check_outline(image, element['bounds'], args.dpr)
                return element

            capture('liquid-actions-action-1', 'liquid-button')
            capture('liquid-fields-name', 'wide-field')
            # Physical Tab input must replace the border with the focus color,
            # retaining the same one-pixel geometry and an uninterrupted contour.
            inputs.click('liquid-actions-action-0')
            page.mouse.move(1438, 898)
            page.keyboard.press('Tab')
            capture('liquid-actions-action-1', 'focused-button', move_pointer=False)
            assert (report['checks']['focused-button']['sides']['top']
                    != report['checks']['liquid-button']['sides']['top']), 'No keyboard focus border'
            inputs.click('liquid-business-composer')
            capture('liquid-composer-variant-select', 'field-trigger')
            assert not report['errors'], report['errors']
            report['passed'] = True
        finally:
            (args.output / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
            browser.close()
    print(json.dumps(report))


if __name__ == '__main__':
    main()

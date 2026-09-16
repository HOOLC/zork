# /// script
# dependencies = ["playwright==1.58.0"]
# ///
"""Keyboard lifetime of content-sized panels through their real source actions."""
import argparse
import json
from pathlib import Path
from playwright.sync_api import sync_playwright
from playground_inputs import PlaygroundInputs

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--url', required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--backend', choices=['auto', 'webgl'], default='auto')
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
report = {'passed': False, 'errors': [], 'panels': {}}
with sync_playwright() as pw:
    binary = Path.home() / 'Library/Caches/ms-playwright/chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing'
    browser = pw.chromium.launch(headless=True, executable_path=str(binary), args=['--force-device-scale-factor=2'])
    context = browser.new_context(viewport={'width': 798, 'height': 837}, device_scale_factor=2)
    context.add_init_script('''if(window.GPUQueue){const submit=GPUQueue.prototype.submit;GPUQueue.prototype.submit=function(...args){window.panelUsesGpu=true;return submit.apply(this,args);};}''')
    page = context.new_page()
    page.on('pageerror', lambda error: report['errors'].append(str(error)))
    try:
        page.goto(args.url + '?story=liquid-gallery&backend=' + args.backend)
        page.wait_for_function('document.documentElement.dataset.ready === "true"', timeout=120000)
        context.set_offline(True)
        report['backend'] = page.evaluate('window.panelUsesGpu ? "webgpu" : "webgl"')
        inputs = PlaygroundInputs(page, lambda: page.evaluate('JSON.parse(zorkStory.snapshot())'))
        def card(kind):
            return page.evaluate('kind => JSON.parse(zorkStory.story_state()).cards.find(c => c.kind === kind)', kind)
        for kind in ['attachments', 'disclosure', 'comments']:
            inputs.click('liquid-inspect-' + kind)
            inputs.click('liquid-' + kind + '-trigger')
            page.wait_for_timeout(700)
            assert card(kind)['open'], kind
            opened = card(kind)['surfaces'][0]['pose']['h']
            for _ in range(3):
                # No click inside the panel may be needed to make Esc work.
                page.keyboard.press('Escape')
                inputs.frame()
                assert not card(kind)['open'], f'{kind}: source focus was lost on expansion'
                page.wait_for_timeout(750)
                assert inputs.element('liquid-' + kind + '-trigger'), kind
                page.keyboard.press('Enter')
                inputs.frame()
                assert card(kind)['open'], f'{kind}: close did not restore source focus'
                page.wait_for_timeout(700)
            panel = card(kind)
            assert all(s['error'] is None and not s['paintError'] for s in panel['surfaces']), panel
            assert abs(panel['surfaces'][0]['pose']['h'] - opened) < .5, kind
            page.screenshot(path=str(args.output / (kind + '-open.png')))
            report['panels'][kind] = {'openHeight': opened, 'escapeReturnCycles': 3}
            page.keyboard.press('Escape')
            page.wait_for_timeout(750)
        assert not report['errors'], report['errors']
        report['passed'] = True
        print(json.dumps(report, ensure_ascii=False))
    finally:
        page.screenshot(path=str(args.output / 'last.png'))
        (args.output / 'result.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
        context.close()
        browser.close()

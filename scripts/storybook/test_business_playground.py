# /// script
# dependencies = ["playwright==1.58.0"]
# ///
"""Walk every advertised business specimen using its real directory and state picker."""
import argparse
import hashlib
import json
import time
from pathlib import Path
from playwright.sync_api import sync_playwright
from playground_inputs import PlaygroundInputs

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--url', required=True)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--wasm-artifact', type=Path, required=True)
p.add_argument('--native-catalog', type=Path, required=True)
p.add_argument('--width', type=int, default=1440)
p.add_argument('--height', type=int, default=1000)
p.add_argument('--families-only', action='store_true')
p.add_argument('--family', action='append')
args = p.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
report = {'passed': False, 'errors': [], 'cases': [], 'viewport': [args.width, args.height]}
with sync_playwright() as pw:
    binary = Path.home() / 'Library/Caches/ms-playwright/chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing'
    browser = pw.chromium.launch(headless=True, executable_path=str(binary), args=["--force-device-scale-factor=2"])
    context = browser.new_context(viewport={'width': args.width, 'height': args.height}, device_scale_factor=2)
    page = context.new_page()
    page.on('pageerror', lambda error: report['errors'].append(str(error)))
    def deliver(route):
        response = route.fetch()
        data = response.body()
        assert data == args.wasm_artifact.read_bytes(), 'Loaded a different build'
        report['sha256'] = hashlib.sha256(data).hexdigest()
        route.fulfill(response=response, body=data)
    page.route('**/pkg/zork_gui_web_bg.wasm.gz', deliver)
    try:
        page.goto(args.url + '?story=liquid-gallery&backend=webgl')
        page.wait_for_function('document.documentElement.dataset.ready === "true"', timeout=120000)
        assert 'sha256' in report
        context.set_offline(True)
        snapshot = lambda: page.evaluate('JSON.parse(zorkStory.snapshot())')
        state = lambda: page.evaluate('JSON.parse(zorkStory.story_state())')
        report['actualViewport'] = snapshot()['viewport']
        assert report['actualViewport'] == {'width': args.width, 'height': args.height}, report['actualViewport']
        inputs = PlaygroundInputs(page, snapshot)
        catalog = page.evaluate('JSON.parse(zorkStory.catalog())')
        native = json.loads(args.native_catalog.read_text())
        assert catalog == native, 'Native and Web expose different component contracts'
        families = state()['businessCatalog']
        report['families'] = families
        report['catalogCount'] = len(catalog)
        def close_business_overlay():
            # Full-screen previews and modal editors cover the toolbar too.
            # Escape is the same physical dismissal used in the application.
            for _ in range(4):
                page.keyboard.press('Escape')
                page.wait_for_timeout(300)
                if not any(e['visible'] and (e['id'].endswith(('-dialog', '-modal')) or e['id'] == 'drive-preview')
                           for e in snapshot()['elements']):
                    break
            page.wait_for_timeout(800)
        def settle(story):
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                value = state()['businessExample']
                if value and value['story'] == story['id']:
                    assert not value['error'], value
                    if value['pending'] == 0:
                        page.wait_for_timeout(600)
                        return
                page.wait_for_timeout(100)
            raise AssertionError(('Specimen did not become ready', story['id'], value))
        for family, _ in families:
            if args.family and family not in args.family:
                continue
            close_business_overlay()
            inputs.click('liquid-section-scenarios')
            inputs.click('liquid-business-' + family)
            choices = [story for story in catalog if story['family'] == family and not story['state'].endswith('-wide')]
            if args.families_only:
                choices = choices[:1]
            for index, story in enumerate(choices):
                if index:
                    close_business_overlay()
                    inputs.click('business-state-' + story['id'])
                settle(story)
                assert state()['businessFamily'] == family
                assert not report['errors'], report['errors']
                observed = snapshot()
                visible = [e for e in observed['elements'] if e['visible'] and (e['id'].startswith('liquid-composer-') or not e['id'].startswith(('liquid-', 'business-state')))]
                assert visible, ('No production component was mounted', story['id'])
                bad = [e for e in visible if e['role'] in ('button', 'text_input', 'option') and e['enabled']
                       and e['visible_bounds']['width'] > 0 and (e['bounds']['x'] < -.5 or e['bounds']['x'] + e['bounds']['width'] > args.width + .5)]
                assert not bad, ('Controls exceed viewport', story['id'], bad)
                (args.output / (story['id'] + '.json')).write_text(json.dumps(observed, ensure_ascii=False, indent=2))
                page.screenshot(path=str(args.output / (story['id'] + '.png')))
                report['cases'].append({'id': story['id'], 'visibleControls': len(visible), 'pending': 0})
                print('PASS', story['id'], flush=True)
        report['passed'] = True
    except Exception as error:
        report['failure'] = str(error)
        page.screenshot(path=str(args.output / 'failure.png'))
        raise
    finally:
        (args.output / 'report.json').write_text(json.dumps(report, ensure_ascii=False, indent=2))
        browser.close()

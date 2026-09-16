# /// script
# dependencies = ["playwright==1.58.0"]
# ///
"""Real browser input through shared GPUI cards and the offline core adapter."""
import argparse
import functools
from http.server import ThreadingHTTPServer, SimpleHTTPRequestHandler
import json
from pathlib import Path
import threading
from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[2]

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--web-root', type=Path, default=ROOT / 'apps/zork-design/components/web')
    parser.add_argument('--output', type=Path, default=ROOT / 'artifacts/interactive-messages/web')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    class Quiet(SimpleHTTPRequestHandler):
        def log_message(self, *_): pass
    server = ThreadingHTTPServer(('127.0.0.1', 0), functools.partial(Quiet, directory=str(args.web_root)))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        with sync_playwright() as pw:
            binary = Path.home() / 'Library/Caches/ms-playwright/chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing'
            browser = pw.chromium.launch(headless=True, executable_path=str(binary))
            context = browser.new_context(viewport={'width': 900, 'height': 700})
            page = context.new_page()
            errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))
            page.goto(f'http://127.0.0.1:{server.server_port}/index.html?story=message-interaction-input-wide')
            page.wait_for_function('document.documentElement.dataset.ready==="true"', timeout=60000)
            context.set_offline(True)
            ime = context.new_cdp_session(page)
            def state(): return page.evaluate('JSON.parse(window.zorkStory.story_state())')
            def click(id):
                element = page.wait_for_function('(id)=>JSON.parse(window.zorkStory.snapshot()).elements.find(e=>e.id===id&&e.visible)', arg=id).json_value()
                page.mouse.click(element['center']['x'], element['center']['y'])
                page.evaluate('()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))')
            checked = []
            for suffix, width, height in [('wide', 900, 700), ('compact', 420, 760)]:
                page.set_viewport_size({'width': width, 'height': height})
                story = 'message-interaction-input-' + suffix
                revision = page.evaluate('JSON.parse(window.zorkStory.snapshot()).revision')
                page.evaluate('(id)=>window.zorkStory.select_story(id)', story)
                page.wait_for_function('([id,rev])=>JSON.parse(window.zorkStory.story_state()).id===id&&JSON.parse(window.zorkStory.snapshot()).revision>rev', arg=[story, revision])
                click('interaction-preview-submit')
                try:
                    page.wait_for_function('JSON.parse(window.zorkStory.story_state()).fields[0].error_key==="input_required"', timeout=5000)
                except Exception:
                    page.screenshot(path=str(args.output / 'failure.png'))
                    (args.output / 'failure.json').write_text(json.dumps({'state': state(), 'ui': page.evaluate('JSON.parse(window.zorkStory.snapshot())'), 'errors': errors}, ensure_ascii=False, indent=2))
                    raise
                click('interaction-preview-title')
                value = '实现消息卡片 ✓'
                end = len(value.encode('utf-16-le')) // 2
                ime.send('Input.imeSetComposition', {'text': value, 'selectionStart': end, 'selectionEnd': end})
                ime.send('Input.insertText', {'text': value})
                click('interaction-preview-target')
                click('interaction-preview-target-1')
                click('interaction-preview-submit')
                try:
                    page.wait_for_function('JSON.parse(window.zorkStory.story_state()).status_key==="interaction_completed"', timeout=5000)
                except Exception:
                    page.screenshot(path=str(args.output / 'failure.png'))
                    (args.output / 'failure.json').write_text(json.dumps({'state': state(), 'ui': page.evaluate('JSON.parse(window.zorkStory.snapshot())'), 'errors': errors}, ensure_ascii=False, indent=2))
                    raise
                result = state()
                assert result['fields'][0]['value'] == '实现消息卡片 ✓'
                assert result['fields'][1]['value'] == 'preview'
                assert not result['editable'] and not result['actions']
                assert not errors, errors
                page.screenshot(path=str(args.output / (story + '.png')))
                (args.output / (story + '.json')).write_text(json.dumps(result, ensure_ascii=False, indent=2))
                checked.append(story)
                print('PASS Web', story, flush=True)
            story = 'message-interaction-create-wide'
            page.set_viewport_size({'width': 900, 'height': 700})
            page.evaluate('(id)=>window.zorkStory.select_story(id)', story)
            page.wait_for_function('(id)=>JSON.parse(window.zorkStory.story_state()).id===id', arg=story)
            assert not page.evaluate('JSON.parse(window.zorkStory.snapshot()).elements.some(e=>e.id==="interaction-preview-/allowed_leaders")')
            click('interaction-preview-/name')
            page.keyboard.press('ControlOrMeta+A')
            value = '网页审查助手'
            end = len(value.encode('utf-16-le')) // 2
            ime.send('Input.imeSetComposition', {'text': value, 'selectionStart': end, 'selectionEnd': end})
            ime.send('Input.insertText', {'text': value})
            click('interaction-preview-settings')
            click('interaction-preview-/allowed_leaders')
            click('interaction-preview-/allowed_leaders-0')
            click('interaction-preview-/name')
            click('interaction-preview-settings')
            click('interaction-preview-settings')
            click('interaction-preview-submit')
            page.wait_for_function('JSON.parse(window.zorkStory.story_state()).status_key==="interaction_agent_created"')
            result = state()
            (args.output / 'create-settings-retained.json').write_text(json.dumps(result, ensure_ascii=False, indent=2))
            assert result['fields'][0]['value'] == '网页审查助手', result['fields'][0]['value']
            assert result['fields'][3]['value'] == '[]'
            checked.append(story + '-settings-retained')
            page.screenshot(path=str(args.output / 'create-settings-retained.png'))
            # Select a different fixture before returning, which creates a fresh request.
            page.evaluate('window.zorkStory.select_story("message-interaction-input-wide")')
            page.wait_for_function('JSON.parse(window.zorkStory.story_state()).id==="message-interaction-input-wide"')
            page.evaluate('(id)=>window.zorkStory.select_story(id)', story)
            page.wait_for_function('(id)=>JSON.parse(window.zorkStory.story_state()).id===id', arg=story)
            click('interaction-preview-decline')
            page.wait_for_function('JSON.parse(window.zorkStory.story_state()).status_key==="interaction_declined"')
            checked.append(story)
            (args.output / 'result.json').write_text(json.dumps({'checked': checked, 'offline_after_load': True, 'errors': errors}, indent=2))
            browser.close()
    finally:
        server.shutdown()
        server.server_close()

if __name__ == '__main__': main()

# /// script
# dependencies = ["playwright==1.58.0"]
# ///
"""Submit the real consent form in Chromium; OAuth itself stays a local fixture."""
import base64,hashlib,json,secrets,sys,urllib.request
from pathlib import Path
from playwright.sync_api import sync_playwright
origin,output=sys.argv[1:]
output=Path(output);output.mkdir(parents=True,exist_ok=True)
secret=lambda:base64.urlsafe_b64encode(secrets.token_bytes(32)).decode().rstrip('=')
identifier,verifier=secret(),secret()
def post(path,body):
 req=urllib.request.Request(origin+path,json.dumps(body).encode(),{'content-type':'application/json'})
 with urllib.request.urlopen(req,timeout=10) as response:return json.load(response)
started=post('/v1/auth/device',{'id':identifier,'code_challenge':base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).decode().rstrip('='),'name':'Browser regression'})
messages=[]
posted=[]
intercepted=[]
try:
 with sync_playwright() as pw:
  binaries=list((Path.home()/'Library/Caches/ms-playwright').glob('chromium-*/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing'))
  browser=pw.chromium.launch(headless=True,executable_path=str(max(binaries,key=lambda p:p.parts[-6])) if binaries else None)
  page=browser.new_page()
  page.on('console',lambda message:messages.append(message.text))
  page.on('requestfailed',lambda request:messages.append(str(request.failure)))
  page.on('request',lambda request:posted.append({'origin':request.headers.get('origin'),'content_type':request.headers.get('content-type')}) if request.method=='POST' else None)
  cdp=page.context.new_cdp_session(page)
  def intercept(params):
   intercepted.append(True)
   cdp.send('Fetch.fulfillRequest',{'requestId':params['requestId'],'responseCode':200,'responseHeaders':[{'name':'Content-Type','value':'text/html; charset=utf-8'}],'body':base64.b64encode(b'<h1 id="google-fixture">Google authorization redirect reached</h1>').decode()})
  cdp.on('Fetch.requestPaused',intercept)
  cdp.send('Fetch.enable',{'patterns':[{'urlPattern':'https://accounts.google.com/*','requestStage':'Request'}]})
  page.goto(started['verification_uri'])
  page.get_by_role('button',name='使用 Google 账号继续').click(no_wait_after=True)
  try:page.locator('#google-fixture').wait_for(timeout=5000);passed=True
  except Exception:passed=False
  try:page.screenshot(path=str(output/'consent.png'),timeout=2000)
  except Exception:pass
  result={'passed':passed,'form_action_blocked':any('form-action' in m for m in messages),'diagnostics':[__import__('re').sub(r'https?://[^\s]+','[url]',m) for m in messages], 'page_origin':__import__('urllib.parse',fromlist=['urlparse']).urlparse(page.url).netloc,'browser':browser.version,'form_requests':posted,'google_requests_intercepted':len(intercepted),'scope':'real Chromium form submission and cross-origin navigation; Google response intercepted as a test fixture'}
  (output/'result.json').write_text(json.dumps(result,indent=2)+'\n')
  browser.close()
  print(json.dumps(result))
finally:post('/v1/auth/device/cancel',{'id':identifier,'code_verifier':verifier})
raise SystemExit(0 if passed else 1)

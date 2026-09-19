"""Unify functional SVG masters; keep product marks and external logos intact."""
from pathlib import Path
import xml.etree.ElementTree as ET
import json,shutil,hashlib
ROOT=Path(__file__).resolve().parents[1]
NATIVE=ROOT.parents[1]/'crates/zork-ui/assets'
ET.register_namespace('', 'http://www.w3.org/2000/svg')
extra={'home':'interface/home.svg','arrow-right':'interface/arrow-right.svg','clock':'interface/clock.svg','panel-right':'interface/panel-right.svg','reload':'interface/reload.svg','file':'interface/file.svg','filter':'interface/filter2.svg','columns':'interface/layout-column.svg','microphone':'interface/microphone-filled.svg','sparkles':'interface/sparkles.svg','checklist':'interface/list-checks.svg','puzzle':'interface/puzzle.svg','shapes':'interface/shapes-plus-x-square-circle.svg','settings-three':'interface/settings-slider-three.svg','loader':'interface/loader.svg','terminal':'icons/phosphor-terminal-window.svg','brain':'icons/phosphor-brain.svg','cube':'icons/phosphor-cube.svg','stop':'icons/phosphor-stop-fill.svg'}
manifest_path=ROOT/'assets/manifest.json';manifest=json.loads(manifest_path.read_text())
for name,source in extra.items():
 p=ROOT/f'assets/icons/interface/{name}.svg'
 if not p.exists():shutil.copy2(NATIVE/source,p)
 if not any(i['path']==str(p.relative_to(ROOT)) for i in manifest['items']):
  manifest['items'].append({'path':str(p.relative_to(ROOT)),'category':'interface-extension','status':'normalized-native-resource','source':str((NATIVE/source).relative_to(ROOT.parents[1])),'origin':'Phosphor / MIT' if source.startswith('icons/') else 'Central Icons shared company resource; existing project authorization','sha256':''})
for p in [*ROOT.joinpath('assets/icons/product').glob('*.svg'),*ROOT.joinpath('assets/icons/interface').glob('*.svg')]:
 backup=ROOT/'archive/pre-rounded-icons'/p.relative_to(ROOT/'assets/icons');backup.parent.mkdir(parents=True,exist_ok=True)
 if not backup.exists():shutil.copy2(p,backup)
 tree=ET.parse(p);root=tree.getroot();box=[float(x) for x in root.get('viewBox','0 0 24 24').split()];scale=24/box[2]
 for el in root.iter():
  if el.get('stroke') and el.get('stroke')!='none':el.set('stroke','currentColor')
  if el.get('stroke-width'):el.set('stroke-width',str(round(1.7/scale,4)))
  if el.tag.rsplit('}',1)[-1] in ['svg','g','path','line','polyline','polygon','rect']:
   el.set('stroke-linecap','round');el.set('stroke-linejoin','round')
  if el.tag.rsplit('}',1)[-1]=='rect' and float(el.get('rx','0'))==0 and float(el.get('width','0'))>8/scale and float(el.get('height','0'))>8/scale:el.set('rx',str(2/scale))
 if scale!=1:
  group=ET.Element('{http://www.w3.org/2000/svg}g',{'transform':f'scale({scale})'})
  for child in list(root):root.remove(child);group.append(child)
  root.append(group);root.set('viewBox','0 0 24 24')
 root.set('width','24');root.set('height','24')
 tree.write(p,encoding='unicode')
# A filled stop control needs rounded geometry, not a stroke-only correction.
(ROOT/'assets/icons/interface/stop.svg').write_text('<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><title>停止</title><rect x="5" y="5" width="14" height="14" rx="3" fill="currentColor"/></svg>\n')
(ROOT/'assets/icons/product/attention.svg').write_text('<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><title>需要处理</title><g fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M10.35 4.35Q12 1.8 13.65 4.35L21 17.55Q22.5 20.3 19.3 20.3H4.7Q1.5 20.3 3 17.55Z"/><path d="M12 8.1v5.2"/></g><circle cx="12" cy="16.65" r=".95" fill="currentColor"/></svg>\n')
for item in manifest['items']:
 p=ROOT/item['path'];item['sha256']=hashlib.sha256(p.read_bytes()).hexdigest()
manifest_path.write_text(json.dumps(manifest,ensure_ascii=False,indent=2)+'\n')
shutil.copy2(NATIVE/'icons/PHOSPHOR_LICENSE.txt',ROOT/'licenses/Phosphor-MIT.txt')
shutil.copy2(NATIVE/'interface/icons.json',ROOT/'licenses/Central-Icons-sources.json')
print('Rounded functional set:',len(list((ROOT/'assets/icons').rglob('*.svg'))))

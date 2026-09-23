#!/usr/bin/env python3
"""The native design browser and Zork client share one visual implementation."""
from pathlib import Path
import json
import re
import tomllib
ROOT=Path(__file__).resolve().parents[2]
ui=tomllib.loads((ROOT/'crates/zork-ui/Cargo.toml').read_text())
assert not {'reqwest','rusqlite','zork-config','zork-mesh','zork-station','axum'} & ui['dependencies'].keys()
native=tomllib.loads((ROOT/'crates/zork-gui/Cargo.toml').read_text())
assert native['dependencies']['zork-ui']['path']=='../zork-ui'
assert any(item['name']=='zork-design-pc' and item['path']=='src/bin/design_pc.rs' for item in native['bin'])
for name in ['activity','brand','message','selection','text_input']:
 p=ROOT/f'crates/zork-gui/src/components/{name}.rs';source=p.read_text()
 # The native message adapter may also re-export its host-side cache types.
 # This permits declarations only, never a copied visual implementation.
 if name=='message':
  source=re.sub(r'pub use super::transcript_cache::\{[\w\s,]+\};\s*','',source)
 s=source.strip().splitlines()
 assert s[0]==f'pub use zork_ui::components::{name}::*;',p
 assert len(s)<3,(p,'app copied the component implementation')
assert (ROOT/'crates/zork-gui/assets').resolve()==(ROOT/'crates/zork-ui/assets').resolve()
workbench = (ROOT/'crates/zork-ui/src/components/workbench.rs').read_text()
assert 'component_story' not in workbench, 'shared workbench depends on a story'
assert 'pub mod workbench;' in (ROOT/'crates/zork-ui/src/components/mod.rs').read_text()
controls = (ROOT/'crates/zork-ui/src/controls.rs').read_text()
assert not re.search(r'\bfn\s+smooth_(?:button|icon_button|quiet_button|choice|segment|busy_button)', controls), 'parallel playground controls returned'
gallery = (ROOT/'crates/zork-ui/src/component_story/mod.rs').read_text()
assert 'self.dialog.render(' in gallery and 'ui::button(' in gallery, 'gallery bypassed production controls'
design = ROOT/'apps/zork-design-pc'
assets = (ROOT/'crates/zork-gui/src/bin/design_pc/assets.rs').read_text()
manifest = json.loads((design/'assets/manifest.json').read_text())
for item in manifest['items']:
 if item['category'] != 'svg/avatars' and item['path'].endswith(('.svg','.png')) and (design/item['path']).is_file():
  assert 'design/'+item['path'] in assets,item['path']
assert 'design/mobile/zork-mobile-v1.png' in assets
for image in (design/'archive/reference-captures').glob('*.png'):
 assert 'design/'+str(image.relative_to(design)) in assets,image
guides = (ROOT/'crates/zork-gui/src/bin/design_pc/guide.rs').read_text()
for document in (design/'docs').glob('*.md'):
 if document.name == '11-avatars.md':
  continue
 assert 'docs/'+document.name in guides,document
print('PASS native design package: shared controls and embedded design sources')

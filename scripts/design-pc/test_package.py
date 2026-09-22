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
for name in ['activity','brand','message','selection','selector_menu','text_input']:
 p=ROOT/f'crates/zork-gui/src/components/{name}.rs';source=p.read_text()
 # The native message adapter may also re-export its host-side cache types.
 # This permits declarations only, never a copied visual implementation.
 if name=='message':
  source=re.sub(r'pub use super::transcript_cache::\{[\w\s,]+\};\s*','',source)
 s=source.strip().splitlines()
 assert s[0]==f'pub use zork_ui::components::{name}::*;',p
 assert len(s)<3,(p,'app copied the component implementation')
assert (ROOT/'crates/zork-gui/assets').resolve()==(ROOT/'crates/zork-ui/assets').resolve()
playground = (ROOT/'crates/zork-ui/src/liquid_story/playground.rs').read_text()
# The playground itself is a consumer of the library, including its chrome.
# Layout/data composition is permitted; private drawing primitives are not.
assert not re.search(r'\b(?:div|canvas|img|svg|rgb|rgba)\s*\(|\.(?:rounded\w*|border_[a-z0-9_]+|bg)\s*\(', playground), 'playground bypassed shared components'
workbench = (ROOT/'crates/zork-ui/src/components/workbench.rs').read_text()
assert 'liquid_story' not in workbench, 'shared workbench depends on a story'
assert 'pub mod workbench;' in (ROOT/'crates/zork-ui/src/components/mod.rs').read_text()
controls = (ROOT/'crates/zork-ui/src/controls.rs').read_text()
specimens = (ROOT/'crates/zork-ui/src/liquid_story/render.rs').read_text()
assert not re.search(r'\bfn\s+smooth_(?:button|icon_button|quiet_button|choice|segment|busy_button)', controls), 'parallel playground controls returned'
assert 'liquid::controls' in playground and 'component::action(' in playground
gallery = (ROOT/'crates/zork-ui/src/liquid_story/mod.rs').read_text()
assert 'navigation: liquid::navigation::Navigation' in gallery and 'self.navigation.render(' in (ROOT/'crates/zork-ui/src/liquid_story/library.rs').read_text(), 'playground bypassed shared navigation'
assert 'controls::segmented_with_surface(' in specimens and 'controls::segmented(' in specimens
assert 'controls::toggle_with_surface(' in specimens and 'component::toggle(' in playground
overlay = (ROOT/'crates/zork-ui/src/components/liquid/overlay.rs').read_text()
assert 'self.dialog.render(' in specimens and re.search(r'crate::modal::panel_contents(?:_with_title_action)?\(', overlay), 'modal example bypassed the shared dialog and panel'
design = ROOT/'apps/zork-design-pc'
assets = (ROOT/'crates/zork-gui/src/bin/design_pc/assets.rs').read_text()
manifest = json.loads((design/'assets/manifest.json').read_text())
for item in manifest['items']:
 if item['path'].endswith(('.svg','.png')) and (design/item['path']).is_file():
  assert 'design/'+item['path'] in assets,item['path']
assert 'design/mobile/zork-mobile-v1.png' in assets
for image in (design/'archive/reference-captures').glob('*.png'):
 assert 'design/'+str(image.relative_to(design)) in assets,image
guides = (ROOT/'crates/zork-gui/src/bin/design_pc/guide.rs').read_text()
for document in (design/'docs').glob('*.md'):
 assert 'docs/'+document.name in guides,document
assert not (ROOT/'crates/zork-gui-web').exists()
assert not (ROOT/'apps/zork-design').exists()
assert not (ROOT/'scripts/storybook').exists()
assert not (ROOT/'vendor/gpui-web-gpui-unofficial').exists()
print('PASS native design package: shared controls, embedded design sources, no Web app')

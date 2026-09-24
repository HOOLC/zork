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
bin_dir = ROOT/"crates/zork-gui/src/bin/design_pc"
assert not (bin_dir/"guide.rs").exists() and not (bin_dir/"assets.rs").exists(), "the preview app renders components only"
print('PASS native design package: shared controls and embedded design sources')

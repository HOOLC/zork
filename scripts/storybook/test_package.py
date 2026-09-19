#!/usr/bin/env python3
"""Architecture guard: the app and web examples must share their visual implementation."""
from pathlib import Path
import re
import tomllib
ROOT=Path(__file__).resolve().parents[2]
ui=tomllib.loads((ROOT/'crates/zork-ui/Cargo.toml').read_text())
assert not {'reqwest','rusqlite','zork-config','zork-mesh','zork-station','axum'} & ui['dependencies'].keys()
native=tomllib.loads((ROOT/'crates/zork-gui/Cargo.toml').read_text())
web=tomllib.loads((ROOT/'crates/zork-gui-web/Cargo.toml').read_text())
assert native['dependencies']['zork-ui']['path']=='../zork-ui'
assert web['target']['cfg(target_family = "wasm")']['dependencies']['zork-ui']['path']=='../zork-ui'
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
assert 'pub use zork_ui::components;' in (ROOT/'crates/zork-gui-web/src/lib.rs').read_text()
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
print('PASS shared component package: one visual source, no Zork service dependencies')

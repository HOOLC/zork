# Zork desktop design

The native client extends the existing Zork workbench style. Shared design definitions and controls live in `crates/zork-ui`; `crates/zork-gui` composes application views.

- Approved layout and behavior: [GUI design](docs/design/interface.md).
- Component state rules: [interaction contract](docs/design/interface.md#interaction).
- Interactive playground layout and density: [playground UI](docs/design/interface.md#component-gallery).
- Design website, editable guidelines and reference assets: [apps/zork-design](apps/zork-design/README.md), a package in this monorepo.
- Android platform geometry and input behavior: [mobile design](apps/android/design.md).

The source token exports are `ZORK_UI`, `BRAND_ACCENT`, `INTERACTION`, `FORM` and `TextRole` in `crates/zork-ui/src/design.rs`; ordinary visual controls belong to the shared component package. The handbook's GPUI examples compile those same controls. Earlier prototypes remain historical design evidence; current approved rules take precedence.

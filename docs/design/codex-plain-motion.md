# Codex-plain motion (`ui/codex-plain-motion`)

Remove liquid deformation from product UI while keeping hover/selection travel and smooth-corner geometry.

- Radii: row/control ~10, cards/composer ~12 (Codex-like ordinary sizes)
- `Material::ordinary()`: no flow/fusion/surface detail; smoothing kept
- `pressed_pose` is identity (no squash)
- Hover slide still uses Travel + pose springs

- `Material::morphs()` is false for ordinary: no source→target pair morph; dialogs/panels settle and fade via Reveal; hover travel kept.

## Plain dialogs

Product and `zork-design-pc` window dialogs use `modal::PlainDialog` (backdrop + `smooth::surface` + opacity fade). They no longer run liquid pair morph / source ink transfer.

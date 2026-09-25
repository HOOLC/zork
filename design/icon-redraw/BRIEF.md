# Zork icon redraw brief

Redraw every interface icon of the Zork app in ONE consistent style, in place.

## Scope (redraw these; keep every file name and path so no code changes are needed)
- `crates/zork-ui/assets/icons/*.svg` (69; now a mix of Lucide, Phosphor and custom marks; viewBoxes 16/24/256)
- `crates/zork-ui/assets/interface/*.svg` (33)
- `crates/zork-ui/assets/history/*.svg` (19; tool/activity types in the conversation history)
- `crates/zork-ui/assets/browser/*.svg` (7; embedded browser toolbar)
- `crates/zork-ui/assets/providers/*.svg` (8; connection/service providers: openai, anthropic, githubcopilot, kimi, openrouter, opencode, xai, compatible)
- NEW model-maker marks in `crates/zork-ui/assets/makers/<key>.svg` for keys:
  deepseek, qwen, zhipu, doubao, moonshot, minimax, mistral, meta, google, anthropic, openai, xai, generic
  (another branch wires these paths; keep exactly these names).

`design/icon-redraw/inventory.json` lists every file with its current viewBox and where the code uses it (`uses`). Read the call sites to understand each icon's meaning and the size it renders at.
Out of scope, do not touch: `assets/brand`, `assets/app`, `assets/illustrations`, `assets/motion`, `assets/loading`, any code.

## Style (define it precisely first, then apply it to every icon)
- Read `docs/design/interface.md`, `crates/zork-ui/assets/interface/README.md` and `icons.json`, and the rounded-corner skill `.agents/skills/zork-rounded-corners/SKILL.md`. The product language is warm and soft: capsule controls, large continuous-curvature radii, rounded "Central Icons"-like line icons, and a folded-corner brand motif.
- One grid for all icons: `viewBox="0 0 24 24"`, `fill="none"`, strokes `stroke="currentColor"`, round caps and joins, one stroke width for the whole set. Pick the width so icons stay crisp at 14, 16 and 20 px (composer icons render at 14 px). Use consistent padding and corner radii and a common optical size (circles vs squares).
- Monochrome only (`currentColor`), no hard-coded colours. Solid fills only where the meaning needs it (e.g. stop, filled microphone), and then also in currentColor.
- Provider and maker marks: simplified monochrome glyphs of each company's logo. They must stay recognisable and share the set's optical size and weight, as a family of "logo badges". Don't invent new logos. Where a logo is inherently filled, use a filled glyph at matching visual weight.
- Keep files small and clean: paths only, no metadata, no transforms, no ids, no editor junk.
- Duplicated meanings across folders (e.g. `icons/arrow-left.svg` and `interface/arrow-left.svg`, the `phosphor-*` variants) should look identical after the redraw, unless a call site needs a filled variant.

## Deliverables
1. `design/icon-redraw/STYLE.md`: the exact rules (grid, stroke width, radii, padding, fill policy, logo-badge rules).
2. All SVGs in scope rewritten in place, plus the new `makers/*.svg`. Keep the `*_LICENSE.txt` files and add a note in `design/icon-redraw/STYLE.md` that the set is now original Zork artwork, with the logos as trademarks of their owners.
3. Contact sheets to review: `design/icon-redraw/sheet-light.png` and `sheet-dark.png`, every icon at 14, 20 and 32 px on the warm paper background (#F7F4EE-ish) and on dark, labelled by file name. Render with headless Chrome (`"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --screenshot=...`) or `qlmanage`. Also a before/after sheet for about 20 representative icons: `design/icon-redraw/before-after.png`.
4. Regenerate the Android vector drawables from the new SVGs with `scripts/android/export_design_assets.py`: read it first and run it as intended. Do not hand-edit drawables.
5. Validate: every SVG parses, has a 24×24 viewBox, and uses only currentColor. Run `python3 scripts/android/export_design_assets.py` and any icon/asset check the repo has (`git grep -n "assets/icons\|icons.json" scripts crates/zork-ui/src | head`). Fix anything that breaks.

Work only inside this worktree. Do not commit, push or build the Rust app. Finish with a short summary: stroke width and style decisions, the list of files changed, anything you could not redraw faithfully.

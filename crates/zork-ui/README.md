# zork-ui

Shared Rust/GPUI visual components used by the native client and the design handbook's interactive WebAssembly examples. The package owns tokens, fonts/SVG assets, buttons, fields, dropdowns, navigation rows, avatars, provider marks, Markdown rendering and text selection, activity and brand motion. It has no Station, database, mesh or HTTP service dependency.

Liquid material, geometry, inward borders, visual transitions and recipes live in
the platform-independent `zork-liquid` crate. GPUI controls adapt that shared
implementation; Android consumes its batched numerical scene through JNI and
keeps Compose input, accessibility and window ownership. The wire contract is
documented beside `zork-liquid::scene`, separate from client business state.
Application integrations use complete controls and retain their instances through
exit. Pass content and real measured sources; let the component measure its target
height and own clipping, focus and paint handoff. Renderer caches and playback
types are implementation details, so performance changes preserve this boundary.

The Android bridge uses one current scene protocol, defined in
`zork-liquid::scene`, including independent content and backdrop outputs.
Use those outputs for composition, while native windows retain Back, IME and
accessibility ownership. Material progress is not content or backdrop opacity.

The `stories` feature adds isolated examples; normal client builds omit it. Business examples and the application call the same complete component entry points with mock or real parameters. The native and Web hosts install the same catalog; settings views that consume core controllers use the shared offline adapter.

Native and Web render the same Rust implementation. Text rasterization and CJK font fallback differ by platform: native uses system fonts; Web embeds Noto Sans SC under OFL. Page examples retain the live original HTML and optional snapshots for comparison. Primitive tabs use GPUI exclusively.

`ComposerInput` shares text editing across chat, comments and settings fields.
Arrow keys move by grapheme or visual row; Shift extends the anchored selection.
On Mac, Command+Left/Right addresses the current wrapped row, Command+Up/Down
addresses the document, and Option+arrows addresses words/paragraphs. Other
keyboards use Home/End, Control+Home/End and Control+arrows. Word and line
deletion, undo/redo, plain-text paste and Mac Control editing commands use the
same edit model. Web selects bindings from the browser's host platform and
accepts browser paste events. Composer Enter submits; Shift+Enter inserts a
newline. Standalone `ComposerInput::multiline` fields use Enter for newlines.
Marked IME candidates cannot submit the field. Readonly and disabled inputs
reject changes through typing, IME replacement and clipboard paste alike.

Undo stores bounded edit deltas, groups typing and commits each IME composition
as one edit. `set_value` preserves cursor/history for an identical projection
echo; `reset_value` starts a restored field or draft with fresh editing history.
Run the keyboard regressions with `cargo test --locked -p zork-ui --features
headless-bench,native-screenshots --lib components::text_input` using the configured
build environment. The ignored `composer_draw_cpu_budget` case runs separately
from builds to compare native text-layout CPU frame costs.

From the repository root:

```sh
python3 scripts/storybook/test_package.py
cargo run --locked -p zork-gui --features headless-bench --bin zork-gui-storybook
python3 scripts/storybook/build.py
uv run scripts/storybook/test_web.py
uv run scripts/storybook/test_web.py --backend webgl --output artifacts/storybook/webgl-checks
```

The build command exports native component/window PNGs and geometry, captures the existing local design reference, builds GPUI Web and generates the comparison gallery under the monorepo `apps/zork-design/components` directory. It uses the design reference server started by `pnpm design:dev`. It does not publish the site or connect to a real node. See `scripts/storybook/build.py --help` for options.

The native `zork-gui-storybook` browser groups its directory by component. Choose
the specimen's scenario in the work area and its viewport separately. Switching
components retains the current specimen and reading position; choosing another
scenario or resetting the current specimen recreates only that component's fixture.
Viewport changes preserve its input and interaction state. `--family` and
`--start-story` select an initial browsing scope; `--export` still uses the original
fixed-size stories, independent of the interactive browser's layout.

Sidebar navigation uses `navigation::TabGroup` for both device/task rows and settings tabs.
Each sidebar owns a group and wraps its complete content in `surface`; `tab` provides
hover/pressed feedback and `column` provides the 2 px row gap. The active tab has an
independently sliding 2 × 14 px leading marker in `BRAND_ACCENT` orange, with no selected fill or change in
font weight. Both surfaces are painted across the whole group, so section gaps do
not clip their motion; settled surfaces respect their target's scroll viewport.

All sliding surfaces retain their painted position and velocity when retargeted.
They accelerate and brake continuously when the target changes; motion and
settling constraints belong to the shared interface contract.

The gallery has one tab per primitive family and one tab per application page. Primitive tabs render every state in a single GPUI canvas using `FamilyStories`, with separate entities and namespaced control IDs. Only page examples retain the live HTML comparison; scene and viewport selection stay inside their page tab. Web embeds explicit 400/500/600/700 font instances because fontdb otherwise indexes the CJK variable font at its Thin default.

Modal backdrops are rendered by the shared liquid overlay with the foreground
content, using the same material geometry on native and Web.

`components::liquid::composer` uses one retained `Scene` and complete renderer
for the application and business examples. Material parcels keep their identity
through input growth, member changes and accepted-send motion. Core supplies
capabilities and accepted intents; a departing bubble does not prove delivery.

`components::attachment_fan` owns file poses and aperture geometry. The complete
`liquid::composer::fan` renderer consumes those results, while the composer Scene
cuts the same aperture out of its live contour. File membership and byte access
remain core inputs. Rendering backends, experimental switches and comparison
fixtures are defined by their source and test entry points.

## Component contract

The approved rules start at [`docs/design/interface.md`](../../docs/design/interface.md), with the [state contract](../../docs/design/interface.md#interaction). `design::INTERACTION`, `design::FORM` and `TextRole` own common feedback and typography. Use the semantic controls rather than overriding ordinary state colors in application views. Platform-specific navigation and touch geometry remain explicit.

Playground chrome uses the same library components as its examples. Read the [component reuse contract](../../docs/design/interface.md#component-gallery) before extending it; gallery startup and diagnostics are below.

`interaction-overview` exercises action feedback; `interaction-form` combines production fields, a select, a switch, text actions and local save/error/busy states. Existing model/connection stories render the actual settings view against isolated fixtures. Fixed state swatches are reference samples; input-driven checks provide behavioral evidence.

`components::collapse` measures uncompressed content at the available width and animates
its clipped layout height with a shared speed limit and smooth acceleration/braking.
The content retracts slightly and fades as the aperture closes. Retain it by stable ID while closed;
use `mounted` to omit settled hidden rows and `interactive` for descendant tab stops.
Its frame callback must invalidate the owning region’s intrinsic height. The
`navigation-fold-open` / `navigation-fold-closed` stories run the same component
on native and Web. `scripts/storybook/test_collapse.py --url <web-index-url>`
checks Web reversal, removal, keyboard activation and idle behavior.

<a id="playground"></a>

## Playground

The [shared interface contract](../../docs/design/interface.md#component-gallery)
defines component reuse, gallery layout and liquid behavior. Run the native
`liquid-gallery` example in `zork-gui` with `--features headless-bench` through
the repository build environment. The application host supplies the production
business component catalog. For Web, build
with `scripts/storybook/build_web.py --release`, serve with `serve_web.py`, and open
`index.html?story=liquid-gallery`; the scripts' `--help` defines options.

Use `backend=webgl` for the compatibility renderer; WebGPU requires a secure
context and a verified actual backend. `trace=client` enables diagnostic sampling;
use a trace directory outside the Web root. The probe records metrics, not input
text or business data, and releases observers and timers after sampling.

Choose `test_playground.py` for real gallery navigation and the liquid scripts for
component interaction. Keep a fixed release artifact, viewport and DPR for timing.
CPU, GPU, submission intervals and normal-display feedback are separate evidence;
builds, screenshot encoding and duplicate draws stay outside samples. Follow
[validation](../../.agents/skills/zork-validation/SKILL.md) for the affected checks.

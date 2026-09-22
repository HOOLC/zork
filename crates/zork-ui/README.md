# zork-ui

Shared Rust/GPUI visual components used by the native client and `zork-design-pc`. The package owns tokens, fonts/SVG assets, buttons, fields, dropdowns, navigation rows, avatars, provider marks, Markdown rendering and text selection, activity and brand motion. It has no Station, database, mesh or HTTP service dependency.

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

The `stories` feature adds isolated examples; normal client builds omit it. Business examples and the application call the same complete component entry points with mock or real parameters. `zork-design-pc` installs the catalog; settings views that consume core controllers use the shared offline adapter.

The design browser renders the same native implementation as the client and embeds editable design documents and curated assets. Historical page references remain archived as design inputs.

`ComposerInput` shares text editing across chat, comments and settings fields.
Arrow keys move by grapheme or visual row; Shift extends the anchored selection.
On Mac, Command+Left/Right addresses the current wrapped row, Command+Up/Down
addresses the document, and Option+arrows addresses words/paragraphs. Other
keyboards use Home/End, Control+Home/End and Control+arrows. Word and line
deletion, undo/redo, plain-text paste and Mac Control editing commands use the
same edit model. Composer Enter submits; Shift+Enter inserts a
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
python3 scripts/design-pc/test_package.py
python3 scripts/design-pc/build.py --verify
pnpm design:pc
```

The build command embeds curated source images, builds the native design browser and exercises its component and document navigation. Use `--export` to save native component/window PNGs and geometry to a local artifact directory; no network service is required. See `scripts/design-pc/build.py --help` for options.

The native `zork-design-pc` browser groups its directory by component. Choose
the specimen's scenario in the work area and its viewport separately. Switching
components retains the current specimen and reading position; choosing another
scenario or resetting the current specimen recreates only that component's fixture.
Viewport changes preserve its input and interaction state. Interactive launch
selectors (`--family`, `--story`, `--start-story`) focus the requested component
without removing other components from the directory. Batch `--export` and
`--list` retain their fixture filters and original fixed-size stories.

Sidebar navigation uses `navigation::TabGroup` for both device/task rows and settings tabs.
Each sidebar owns a group and wraps its complete content in `surface`; `tab` provides
hover/pressed feedback and `column` provides the 2 px row gap. The active tab has an
independently sliding 2 × 14 px leading marker in `BRAND_ACCENT` orange, with no selected fill or change in
font weight. Both surfaces are painted across the whole group, so section gaps do
not clip their motion; settled surfaces respect their target's scroll viewport.

All sliding surfaces retain their painted position and velocity when retargeted.
They accelerate and brake continuously when the target changes; motion and
settling constraints belong to the shared interface contract.

The gallery has one tab per primitive family and one tab per application page. Primitive tabs render every state in a single GPUI canvas using `FamilyStories`, with separate entities and namespaced control IDs. Scenario and viewport selection stay inside each component tab; design documents and assets have their own native sections.

Modal backdrops are rendered by the shared liquid overlay with the foreground
content, using the same native material geometry.

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
as the native client and are exercised through `zork-design-pc`.

<a id="playground"></a>

## Playground

The [shared interface contract](../../docs/design/interface.md#component-gallery)
defines component reuse, gallery layout and liquid behavior. Run the native
`liquid-gallery` example in `zork-design-pc` with `--features headless-bench` through
the repository build environment. The application host supplies the production
business component catalog. Choose `scripts/design-pc/test_native_workbench.py`
for navigation and `scripts/design-pc/test_native_playground_performance.py` for
native workload measurement. Build and measurement are separate; follow
[validation](../../.agents/skills/zork-validation/SKILL.md) for the affected checks.

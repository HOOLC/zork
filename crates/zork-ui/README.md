# zork-ui

Shared Rust/GPUI visual components used by the native client and `zork-design-pc`. The package owns tokens, fonts/SVG assets, buttons, fields, dropdowns, navigation rows, provider marks, Markdown rendering and text selection, activity and brand motion. It has no Station, database, mesh or HTTP service dependency.

Desktop controls use GPUI, `gpui-base` and `gpui-component` for input, selection, menus, popovers and popup positioning. Zork supplies shared colors, dimensions, content and intent callbacks. Android uses Compose Material3 controls with the same visual semantics and native touch behavior. `PlainDialog` retains its panel and backdrop opacity fade; other controls use ordinary static surfaces. Client business state and operations remain in `zork-client-core`.

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

Sidebar navigation uses ordinary GPUI rows through `navigation::TabGroup`; selected and hover states are static and focusable. The component gallery renders the same production controls in isolated fixtures. `components::widgets::composer` arranges the editor and activity chips with static layout, while core supplies capabilities and accepted intents. Attachments appear in a scrollable preview ribbon; file membership and byte access remain core inputs.

## Component contract

The `unified-design-overview` story (`src/unified_story.rs`, listed as 统一设计 in `zork-design-pc`) is the visual baseline: palette, role radii, smooth corners, device marks and both themes on one page, rendered by production components. Check changes against it in light and dark.

The approved rules start at [`docs/design/interface.md`](../../docs/design/interface.md), with the [state contract](../../docs/design/interface.md#interaction). `design::INTERACTION`, `design::FORM` and `TextRole` own common feedback and typography. Use the semantic controls rather than overriding ordinary state colors in application views. Platform-specific navigation and touch geometry remain explicit.

The component gallery uses the same library components as its examples. Read the [component reuse contract](../../docs/design/interface.md#component-gallery) before extending it; gallery startup and diagnostics are below.

`interaction-overview` exercises action feedback; `interaction-form` combines production fields, a select, a switch, text actions and local save/error/busy states. Existing model/connection stories render the actual settings view against isolated fixtures. Fixed state swatches are reference samples; input-driven checks provide behavioral evidence.

<a id="playground"></a>

## Component gallery

The [shared interface contract](../../docs/design/interface.md#component-gallery) defines component reuse and gallery layout. `zork-design-pc` mounts the production views against isolated fixtures. Use `scripts/design-pc/test_native_workbench.py` for browser navigation and `scripts/design-pc/test_native_component_performance.py` for the native component workload. Build and measurement are separate; follow [validation](../../.agents/skills/zork-validation/SKILL.md) for affected checks.

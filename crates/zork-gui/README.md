# zork-gui

A GPUI (Zed UI framework) desktop client for zork-station's built-in
`local_gui` IM entry, with a native light task shell grounded in the current
approved Zork Fold v2 assets and layout. The gateway owns conversations and deliberate message
delivery; `zork-agent` remains an internal execution service.

The default native client follows the approved Zork design: **Device → Leader → Task** in a two-pane conversation workspace. The sidebar starts at 240 px, can be dragged from 200 to 420 px (also constrained by the available content width), and remembers its pixel width. Every navigation row uses the same 32 px geometry and full-width hover/active background; indentation changes only the content position.

Device folds are local navigation state; tasks remain directly visible under each Leader. Device folds animate their measured child height, including row gaps, and move following devices with the aperture. Rapid toggles continue from the current height; reduced motion switches immediately. Hidden children leave keyboard traversal and are unmounted after closing. Unread conversation markers come from actual Station message IDs, are ordered first within their scope, and are marked read only for the visible device and conversation while following its tail. Device switches retain drafts, comments, history, sidebar scroll and the existing offline outbox.

The right panel keeps its tabs, active page, open/expanded state, width and address draft per device/conversation during the client session. Switching chats restores that chat’s panel, including its history view and reading position; closing a tab does not affect other chats. Inactive history views suspend subscriptions and clocks until revisited.

Visible history rows and the timeline hover card observe changes to their own
entry IDs. Relative labels wake at their next displayed boundary; running
durations and timeline spans update each second. Layout changes, including wheel
zoom and pan, recheck the stationary pointer against the newly laid out spans.
Member previews share a floating surface above the avatar group and retain the
member's own conversation feed while open. Navigation and floating surfaces use
the shared distance-adaptive slide curve: near targets remain quick, long travel
accelerates toward a roughly 240 ms settling limit, and reversals retain velocity.

Selecting a passage in rendered Markdown opens a comment popover. Enter adds it to that conversation's structured draft queue; comments can be edited or removed. The main composer grows automatically and sends all comments plus optional prose in one explicit Station request. The compatibility payload retains exact selected text, author identity and source message IDs. The outbox and source-draft clearing commit atomically. Station-provided message metadata supplies sender identity and timestamps; the UI does not invent authors for legacy messages.

Settings use a separate navigation layout with separate Help and diagnostics and About tabs, plus each device's own Station, Agent and model-connection pages. Connection setup starts with Subscription/API access, then a compact icon-bearing provider selector. Authentication, model discovery (where the provider actually supports it), manual model configuration and Agent Profile/model assignment use the selected device's authenticated APIs. Creating a connection does not imply that template models are verified account capabilities. Station version information is real; in-client self-upgrade remains unsupported.

The sidebar and settings navigation also expose **Mesh resources**: a read-only, device-filtered inventory of managed skills, MCP servers and services. Search and type filters lead to binding, ownership, status and access details. Opening or refreshing fetches current records; stale and unavailable devices remain explicit. Wide windows retain a detail column, while compact windows show a returnable detail page. See [the resource contract](../../docs/design/external-capabilities.md#publication).

“Connect device” offers separate phone and other-device panes. Phone access uses a short QR invitation and explicit desktop approval; other devices use the Station join command. Delivered conversation artifacts remain available for native preview, cached offline viewing and saving. Local files, drops and clipboard images become immutable conversation-owned snapshots before delivery.

The native asset family now uses semantic `icons/*.svg` paths, all 12 Agent avatars and the current 136 × 44 Fold wordmark. Historical Cue files remain embedded only for compatibility; the actual Cue account login retains its external service identity. [`assets/usage-v2.json`](assets/usage-v2.json) records every mounted icon, scene and motion carrier, including assets with no native carrier.

The sidebar and settings header place the Zork wordmark alongside the native traffic lights, without an extra title row. Hover scrubs the approved 2000 ms contour animation toward the icon; leaving reverses from the currently painted frame, and re-entering continues forward from that point. Other approved carriers retain their linked icon/letters (805 ms), wordmark (675 ms), icon (680 ms), and onboarding contour animation (2000 ms). GPUI timers are capped at 60 fps and stop at the endpoint. The upper half squashes, blinks and jumps down as one group with rounded separation; the lower half crawls left into Z, then the upper-left piece becomes o and the upper-right piece separates into rk with soft edges and a short settling overshoot. Every frame centres the visible body bounds. SVG and GPUI share sampled 128-point contours, with the notch retained on the lower contour. On macOS, the actual `NSWorkspace.accessibilityDisplayShouldReduceMotion` preference selects static endpoints without autoplay or loops.

Run `python3 scripts/test-device-sidebar.py` for the isolated native regression; `CARGO_TARGET_DIR` selects the shared build directory and `ZORK_GUI_TEST_WINDOW_SIZE=900x600` exercises the minimum window. This test uses real isolated Stations/Agents with a fake model runtime, never user devices or model credentials, and never generates an enrollment invitation. Run the same script with `--brand-only` to compare real first/intermediate/final GPU frames; add `ZORK_GUI_TEST_REDUCE_MOTION=1` to verify static endpoints in a debug build without modifying the system setting. Frame hashes and PNGs are saved under `artifacts/gui-approved-design/brand-motion` or `brand-reduced`.

## Running

```sh
cargo run --locked -p zork-gui                 # current device/Leader desktop
```

### Update the MBA app

Run from the repository with one command:

```sh
python3 scripts/update-mba.py
```

For this personal test-device install, the script directly
builds on the configured development host with the locked dependency graph and bounded Cargo settings,
packages the GUI and node binaries, verifies the transferred archive and signature,
temporarily stages the old application, installs the new app, and checks readiness.
Successful updates delete the staged old app and keep no historical app backups.
Failed installation or startup restores the old app. Client data and configuration remain in place.
SSH authentication is reused for the whole update; a password may be requested once.
Invocations from another host forward to the configured development checkout.

The local-node switch is persistent: once enabled, the node starts with Zork until
explicitly disabled. Quitting stops its processes without changing that preference;
startup failure also preserves it and provides a retry action. Older installations with
a saved local node migrate to enabled; a fresh installation starts disabled.

### Dev automation API

`--dev` starts a token-protected HTTP API on loopback only. The default port is
`8765`; pass `--dev-port 0` to choose an available port. A token is generated
and printed at startup unless `--dev-token` or `ZORK_GUI_DEV_TOKEN` supplies
one.

```sh
ZORK_GUI_DEV_TOKEN=local-dev cargo run -p zork-gui -- --dev

curl -H "Authorization: Bearer $ZORK_GUI_DEV_TOKEN" \
  http://127.0.0.1:8765/v1/elements

curl -H "Authorization: Bearer $ZORK_GUI_DEV_TOKEN" \
  http://127.0.0.1:8765/v1/screenshot -o /tmp/zork-gui.png

curl -H "Authorization: Bearer $ZORK_GUI_DEV_TOKEN" \
  -H 'Content-Type: application/json' \
  -X POST http://127.0.0.1:8765/v1/actions \
  -d '{"type":"type_text","target":{"element_id":"composer-input"},"text":"hello"}'
```

| Route                                               | Purpose                                                                                     |
| --------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| `GET /health`                                       | Unauthenticated process readiness; contains no UI data                                      |
| `GET /v1`                                           | Protocol metadata and supported action names                                                |
| `GET /v1/elements`                                  | Visible actionable elements with IDs, roles, labels, bounds, centers, and supported actions |
| `GET /v1/elements?include_hidden=true`              | Also include rendered elements clipped by a viewport or scroll mask                         |
| `GET /v1/elements?after_revision=N&timeout_ms=3000` | Wait for a newer rendered frame, up to 30 seconds                                           |
| `GET /v1/screenshot`                                | Current window as PNG, with dimensions and UI revision in response headers                  |
| `POST /v1/actions`                                  | Dispatch `click`, `move`, `type_text`, `key`, `scroll`, or `drag` input                     |

Coordinates use logical pixels relative to the window content. The element
response includes `scale_factor`; PNG dimensions are logical dimensions times
that factor. An action target is either `{"element_id":"..."}` or an explicit
`{"x":10,"y":20}` point. Scroll deltas are logical pixels; a negative
`delta_y` reveals content below in GPUI scroll areas.

The automation boundary is intentionally black-box. It can read rendered
geometry and pixels, but its only mutations are GPUI mouse and keyboard events.
There are no commands for creating sessions, changing models, sending messages,
or mutating `RootView` directly. Component-ID actions resolve to the visible
center and then dispatch the same events as coordinate actions. The API is not
started without `--dev`, does not bind a non-loopback interface, and does not
enable CORS.

The real-process fixture starts a fake-model Agent plus the production gateway
on isolated ports and verifies the explicit-message boundary:

```sh
cargo build --locked -p zork-agent-server -p zork-station -p zork-gh
python3 crates/zork-gui/tests/test_gateway_entry.py
```

The fixture proves that ordinary assistant transcript text stays hidden, an
explicit `chat.post_message` becomes one persistent assistant row,
and two tasks sharing one workspace still resolve their exact gateway binding.

## Station-owned conversation API

| UI piece                        | Source                                                         |
| ------------------------------- | -------------------------------------------------------------- |
| Device conversations and status | Shared client state, updated by Station events                 |
| Delivered message history       | `GET /v1/im/sessions/{id}/messages` (cursor pages of 100)      |
| Live delivery/activity          | SSE `GET /v1/im/sessions/{id}/events` — `message` and `status` |
| Composer send                   | Durable client outbox → `POST /v1/im/sessions/{id}/messages`   |
| Cancel                          | `POST /v1/im/sessions/{id}/cancel`                             |

### Status projection

The gateway projects `clear`, `thinking`, `tools_started`, `tool_finished`,
`waiting`, `failed`, `finished`, and `interrupted` as activity. These events may
change the task status label/footer but never add chat rows. Successful
`finished` updates task/header state only and does not render inline activity.
Agent commentary, final transcript text, streaming deltas, tool results, and waits are internal.
A visible assistant reply exists only after the Agent explicitly invokes
`chat.post_message`.

## Native component layer

- `ComposerInput` uses GPUI's platform input handler rather than raw key-string
  mutation. It supports IME marked text, UTF-16 selection exchange, Unicode
  grapheme editing, mouse selection, clipboard actions, explicit newlines, soft
  wrap, and a fixed-height scrolled viewport. Enter submits and Shift+Enter
  inserts a newline.
- `MessageDocument` parses the coding-message subset of GFM and renders
  headings, emphasis, strike-through, inline/fenced code, lists/task lists,
  block quotes, rules, tables, and links with the shared Cue palette.
  Fenced code keeps its language label, cached syntax colors and horizontal
  scrolling in both selectable messages and component previews. Inline code
  uses a subdued monospace treatment without a rectangular background.
  Links show a hand cursor and their destination on hover.
  Incomplete Markdown and unsupported inline content remain readable.
- Transcript projection keeps optimistic user sends to one visible copy,
  reconciles HTTP/SSE/reconnect ordering, preserves full delivered text, and
  keeps partial-history titles on the stable task id until the true first page
  is known.
- These components are local adapters for the existing `gpui-unofficial`
  runtime. They do not import Longbridge's theme or a second incompatible GPUI
  runtime.

## Known limitations

- Markdown intentionally does not fetch remote images, execute embedded HTML,
  or render Mermaid/math. Unknown code languages and large code outputs remain
  plain monospace text.
- One window; no diff/handoff pane (P3).

The executable UI contracts live in `tests/cue_ui_contract.rs`,
`tests/component_port_regression.rs`, and `tests/message_rendering_regression.rs`.
The palette and geometry are defined in `src/design.rs`.

## Verification

```sh
cargo test --locked -p zork-gui
cargo clippy --locked -p zork-gui --all-targets -- -D warnings
cargo build --locked -p zork-agent-server -p zork-station -p zork-gh
python3 crates/zork-gui/tests/test_gateway_entry.py
```

## Desktop UI and retained contracts

The current desktop uses Device → Leader → Task navigation, a conversation composer,
member details with execution history, and conversation file previews. The old
Home composer, global Inbox/task board/Drive, manual review controls, direct-Station
CLI mode and SSH app launcher have been retired.

Sendable conversations stack idle member avatars immediately above the composer.
Active members first move upward with a critically damped spring (500 ms settling window), then extend into individual action rows at a fixed width speed; label changes
do not restart the motion. Idle waits 1200 ms before returning, and resumed work
cancels that return. Waiting and failed members stay expanded. Motion respects the
system's reduced-motion preference and stops scheduling frames when settled.
Motion follows display frame callbacks, including 120 Hz displays; interrupted
transitions retain their current position and velocity.
The composer remains bottom-anchored. Transcript bottom spacing and member positions
use the same spring sample on each display frame, so the latest messages move
with expanding and returning members. Historical scroll anchors remain stable;
only visible rows are rendered and settled motion stops requesting frames. Clicking an avatar opens member details and
execution history; read-only conversations retain their member entries in the right panel.
Hovering a composer member avatar opens a shared details card with current status,
two recent completed executions, and available model/context information. History
is loaded for that member’s session only after the hover delay; leaving the card
releases its subscriptions. The excerpt is bounded to the latest 100 loaded
entries and omits successful tool output and internal model text.
The composer and transparent portraits share a GPUI liquid silhouette: idle
portraits perch on the light upper edge; moving bubbles stretch a connecting
neck before separating into light capsules. A smooth distance field produces continuous
connections between nearby shapes. CPU rendering visits only contour-adjacent cells, with optimized Lyon tessellation.
Settled meshes are cached. An opt-in Metal experiment (`ZORK_LIQUID_GPU=1`)
evaluates the field into a texture; its synchronous readback is not the default. The approved geometry uses 16 px bubble radii,
22 px idle spacing and edge inset, 0 px active edge inset, 8 px immersion,
35 px active row spacing,
a 3 px gap above the composer, and separate 4 px member/dock fusion.
The single-line composer is 84 px tall with 24 px corners and 8 px icon insets.
It grows to three visual lines (60 px editor height), then scrolls internally
with the wheel/trackpad and follows the cursor while editing.
The composer matches the sidebar with solid `#F6F5F1`, `#24272B` text and a
`#24282B` send button with a white glyph. A 0.5 px `#DEDFDF` border follows the
complete fused contour, keeping its geometric width at necks without internal
intersection lines. Fill and stroke share contour extraction and cache lifetime.
A CPU blur reference is available in headless builds for comparisons.
The original GPUI editor handles input, IME and sending on every
platform. Stable geometry is cached and does not schedule animation frames.

`cargo test --locked -p zork-gui --features headless-bench --test headless_presence`
checks native offscreen motion, geometry, idle scheduling and frame cost at wide
and compact window sizes.

Native windows support 900×600 and larger. Cmd+B toggles the device sidebar;
Escape dismisses the active conversation popover. Model and Agent configuration
belongs to device settings. The composer accepts local files, file drops and clipboard
images. Conversation-owned snapshots appear as a fused fan of previews; hover unfolds
the fan and clicking pins it. See [conversation files](../../docs/design/shared-files.md#attachments).

Embedded `locales/zh-CN.json` and `locales/en.json` catalogs supply shared labels.
`ZORK_GUI_LOCALE=zh-CN|en` overrides the saved startup locale, and
`ZORK_GUI_PREFERENCES_PATH` selects the preferences file for isolated tests.
Missing translations fall back to English. Catalog tests require matching keys
and nonempty values; user content and backend errors remain untranslated.

Backend task lifecycle and artifact APIs remain available. Removing the old UI
does not remove stored task/message formats or their process tests:

```sh
python3 crates/zork-gui/tests/test_product_tasks.py
python3 crates/zork-gui/tests/test_drive_artifacts.py
python3 scripts/test-client-settings.py
```

Delivered files can be previewed and saved from the current conversation. Immutable
snapshots survive changes to their original source. See the [task API](../../docs/design/chat.md#compatibility),
[file storage contracts](../../docs/design/shared-files.md), and [Mesh transport](../../docs/design/devices.md).

### Headless rendering and performance acceptance

`python3 scripts/test-desktop-headless.py` is an opt-in desktop validation workflow;
MBA test-device installation does not run it automatically.
It uses GPUI `HeadlessAppContext` with the real embedded assets and platform text
system, seed 0, UTC, fixed 600-row fixtures and a virtual 120 Hz clock. Two fresh
replays must produce identical per-frame row/scroll traces. It checks bounded
virtualized rendering, continuous draws and a p95 CPU draw budget of 8.33 ms.
The views render through production GPUI code; no system test windows are opened,
and screenshots come from the offscreen Metal renderer. CPU timing remains a
hardware-dependent measurement, separate from deterministic replay assertions.
Reports and screenshots are in `artifacts/headless-render/latest`.

The same gate replays real mouse and keyboard events through GPUI for Markdown
selection (including table columns and reverse UTF-8 selection), plus connection,
model and Agent modals at 900×600 and 1280×800. Modal checks cover opening, input,
viewport clipping, Escape, close buttons and backdrop click isolation. Screenshots
are in `artifacts/headless-interactions/modals`. These tests do not contact a live
backend or open system windows.

### Optional native display benchmark

`python3 scripts/test-desktop-performance.py --output artifacts/scroll-push/latest`
builds the native profiler and exercises history/chat/Markdown fixtures, 100000 mixed
messages at the beginning/middle/end, and the expanded file list. It checks bounded
visible content, frame coverage and CPU drawing budgets. An unlocked macOS desktop session is required. Run it
explicitly, or use `python3 scripts/update-mba.py --native-performance` to include
it before installation. The installed app excludes both benchmark features.

Scroll rendering follows the display's native refresh clock. The 8.33 ms check
is a CPU budget for 120 Hz, not a 120 FPS claim on a 60 Hz display. See
measurements and method（本地生成的验收记录）.

## Shared components

Visual primitives and assets live in `../zork-ui`; application settings views compose them. The design handbook uses the same Rust through `../zork-gui-web`. Run `python3 scripts/storybook/build.py` from the repository root to rebuild the native/Web comparison gallery. Missing historical design states remain visible in its coverage filter.

The default shell uses the original light palette: white canvas and `#F6F5F1`
sidebar and composer. Conversation chrome is a floating top-right page-panel
toggle, without a header row or shell dividers. Existing file and read-only
member controls live in the right panel.

Floating conversation controls have a persistent white background. The page-panel
toggle uses the shared icon-button radius and hover/pressed feedback, and stays
12 px from the workspace right edge in both states. Opening and closing use a
280 ms critically damped width transition; interrupted toggles preserve velocity,
reduced motion snaps to the destination, and completed motion stops frame requests.

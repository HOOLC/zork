# zork-gui

GPUI desktop client for Zork. The navigation lists Chat by message time and labels each execution device. Creating a Chat selects a device, model, thinking depth and optional connection; the first send creates its Session. Existing Agent definitions and task records remain readable as history. See [Chat and collaboration](../../docs/design/chat.md#navigation) and [shared interface design](../../docs/design/interface.md).

Model connections and device settings live in separate pages. Conversation drafts, comments, history position and file previews remain scoped to their Chat. UI controls use GPUI and the vendored component library; client business operations go through `zork-client-core`.

Run `cargo test --locked -p zork-gui --features headless-bench --test headless_navigation_states --test headless_isolation` for navigation and current brand hover behavior. `python3 scripts/test-chat-navigation.py` verifies first-send Chat behavior across isolated nodes.

## Running

```sh
cargo run --locked -p zork-gui                 # current desktop client
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

The real-process fixture starts a fake-model Agent plus the production station
on isolated ports and verifies the explicit-message boundary:

```sh
cargo build --locked -p zork-agent-server -p zork-station -p zork-gh
python3 crates/zork-gui/tests/test_station_entry.py
```

The fixture proves that ordinary assistant transcript text stays hidden, an
explicit `chat.post_message` becomes one persistent assistant row,
and two tasks sharing one workspace still resolve their exact station binding.

## Station-owned conversation API

| UI piece                        | Source                                                         |
| ------------------------------- | -------------------------------------------------------------- |
| Device conversations and status | Shared client state, updated by Station events                 |
| Delivered message history       | `GET /v1/im/sessions/{id}/messages` (cursor pages of 100)      |
| Live delivery/activity          | SSE `GET /v1/im/sessions/{id}/events` — `message` and `status` |
| Composer send                   | Durable client outbox → `POST /v1/im/sessions/{id}/messages`   |
| Cancel                          | `POST /v1/im/sessions/{id}/cancel`                             |

### Status projection

The station projects `clear`, `thinking`, `tools_started`, `tool_finished`,
`waiting`, `failed`, `finished`, and `interrupted` as activity. These events may
change the task status label/footer but never add chat rows. Successful
`finished` updates task/header state only and does not render inline activity.
Agent commentary, final transcript text, streaming deltas, tool results, and waits are internal.
A visible assistant reply exists only after the Agent explicitly invokes
`chat.post_message`.
While a member works, the transcript can show a bounded, read-only preview of
that member's Session activity. Its rows open Session history and never become
Chat messages.

## Native component layer

- `ComposerInput` uses GPUI's platform input handler rather than raw key-string
  mutation. It supports IME marked text, UTF-16 selection exchange, Unicode
  grapheme editing, mouse selection, clipboard actions, explicit newlines, soft
  wrap, and a fixed-height scrolled viewport. Enter submits and Shift+Enter
  inserts a newline.
- `MessageDocument` parses the coding-message subset of GFM and renders
  headings, emphasis, strike-through, inline/fenced code, lists/task lists,
  block quotes, rules, tables, and links with the shared Zork palette.
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

The executable UI contracts live in `tests/ui_contract.rs`,
`tests/component_port_regression.rs`, and `tests/message_rendering_regression.rs`.
The palette and geometry are defined in `src/design.rs`.

## Verification

```sh
cargo test --locked -p zork-gui
cargo clippy --locked -p zork-gui --all-targets -- -D warnings
cargo build --locked -p zork-agent-server -p zork-station -p zork-gh
python3 crates/zork-gui/tests/test_station_entry.py
```

## Desktop UI and retained contracts

The current desktop lists Chat by message time, with the execution device on each row. It provides a conversation composer, Session execution history and file previews. Historical Agent definitions and tasks remain readable through their original identities.

Session activity appears in the transcript. The composer remains available while activity changes; opening execution history loads the selected Session on demand.

The composer uses a warm-white GPUI rounded panel, the ordinary `ComposerInput` for IME and selection, and a scrollable attachment preview ribbon. It grows to three editor lines and then scrolls internally. Attachment membership comes from core; previews are cached on demand without contour clipping or per-frame shape updates.

`cargo test --locked -p zork-gui --features headless-bench --test headless_chat_activity`
checks native offscreen motion, geometry, idle scheduling and frame cost at wide
and compact window sizes.

Native windows support 900×600 and larger. Cmd+B toggles the sidebar;
Escape dismisses the active conversation popover. Model connections belong to device settings. The composer accepts local files, file drops and clipboard
images. Conversation-owned snapshots appear in a scrollable preview ribbon; hover expands
the previews and clicking pins them. See [conversation files](../../docs/design/shared-files.md#attachments).

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
selection (including table columns and reverse UTF-8 selection), plus connection and
model modals at 900×600 and 1280×800. Modal checks cover opening, input,
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

Visual primitives and assets live in `../zork-ui`; application settings views compose them. `zork-design-pc` runs the same native Rust components and embeds the curated design sources. Build and verify it with `python3 scripts/design-pc/build.py --verify` from the repository root.

The default shell uses the original light palette: white canvas and `#F6F5F1`
sidebar and composer. Conversation chrome is a floating top-right page-panel
toggle, without a header row or shell dividers. Existing file and read-only
member controls live in the right panel.

Floating conversation controls have a persistent white background. The page-panel
toggle uses the shared icon-button radius and hover/pressed feedback, and stays
12 px from the workspace right edge in both states. Opening and closing use a
280 ms critically damped width transition; interrupted toggles preserve velocity,
reduced motion snaps to the destination, and completed motion stops frame requests.

# gpui-base compatibility patch

Upstream: `gpui-base-uo` 1.17.0-pre, crates.io archive SHA-256 `739aeb438e3edabe71600a649cec14414835b537d913096b938f361158099cfd`.

The compatibility manifest patch: remove the unconditional `gpui/profiler` feature from the normal dependency. The library source does not call profiler APIs. Applications can still enable GPUI profiling explicitly; the upstream development dependency retains its profiling/test features.

Why: GPUI 1.17's `profiler/actions.rs` uses `std::time::Instant`; when the profiler is pulled into WASM, keyboard action dispatch panics with “time not implemented on this platform”. Input semantics and native/WASM components remain shared. The input fixes added during integration are listed below.

Keep the Apache license and this note when updating. Remove this patch once the upstream dependency no longer forces a non-portable optional profiler into Web builds. The workspace excludes this vendored dependency from workspace-member tests and examples.

## Input integration fixes

Behavior regressions in `crates/zork-ui/src/components/text_input/tests.rs` cover these narrowly scoped changes to the published engine:

- Textarea Home/End and their selection variants follow visual wrapped rows and retain end affinity. Vertical selection reuses vertical movement and preserves its column/anchor.
- Character navigation/deletion uses extended Unicode grapheme boundaries across Rope chunks, keeping combining characters and ZWJ emoji whole.
- Typing over a selection can coalesce with continued typing. History records directed selection endpoints so undo restores the original selection direction.
- `content_height()` exposes the engine's measured wrapped height for the application's composer silhouette, removing duplicate application text shaping.
- Mouse placement at wrapped row ends retains visual affinity; reversing word selections moves from the active endpoint.
- Supplemental platform commands preserve paragraph/word-start navigation, page selection, viewport scrolling, kill/yank, transpose and open-line editing. These use the same Rope, layout and undo transactions; open-line redo restores its caret before the inserted newline.
- Painting an unchanged editor and applying an unchanged scroll offset no longer emit notifications, allowing idle rendering to settle.
- The optional scrollbar chrome can be hidden while retaining scrolling. Zork keeps its existing plain input appearance and avoids an extra scrollbar layout pass.
- Page navigation to an unshaped line clips fallback byte columns to grapheme boundaries, preventing invalid UTF-8 caret positions in mixed-language text.

The patched implementation remains the upstream Rope/display-map/history engine; the application adapter does not maintain its own editor. Keep regression tests and this patch list when updating the pinned dependency.

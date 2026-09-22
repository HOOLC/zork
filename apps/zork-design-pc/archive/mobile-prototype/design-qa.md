# Design QA

- Current design authority: `../design-spec.md` and nav7 prototype, incorporating user feedback.
- Historical visual concept: `../zork-mobile-v1.png`, three-screen initial concept; later interaction decisions supersede it.
- Target viewport: 390px mobile content width; desktop wrapper capped at 844px height; phone uses actual visual viewport height.
- Source and implementation pixel dimensions: source composite 1506×1045; implementation screenshot unavailable. No density-normalized visual comparison performed.
- Implementation screenshot: unavailable.
- States: conversation navigation, group conversation and pending comments, device settings. Additional model and Agent configuration are functional extensions of those flows.
- Full-view comparison evidence: blocked by unavailable in-app browser and repeated Chrome extension control timeouts.
- Focused-region comparison evidence: unavailable for the same reason.

## Findings

Visual QA is blocked; no visual fidelity pass is claimed. Real-device keyboard behavior, selection handles, text wrapping and viewport layout need browser verification. The user's later mobile interaction feedback intentionally supersedes the concept's small tap targets and vertical footer: there are no hover rules, actions stay visible, primary controls have at least 44px targets, bottom actions are horizontal, selected-fragment actions sit above the measured composer.

## Required fidelity surfaces

- Typography: existing Inter variable font plus system Chinese sans serif; CSS source reviewed, actual rendering not verified.
- Spacing: 44px touch targets and 390px preview width specified; no rendered geometry evidence.
- Colors: neutral warm surfaces and original charcoal palette in CSS; actual pixels not sampled.
- Asset quality: original brand, avatar and icon SVG files reused and all served successfully.
- Copy: core source semantics retained; local file attachment is an actual Markdown design note instead of a fictitious PDF. Device Station version is pending and update action explains the prototype limitation.

## Functional evidence

`node --check` passed. JSDOM DOM-event smoke checks passed for navigation, folding, per-conversation drafts, comment editing and combined send, escaping, device-scoped switches and connections, model creation/duplicate validation, Agent model selection, and Leader assistance preparing a draft. No DOM runtime errors. All static files return HTTP 200. This does not replace browser console or touch-device testing.

## Comparison history

No screenshot-based comparison iteration completed. Code review fixed inconsistent device-group padding, replaced all hover styling with pressed feedback, enlarged touch targets, changed the footer to horizontal actions, and made comment positioning depend on composer height. Post-change DOM checks passed.

final result: blocked

## User screenshot follow-up (2026-09-06)

Source evidence: user screenshot `codex-clipboard-649f9298-7829-4759-9077-70d90bbb7bf8.png` shows a partial Leader background stopping before the disclosure button. Root cause: generic focus-visible and active backgrounds painted on the nested name button. Fixed by moving those states to the full Leader row via `:has()` and excluding child controls from generic backgrounds. Post-fix browser capture is still unavailable; visual result remains blocked rather than claiming a pass.

## Full navigation contract correction (nav4)

Re-read referenced task and `brand-review-site/src/gui/navigation.css`. The earlier local fix did not unify the whole navigation geometry. A dedicated final navigation stylesheet now owns all row backgrounds: equal full content width independent of text indent, 48px touch height, 6px radius, 2px gaps; desktop hover/selected tokens reused as mobile pressed/selected tokens. Leader's two child buttons are always transparent, with a single full-row background. Pointer-event checks passed for Device, Leader name, disclosure, Task and footer, including pointerup, dragging and pointercancel. Screenshot validation remains unavailable; no visual pass claimed.

## Settings hit-area correction (nav6)

User annotation on mini1 settings row showed container padding reduced both the button hit area and pressed background. Moved horizontal padding from `.setting-group` to each `.setting-row`, preserving content alignment while expanding hit and paint areas to the whole group width. Applied the shared pressed-state controller to interactive settings rows; static rows remain noninteractive. Source evidence is the user-provided annotated settings screenshot. Browser screenshot verification remains unavailable.

## Header device action and contextual return (nav7)

User annotation identified unattractive tight rectangular feedback and loss of direct chat return. Header device action now uses 6px radius, 10px horizontal inset, minimum 44px target, and the shared pressed-state controller/color. Device pages preserve entry origin across their nested pages. DOM-event regression passed for chat → device → models → device → original chat with exact draft/scroll restored, and settings → device → settings. Browser-rendered verification is still pending.

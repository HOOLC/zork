# Client state subscriptions

This contract defines how a consumer stays consistent with Rust core while updates
are coalesced, delayed or cancelled. Business ownership follows the
[core/UI boundary](client-core.md); server invalidation follows
[Mesh notifications](synchronization.md#invalidation).

## Ownership and lifetime

Core owns business state, merging, change detection and publication. Platforms
consume immutable snapshots or ordered changes and retain only presentation state.
Projection identity names a business object and scope, independently of widgets
and frame clocks. Observers of the same device or conversation share its producer.
Closing a view releases its observation; it does not cancel a business operation.

Publish the authoritative device snapshot before a derived navigation projection
advertises its actions. Input resolves identities from the current core snapshot;
a sibling view's newer navigation must not depend on a catalog from the previous
painted frame.

A latest-value source may skip intermediate display states. Ordered list or text
changes must remain reconstructable. Delivery, unread state and operation completion
cannot depend on UI consuming a callback. In particular, an outbox row disappearing
is not evidence that its message was delivered.

## Applied-version protocol

The public implementation is [zork-observe](../../crates/zork-observe/src/lib.rs).
Its consumer protocol is:

1. Register before reading. The first preparation returns an explicit Reset,
   including when the source is empty.
2. Wait for readiness and prepare when able to consume. Readiness is a hint,
   not an event count; it must not perform diffing or encoding.
3. Prepare against this consumer's **applied** version. A reader holds at most
   one prepared batch; repeated preparation returns that batch until resolved.
4. Apply all changes in order, then acknowledge. Each list edit addresses the list
   produced by preceding edits. Discard, cancellation or failed delivery preserves
   the old baseline; reading or encoding alone never advances it.
5. Before parking after acknowledgement, recheck concurrent publications so an
   update cannot be lost between applying a batch and waiting again.
6. Source replacement, authorization changes and projection changes invalidate
   incompatible batches. Revalidate immediately before applying a foreign result.
   Revocation clears retained content urgently, without waiting for a visible frame.
7. Closing a source may leave its final committed state readable. Dropping a
   consumer releases its registration and closes its detached readiness signal.

State, source-scoped version and change boundary commit together. Object revisions,
UI applied versions and durable sync cursors are separate coordinates. No-op
business input does not advance a UI version; storage bookkeeping alone must not
wake views. A reducer identifies touched changes rather than deep-comparing an
entire state tree in a generic publisher.

Change retention is bounded by count and bytes. A consumer that falls behind resets
to its requested projection, without blocking fast consumers or retaining an
unbounded journal. These bounds do not cap business history or snapshots retained
by external readers. A reset for a windowed reader encodes its requested window,
not every record loaded by another reader.

## Platform scheduling

Coalesce before expensive preparation, conversion and encoding. Route by topic or
key before waking unrelated consumers. Do not call platforms or encode data while
holding the state/journal lock. Waiting must be cancellable and idle without polling.

- GPUI schedules ordinary updates for its next frame and catches up on attachment.
  Retained regions keep the same geometry on fresh and cached rendering paths;
  focus, accessibility and automation must still see valid content and targets.
- Android waits independently of the command lock and Java IO thread pool. A small
  readiness callback schedules work; encoding/decoding happens off the main thread.
  Main applies an ordered batch in one Compose transaction before acknowledgement.
- FFI handles, generations and batch IDs fence close/reopen and A → B → A races.
  IDs from different handles are not comparable. Closing aborts the waiter and
  releases foreign references, including callbacks already in flight.
- Background OS notifications consume committed state without a display-frame wait.
  Core rechecks privacy and access before delivery. OS acceptance is not reading.
- Native design fixtures use the public core contracts and the same business rules.
  A component story does not prove the full desktop client applies every projection.

Only readiness hints or complete latest values may conflate freely. Dependent raw
patches cannot be placed in a conflated flow that discards their predecessors.

## History and bounded projections

Execution history is authoritative on the execution node. Core loads requested
pages into RAM; records, derived entries, indexes and pagination cursors do not form
a second client history database. Retained immutable lists share memory across
versions; that use of “persistent” does not mean disk persistence. Delivered chat
messages follow their separate [cache contract](synchronization.md#message-cache).

Every Session SSE connection, including a Last-Event-ID reconnect, begins with a
current snapshot and then future events. Lag rebuilds that baseline without
replaying the archive. Cumulative usage, run count and bounded recent activity
belong to the producer's durable snapshot. Idle overview reads use that snapshot
and its append-only suffix; missing coverage stays unknown instead of triggering
an all-archive fallback. Context changes preserve cumulative totals.

Overview and explicit detail reading remain independent: opening statistics or a
hover preview does not load history, and detail pages do not change cumulative totals.
Ordinary Chat observation does not start a history loader. A visible Chat may share
the core History source while a member is working to show a bounded recent activity
window; stopping or leaving releases that observation. The History reading
page labels its separate loaded-call statistics; pagination may change that window. Apply the initial overview without
waiting for delivered-message catch-up. Revocation fences both sources.

Windowed readers keep stable anchors while new records arrive, with explicit
older/newer loading and return-to-tail. Full detail is encoded only for the selected
record. Navigation, resource inspections and files follow their own scope and
lifetime rules: [Chat navigation](chat.md#navigation),
[resources](external-capabilities.md#publication), [shared files](shared-files.md).

## Performance and verification

High-frequency work should depend on changed records and index paths. Sharing an
`Arc<Vec<Arc<T>>>` still copies N pointers on modification; appending text must not
copy its accumulated prefix per token. Initial materialization and genuinely changed
dependency ranges may require broader work. Core aggregation, wire conversion,
retained UI layout and GPU rendering are separate costs.

Correctness checks interleave prepare/discard/ack with duplicate input, bursts,
slow consumers, source replacement and revocation. The final platform mirror must
equal its core projection. Compare affected workloads at 1,000/10,000/100,000 mixed
records, including append, distant edit, prepend, deletion and activity-only changes.
Record latency, allocated/transferred bytes, wakeups, retained peak and resets
separately from builds. Microbenchmarks cannot establish device FPS.

Test selection and evidence reporting follow
[zork-client-subscriptions](../../.agents/skills/zork-client-subscriptions/SKILL.md)
and [zork-validation](../../.agents/skills/zork-validation/SKILL.md).

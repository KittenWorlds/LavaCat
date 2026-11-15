# Opt-in JsRunner process boundary (#4)

This additive prototype advances the issue's draft architecture without silently
replacing the accepted same-thread JSRT/window path. The existing `lavacat`
window still owns its DOM; `lavacat-jsrunner` is a separate, testable path whose
controller does not own a live Document or an engine.

## Dependencies and ownership

- `ipc-channel 0.23.1`: Servo-maintained native IPC and refcounted shared memory;
  MIT/Apache-2.0, MSRV 1.86. Reuses platform implementations rather than writing
  an IPC middleware (the independent project in #16 remains outside this repo).
- `rkyv 0.8.18`: maintained archive/bytecheck implementation; MIT, MSRV below 1.90.
  Safe validated access to fixed-endian, unaligned, 32-bit-offset archives.
- `serde 1`: maintained control-message serialization only; MIT/Apache-2.0.

The new crate forbids unsafe. Platform IPC/mapping unsafe stays inside the
upstream dependency; validated archive access uses no unchecked APIs. Rust
stable/MSRV and dependency-license gates cover the transitive graph. Public
controller/view APIs expose project types, standard types and opaque iterators,
not ipc-channel/rkyv/serde types.

One executable distribution selects QuickJS-NG; one controller owns one child
and one page. The child constructs its arena DOM, engine, style system and layout
on the same 32 MiB-stack thread. The parent only receives immutable flat layout
metadata, with stable node IDs and parent indices. Inline atomic boxes are
included at their line origin. Frame views borrow strings/geometry directly
from the validated archive; there is no tree deserialization/reconstruction.
This first layout IR does **not** contain the full paint/glyph/image/hit-test
payload required to replace the current renderer.

## Concrete answers to the draft's eight open questions

| Question | Prototype decision / remaining boundary |
| --- | --- |
| OQ-1 baseline | Add one workspace crate; native ipc-channel control; rkyv layout IR; explicit ABI/capability Hello. No new hand-written IPC implementation. |
| OQ-2 merging | One process/page. Background merging is deferred until workload measurements establish a policy. |
| OQ-3 frame lifecycle | Whole immutable shared-memory allocation per frame, <=16 MiB and 100000 nodes; IPC publication after layout/serialization; handle lifetime pins frame. Recycled ring slots and producer backpressure are not implemented. |
| OQ-4 snapshot | DOM checkpoint after successful context creation/evaluation/event; parent-held, <=16 MiB and 100000 nodes. Preserve arena identities, detached nodes, namespaces, attributes, quirks and template contents. No JS heap/listeners/timers/network/scroll state. |
| OQ-5 reconstruction | Do not replay scripts. Restore DOM and create a fresh engine; applications must explicitly rebuild execution state. This is DOM continuity, not transparent JS-state migration. |
| OQ-6 DOM memory | One existing arena per process/context; no merged multi-DOM arena. |
| OQ-7 old instance | Tear down only after candidate import and (when a viewport exists) successful layout. Failed handoff resumes source. No arbitrary two-second sleep. Renderer-consumption acknowledgement is still future work. |
| OQ-8 layout location | Child owns style/layout; protocol can evolve independently of the engine. Existing window path is retained until the complete paint/input IR is ready. |

Bootstrap, IPC send/backpressure and replies have finite deadlines. On timeout
or disconnect the controller kills/reaps the child; a late response cannot be
consumed as another request's response. ABI/capabilities are checked before
context creation. Input in ABI 1 is a basic Event with a stable target identity;
typed pointer/key payloads and render-side hit testing are not yet integrated.

## Recovery and handoff

`checkpoint` exports engine-neutral DOM IR. `recover_if_unhealthy` probes, then
starts a replacement and imports the last committed checkpoint. `switch_distribution`
freezes/exports the source, validates/imports a candidate and swaps only on
success; failure resumes the source. Tests use a second instance of the same
QuickJS distribution. An actual second engine is not supplied or validated.

These are crash-isolation mechanisms for trusted engine distributions, **not a
hostile-process sandbox**. The shared-memory handle is immutable through our
safe API, but publication is not OS-sealed against a compromised child modifying
its mapping. CPU/memory sandboxing and sealed publication require a separate
security design before executing hostile distributions.

## Verification and remaining requirements

- In-process tests cover script/DOM/layout behavior, import/export (including
  templates/detached nodes), invalid ownership/cycles/indices, shared archive
  validation, inline atomic content and embedded stylesheet preservation.
- Native subprocess tests cover isolation, event dispatch, retained frame life,
  crash recovery, handoff/rollback, suspension, ABI rejection, hung scripts and
  control-channel backpressure. CI explicitly enables `process-tests` on all
  three platforms. The local Work container rejects AF_UNIX SOCK_SEQPACKET with
  EPERM, so those tests require native CI rather than being silently skipped as
  passing locally.
- `cargo test -p lavacat-jsrunner --features process-tests --test process`
  is the reproducible native command.

Issue #4 must stay open. FR-1/7 renderer integration (full paint IR, zero-copy
resource consumption, frame-slot acknowledgement), complete JS-state snapshots,
transparent hot switching/recovery, second-engine validation, typed input,
resource reporting, merging/power policy, package manifests and observability
are not delivered by this prototype. No claim is made that the full PRD is done.

References: issue #4; `2026-10-31-jsrt.md`; ipc-channel's official cross-process
bootstrap and IpcSharedMemory documentation; rkyv's validation documentation.

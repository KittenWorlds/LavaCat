# 2026-11-15 — Open issue follow-up

Branch: `fix/open-issues-scroll-performance`, base: `22bdf71`.

| Item | Implementation / exit criteria |
| --- | --- |
| #12 | Explicit line-height in six container tests; restore macOS to ordinary CI matrix; require native macOS pass. |
| #9.2 | Existing id/class/tag rule buckets independently compared with full scan; preserve matching semantics. |
| #9.3 | Bounded width-independent shaping/metrics cache; text/style/font-registration invalidation and cold-result equivalence tests. |
| #9.1 | Bounded immutable CPU page tiles; generation/width invalidation, fractional fallback, page/canvas/chrome separation, rich pixel regression and benchmark smoke. |
| #4 (prototype milestone) | Child-owned DOM/QuickJS/layout, engine-independent controller, safe shared-memory archive view, bounded DOM IR checkpoints, deadline/health/recovery/handoff rollback. Existing window path migration and full PRD completion remain out of this milestone; keep issue open. |
| #16 | Explicitly excluded from this repository's version control by its issue; no independent IPC middleware added. |

Validation: check/test/clippy/fmt, MSRV 1.90, feature-only builds, license/advisory
checks, generated dependency notices, independent architecture review, native
Linux/macOS/Windows process tests in CI. Native socket IPC cannot run inside the
local Work container (AF_UNIX SOCK_SEQPACKET returns EPERM); no local pass is
claimed for those tests. Manual visual acceptance is recorded separately from
automated pixel comparisons.

Delivery: push commits to the original repository's branch and open one ordinary
(non-draft) PR. Verify required checks and mergeability before reporting ready.

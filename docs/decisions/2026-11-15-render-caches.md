# Scroll bitmaps and width-independent shaping (#9, #12)

## Scope

Reuse the existing vello_cpu/parley implementations. Neither exposes an
application-level cache keyed by page generation. The additional code is
chrome orchestration / cache ownership, not another rasterizer or shaper.

CSS matching already has id/class/tag buckets (`SheetIndex`) and an indexed vs
full-scan semantic test. No second selector index is introduced for #9.2.

## Shaping

CSS Text 3 §5 separates shaping from width-dependent line placement. Cache
successful `ParleyTextShaper` results by exact text, ordered family list and
font-size bits. Font registration synchronizes before lookup and clears shape
and metrics caches. Errors are never cached. Results are independently owned;
font bytes continue to use the existing shared font cache.

Retained shape accounting is capped at 32 MiB / 16384 entries; eviction clears
the table. Metrics have at most 256 keys, each bounded at 4 KiB including family
vector storage. This simple bounded policy may miss on documents larger than
the budget; it cannot grow indefinitely. System font changes require a new
fontique collection, as before. No CSS property or line-breaking rule changes.

Regression: repeated text + metrics did 12 backend builds before the cache and
2 afterwards. Key changes, web-font registration, result ownership and budget
invalidation have separate tests. Existing layout/line-breaking tests still run.

## Scroll rasterization

Cache immutable page-coordinate 256px-high CPU tiles. Integer viewport scrolls
copy retained tile rows to the output buffer; only newly exposed tiles are
rasterized. Build a command vertical-bound index once per display-list revision;
retain all clip transitions and send only intersecting paint commands to new
tiles. Text bounds come from skrifa OpenType glyph outlines through the text
facade, including actual glyph offsets and a coverage guard; unsupported or
invalid bounds disable culling for that command. Fonts with COLR, bitmap or SVG
paint tables also bypass outline-only culling, because their chosen painted
representation can exceed an empty or smaller base outline. No font-size heuristic is used.
`skrifa` was already present as a parley dependency and text dev-dependency; it
is promoted within the text facade for this exact-bounds API (Apache-2.0/MIT,
existing dependency graph / license policy). Retain only visible tiles and at most 64 MiB. Oversized viewports and
fractional offsets use the original complete raster path. Fractional phases
must not reuse integer-phase glyph coverage.

The key is display-list revision plus width. The app uses SceneCache's monotonic
emit count, which changes for layout, font, selection and container-offset
changes. Canvas background and toolbar/scrollbar layers remain in viewport
coordinates and are composed for every frame. Height-dependent background
painting therefore does not enter cached page tiles. A backend error/panic
clears the tile cache; work stays inside the existing isolated render thread.

CSS Overflow 3 §2.2 and Compositing and Blending 1 §5.1 / §10.1 define the relevant
clipping and source-over behavior. Rasterized RGBA8 layers introduce small
rounding differences: the rich regression corpus (text, alpha images, nested
clips, gradients, rounded shapes, tile boundaries and fractional offsets) permits
1 level in premultiplied color/alpha for transparent tiles and 2 for the
final opaque composite. Unpremultiplied RGB is not a useful error metric for
very low-alpha edge pixels, where division magnifies tiny rounding differences. This is a measured test
tolerance, not a guarantee for every future backend or unmodeled effect.

Manual acceptance page: `/performance/scroll-cache.html`.

## macOS tests (#12)

The failed container tests assumed default-font text would overflow 100px and
200px boxes. The native scrollbar hypothesis was incorrect: LavaCat uses a
fixed 8px custom track on every platform. Explicit 24px line-height makes the
ASCII fixtures overflow on all platforms. Production scrolling is unchanged.
macOS is restored to the ordinary Linux/macOS/Windows test matrix.

## References

- https://www.w3.org/TR/css-text-3/#line-breaking
- https://www.w3.org/TR/CSS22/visudet.html#line-height
- https://www.w3.org/TR/css-overflow-3/#scrollable
- https://www.w3.org/TR/compositing-1/#simplealphacompositing

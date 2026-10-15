# twinkle-charts

`twinkle-charts` is Twinkle's optional, bounded native chart engine. It exposes
ordinary Rust data, grammar, recipe, state, diagnostics, table, and export types.
Applications that do not depend on this crate do not acquire chart or timezone
code. The optional JSX adapter in `twinkle-presentation` lowers typed JSON-safe
declarations into this same engine.

## Ownership and revisions

- The application owns immutable `Arc<ChartData>`, `Arc<ChartSpec>`, and
  `ChartState` values. Row keys are durable identity; vector positions are not.
- Increment `ChartData::revision()` whenever any admitted cell or row identity
  changes. Reusing a revision after mutation violates retained-cache identity.
- Keep `ChartView` in application state. Calling `element` projects its accepted
  snapshot into the ordinary Twinkle tree; rebuilding that projection does not
  replace the retained model.
- `replace_and_update` and `update_state` stage a candidate. Validation,
  preparation, or host-admission failure preserves the previous paint, hits,
  semantics, and revision. `last_error` exposes the rejected candidate. If the
  first preparation fails, `snapshot` instead exposes a bounded semantic error
  presentation whose activation still requests the exact data table.
- `retained_diagnostics` reports host-owned output/model bytes. `renderer_lost`
  drops accepted renderer output while retaining reconstructable indexes;
  `detach` contracts chart indexes for a later reattach, and `destroy` is a
  terminal release after which the application must create a new `ChartView`.

```rust
use std::sync::Arc;
use twinkle_charts::{ChartData, ChartLimits, ChartModel, ChartView, Recipes};

# fn build_data() -> ChartData { todo!() }
# fn fields() -> (twinkle_charts::FieldRef, twinkle_charts::FieldRef) { todo!() }
let limits = ChartLimits::default();
let data = Arc::new(build_data());
let (x, y) = fields();
let spec = Arc::new(Recipes::line("Latency", x, y));
let mut chart = ChartView::new(ChartModel::new(spec, data, limits)?)?;
# Ok::<(), twinkle_charts::ChartError>(())
```

See `examples/dashboard.rs` for complete data construction, recipes, controlled
series state, native preparation, accessible table access, CSV, and raster export.
The serial release admission harness is `twinkle-apps/benchmarks/twinkle/src/bin/chart_workloads/measure.rs`; its pinned
reference results and exact scope are documented in
[`../../docs/chart-performance.md`](../../docs/chart-performance.md).

## Grammar and recipes

`ChartSpec` composes `Layer`, `Mark`, `Encoding`, named `Scale`, ordered
`Transform`, `Axis`, `Legend`, `Annotation`, and `Facet` descriptors. Named scales
are shared across layers. Facets share domains by default and can explicitly use
independent domains. V1 never infers a secondary axis: callers explicitly bind
layers and axes to named scales. One vertical scale is placed on the left; an
explicit second is placed on the right with reserved guide space. Both require
a title or unit when independent, render once even when shared by several layers,
and a third vertical axis is rejected instead of overlapping ambiguously.
Convenience recipes return normal inspectable specs, so an
application can append layers or guides before creating a model.

The v1 recipe surface includes line/step/multi-series, vertical and horizontal
simple/grouped/stacked/normalized bars, area/stacked area, scatter/bubble,
pie/donut, histogram, heatmap, and sparkline. `Rule` and `Text` marks and layered
recipes form combo charts without a separate rendering implementation.

Null is distinct from zero. Lines and areas break at missing observations. Bar
and area quantitative domains include zero unless an explicit domain or viewport
overrides it. Pie values must be finite and nonnegative. Bubble radius is the
square root of the size ratio, so visible area is proportional to the value.
Normalized positive and negative stacks are computed independently and preserve
raw/source provenance.

## Interaction and accessibility

`ChartView::nearest`, `inspect_nearest`, and `navigate` use accepted indexed hit
records and return `ChartEvent` values carrying stable row keys and the accepted
data revision. `inspection`, `active_inspection`, and `inspect_nearest_detail`
resolve those hits against the exact accepted dataset. Their `ChartInspection`
cells retain column names, units, typed values, and formatting; transformed marks
therefore expose derived stack, bin, aggregate, and provenance columns rather
than presenting reduced geometry as source truth. Stale or mismatched hits do not
resolve. Applications own selection, visible-series, and viewport state and
feed the next `ChartState` back through `update_state`; the chart never mutates a
second hidden store. The native chart entry emits `ChartEvent::Navigate` for
keyboard/controller/semantic previous, next, and select requests; feed its command
to `ChartView::navigate` in the ordinary application update path. `PanZoomPolicy`
provides pure anchored zoom, constrained pan, and reset operations for the same
event flow.
`ChartNavigation::reconcile` preserves the active dataset/row/series identity when
accepted hit records reorder. If that identity is removed, it selects the surviving
hit at the old bounded index (the following neighbor, or the final preceding hit);
an empty hit set clears inspection.
Home and End emit `NavigationCommand::Home` and `End` for the focused chart rather
than moving focus to unrelated native boundary controls. While the chart is in
adjustment mode, up/down emit `PreviousSeries`/`NextSeries`; left/right retain
their previous/next datum behavior.

Once a datum is active, the native projection adds a plot-bounded visible and
semantic tooltip containing up to six exact fields; further fields remain
available through `ChartInspection` and the data table. The tooltip is passive,
does not take focus, and is never the sole exact-value surface.
`ChartView::element_with_pointer` adds one hit-tested pointer-motion target for
the whole chart. Its coordinate-bearing application message should call
`inspect_nearest` during update; it does not allocate an interaction node for
each datum and does not consume wheel input from an enclosing scroll view.
`element_with_input` additionally accepts a captured-drag mapper and an explicit
wheel mapper through `ChartInputBindings`. Drag phases continue outside chart
bounds and cancel through Twinkle's native capture lifecycle. A wheel binding is
opt-in and consumes the gesture for application-owned zoom; without one, wheel
input continues to the enclosing scroll container.
Opting into drag or wheel input also exposes visible, focusable Zoom in, Zoom out,
and Reset controls. They emit `ChartEvent::Viewport`; pass the request and your
declared `PanZoomPolicy` to `ChartView::viewport_event` to obtain the concrete
controlled `ViewportChanged` event.

The `twinkle-testkit` integration scenarios exercise the public application
boundary rather than model internals: pointer inspection and passive tooltip
semantics, keyboard activation, accessibility and controller adjustment parity,
wheel-controlled zoom and reset, clipped pointer routing, focus-loss drag
cancellation, and nested-scroll ownership when no chart wheel binding is declared.
They toggle a controlled legend, transactionally reproject a dark chart theme,
and replace accepted data through public controls to prove selection keeps stable
identity across reorder before using the documented surviving neighbor after
removal. A separate exact-table scenario pages by pointer and controller, then
activates a stable unit-bearing row by keyboard. `ChartView::update_theme` is the
transactional native theme boundary; a rejected theme preserves accepted paint
and active navigation.

The native projection has one bounded graphical-control entry plus ordinary
focusable legend controls. Its mechanical summary reports source rows,
inspectable/displayed values, missing data, selection, and reduction. Activation
selects the active datum, while a visible, separately focusable **Data table**
control requests exact paged table mode. `ChartTable` keeps all admitted rows
reachable without creating one UI node per row. `page_filtered` applies the
controlled visible-series policy, while `ChartTable::element` projects the page
as a named native grid with exact units, stable row inspection, announced row
positions, Previous/Next controls, and an explicit hidden-series toggle.
Platform screen-reader support is separate from these semantic records; Twinkle
does not claim a tested Linux screen-reader bridge yet.

## Limits, diagnostics, and reduction

`ChartLimits` bounds input rows, columns, series, facets, layers, transforms,
geometry, ticks, annotations, table pages, label bytes, and steady/peak ownership.
The per-label byte cap is enforced for data/schema identity, categorical values,
units, titles, summaries, scales, guides, and annotations before publication;
chart-owned presentation labels additionally share the total-label budget.
The retained-surface host independently bounds paint, interaction, semantic, and
damage records. Raise limits deliberately; their Cartesian product is not an
admission promise. Before allocating large preparation vectors, admission checks
a conservative peak ceiling covering candidate hit/index ownership plus separate
coordinate/reduction, tessellation-geometry, and transform-table scratch owners.
`ChartDiagnostics` reports each scratch ceiling, their total transient bytes,
accepted steady bytes, and the combined peak; caller-owned source storage remains
outside chart ownership.

Retained-update tests use a forced-fresh model as an oracle after data replacement,
visibility and viewport changes, theme changes, bounds changes, fractional scale,
and font revision changes. Paint, interactions, semantics, measurement, and hit
identity must match; damage records are intentionally excluded because a retained
update may report a smaller valid damage region than a cold frame.
Model construction and replacement revalidate immutable data against the model's
own limits, even when that snapshot was originally created under looser limits.
The cap-churn matrix rejects oversized rows, columns, layers, transforms, ticks,
annotations, and per-item/total labels while preserving accepted paint, hits,
semantics, inspection data, and revision; each case then shrinks, detaches, and
destroys the chart to prove contraction. Dedicated tests cover series, facets,
geometry, steady/peak bytes, and exact-table page limits.

Line/area geometry is reduced against logical pixel width while retaining
endpoints and bucket extrema. Exact accepted rows remain in the hit index and
table. `ChartDiagnostics` exposes row/display counts, geometry vertices, index
bytes, guide/command/semantic counts, steady/transient/peak byte estimates, rejection and
cache counters, preparation time, and reduction notices.

## Time, themes, and export

Timestamps are Unix-epoch milliseconds. UTC is deterministic by default;
fixed-offset calendar policies are explicit, and unsupported named zones return
an error rather than silently using process-local time. Light, dark, and
high-contrast semantic themes use color-blind-conscious categorical cues and
secondary shape/dash cues when the palette is exhausted.

`export_csv` quotes every cell and can protect spreadsheet formula prefixes.
`export_raster` renders one accepted revision through the software renderer with
explicit logical dimensions, scale, background, pixel, and byte bounds. File
dialogs, paths, clipboard access, and effects remain application/host authority.

Renderer admission uses `GpuOffscreenRenderer`, which executes the production GPU
command preparation, shaders, clipping, blending, and texture path into a bounded
RGBA target and performs a real mapped-buffer readback. The ignored
`every_family_software_and_gpu_rasters_agree_within_declared_tolerance` matrix is
run explicitly on the pinned headless-GPU environment. It covers every family,
light/dark/high-contrast themes, 112×84 and 960×540 logical allocations, and
1×/1.25×/1.5×/2× scales. Backend-specific curve, thin-stroke, and glyph coverage
may differ: at least 91% of pixels must stay within 32/255 on every channel and
the full-image mean channel delta must remain below 8.25/255.
The companion ignored `every_family_matches_pinned_software_raster_goldens` test
hashes every rendered RGBA pixel for each family over all 24 combinations in
that matrix. Its checked-in per-family digests make intentional renderer changes
reviewable while avoiding hundreds of redundant binary fixture files.

## Runnable consumers

```text
cargo run -p twinkle-charts --example dashboard
cargo run --manifest-path ../twinkle-apps/Cargo.toml --release -p twinkle-benchmarks --features charts --bin twinkle-chart-measure
cargo run --manifest-path ../twinkle-apps/Cargo.toml -p twinkle-benchmarks --features charts --bin twinkle-chart-dashboard
```

The optional workbench dashboard is an external stateful consumer of the public
crate API. It demonstrates hover inspection, keyboard/controller navigation,
legend state, captured pan, wheel and button zoom, exact table mode, responsive
containment, and bounded in-memory CSV/raster export. The base workbench does not
acquire the chart dependency.
The standalone Rust `dashboard` example prepares every P0 recipe and required
orientation/stack/normalization variant plus a layered bar-and-line composition,
then exercises the exact table and both export paths using only public APIs.

With `twinkle-presentation/charts`, JSX `Chart` declarations use this same native
engine. `onTableMode`, `onToggleSeries`, `onNavigate`, `onSelectDatum`,
`onInspectDatum`, `onViewportChange`, `onViewportCommand`, `onPan`, and `onZoom`
are independent generation-scoped callbacks. `onPan` opts into captured native
drag phases and `onZoom` opts into wheel/trackpad delivery; omitting `onZoom`
leaves the gesture with an enclosing scroll container. Applications translate
these gestures into their controlled viewport state. Optional event-specific callbacks fall back to
`onTableMode` for source compatibility; new applications should handle them on
their typed channels. When `onInspectDatum` is present, pointer motion uses an
immutable accepted-generation `ChartInspector` and the native bounded hit index;
the callback receives exact typed cells, units, formatted values, row identity,
and revision without a JavaScript-side scan. Feeding that stable dataset/row
identity back through `state.inspection` renders the same bounded exact-value
inspector as passive plot-local paint and tooltip semantics; a removed identity
clears it without reviving stale data. The compiling gallery is
`crates/twinkle-jsx-runtime/examples/charts.tsx`; it instantiates every P0 recipe
and required grouped/stacked/normalized/horizontal variant plus a layered custom
composition. The TypeScript gate and native admission matrix cover the same
surface, so the gallery cannot silently drift into type-only examples.

Primitive JSX `width`/`height` patches fork the accepted retained chart authority,
reprepare only the bounds-dependent closure, and publish atomically. Data/spec,
indexes, controlled inspection, and callbacks survive the resize; an invalid size
rolls the entire native patch back. Fully constraint-driven sizing from a parent
Flex/Grid allocation uses Twinkle's layout-time child projection: the retained
chart is prepared from the final content rectangle and its paint, hit testing,
focus/controller actions, and semantics remain in the enclosing `UiFrame`.
Intrinsic measurement uses the declared preferred size without executing chart
preparation, unchanged allocations reuse the retained result, and zero-sized
allocations keep a semantic chart surface without emitting invalid paint.

JSX is not limited to the recipe names. A `family: "custom"` specification maps
bounded JSON-safe descriptors directly into the Rust `ChartSpec` grammar:
named linear/log/time/ordinal/band/color/size scales; rect, bar, line, area,
point, arc, rule, and text marks; every encoding channel; ordered sort, bin,
aggregate, and stack transforms; axes, legends, rule/band/text annotations;
shared or independent facets; and original/visible-series domain policy. Field
references may override the declaration's default dataset. Admission validates
the resulting typed Rust specification and uses the same preparation, limits,
rendering, interaction, semantics, diagnostics, and rollback path as recipes.
An equivalent recipe/custom-descriptor fixture is required to produce the same
accepted paint, interactions, semantics, and exact inspection result.


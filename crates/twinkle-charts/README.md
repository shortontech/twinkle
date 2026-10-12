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
The serial release admission harness is `examples/measure.rs`; its pinned
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
wheel-controlled zoom, focus-loss drag cancellation, and nested-scroll ownership
when no chart wheel binding is declared.

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

## Runnable consumers

```text
cargo run -p twinkle-charts --example dashboard
cargo run --release -p twinkle-charts --example measure
cargo run -p twinkle-workbench --features charts --bin twinkle-chart-dashboard
```

The optional workbench dashboard is an external stateful consumer of the public
crate API. It demonstrates hover inspection, keyboard/controller navigation,
legend state, captured pan, wheel and button zoom, exact table mode, responsive
containment, and bounded in-memory CSV/raster export. The base workbench does not
acquire the chart dependency.

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
`crates/twinkle-jsx-runtime/examples/charts.tsx`.

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


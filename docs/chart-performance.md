# Native chart release measurements

These are Twinkle's own chart admission measurements, not comparisons with
other UI frameworks. Run the checked-in serial harness with:

```sh
cargo run --release -p twinkle-charts --example measure
```

For allocator-instrumented admission evidence, run the exact same workloads
through the workbench wrapper:

```sh
cargo run --release -p twinkle-workbench \
  --features charts,pipeline-allocator --bin twinkle-chart-admission
```

The harness constructs source data before timing, warms each operation once,
uses nearest-rank percentiles, and uses a 1200×800 logical viewport at scale 1.
Cold samples create a new model/view and prepare its first accepted snapshot.
Warm samples reuse accepted state. `steady_bytes` is the chart diagnostic's
estimate for prepared geometry and indexes; it excludes caller-owned source
data and is not process RSS. Admission also computes coordinate/reduction,
tessellation-geometry, and transform-table scratch ceilings before large row
scratch allocation; their sum is `transient_bytes`, and `peak_bytes` is the
accepted steady ownership plus that conservative simultaneous scratch ceiling.

The instrumented wrapper installs Twinkle's process-wide counting system
allocator. It reports per-operation p50/p95 allocation/reallocation calls,
deallocation calls, and requested bytes. It also samples Linux `VmRSS` before
and after each scenario and `VmHWM` afterward. Allocation figures exclude
foreign driver allocations, allocator metadata, reserved arenas, and
fragmentation; RSS/HWM covers the whole serial process and therefore carries
forward earlier scenarios rather than attributing memory exclusively to one.

## Reference run

Measured 2026-10-12 on Linux x86-64, AMD Ryzen 9 5900X, 128 GiB RAM, rustc
1.99.0 (`b940084d7`, LLVM 23.1.1), optimized release profile. Samples ran
serially: 11 cold samples and 101 warm samples.

| Scenario | p50 | p95 | Source rows | Displayed rows | Steady bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| Line, 1k | 130 µs | 164 µs | 1,000 | 1,000 | 146,064 |
| Scatter, 1k | 127 µs | 133 µs | 1,000 | 1,000 | 98,112 |
| Line, 100k | 13.37 ms | 17.33 ms | 100,000 | 4,549 | 12,552,592 |
| Scatter, 100k | 15.63 ms | 16.92 ms | 100,000 | 50,001 | 12,334,336 |
| Line, 1M | 162.13 ms | 167.86 ms | 1,000,000 | 4,581 | 100,494,480 |
| Scatter, 1M | 164.07 ms | 166.43 ms | 1,000,000 | 50,001 | 100,274,688 |
| Heatmap, 10k | 882 µs | 1.01 ms | 10,000 | 10,000 | 1,521,792 |
| Grouped bars, 10k | 2.78 ms | 2.89 ms | 10,000 | 10,000 | 1,521,792 |
| Stacked bars, 10k | 17.54 ms | 18.44 ms | 10,000 | 10,000 | 4,181,253 |
| 64 facets | 28.52 ms | 30.93 ms | 6,400 | 6,400 | 2,026,218 |
| Indexed inspection, 1M line | 18.72 µs | 18.84 µs | 1,000,000 | 4,581 | 100,494,480 |
| 16 unchanged 10k charts | 241 ns | 241 ns | 160,000 | no preparation | 27,421,440 |
| 15 unchanged + one viewport update | 538 µs | 607 µs | 160,000 | 4,003 updated | 26,668,552 |
| Complete native-frame construction, 10k line | 11.41 µs | 18.35 µs | 10,000 | 4,003 | 960,952 |

## Allocation and process-memory run

Measured with the same compiler, machine, release profile, viewport, and serial
sample counts. Requested bytes are allocation requests per operation, not live
or retained bytes.

| Scenario | Allocation calls p50/p95 | Requested bytes p50/p95 | RSS before/after | Process HWM |
| --- | ---: | ---: | ---: | ---: |
| Line, 1k cold | 73 / 73 | 512,970 / 512,970 | 5,288 / 5,288 KiB | 5,288 KiB |
| Line, 100k cold | 104 / 104 | 44,842,808 / 44,842,808 | 39,484 / 34,684 KiB | 51,956 KiB |
| Line, 1M cold | 116 / 116 | 366,688,568 / 366,688,568 | 309,688 / 301,752 KiB | 487,672 KiB |
| Indexed inspection, 1M line | 0 / 0 | 0 / 0 | 434,860 / 434,860 KiB | 487,724 KiB |
| 16 unchanged 10k charts | 0 / 0 | 0 / 0 | 434,860 / 434,860 KiB | 487,724 KiB |
| 15 unchanged + one viewport update | 125 / 140 | 4,884,697 / 4,886,627 | 434,860 / 434,860 KiB | 487,724 KiB |
| Complete native-frame construction, 10k line | 74 / 74 | 395,810 / 395,810 | 434,860 / 434,860 KiB | 487,724 KiB |

The zero-allocation unchanged-dashboard result strengthens the retained-path
claim: all 16 views pass through their update entry point without model work,
tessellation, or Rust allocation. Million-point indexed inspection is likewise
allocation-free and source-sublinear. The current native-frame allocation count
includes construction of a fresh declarative tree and `UiFrame`; it is not an
unchanged retained-host presentation.

## GPU presenter admission

The native dashboard has an opt-in deterministic admission sequence over 16
independently retained chart views: initial paint, unchanged rebuild, hover
inspection of the first chart, then controlled zoom of that chart. Build with
attribution and run it against a real GPU backend, then validate the trace:

```sh
cargo build --release -p twinkle-workbench \
  --features charts,pipeline-attribution,pipeline-allocator \
  --bin twinkle-chart-dashboard
TWINKLE_RENDERER=gpu TWINKLE_PIPELINE_TRACE=/tmp/twinkle-chart-gpu.jsonl \
  timeout 4s target/release/twinkle-chart-dashboard --gpu-admission
python3 scripts/summarize_chart_gpu_admission.py \
  /tmp/twinkle-chart-gpu.jsonl
```

Measured 2026-10-12 on the reference machine with an NVIDIA GeForce RTX 4080
SUPER, Vulkan, driver 595.99.02, and a 1200×768 physical surface at scale 1:

| Transition | Prepared commands | Scene draws | Upload calls | Upload bytes | Presenter call |
| --- | ---: | ---: | ---: | ---: | ---: |
| Initial 16-chart frame | 213 | 165 | 1 | 102,816 | 19.96 ms |
| 16 unchanged charts | 0 | 0 | 0 | 0 | 310 µs |
| First-chart hover inspection | 214 | 166 | 1 | 103,104 | 837 µs |
| First-chart controlled zoom | 214 | 166 | 1 | 103,104 | 1.24 ms |

The presenter owns two immutable fullscreen auxiliary quads (576 bytes total),
uploaded once during presenter construction rather than once per frame. The
table counts per-frame vertex uploads. Submission timing is the CPU call to
finalize and enqueue commands; GPU completion and compositor presentation
timestamps are unavailable and are not inferred.

The unchanged dashboard result covers calls through all 16 `ChartView`s and
confirms the identical-input path does not clone indexes or prepare models.
The 1M inspection result uses the accepted ordered index rather than scanning
source rows. Warm inspection, viewport update, and native-frame construction
are below the specification's 16.7 ms p95 interaction target on this machine.
Cold costs are reported separately and are not represented as interactive work.

## Renderer evidence boundary

The renderer trace now captures the required 16-chart upload and CPU submission
accounting. Software/GPU raster agreement is independently verified by the
production-path `GpuOffscreenRenderer`: the explicit admission test reads actual
GPU RGBA bytes for every chart family over the pinned theme/size/scale matrix and
applies the documented 9% boundary-pixel and 8.25/255 mean-channel tolerances.
Per-family software RGBA digests pin the corresponding full raster matrix.

GPU execution-completion and compositor display timestamps remain unavailable on
the current window-presenter boundary and are not inferred from submission time.
This limits claims about physical display latency, but no longer leaves raster
agreement or the specified CPU-side warm-frame budget unmeasured.

## Application-owned benchmark attribution

Applications may publish a payload-free `NativeFrameCorrelation` through
`Application::take_frame_correlation`. The standalone native runtime now calls
`Application::native_frame_submitted` after a successful GPU presenter call or
software buffer present. `NativeFrameSubmission` carries the correlation captured
with that frame, its native frame generation, an `Instant` acknowledgement,
actual backend, scale factor, and physical surface size. Software raster results
that are dirty, stale, or mismatched in size do not produce an acknowledgement.
Applications can use missing correlation generations to detect coalesced frames;
this callback does not disable the runtime's coalescing policy.

This evidence ends at CPU submission. GPU completion and compositor presentation
remain unavailable; neither refresh cadence nor this callback proves display.

Explicit heatmap color domains now remain fixed across snapshot extrema changes.
Explicit bubble size domains map normalized value to marker area (10 logical-pixel
maximum radius), with a 2-pixel minimum radius so zero-size entities remain
inspectable. Implicit domains retain their existing maximum-relative behavior.

### Optional Wayland compositor feedback

Enable `twinkle-ui/wayland-presentation` on Linux to attach a guest event queue to
the standalone winit-owned Wayland display and surface. Where the compositor
advertises `wp_presentation` with `CLOCK_MONOTONIC`, the runtime requests feedback
before each native present commit and delivers `NativeFramePresentation` through
`Application::native_frame_presented`. It also announces capability through
`native_presentation_supported`. Feedback contains the captured correlation,
actual reported presentation time mapped to Rust `Instant`, callback observation
time, refresh interval, display sequence, and explicit discarded-frame state.

Timestamp mapping samples Linux CLOCK_MONOTONIC and Instant back to back; its
uncertainty is the sampling overhead, not event-loop callback delay. Future or
invalid timestamps remain unavailable. Unsupported clocks and non-Wayland
surfaces remain unsupported. The guest queue is bounded to 128 outstanding
feedback objects, never owns or closes winit's display, and is dropped before
its surface/window owner. This feature provides compositor display evidence,
not GPU execution timestamps. Earlier uninstrumented measurements in this
file retain their original unavailable-presentation boundary.

Categorical axis guides now use the admitted category labels, preserving source
order and thinning labels to the requested tick budget. Guide measurement and
painting use the same labels; linear numeric ticks are computed from the domain
in f64 so label values do not acquire f32 mapping noise. Initial native frames
capture their application correlation, including zero-warm-up benchmarks.
`NativeFrameSubmission::present_call_duration` reports CPU time inside the native
presenter call, independently of end-to-end deadline latency.

Controlled line/area legend toggles preserve the palette slots of hidden series,
so remaining series keep their original colors and agree with the legend. State
updates still require an advanced retained content revision from the application.

Native submission timestamps are captured immediately when the present call
returns, before attribution output. The Wayland guest queue retains cloned
window/display owners until its own proxies and queue are gone, including
exceptional teardown paths.

### Generated chart subtree invalidation

A long-running consumer exposed stale output reuse above `with_layout_child`
callbacks. Such nodes now mark their entire ancestry unsafe for bulk resolved
and independent output reuse. A regression repeatedly changes generated content
and identities, asserting current paint plus bounded hit/message counts. This
preserves responsive generated chart content instead of reusing old descendants.

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
data and is not process RSS.

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
| Line, 1k | 129 µs | 156 µs | 1,000 | 1,000 | 129,680 |
| Scatter, 1k | 115 µs | 127 µs | 1,000 | 1,000 | 81,728 |
| Line, 100k | 12.56 ms | 13.07 ms | 100,000 | 4,548 | 10,455,440 |
| Scatter, 100k | 15.60 ms | 17.24 ms | 100,000 | 50,000 | 10,237,184 |
| Line, 1M | 151.93 ms | 153.50 ms | 1,000,000 | 4,580 | 83,717,264 |
| Scatter, 1M | 145.64 ms | 147.71 ms | 1,000,000 | 50,000 | 83,497,472 |
| Heatmap, 10k | 842 µs | 925 µs | 10,000 | 10,000 | 1,259,648 |
| Grouped bars, 10k | 2.65 ms | 2.74 ms | 10,000 | 10,000 | 1,259,648 |
| Stacked bars, 10k | 16.97 ms | 17.07 ms | 10,000 | 10,000 | 1,259,648 |
| 64 facets | 25.65 ms | 25.99 ms | 6,400 | 6,337 | 816,128 |
| Indexed inspection, 1M line | 18.43 µs | 18.48 µs | 1,000,000 | 4,580 | 83,717,264 |
| 16 unchanged 10k charts | 240 ns | 241 ns | 160,000 | no preparation | 23,227,136 |
| 15 unchanged + one viewport update | 522 µs | 544 µs | 160,000 | 4,002 updated | 22,605,320 |
| Complete native-frame construction, 10k line | 10.43 µs | 11.42 µs | 10,000 | 4,002 | 829,880 |

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

The unchanged dashboard result covers calls through all 16 `ChartView`s and
confirms the identical-input path does not clone indexes or prepare models.
The 1M inspection result uses the accepted ordered index rather than scanning
source rows. Warm inspection, viewport update, and native-frame construction
are below the specification's 16.7 ms p95 interaction target on this machine.
Cold costs are reported separately and are not represented as interactive work.

## Scope still requiring evidence

This harness now measures allocator requests and process RSS/HWM, but does not
yet capture renderer upload counts, GPU submission, or software/GPU raster
agreement. Those require the renderer admission harness rather than inference
from preparation timings. The results above therefore establish chart
preparation/index/native-tree/allocation behavior only; they are not the
complete specification-0025 performance gate.

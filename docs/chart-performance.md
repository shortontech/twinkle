# Native chart release measurements

These are Twinkle's own chart admission measurements, not comparisons with
other UI frameworks. Run the checked-in serial harness with:

```sh
cargo run --release -p twinkle-charts --example measure
```

The harness constructs source data before timing, warms each operation once,
uses nearest-rank percentiles, and uses a 1200×800 logical viewport at scale 1.
Cold samples create a new model/view and prepare its first accepted snapshot.
Warm samples reuse accepted state. `steady_bytes` is the chart diagnostic's
estimate for prepared geometry and indexes; it excludes caller-owned source
data and is not process RSS.

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

The unchanged dashboard result covers calls through all 16 `ChartView`s and
confirms the identical-input path does not clone indexes or prepare models.
The 1M inspection result uses the accepted ordered index rather than scanning
source rows. Warm inspection, viewport update, and native-frame construction
are below the specification's 16.7 ms p95 interaction target on this machine.
Cold costs are reported separately and are not represented as interactive work.

## Scope still requiring evidence

This harness does not yet measure allocator call counts, process RSS, renderer
upload counts, GPU submission, or software/GPU raster agreement. Those require
the renderer admission harness rather than inference from preparation timings.
The results above therefore establish chart preparation/index/native-tree
behavior only; they are not the complete specification-0025 performance gate.

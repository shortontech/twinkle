# Native Redux performance record

Benchmark sources live in the sibling `twinkle-apps/benchmarks/` repository.
Commands below assume it is checked out at `../twinkle-apps`.

Baseline measured on 2026-10-12 at repository revision `20b4c11` plus the
completed, uncommitted specification 0024 implementation. Host: Linux 7.0 x86-64, AMD
Ryzen 9 5900X; Rust/Cargo 1.99.0. The command was:

```sh
cargo run --manifest-path ../twinkle-apps/Cargo.toml --release -p twinkle-benchmarks --bin twinkle-redux-measure
```

Each scenario records 10,000 complete operations with `Instant`, after the
release binary starts. Values are nanoseconds and include store transaction
capture, reducer, selector scan, finalization, and observer delivery where
applicable. They are native-core measurements, not native frame timings and
not evidence of a speedup over another implementation.

| Scenario | Median ns | p95 ns |
| --- | ---: | ---: |
| Idle snapshot read | 20 | 30 |
| No-op immediate dispatch | 220 | 221 |
| One changed selected leaf | 260 | 261 |
| Unrelated slice with one selector | 260 | 261 |
| 100-dispatch transaction | 5,941 | 6,512 |
| Changed root with 1,000 selectors | 81,053 | 86,233 |
| One changed dispatch then rollback | 140 | 141 |
| One-MiB structurally shared state update | 240 | 241 |
| Store plus observer retirement | 110 | 120 |

The harness is intentionally dependency-free and lives at
`../twinkle-apps/benchmarks/twinkle/src/bin/twinkle_redux_measure.rs`. Run it on the release target used
for shipping; scheduler noise, allocator choice, power state, and hardware make
cross-machine comparison invalid.

The separate adapter harness includes V8 calls, Redux graph admission, selector
reconciliation, and the full prepare/finalize/after-accept boundary:

```sh
cargo run --manifest-path ../twinkle-apps/Cargo.toml --release -p twinkle-benchmarks --features jsx --bin twinkle-redux-jsx-measure
```

It uses 1,000 samples for the first three scenarios and 250 for the dispatch
burst. These are complete adapter turns in microseconds; they still stop before
a platform renderer/frame boundary.

| Adapter scenario | Median µs | p95 µs |
| --- | ---: | ---: |
| No-op dispatch | 88 | 171 |
| One selected leaf | 86 | 157 |
| Unrelated selected slice | 92 | 209 |
| 100-dispatch burst | 472 | 653 |

The standalone `twinkle-benchmarks` package supplies the end-to-end renderer comparison. All four
runs use 1,000 measured turns after 10 warmups, an 800x720 viewport, the release
profile, and the same retained-presentation/native-frame path. The Redux
variants replace the external snapshot source with `createStore`, `Provider`,
and `useSelector`, and include selector flushing, V8 patch transport, native
store prepare/finalize/after-accept, retained-tree admission, and `UiHost` frame
work. Run each test separately to avoid benchmark contention:

```sh
cargo test --manifest-path ../twinkle-apps/Cargo.toml --release -p twinkle-benchmarks --features jsx --test blocks_admission \
  blocks_shaped_jsx_updates_emit_end_to_end_release_distribution -- --ignored --nocapture
cargo test --manifest-path ../twinkle-apps/Cargo.toml --release -p twinkle-benchmarks --features jsx --test blocks_admission \
  redux_blocks_updates_emit_end_to_end_release_distribution -- --ignored --nocapture
cargo test --manifest-path ../twinkle-apps/Cargo.toml --release -p twinkle-benchmarks --features jsx --test blocks_admission \
  rich_accessible_form_updates_emit_end_to_end_release_distribution -- --ignored --nocapture
cargo test --manifest-path ../twinkle-apps/Cargo.toml --release -p twinkle-benchmarks --features jsx --test blocks_admission \
  redux_rich_form_updates_emit_end_to_end_release_distribution -- --ignored --nocapture
```

| Workload | Complete p50 µs | Complete p95 µs | Native frame p50 µs | Native frame p95 µs | Live / peak owned bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| Blocks external snapshot | 1,767.138 | 2,231.295 | 810.291 | 902.254 | 930,938 / 1,715,764 |
| Blocks Redux adapter | 2,085.760 | 2,515.582 | 807.987 | 875.434 | 930,974 / 1,715,836 |
| Rich form external snapshot | 880.814 | 1,221.938 | 372.874 | 407.460 | 240,955 / 454,306 |
| Rich form Redux adapter | 1,031.919 | 1,478.142 | 384.998 | 491.068 | 239,259 / 450,914 |

The Redux median overhead is 18.0% for Blocks and 17.2% for the rich form. The
measured bridge/checkpoint medians are 884.270 µs and 464.888 µs respectively;
native admission medians are 125.627 µs and 50.566 µs. These results demonstrate
cost rather than a speedup. The existing non-Redux workload code path was not
changed; repeated current-tree runs were within the frozen-run 5% median/10%
p95 thresholds (a first rich-form p95 excursion did not reproduce).

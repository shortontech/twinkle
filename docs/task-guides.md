# Common application tasks

These recipes continue from the [counter application](getting-started.md). They describe where
state belongs and point to executable repository examples; API details are available in rustdoc.

Run the complete form example with `cargo run -p twinkle-ui --example form_validation`, or inspect
the window-free test example with `cargo run -p twinkle-ui --example headless`.

## Forms and validation

Keep entered values and validation errors in the `Application`, not in a component-local shadow
model. A `TextField` emits typed messages; `update` validates the new value, and `view` renders the
current value and error. Give each editable field a stable `id!` so focus and selection survive a
view rebuild. Use `SettingsStatus::error` or ordinary semantic text for an error message, and do not
disable submission unless the same reason is communicated to accessibility consumers.

## Dynamic collections

Use stable domain keys for collection items. Reordering a vector must not change the `UiId` of an
existing row. `CollectionSource::try_new` rejects duplicate keys instead of allowing retained
focus, selection, or scroll state to attach to the wrong item. Use `VirtualColumn` for large data
sets and an ordinary `Column` when every item is small enough to measure each frame.

## Images and assets

Decode assets outside `view`, retain the resulting `Arc<image::RgbaImage>` in application state,
and pass it to `Image`. Repeated decoding in `view` defeats retained resource identity. Use `Icon`
for tintable alpha-mask artwork and supply either an accessible label or mark it decorative.

## Background work

The `Application` update loop is synchronous. Run file, network, or CPU-heavy work on an owned
worker and deliver bounded results through a `HostAdapter`; do not block `update` or `view`.
`AsyncRasterWorker` is renderer infrastructure, not a general application executor. Make shutdown
and stale-result behavior explicit in the adapter.

## Headless testing

Construct `UiHost` or `UiFrame` at a fixed logical size, dispatch typed or semantic actions, and
assert against `ResolvedLayout`, accessibility nodes, or `deterministic_snapshot`. The
`twinkle-testkit` crate adds fixture registries and RGBA rendering. Keep native-window smoke tests
separate from deterministic semantic tests so CI failures identify the affected layer.

The independent example in
[`crates/twinkle/tests/fixtures/external-consumer`](../crates/twinkle/tests/fixtures/external-consumer)
shows a minimal downstream application. The workbench demonstrates reusable fixtures:

```sh
cargo run -p twinkle-workbench -- list
cargo run -p twinkle-workbench -- validate
```

## Packaging an application

Pin a tested Twinkle revision while the project is pre-1.0, commit `Cargo.lock` for applications,
and build on every supported target. Linux distribution packages must declare the native libraries
listed in the getting-started guide. Bundle fonts and image assets using your application packager;
Twinkle does not currently install desktop entries, icons, or platform bundles.

Before shipping, exercise keyboard-only input, scaling, both Wayland and X11 when supported, and
the accessibility behavior described in [Accessibility](accessibility.md). See
[Troubleshooting](troubleshooting.md) for logging and native startup failures.

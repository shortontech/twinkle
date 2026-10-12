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

## Direct controller input and semantic menus

Use Twinkle's ordinary semantic controller navigation for buttons, menus, dialogs, sliders, and
other focusable UI. Games and other direct-input surfaces can instead implement
`Application::controller_input`. The callback runs before Twinkle derives semantic navigation from
the same physical event. Return `ControllerInputOutcome::handled(changed)` while gameplay owns the
control, or `ControllerInputOutcome::unhandled()` while a menu is open so D-pad/stick navigation and
confirm continue through the declarative UI.

```rust
fn controller_input(
    &mut self,
    event: ApplicationControllerEvent,
) -> ControllerInputOutcome {
    if self.menu_open {
        return ControllerInputOutcome::unhandled();
    }
    match event.input {
        ApplicationControllerInput::Button {
            button: ControllerButton::DPadLeft,
            edge: KeyEdge::Pressed,
            repeat: false,
        } => {
            self.move_left();
            ControllerInputOutcome::handled(true)
        }
        ApplicationControllerInput::Button { edge: KeyEdge::Released, .. } => {
            self.release_controller_button();
            ControllerInputOutcome::handled(false)
        }
        _ => ControllerInputOutcome::unhandled(),
    }
}
```

Use `poll_controller` for simulation-owned held input instead of reconstructing state or depending
on backend key repeat. Its `ControllerSnapshot` retains held buttons and latest axes; pressed and
released edges accumulate between application polls and are visible for exactly one poll.

```rust
fn poll_controller(&mut self, input: &ControllerSnapshot) -> bool {
    if input.button_pressed(&ControllerButton::South) {
        self.rotate();
    }
    if input.button_down(&ControllerButton::DPadDown) {
        self.soft_drop();
    }
    self.steer(input.axis(&ControllerAxis::LeftX));
    true
}

fn poll_interval_controller(&self, input: &ControllerSnapshot) -> Option<Duration> {
    Some(if input.button_down(&ControllerButton::DPadDown) {
        Duration::from_millis(40)
    } else {
        self.gravity_interval()
    })
}
```

Use the device-specific queries when players or seats must remain distinct. Combined axis queries
select the value with greatest absolute magnitude deterministically. Axis values are finite and
clamped to `-1.0..=1.0`; the native reader suppresses sub-epsilon changes and coalesces superseded
samples for one device/axis before waking the host. Button edges are never coalesced.

Twinkle blocks on its controller reader and wakes the native event loop when useful input arrives;
an idle connected controller does not impose a UI polling timer. Do not create one focusable button
per game action or open a second controller backend. Physical application input and semantic
fallback share Twinkle's focused, fenced reader. Hosted sources currently carry semantic actions
only and do not fabricate physical events.

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

## Dashboards and charts

Add `twinkle-charts` explicitly; the base `twinkle-ui` dependency deliberately
does not pull it in. Build immutable `ChartData` with stable dataset and row keys,
choose a `Recipes` constructor or compose a `ChartSpec`, and retain `ChartView` in
application state. Call `update`, `update_state`, `update_theme`, or
`replace_and_update` during the application update path, then project it with
`element` or `element_with_input`. Do not construct a fresh chart model in every
`view` call.

Selection, visible series, viewport, and table mode are controlled application
state. Handle `ChartEvent`, publish the resulting `ChartState`, and keep the data
revision aligned with the immutable snapshot. Use `ChartTable` as the exact-value
accessible fallback; reduced plot geometry never replaces the admitted rows.
Controller, keyboard, pointer, and semantic actions all enter through the same
event boundary.

Run the complete public Rust consumer and the optional live workbench dashboard:

```sh
cargo run -p twinkle-charts --example dashboard
cargo run -p twinkle-workbench --features charts --bin twinkle-chart-dashboard
```

For TSX hosts, enable `twinkle-presentation/charts` and use the typed `Chart`
declaration. The compiling gallery at
[`crates/twinkle-jsx-runtime/examples/charts.tsx`](../crates/twinkle-jsx-runtime/examples/charts.tsx)
covers every P0 recipe plus custom composition. Charts expose semantic records and
an exact table, but those records do not by themselves create the platform
screen-reader bridge described in [Accessibility](accessibility.md).

## Packaging an application

Pin a tested Twinkle revision while the project is pre-1.0, commit `Cargo.lock` for applications,
and build on every supported target. Linux distribution packages must declare the native libraries
listed in the getting-started guide. Bundle fonts and image assets using your application packager;
Twinkle does not currently install desktop entries, icons, or platform bundles.

Before shipping, exercise keyboard-only input, scaling, both Wayland and X11 when supported, and
the accessibility behavior described in [Accessibility](accessibility.md). See
[Troubleshooting](troubleshooting.md) for logging and native startup failures.

For reusable design-system components, see the
[Rust component guide](../crates/twinkle/docs/reusable-components.md). For hosts that execute TSX,
see the [JSX and CSS guide](../crates/twinkle-presentation/docs/jsx-host.md) and
[CSS reference](../crates/twinkle-presentation/docs/css.md).

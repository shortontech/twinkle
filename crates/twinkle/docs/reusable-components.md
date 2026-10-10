# Building reusable Rust UI systems

Twinkle applications own durable domain state and typed messages. Components are ordinary Rust
functions that turn caller-owned values into a typed `View<Message>`; they do not own a second copy
of application state. This keeps a design system reusable across applications and lets each
application decide what an interaction means.

## Define a component

Use `#[component]` when you want a function's named parameters to be available as properties in
`ui!`. Required parameters become required properties. Parameters typed as `Option<T>` are optional
and default to `None`. Property order at the call site is flexible, and unknown, duplicate, or
missing required properties produce errors where the component is used.

The executable [`examples/component_system.rs`](../examples/component_system.rs) defines a
`PreferenceCard` with caller-supplied identity, content, current value, and typed action. The core
pattern is:

```sh
cargo run -p twinkle-ui --example component_system
```

```rust,ignore
#[component]
fn PreferenceCard(
    title: &str,
    enabled: bool,
    on_toggle: Message,
) -> impl View<Message> {
    ui! {
        <Container>
            <Column gap={8.0}>
                <Text>{title}</Text>
                <Button on_press={on_toggle}>
                    {if enabled { "Turn off" } else { "Turn on" }}
                </Button>
            </Column>
        </Container>
    }
}
```

Pass retained identity as an `id` property at the call site (`id={id!(notifications)}`). Twinkle
consumes `id` and `key` as tree identity, separately from the component's declared properties;
they are not forwarded into the generated component macro.

Prefer values and messages as component inputs. Keep fetched data, validation, persistence, and
business rules in the application or an application-owned service. A component can expose more
than one typed action by accepting separate messages or mapper functions; the parent maps each
message into its own `Message` enum.

## Compose a system

Build larger systems from small components and existing primitives. Use `ui!` when the shape is
mostly static and builder methods when ordinary Rust control flow is clearer; both produce the
same component tree. `AnyView<Message>` is useful when different branches need one concrete return
type. For collections, use stable domain keys and stable `UiId`s so retained focus, selection, and
scroll state stay with the same logical item after reordering. See the
[dynamic collections recipe](../../../docs/task-guides.md#dynamic-collections).

The application is the state owner. Each event becomes a typed message, `update` changes domain
state, and `view` projects the new state back into components. Avoid storing state in a component
that is recreated during `view`; use stable IDs for Twinkle's retained interaction state instead.

## Add behavior and semantics

Use the semantic component constructors and semantic theme tokens when building controls that
need consistent hover, pressed, keyboard-focus, controller-focus, disabled, and selected states.
Give controls stable IDs when they own editing, focus, selection, or controller-navigation state.
Use explicit semantic roles and accessible labels for custom-painted or icon-only controls.

Use `CustomPaint` only for graphical content that cannot be expressed with ordinary components.
Its callback receives the allocated rectangle and returns bounded paint commands; interaction,
identity, semantics, and actions remain declared on the component itself.

## Cross a host boundary only for an embedding use case

Most application developers should import the `twinkle_ui::prelude` and call `run(app)`. Use
`UiHost`, `UiFrame`, protocol types, or backend APIs when implementing an embedding, deterministic
test harness, renderer, or platform adapter. Host adapters should translate external events into
typed messages and keep service policy and authorization in the host.

See the [full crate guide](../README.md), the
[headless example](../examples/headless.rs), and the
[task recipes](https://github.com/shortontech/twinkle/blob/main/docs/task-guides.md) for testing and application-level patterns.

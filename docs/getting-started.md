# Getting started with Twinkle

This guide takes a new Rust project from installation to its first native Twinkle window. Twinkle
currently requires Rust 1.96 or newer.

## 1. Install platform prerequisites

Twinkle opens a native window and includes controller input, so Linux builds need system
development packages. The packages used by CI on Ubuntu are:

```sh
sudo apt-get update
sudo apt-get install -y pkg-config libudev-dev libwayland-dev libxkbcommon-dev \
  libx11-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev
```

Windows builds need the MSVC Rust toolchain and the Visual Studio C++ build tools. Other Linux
distributions use differently named packages; install the development packages for udev, Wayland,
XKB, X11/Xcursor/Xrandr/XInput, and OpenGL. macOS is not currently tested in CI; see
[support and compatibility](../SUPPORT.md).

## 2. Create a project

```sh
cargo new hello-twinkle
cd hello-twinkle
```

Until a crates.io release is available, add Twinkle as a Git dependency and pin the revision to a
commit you have tested:

```toml
[dependencies]
twinkle-ui = { git = "https://github.com/shortontech/twinkle.git", rev = "REPLACE_WITH_A_COMMIT_SHA" }
```

After publication, use the versioned dependency `twinkle-ui = "0.2"` instead.

## 3. Create the application

Put this in `src/main.rs`:

```rust,no_run
use twinkle_ui::prelude::*;

#[derive(Default)]
struct Counter {
    value: i32,
}

#[derive(Clone)]
enum Message {
    Increment,
    Decrement,
}

impl Application for Counter {
    type Message = Message;

    fn update(&mut self, message: Message) {
        match message {
            Message::Increment => self.value += 1,
            Message::Decrement => self.value -= 1,
        }
    }

    fn view(&self, _context: twinkle_ui::ViewContext) -> impl View<Message> {
        ui! {
            <Column gap={12.0} padding={Insets::all(24.0)}>
                <Text scale={2.0}>{format!("Count: {}", self.value)}</Text>
                <Row gap={8.0}>
                    <Button on_press={Message::Decrement}>{"−"}</Button>
                    <Button on_press={Message::Increment}>{"+"}</Button>
                </Row>
            </Column>
        }
    }

    fn title(&self) -> &str {
        "Twinkle Counter"
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run(Counter::default())
}
```

Run it with `cargo run`.

## How the application model works

Your `Application` owns domain state. User interactions produce values of your `Message` type.
Twinkle passes each message to `update`, and then calls `view` to describe the current interface.
The runtime owns the native window, event loop, presentation, and per-window interaction state.

`ui!` is optional syntax for building the typed component tree. The public builder API produces the
same tree and is useful when ordinary Rust control flow is more convenient. See
[`examples/builder.rs`](../crates/twinkle/examples/builder.rs).

## What to learn next

1. Add `Text`, `Button`, `Slider`, `TextField`, `Row`, and `Column` from the prelude.
2. Keep durable application state in your `Application`; give stateful UI stable IDs with `id!`.
3. Apply semantic colors and sizing with `SemanticTheme` and the semantic component constructors.
4. Add controller navigation scopes only when the basic keyboard and pointer flow works.
5. Exercise layout and semantic output headlessly with `UiFrame`, resolved layout, and snapshots.
6. Use host adapters when the application needs services or protected effects.
7. Enable the separate JSX tooling only if the application needs TSX execution.

The [crate guide](../crates/twinkle/README.md) documents controller navigation, themes, settings and
launcher composition, layout, custom painting, and the compile-time guarantees of `ui!`. Generate
the complete API reference with:

```sh
cargo doc -p twinkle-ui --open
```

Continue with the [common application tasks](task-guides.md) for forms, collections, assets,
background work, testing, and packaging. The [accessibility statement](accessibility.md) separates
semantic-tree support from native assistive-technology bridges, and
[troubleshooting](troubleshooting.md) covers logging and common platform failures.

## Current feature surface

The default `twinkle-ui` dependency includes native controller discovery. Hosted-controller or
controller-free applications can omit gilrs with
`twinkle-ui = { version = "0.2", default-features = false }`; the controller event vocabulary
remains available. Native windowing, clipboard support, image support, and software presentation
currently remain core capabilities. It does not include Oxc or V8. JSX support belongs to the repository's `twinkle-presentation` and
`twinkle-workbench` crates and is enabled there with their `jsx` feature.

If this dependency surface does not suit your deployment target, open an issue describing the
target and the capability split you need before building around private modules.

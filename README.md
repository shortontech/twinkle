# Twinkle

Twinkle is a native Rust UI engine with declarative, typed views; intrinsic layout; keyboard and
controller input; accessibility semantics; and an included software presenter.

The project is currently pre-1.0 (`0.2`). It is suitable for evaluation and early adoption, but
public APIs may still change between minor releases. See [support and compatibility](SUPPORT.md)
before adopting it in a production application.

## Try it

Twinkle requires Rust 1.96 or newer. On Ubuntu, install the native window and controller
development libraries first:

```sh
sudo apt-get update
sudo apt-get install -y pkg-config libudev-dev libwayland-dev libxkbcommon-dev \
  libx11-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev
```

Then clone the repository and run the standalone counter:

```sh
git clone https://github.com/shortontech/twinkle.git
cd twinkle
cargo run -p twinkle-ui --example standalone
```

To use Twinkle from another project while releases are not yet published, add the Git dependency:

```toml
[dependencies]
twinkle-ui = { git = "https://github.com/shortontech/twinkle.git", rev = "REPLACE_WITH_A_COMMIT_SHA" }
```

Pinning a commit keeps builds reproducible while the API is evolving. Once `twinkle-ui` is published
on crates.io, the equivalent versioned dependency will be `twinkle-ui = "0.2"`.

Copy [`crates/twinkle/examples/standalone.rs`](crates/twinkle/examples/standalone.rs) into
`src/main.rs`, then run `cargo run`. The example shows the complete application lifecycle: state,
typed messages, `update`, `view`, and native startup.

For an explanation of the application model and the next practical steps, read the
[getting-started guide](docs/getting-started.md). The detailed component and layout reference lives
in the [`twinkle-ui` crate guide](crates/twinkle/README.md). Build local API documentation with
`cargo doc -p twinkle-ui --open`; after publication it will also be available on
[docs.rs](https://docs.rs/twinkle-ui).

## Repository commands

```sh
cargo run -p twinkle-ui --example standalone
cargo run -p twinkle-workbench
cargo run -p twinkle-workbench --features jsx --bin twinkle-jsx-preview
cargo check --workspace --all-targets --all-features
cargo test --workspace --all-features
```

The main `twinkle-ui` package currently has no Cargo features: native windowing, software presentation,
clipboard support, and controller support are included together. JSX is not part of its dependency
closure. Repository tools that execute JSX enable the separate `jsx` feature on
`twinkle-workbench` or `twinkle-presentation`; those builds include Oxc and V8.

The standalone TSX preview uses native controls and controller input with an explicit bounded
ordinary effect dispatcher. It installs no host services or privileged action authority.
Applications add their own service schemas, policy, and protected effect dispatch through host
adapters.

Twinkle uses upstream winit's cross-platform data-transfer API; see
[window integration](crates/twinkle/docs/winit-provenance.md). Source is licensed under MIT OR
Apache-2.0 except separately licensed dependencies. Twinkle is adopted by Nickel.

## Project information

- [Getting started](docs/getting-started.md)
- [Support, platforms, and compatibility](SUPPORT.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)
- [Changelog](CHANGELOG.md)
- [License](LICENSE-MIT) / [Apache license](LICENSE-APACHE)

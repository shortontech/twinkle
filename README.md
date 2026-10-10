# Twinkle

Twinkle is a native Rust UI engine with reusable input, layout, controls, rendering assets, CSS/native presentation and optional JSX/TSX execution.

Ordinary Rust applications depend on `twinkle`; its dependency closure excludes Oxc and V8. Native presentation defaults to executor-independent admission/CSS. Enable its `jsx` feature when choosing the JavaScript executor. The default component gallery also excludes the executor.

```sh
cargo run -p twinkle --example standalone
cargo run -p twinkle-workbench
cargo run -p twinkle-workbench --features jsx --bin twinkle-jsx-preview
cargo check --workspace --all-targets --all-features
cargo test --workspace --all-features
```

The standalone TSX preview uses native controls and controller input with an explicit bounded ordinary effect dispatcher. It installs no host services or privileged action authority. Applications add their own service schemas, policy and protected effect dispatch through host adapters.

The workspace uses upstream winit's cross-platform data-transfer API; see [window integration](crates/twinkle/docs/winit-provenance.md). Linux builds require native Wayland/X11 development libraries and libudev for controller input. Optional JSX builds include Oxc and V8; plain Rust builds do not require them. Source is MIT OR Apache-2.0 except separately licensed dependencies such as Apache-2.0 winit.

Twinkle is adopted by Nickel.

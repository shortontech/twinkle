# Twinkle

Twinkle is a native Rust UI engine with reusable input, layout, controls, rendering assets, CSS/native presentation and optional JSX/TSX execution. It runs without Nickel or a desktop-shell catalog.

Ordinary Rust applications depend on `twinkle`; its dependency closure excludes Oxc and V8. Native presentation defaults to executor-independent admission/CSS. Enable its `jsx` feature when choosing the JavaScript executor. The default component gallery also excludes the executor.

```sh
cargo run -p twinkle --example standalone
cargo run -p twinkle-workbench
cargo run -p twinkle-workbench --features jsx --bin twinkle-jsx-preview
cargo check --workspace --all-targets --all-features
cargo test --workspace --all-features
```

The standalone TSX preview uses native controls and controller input with an explicit bounded ordinary effect dispatcher. It installs no shell services or privileged action authority. Nickel keeps its compositor/session ownership, grants, manifests, catalog, Settings/provider policy and protected action dispatch in downstream host adapters.

The workspace uses upstream winit's cross-platform data-transfer API; see [window integration](crates/twinkle/docs/winit-provenance.md). Linux builds require native Wayland/X11 development libraries and libudev for controller input. Optional JSX builds include Oxc and V8; plain Rust builds do not require them. Source is MIT OR Apache-2.0 except separately licensed dependencies such as Apache-2.0 winit.

Relevant history was imported from Nickel with Git fast-export/import across the old and new crate paths, bootstrap/declaration history, portable core source origins, licenses and the former winit fork. Nickel-only current source was removed after import.

Repository extraction is complete. Nickel's downstream adoption and platform interaction acceptance remain ongoing work.

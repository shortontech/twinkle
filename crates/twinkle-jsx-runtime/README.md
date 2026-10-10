# `twinkle-jsx-runtime`

`twinkle-jsx-runtime` provides an opt-in V8 evaluator for trusted TSX packages. It owns generic
module preparation, rendering and event transactions; the embedding Rust host owns package
discovery, component admission, data/effect schemas, validation, authorization, and native
presentation. It is not needed by Rust-only `twinkle-ui` applications.

Most hosts should start with the higher-level
[`twinkle-presentation` JSX host guide](https://github.com/shortontech/twinkle/blob/main/crates/twinkle-presentation/docs/jsx-host.md),
which includes a complete native-window example. The
[API ownership reference](docs/api-ownership.md) explains which layer owns each contract. The
[type declaration guide](types/README.md) describes the native TSX vocabulary and editor setup.

This crate alone does not turn arbitrary JavaScript values into safe application operations.
Treat package source as executable code and validate every requested effect in the host before
committing runtime state or performing an operation.

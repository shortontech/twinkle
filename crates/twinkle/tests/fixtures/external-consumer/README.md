# External consumer smoke test

This independent Cargo workspace compiles a minimal Twinkle application through
the same public API available to a downstream project. It intentionally does not
inherit dependencies or settings from Twinkle's workspace.

Run it from the repository root with:

```sh
cargo check --manifest-path crates/twinkle/tests/fixtures/external-consumer/Cargo.toml
```

The release-readiness script runs this check after packaging every publishable
workspace crate.

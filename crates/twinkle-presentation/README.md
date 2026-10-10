# `twinkle-presentation`

`twinkle-presentation` adapts validated Twinkle protocol trees into native `twinkle-ui` components.
It is intended for Rust hosts that accept declarative UI packages or need protocol presentation
adapters. A normal Rust application should depend on
[`twinkle-ui`](https://github.com/shortontech/twinkle/blob/main/crates/twinkle/README.md) directly.

The default feature set includes protocol component admission, virtual collection sources, image
registries, and the bounded CSS compiler. The optional `jsx` feature adds the V8-based JSX runtime
and the `page` adapter; enabling it adds a substantial native dependency and is unnecessary for
Rust-only applications.

Start here:

- [Embedding JSX and CSS in a Rust host](docs/jsx-host.md) for an end-to-end host example.
- [Supported CSS subset](docs/css.md) for selectors, properties, values, limits, and examples.
- [Generated API reference](https://docs.rs/twinkle-presentation) for public Rust types.

Hosts own package discovery, admission policy, validation, service schemas, authorization, and
effect dispatch. The generic JSX runtime provides an isolated evaluator and transaction machinery;
it does not grant packages operating-system or application authority.

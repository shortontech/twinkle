# Embedding JSX and CSS in a Rust host

This guide is for an application that owns a native Twinkle window and wants to render a TSX
package inside it. Rust-only applications should use `twinkle-ui` directly and do not need V8.
The complete runnable implementation is
[`examples/jsx_host.rs`](../examples/jsx_host.rs), with its
[TSX source](../examples/jsx_host.tsx) and [stylesheet](../examples/jsx_host.css).

## Dependencies

Use the same pinned repository revision for all pre-release Twinkle crates, or matching published
versions after release:

```toml
[dependencies]
twinkle-ui = { package = "twinkle-ui", git = "https://github.com/shortontech/twinkle.git", rev = "REPLACE_WITH_A_COMMIT_SHA" }
twinkle-presentation = { git = "https://github.com/shortontech/twinkle.git", rev = "REPLACE_WITH_A_COMMIT_SHA", features = ["jsx"] }
twinkle-jsx-runtime = { git = "https://github.com/shortontech/twinkle.git", rev = "REPLACE_WITH_A_COMMIT_SHA" }
twinkle-protocol = { git = "https://github.com/shortontech/twinkle.git", rev = "REPLACE_WITH_A_COMMIT_SHA" }
serde_json = "1"
```

The `jsx` feature on `twinkle-presentation` enables `twinkle-jsx-runtime`; listing that crate
directly makes its Rust loader/runtime types available to your host code. This adds V8 and Oxc to
the dependency graph. It is a deliberate cost of executing TSX and is not required for normal Rust
applications.

## Host setup

The example follows this sequence:

1. Build a `JsxModuleGraph` from package-relative `.tsx`/`.js` modules and `.css` files. CSS
   side-effect imports join the graph's deterministic stylesheet output.
2. Compile that output with `StyleSheet::compile` (or provide your palette/host properties).
3. Create a `SurfaceBounds` manifest. The JSX `Window` request must match a surface explicitly
   granted by the host; the package cannot enlarge that grant.
4. Create `JsxRuntime::new_modules`, render `__twinkleRender()` through
   `render_retained_panel`, and validate the initial candidate against the stylesheet and grant.
5. Return the admitted `PanelNode` as `node.view(images, stylesheet)` from the host's
   `Application::view`.
6. Convert native `PluginMessage` events to runtime actions. Start a runtime transaction, dispatch
   the event, apply any bounded native patch to a clone of the accepted tree, validate requested
   effects against the host's own schema and policy, then accept both the tree and JS state only if
   validation succeeds. On failure, reject the transaction and keep the prior native tree.

Run the example from the workspace root:

```sh
cargo run -p twinkle-presentation --features jsx --example jsx_host -- validate
cargo run -p twinkle-presentation --features jsx --example jsx_host
```

The `validate` command builds the module graph and stylesheet, admits and lays out the bounded
native tree, dispatches the example button, and confirms Rust accepts the declared `set-count`
effect without opening a native window.

The button requests only `{type: "set-count", value: N}`. Rust accepts that exact operation with
an unsigned integer payload; unknown operations and malformed values are rejected. This is the
important host boundary: JSX can request an operation, while Rust decides whether the operation is
recognized and what authority it receives. Do not dispatch arbitrary guest-provided objects to
application services or operating-system APIs.

## Package modules and TypeScript

`JsxModuleGraph` supports relative static imports, default/named/side-effect imports, local default
and named exports, and CSS imports. It rejects package imports, URLs, native paths, dynamic module
specifiers, and unsupported module extensions. Hosts remain responsible for discovering a package,
choosing its entry point, checking size/resource limits, and admitting its exports.

For editor type checking, use the declaration vocabulary shipped in
[`twinkle-jsx-runtime/types/twinkle.d.ts`](https://github.com/shortontech/twinkle/blob/main/crates/twinkle-jsx-runtime/types/twinkle.d.ts)
and its [declaration guide](https://github.com/shortontech/twinkle/blob/main/crates/twinkle-jsx-runtime/types/README.md).
These describe native controls and hooks, not browser DOM elements or APIs. Add your host-specific
effect and data schemas in your application's own declarations.

Read the [CSS subset reference](css.md) before authoring stylesheets. In particular, this runtime
does not provide browser CSS, a DOM, or implicit service permissions. The ownership boundaries and
transaction model are summarized in the
[JSX API ownership document](https://github.com/shortontech/twinkle/blob/main/crates/twinkle-jsx-runtime/docs/api-ownership.md).

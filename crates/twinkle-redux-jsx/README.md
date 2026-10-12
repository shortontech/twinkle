# `twinkle-redux-jsx`

Optional JSX/TSX adapter for the standalone `twinkle-redux` crate. It admits
only the host-provided `twinkle/redux` module. Rust owns store identity, opaque
root handles, generations, and acceptance; immutable state graphs remain rooted
inside the checkpointed isolate and are not JSON-cloned. Native Rust
applications do not depend on this crate and therefore do not compile V8 or Oxc.

Import its bounded API from `twinkle/redux`. See the
[native Redux guide](../../docs/native-redux.md) for lifecycle and compatibility
details and `types/twinkle-redux.d.ts` for declarations.

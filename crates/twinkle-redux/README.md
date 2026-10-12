# `twinkle-redux`

`twinkle-redux` is Twinkle's native, transactional Redux-style state library.
It provides immutable-root reducers, typed actions, commit-only subscriptions,
equality-filtered selector observation, and parent-linked store scopes without
depending on JSX, JavaScript, V8, React, or npm.

The transaction lifecycle is explicit: capture/begin, provisional dispatch,
prepare, callback-free finalize, and after-accept observer delivery. Dropping a
transaction before finalization restores the accepted root and suppresses its
notifications.

The API implements completed specification 0024.

See the [native Redux guide](../../docs/native-redux.md) for the Rust API,
transaction lifecycle, Provider-style scopes, compatibility differences, and
limits. Run the dependency-free example with:

```sh
cargo run -p twinkle-redux --example native_counter
```

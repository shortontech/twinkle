# Native transactional Redux

`twinkle-redux` provides Redux-style state management to Rust applications
without enabling JSX or linking V8. Reducers receive an immutable state borrow
and a typed action, then explicitly return `Reduction::Changed(next)` or
`Reduction::Unchanged`.

```rust
use twinkle_redux::{Reduction, RollbackSafe, Store};

#[derive(Clone)]
struct State { count: i64 }
impl RollbackSafe for State {}

enum Action { Add(i64) }

let store = Store::new(State { count: 0 }, |state: &State, action: &Action| {
    let Action::Add(amount) = action;
    Ok::<_, &'static str>(Reduction::Changed(State {
        count: state.count + amount,
    }))
});

store.dispatch_handle().dispatch(&Action::Add(2))?;
assert_eq!(store.snapshot().count, 2);
# Ok::<(), Box<dyn std::error::Error>>(())
```

`RollbackSafe` is an explicit state-policy promise. A state clone must not share
interior-mutability that can alter the accepted root. The crate implements the
trait for scalar values and recursively safe standard collections. Applications
may implement it for their own immutable replacement types; that implementation
is a trusted host obligation. Heap-owning implementations must override
`rollback_bytes` so state and accepted/provisional overlap can be bounded; the
default accounts only the inline Rust value.

## Acceptance lifecycle

Standalone stores use an immediate transaction per dispatch. Native UI hosts
should construct stores with `Store::coordinated`, enlist them in a
`TransactionCoordinator`, and follow this sequence:

1. Enlist all stores before application callbacks.
2. Dispatch typed actions through stable dispatch handles. Reads see the
   provisional root synchronously.
3. Call `prepare_accept`. Selector observers run against final provisional
   roots and return the deduplicated `OwnerId` values whose selections changed.
4. Rebuild and validate the affected native owners.
5. Call `finalize_accept` only after every native candidate is ready. This step
   invokes no application callbacks.
6. Call `after_accept` to deliver committed selector and public observers.

Dropping or aborting the coordinator before finalization restores every store.
Observer delivery is post-acceptance: a panic is reported after other observers
run and cannot roll back already accepted state. Dispatch during observer
delivery is rejected.

Public observers run once for each accepted transaction containing a dispatch,
including an unchanged dispatch. Selector observers notify only when their
equality function reports a changed selected value. Unlike upstream Redux,
recursive listener dispatch is deliberately unsupported.

## Provider-style native scopes

`StoreScope` is the Rust equivalent of a Provider hierarchy. A `StoreKey<T>`
selects a typed value, children inherit missing keys, and the nearest override
wins. Use `StoreScope::begin_transaction` to stage replacements or removals;
accepted readers continue to see the old topology until `accept`, and dropping
the scope transaction discards the candidate.
Direct `StoreScope::provide` returns an error while a scope transaction is
pending, so accepted topology cannot bypass that journal.

`StoreScope::observe_scoped_owner` retains the resolved store incarnation,
selected snapshot, equality policy, owner, and registration token. After an
accepted scope replacement, call `refresh_after_scope_accept`; it resolves the
nearest store again, retires the old token, registers against the new identity,
and invalidates through the supplied notification even if both stores selected
equal values. Rejected scope transactions require no refresh and preserve the
old registration.

`Store::observe_owner` associates a selection with a renderer-owned `OwnerId`.
Prepared transactions expose changed owners before finalization so the host can
build its candidate frame without prematurely publishing observer callbacks.
`TransactionCoordinator::prepare_with_host` invokes an `InvalidationSink` for
those deduplicated owners while every store can still roll back. A rejected
native owner or surface aborts the whole coordinated store set.

With `twinkle-ui`'s optional `redux` feature, `ReduxUiOwners` maps these
incarnations to concrete `UiId` owners. It bounds registrations, deduplicates
changed native owners, rejects retired owners during preparation, and supports
multiple surfaces under one coordinator boundary.

For dense native surfaces, `RetainedSurfaceHost::begin_candidate` clones the
accepted model under its explicit `Clone` rollback policy. Candidate `update`
performs all model work and admission checks without publishing. Once every
store and surface is prepared, `finalize_candidate` is a callback-free swap and
returns the displaced host so arbitrary destruction can be deferred until all
participants switched. The Redux native fixture covers one shared store, two
real retained surfaces, a late second-surface rejection, successful retry, and
bridging both accepted snapshots into ordinary `UiFrame` authority.

## Limits and threading

Stores are deliberately single-threaded and use typed `Rc` handles. Background
producers can use `action_channel` for a bounded `SyncSender`/receiver handoff;
the receiver only yields actions and the host must dispatch them inside an
admitted turn. Retiring a store releases registrations and makes retained read
and dispatch handles return `RetiredStore`. Defaults
admit 1,024 dispatches per transaction, 4,096 total observers per store, 64 MiB
per root, 128 MiB for accepted/provisional journal overlap, 50 ms cumulative
cooperative reducer/selector callback time per transaction, and 64 stores per
coordinator. Native callbacks are measured before and after invocation; a hung
in-process callback cannot be preempted, but a late result rejects the turn.
Limit failures occur before acceptance.

The complete executable example is
[`crates/twinkle-redux/examples/native_counter.rs`](../crates/twinkle-redux/examples/native_counter.rs).
The optional JavaScript/TSX adapter described by specification 0024 is separate;
the native dependency closure contains no JSX runtime, Oxc, V8, React, or npm.
Verify that boundary with `scripts/verify-redux-dependency-boundary.sh`.
Release core measurements and their limitations are recorded in
[`native-redux-performance.md`](native-redux-performance.md).

## Optional TSX adapter

Applications that deliberately embed V8 may add `twinkle-redux-jsx`. Construct
the module graph and runtime through `ReduxJsxRuntime`; it admits the exact bare
module `twinkle/redux`, installs a bounded native JSON callback before package
initialization, and coordinates its Rust-owned stores with the JSX checkpoint.
Unregistered bare imports, including `redux` and `react-redux`, remain rejected.

Actual JavaScript state never crosses that JSON callback. Rust owns the store
incarnation, opaque accepted/provisional root handle, generation, transaction,
and acceptance authority;
the adapter roots the immutable V8 graph in its checkpoint participant. The
admitted graph supports primitives, plain/null-prototype objects, arrays,
cycles/aliases, sparse holes, `undefined`, nonfinite numbers, and signed zero.
It is bounded to 16,384 objects, depth 128, and 1 MiB of string code units, then
deep-frozen. Functions, symbols/symbol keys, accessors, classes and other exotic
objects are rejected. JavaScript proxy traps are application code and remain
outside a sandbox guarantee.

The initial adapter exports `createStore`, `configureStore`, `combineReducers`,
`createAction`, return-value-only `createSlice`, `Provider`, `useStore`,
`useDispatch`, `useSelector`, and `shallowEqual`. It intentionally omits Immer
mutation recipes, middleware, thunks, RTK Query, DevTools, React, and npm
resolution. Declarations are shipped in
`crates/twinkle-redux-jsx/types/twinkle-redux.d.ts`.

The host follows the same explicit boundary: `begin_transaction`, application
dispatch/render and native candidate validation, `prepare_accept`,
`finalize_accept`, then `after_accept`. Call `abort` on any pre-finalization
failure. Public JS subscribers run only from `after_accept`; rejected work is
restored by both the native coordinator and the JSX checkpoint participant.
Prepared listener membership is snapshotted before finalization. Stores created
inside an admitted turn join the same coordinator and are removed/retired on
rollback.

Accepted passive effects are not run inside the completed transaction. If
`effect_turn_pending()` is true, call `begin_effect_turn()`, render and validate
the requested follow-up candidate, then use the same prepare/finalize/after
sequence. Effect dispatch therefore receives a fresh native/JS checkpoint and
cannot inherit mutation authority from the preceding event.

The test-only oracle in `crates/twinkle-redux-jsx/tests/oracle` pins Redux 5.0.1
and Redux Toolkit 2.9.0 with npm integrity digests. Those packages are never
installed or loaded by normal Rust builds. The strict declaration fixture is in
`crates/twinkle-redux-jsx/tests/types`.

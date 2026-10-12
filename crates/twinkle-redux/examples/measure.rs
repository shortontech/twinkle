use std::{hint::black_box, sync::Arc, time::Instant};
use twinkle_redux::{Reduction, RollbackSafe, Store};

#[derive(Clone)]
struct State {
    count: u64,
    unrelated: u64,
}
impl RollbackSafe for State {}

#[derive(Clone)]
struct SharedState {
    values: Arc<[u64]>,
    selected: usize,
}

impl RollbackSafe for SharedState {
    fn rollback_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + std::mem::size_of_val(self.values.as_ref())
    }
}

enum Action {
    Add(u64),
    Unrelated,
    Noop,
}

fn store() -> Store<State, Action, impl twinkle_redux::Reducer<State, Action, Error = &'static str>>
{
    Store::new(
        State {
            count: 0,
            unrelated: 0,
        },
        |state: &State, action: &Action| {
            Ok(match action {
                Action::Add(by) => Reduction::Changed(State {
                    count: state.count + by,
                    unrelated: state.unrelated,
                }),
                Action::Unrelated => Reduction::Changed(State {
                    count: state.count,
                    unrelated: state.unrelated + 1,
                }),
                Action::Noop => Reduction::Unchanged,
            })
        },
    )
}

fn measure(mut operation: impl FnMut()) -> (u128, u128) {
    const SAMPLES: usize = 10_000;
    let mut nanos = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        operation();
        nanos.push(start.elapsed().as_nanos());
    }
    nanos.sort_unstable();
    (nanos[SAMPLES / 2], nanos[SAMPLES * 95 / 100])
}

fn report(name: &str, result: (u128, u128)) {
    println!("{name},{},{}", result.0, result.1);
}

fn main() {
    println!("scenario,median_ns,p95_ns");
    let idle = store();
    report(
        "idle_read",
        measure(|| {
            black_box(idle.snapshot());
        }),
    );

    let noop = store();
    report(
        "noop_dispatch",
        measure(|| {
            black_box(noop.dispatch_immediate(&Action::Noop).unwrap());
        }),
    );

    let leaf = store();
    let _selection = leaf.observe_eq(|state| state.count, |_| {}).unwrap();
    report(
        "one_leaf",
        measure(|| {
            black_box(leaf.dispatch_immediate(&Action::Add(1)).unwrap());
        }),
    );

    let unrelated = store();
    let _selection = unrelated.observe_eq(|state| state.count, |_| {}).unwrap();
    report(
        "unrelated_slice",
        measure(|| {
            black_box(unrelated.dispatch_immediate(&Action::Unrelated).unwrap());
        }),
    );

    let burst = store();
    report(
        "hundred_dispatch_burst",
        measure(|| {
            let mut transaction = burst.begin_transaction().unwrap();
            for _ in 0..100 {
                transaction.dispatch(&Action::Add(1)).unwrap();
            }
            transaction.prepare_accept().unwrap();
            transaction.finalize_accept().unwrap();
            transaction.after_accept().unwrap();
        }),
    );

    let observed = store();
    let _subscriptions = (0..1_000)
        .map(|_| observed.observe_eq(|state| state.count, |_| {}).unwrap())
        .collect::<Vec<_>>();
    report(
        "thousand_selectors",
        measure(|| {
            black_box(observed.dispatch_immediate(&Action::Add(1)).unwrap());
        }),
    );

    let rollback = store();
    report(
        "rollback",
        measure(|| {
            let mut transaction = rollback.begin_transaction().unwrap();
            transaction.dispatch(&Action::Add(1)).unwrap();
            transaction.abort().unwrap();
        }),
    );

    let shared = Store::new(
        SharedState {
            values: Arc::from(vec![0_u64; 128 * 1024]),
            selected: 0,
        },
        |state: &SharedState, action: &usize| -> Result<_, &'static str> {
            Ok(Reduction::Changed(SharedState {
                values: Arc::clone(&state.values),
                selected: *action,
            }))
        },
    );
    report(
        "one_megabyte_structurally_shared_state",
        measure(|| {
            let next = shared.snapshot().selected.wrapping_add(1);
            black_box(shared.dispatch_immediate(&next).unwrap());
        }),
    );

    report(
        "store_observer_retirement",
        measure(|| {
            let retiring = store();
            let subscription = retiring.observe_eq(|state| state.count, |_| {}).unwrap();
            black_box(subscription.value);
            retiring.retire().unwrap();
            black_box(retiring.counters());
        }),
    );
}

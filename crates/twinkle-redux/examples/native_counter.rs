use std::{cell::Cell, rc::Rc};
use twinkle_redux::{OwnerId, Reduction, RollbackSafe, Store, StoreKey, StoreScope};

#[derive(Clone, Debug)]
struct CounterState {
    value: i64,
}

impl RollbackSafe for CounterState {}

#[derive(Clone, Copy, Debug)]
enum CounterAction {
    Add(i64),
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store = Store::new(
        CounterState { value: 0 },
        |state: &CounterState, action: &CounterAction| {
            let CounterAction::Add(amount) = action;
            Ok::<_, &'static str>(if *amount == 0 {
                Reduction::Unchanged
            } else {
                Reduction::Changed(CounterState {
                    value: state.value + amount,
                })
            })
        },
    );

    let selected = store.observe_eq(|state| state.value, |value| println!("value: {value}"))?;
    let dispatch = store.dispatch_handle();
    dispatch.dispatch(&CounterAction::Add(2))?;
    assert_eq!(selected.value, 0);
    assert_eq!(store.snapshot().value, 2);

    type CounterStore = Store<
        CounterState,
        CounterAction,
        fn(&CounterState, &CounterAction) -> Result<Reduction<CounterState>, &'static str>,
    >;
    const COUNTER: StoreKey<CounterStore> = StoreKey::new("counter");
    fn reducer(
        state: &CounterState,
        action: &CounterAction,
    ) -> Result<Reduction<CounterState>, &'static str> {
        let CounterAction::Add(amount) = action;
        Ok(Reduction::Changed(CounterState {
            value: state.value + amount,
        }))
    }
    let scope = StoreScope::root();
    scope
        .provide(COUNTER, Store::new(CounterState { value: 10 }, reducer))
        .unwrap();
    assert_eq!(scope.resolve(COUNTER).unwrap().snapshot().value, 10);

    let child = scope.child();
    let child_store = Store::new(
        CounterState { value: 40 },
        reducer
            as fn(&CounterState, &CounterAction) -> Result<Reduction<CounterState>, &'static str>,
    );
    child.provide(COUNTER, child_store.clone()).unwrap();
    assert_eq!(child.resolve(COUNTER).unwrap().snapshot().value, 40);
    assert_eq!(scope.resolve(COUNTER).unwrap().snapshot().value, 10);

    let invalidations = Rc::new(Cell::new(0));
    let notified = Rc::clone(&invalidations);
    let _owner_selection = child_store.observe_owner_eq(
        OwnerId::new(1),
        |state| state.value,
        move |_| notified.set(notified.get() + 1),
    )?;
    child_store
        .dispatch_handle()
        .dispatch(&CounterAction::Add(2))?;
    assert_eq!(invalidations.get(), 1);
    assert_eq!(child_store.snapshot().value, 42);
    Ok(())
}

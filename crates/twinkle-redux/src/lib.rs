//! Native, transactional Redux-style state management for Twinkle applications.
//!
//! This crate deliberately has no dependency on JSX, JavaScript, V8, React, or
//! a particular UI renderer. Stores use immutable replacement: reducers borrow
//! the current state and return either a new root or [`Reduction::Unchanged`].
//! Public observers run only after the transaction has been accepted.

use std::{
    any::{Any, TypeId},
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap},
    fmt,
    marker::PhantomData,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    rc::{Rc, Weak},
    sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel},
    time::Instant,
};

const DEFAULT_MAX_DISPATCHES: usize = 1_024;
const DEFAULT_MAX_OBSERVERS: usize = 4_096;
const DEFAULT_MAX_STATE_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_MAX_JOURNAL_BYTES: usize = 128 * 1024 * 1024;
const DEFAULT_MAX_CALLBACK_MICROS: u64 = 50_000;
const MAX_COORDINATED_STORES: usize = 64;

/// Marks state whose clone can safely preserve an accepted transaction root.
///
/// Implementations are intentionally explicit. Types with interior mutability
/// must not implement this trait: mutating a clone through a shared cell would
/// also mutate the accepted root and make rollback impossible.
pub trait RollbackSafe: Clone + 'static {
    /// Account retained bytes used by one immutable root. Implementors with
    /// heap-owned data must override the conservative inline-size default.
    fn rollback_bytes(&self) -> usize {
        std::mem::size_of_val(self)
    }
}

macro_rules! rollback_safe_scalars {
    ($($ty:ty),* $(,)?) => {$(
        impl RollbackSafe for $ty {}
    )*};
}

rollback_safe_scalars!(
    (),
    bool,
    char,
    i8,
    i16,
    i32,
    i64,
    i128,
    isize,
    u8,
    u16,
    u32,
    u64,
    u128,
    usize,
    f32,
    f64
);

impl RollbackSafe for String {
    fn rollback_bytes(&self) -> usize {
        std::mem::size_of::<Self>().saturating_add(self.capacity())
    }
}

impl<T: RollbackSafe> RollbackSafe for Option<T> {
    fn rollback_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.as_ref().map_or(0, RollbackSafe::rollback_bytes))
    }
}
impl<T: RollbackSafe, E: RollbackSafe> RollbackSafe for Result<T, E> {
    fn rollback_bytes(&self) -> usize {
        std::mem::size_of::<Self>().saturating_add(match self {
            Ok(value) => value.rollback_bytes(),
            Err(error) => error.rollback_bytes(),
        })
    }
}
impl<T: RollbackSafe> RollbackSafe for Vec<T> {
    fn rollback_bytes(&self) -> usize {
        let elements = self
            .iter()
            .map(RollbackSafe::rollback_bytes)
            .fold(0, usize::saturating_add);
        let spare = self
            .capacity()
            .saturating_sub(self.len())
            .saturating_mul(std::mem::size_of::<T>());
        std::mem::size_of::<Self>()
            .saturating_add(elements)
            .saturating_add(spare)
    }
}
impl<T: RollbackSafe, const N: usize> RollbackSafe for [T; N] {
    fn rollback_bytes(&self) -> usize {
        self.iter()
            .map(RollbackSafe::rollback_bytes)
            .fold(0, usize::saturating_add)
    }
}
impl<K: RollbackSafe + Ord, V: RollbackSafe> RollbackSafe for BTreeMap<K, V> {
    fn rollback_bytes(&self) -> usize {
        let node_overhead = self
            .len()
            .saturating_mul(4_usize.saturating_mul(std::mem::size_of::<usize>()));
        std::mem::size_of::<Self>().saturating_add(self.iter().fold(
            node_overhead,
            |bytes, (key, value)| {
                bytes
                    .saturating_add(key.rollback_bytes())
                    .saturating_add(value.rollback_bytes())
            },
        ))
    }
}

/// A reducer's explicit state result.
#[derive(Clone, Debug, PartialEq)]
pub enum Reduction<S> {
    /// Preserve root identity and avoid selector work.
    Unchanged,
    /// Replace the root with a rollback-safe value.
    Changed(S),
}

/// Pure state transition used by a [`Store`].
pub trait Reducer<S, A>: 'static {
    type Error;

    fn reduce(&mut self, state: &S, action: &A) -> Result<Reduction<S>, Self::Error>;
}

impl<S, A, E, F> Reducer<S, A> for F
where
    F: FnMut(&S, &A) -> Result<Reduction<S>, E> + 'static,
{
    type Error = E;

    fn reduce(&mut self, state: &S, action: &A) -> Result<Reduction<S>, E> {
        self(state, action)
    }
}

/// Stable identity for a store incarnation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StoreId(u64);

/// Stable identity for a public or selector observer.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SubscriptionId(u64);

/// Renderer-owned identity invalidated by a changed selection.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OwnerId(u64);

impl OwnerId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Accepted state generation. Unchanged transactions preserve it.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Generation(u64);

impl Generation {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Finite store limits applied across one transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoreLimits {
    pub max_dispatches_per_transaction: usize,
    pub max_observers: usize,
    pub max_state_bytes: usize,
    pub max_journal_bytes: usize,
    pub max_callback_micros: u64,
}

impl Default for StoreLimits {
    fn default() -> Self {
        Self {
            max_dispatches_per_transaction: DEFAULT_MAX_DISPATCHES,
            max_observers: DEFAULT_MAX_OBSERVERS,
            max_state_bytes: DEFAULT_MAX_STATE_BYTES,
            max_journal_bytes: DEFAULT_MAX_JOURNAL_BYTES,
            max_callback_micros: DEFAULT_MAX_CALLBACK_MICROS,
        }
    }
}

/// Observable accounting that does not require a JSX inspector.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StoreCounters {
    pub attempted_dispatches: u64,
    pub accepted_dispatches: u64,
    pub rejected_transactions: u64,
    pub accepted_transactions: u64,
    pub state_generations: u64,
    pub selector_checks: u64,
    pub selector_changes: u64,
    pub observer_deliveries: u64,
}

/// Store operation failure.
#[derive(Debug, PartialEq)]
pub enum StoreError<E> {
    RetiredStore { store: StoreId },
    TransactionAlreadyPending,
    TransactionUnavailable,
    CoordinatorRequired,
    WrongPhase(&'static str),
    DispatchLimit { limit: usize },
    GenerationExhausted,
    ObserverLimit { limit: usize },
    StateLimit { limit: usize },
    JournalLimit { limit: usize },
    CallbackBudget { limit_micros: u64 },
    ObserverPanicked { count: usize },
    DeferredDropPanicked { count: usize },
    Reducer(E),
    Selector(String),
}

impl<E: fmt::Display> fmt::Display for StoreError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RetiredStore { store } => write!(formatter, "store {:?} is retired", store),
            Self::TransactionAlreadyPending => {
                formatter.write_str("store transaction already pending")
            }
            Self::TransactionUnavailable => formatter.write_str("store transaction is unavailable"),
            Self::CoordinatorRequired => {
                formatter.write_str("UI-bound store dispatch requires an active host transaction")
            }
            Self::WrongPhase(phase) => {
                write!(formatter, "store operation is forbidden during {phase}")
            }
            Self::DispatchLimit { limit } => {
                write!(formatter, "store transaction exceeds {limit} dispatches")
            }
            Self::GenerationExhausted => formatter.write_str("store generation is exhausted"),
            Self::ObserverLimit { limit } => write!(formatter, "store exceeds {limit} observers"),
            Self::StateLimit { limit } => write!(formatter, "store state exceeds {limit} bytes"),
            Self::JournalLimit { limit } => {
                write!(formatter, "store journal exceeds {limit} bytes")
            }
            Self::CallbackBudget { limit_micros } => write!(
                formatter,
                "store callback work exceeds {limit_micros} microseconds"
            ),
            Self::ObserverPanicked { count } => {
                write!(formatter, "{count} committed store observers panicked")
            }
            Self::DeferredDropPanicked { count } => {
                write!(
                    formatter,
                    "{count} post-acceptance values panicked while dropping"
                )
            }
            Self::Reducer(error) => write!(formatter, "store reducer failed: {error}"),
            Self::Selector(error) => write!(formatter, "store selector failed: {error}"),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for StoreError<E> {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Idle,
    Mutating,
    Prepared,
    Finalized,
    Notifying,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CallbackPhase {
    Idle,
    Reduction,
    Selection,
    Notification,
}

thread_local! {
    static CALLBACK_PHASE: Cell<CallbackPhase> = const { Cell::new(CallbackPhase::Idle) };
}

struct CallbackGuard(CallbackPhase);

impl CallbackGuard {
    fn enter(phase: CallbackPhase) -> Self {
        let previous = CALLBACK_PHASE.with(|current| current.replace(phase));
        Self(previous)
    }
}

impl Drop for CallbackGuard {
    fn drop(&mut self) {
        CALLBACK_PHASE.with(|current| current.set(self.0));
    }
}

fn reject_callback_operation<E>(allow_notification: bool) -> Result<(), StoreError<E>> {
    CALLBACK_PHASE.with(|phase| match phase.get() {
        CallbackPhase::Idle => Ok(()),
        CallbackPhase::Reduction => Err(StoreError::WrongPhase("reduction callback")),
        CallbackPhase::Selection => Err(StoreError::WrongPhase("selector/equality callback")),
        CallbackPhase::Notification if allow_notification => Ok(()),
        CallbackPhase::Notification => Err(StoreError::WrongPhase("observer delivery")),
    })
}

/// Determines who owns the acceptance boundary for a dispatch.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StoreMode {
    /// Each idle dispatch creates and accepts one complete transaction.
    #[default]
    Immediate,
    /// Dispatch is legal only while an embedding host transaction is active.
    Coordinated,
}

trait ErasedSelector<S> {
    fn prepare(&mut self, state: &S) -> Result<bool, String>;
    fn accept(&mut self);
    fn take_retired(&mut self) -> Option<Box<dyn Any>>;
    fn abort(&mut self);
    fn notify(&mut self);
    fn changed_owner(&self) -> Option<OwnerId>;
}

type PublicObserver<S> = Box<dyn FnMut(&S)>;
type PublicObservers<S> = BTreeMap<SubscriptionId, PublicObserver<S>>;

struct Selector<S, T, Select, Equal, Notify> {
    accepted: T,
    pending: Option<T>,
    retired: Option<T>,
    changed: bool,
    select: Select,
    equal: Equal,
    notify: Notify,
    owner: Option<OwnerId>,
    _state: PhantomData<fn(&S)>,
}

impl<S, T, Select, Equal, Notify> ErasedSelector<S> for Selector<S, T, Select, Equal, Notify>
where
    T: 'static,
    Select: FnMut(&S) -> Result<T, String> + 'static,
    Equal: FnMut(&T, &T) -> bool + 'static,
    Notify: FnMut(&T) + 'static,
{
    fn prepare(&mut self, state: &S) -> Result<bool, String> {
        let next = (self.select)(state)?;
        self.changed = !(self.equal)(&self.accepted, &next);
        self.pending = Some(next);
        Ok(self.changed)
    }

    fn accept(&mut self) {
        if let Some(next) = self.pending.take() {
            self.retired = Some(std::mem::replace(&mut self.accepted, next));
        }
    }

    fn take_retired(&mut self) -> Option<Box<dyn Any>> {
        self.retired
            .take()
            .map(|value| Box::new(value) as Box<dyn Any>)
    }

    fn abort(&mut self) {
        self.pending = None;
        self.changed = false;
    }

    fn notify(&mut self) {
        if self.changed {
            self.changed = false;
            (self.notify)(&self.accepted);
        }
    }

    fn changed_owner(&self) -> Option<OwnerId> {
        self.changed.then_some(self.owner).flatten()
    }
}

struct TransactionState<S, R> {
    provisional: Option<S>,
    dispatches: usize,
    changed: bool,
    original_reducer: Option<R>,
    new_public: Vec<SubscriptionId>,
    new_selectors: Vec<SubscriptionId>,
    removed_public: PublicObservers<S>,
    removed_selectors: BTreeMap<SubscriptionId, Box<dyn ErasedSelector<S>>>,
    callback_micros: u128,
}

struct Inner<S, A, R>
where
    R: Reducer<S, A>,
{
    id: StoreId,
    retired: bool,
    reducer: R,
    accepted: S,
    generation: Generation,
    phase: Phase,
    transaction: Option<TransactionState<S, R>>,
    public_observers: PublicObservers<S>,
    selectors: BTreeMap<SubscriptionId, Box<dyn ErasedSelector<S>>>,
    subscription_activity: BTreeMap<SubscriptionId, Weak<Cell<bool>>>,
    next_subscription: u64,
    limits: StoreLimits,
    counters: StoreCounters,
    mode: StoreMode,
    notify_public: bool,
    notifying_observers: usize,
    _action: PhantomData<fn(A)>,
}

thread_local! {
    static NEXT_STORE_ID: Cell<u64> = const { Cell::new(1) };
}

fn next_store_id() -> StoreId {
    NEXT_STORE_ID.with(|next| {
        let id = next.get();
        next.set(id.checked_add(1).expect("store identity exhausted"));
        StoreId(id)
    })
}

/// A single-threaded immutable-root store.
pub struct Store<S, A, R>
where
    R: Reducer<S, A>,
{
    inner: Rc<RefCell<Inner<S, A, R>>>,
}

impl<S, A, R> Clone for Store<S, A, R>
where
    R: Reducer<S, A>,
{
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
        }
    }
}

impl<S, A, R> Store<S, A, R>
where
    S: RollbackSafe,
    A: 'static,
    R: Reducer<S, A>,
{
    pub fn new(initial: S, reducer: R) -> Self {
        Self::with_options(
            initial,
            reducer,
            StoreMode::Immediate,
            StoreLimits::default(),
        )
    }

    /// Create a store whose dispatches must join a host-owned transaction.
    pub fn coordinated(initial: S, reducer: R) -> Self {
        Self::with_options(
            initial,
            reducer,
            StoreMode::Coordinated,
            StoreLimits::default(),
        )
    }

    pub fn with_limits(initial: S, reducer: R, limits: StoreLimits) -> Self {
        Self::with_options(initial, reducer, StoreMode::Immediate, limits)
    }

    pub fn with_options(initial: S, reducer: R, mode: StoreMode, limits: StoreLimits) -> Self {
        Self {
            inner: Rc::new(RefCell::new(Inner {
                id: next_store_id(),
                retired: false,
                reducer,
                accepted: initial,
                generation: Generation::default(),
                phase: Phase::Idle,
                transaction: None,
                public_observers: BTreeMap::new(),
                selectors: BTreeMap::new(),
                subscription_activity: BTreeMap::new(),
                next_subscription: 1,
                limits,
                counters: StoreCounters::default(),
                mode,
                notify_public: false,
                notifying_observers: 0,
                _action: PhantomData,
            })),
        }
    }

    pub fn id(&self) -> StoreId {
        self.inner.borrow().id
    }

    pub fn generation(&self) -> Generation {
        self.inner.borrow().generation
    }

    pub fn snapshot(&self) -> S {
        let inner = self.inner.borrow();
        inner.transaction.as_ref().map_or_else(
            || inner.accepted.clone(),
            |transaction| transaction.provisional.as_ref().unwrap().clone(),
        )
    }

    pub fn is_retired(&self) -> bool {
        self.inner.borrow().retired
    }

    /// Retire this incarnation and every observer. Existing typed handles keep
    /// their identity but reject later reads and dispatches.
    pub fn retire(&self) -> Result<(), StoreError<R::Error>> {
        let mut inner = self.inner.borrow_mut();
        if inner.phase != Phase::Idle {
            return Err(StoreError::WrongPhase("store retirement"));
        }
        inner.retired = true;
        inner.public_observers.clear();
        inner.selectors.clear();
        for activity in inner
            .subscription_activity
            .values()
            .filter_map(Weak::upgrade)
        {
            activity.set(false);
        }
        inner.subscription_activity.clear();
        Ok(())
    }

    pub fn accepted_snapshot(&self) -> S {
        self.inner.borrow().accepted.clone()
    }

    pub fn counters(&self) -> StoreCounters {
        self.inner.borrow().counters
    }

    pub fn handle(&self) -> StoreHandle<S, A, R> {
        StoreHandle {
            store: self.clone(),
        }
    }

    pub fn dispatch_handle(&self) -> DispatchHandle<S, A, R> {
        DispatchHandle {
            store: self.clone(),
        }
    }

    pub fn begin_transaction(&self) -> Result<StoreTransaction<S, A, R>, StoreError<R::Error>> {
        reject_callback_operation(false)?;
        {
            let mut inner = self.inner.borrow_mut();
            if inner.retired {
                return Err(StoreError::RetiredStore { store: inner.id });
            }
            if inner.phase != Phase::Idle {
                return Err(StoreError::TransactionAlreadyPending);
            }
            let state_bytes = inner.accepted.rollback_bytes();
            if state_bytes > inner.limits.max_state_bytes {
                return Err(StoreError::StateLimit {
                    limit: inner.limits.max_state_bytes,
                });
            }
            if state_bytes.saturating_mul(2) > inner.limits.max_journal_bytes {
                return Err(StoreError::JournalLimit {
                    limit: inner.limits.max_journal_bytes,
                });
            }
            inner.phase = Phase::Mutating;
            inner.transaction = Some(TransactionState {
                provisional: Some(inner.accepted.clone()),
                dispatches: 0,
                changed: false,
                original_reducer: None,
                new_public: Vec::new(),
                new_selectors: Vec::new(),
                removed_public: BTreeMap::new(),
                removed_selectors: BTreeMap::new(),
                callback_micros: 0,
            });
        }
        Ok(StoreTransaction {
            store: self.clone(),
            live: true,
            retired_state: None,
            retired_transaction: None,
        })
    }

    pub fn dispatch_immediate(&self, action: &A) -> Result<DispatchResult, StoreError<R::Error>> {
        if self.inner.borrow().mode == StoreMode::Coordinated {
            return Err(StoreError::CoordinatorRequired);
        }
        let mut transaction = self.begin_transaction()?;
        let result = transaction.dispatch(action)?;
        transaction.prepare_accept()?;
        transaction.finalize_accept()?;
        transaction.after_accept()?;
        Ok(result)
    }

    pub fn subscribe(
        &self,
        observer: impl FnMut(&S) + 'static,
    ) -> Result<Subscription<S, A, R>, StoreError<R::Error>> {
        reject_callback_operation(true)?;
        let mut inner = self.inner.borrow_mut();
        if inner.retired {
            return Err(StoreError::RetiredStore { store: inner.id });
        }
        Self::ensure_registration_phase(&inner)?;
        if inner.public_observers.len() + inner.selectors.len() + inner.notifying_observers
            >= inner.limits.max_observers
        {
            return Err(StoreError::ObserverLimit {
                limit: inner.limits.max_observers,
            });
        }
        let id = inner.allocate_subscription();
        let activity = Rc::new(Cell::new(true));
        inner.public_observers.insert(id, Box::new(observer));
        inner
            .subscription_activity
            .insert(id, Rc::downgrade(&activity));
        if let Some(transaction) = &mut inner.transaction {
            transaction.new_public.push(id);
        }
        Ok(Subscription::new(
            self,
            id,
            SubscriptionKind::Public,
            activity,
        ))
    }

    pub fn observe<T>(
        &self,
        mut selector: impl FnMut(&S) -> T + 'static,
        equality: impl FnMut(&T, &T) -> bool + 'static,
        notify: impl FnMut(&T) + 'static,
    ) -> Result<Selection<S, A, R, T>, StoreError<R::Error>>
    where
        T: Clone + 'static,
    {
        reject_callback_operation(true)?;
        let accepted = {
            let _guard = CallbackGuard::enter(CallbackPhase::Selection);
            selector(&self.inner.borrow().accepted)
        };
        self.observe_fallible(
            move |state| Ok(selector(state)),
            equality,
            notify,
            accepted,
            None,
        )
    }

    /// Observe a selected value on behalf of a native view owner.
    ///
    /// The owner appears in [`StoreTransaction::changed_owners`] after prepare
    /// and before native candidate admission. Rejection clears it.
    pub fn observe_owner<T>(
        &self,
        owner: OwnerId,
        mut selector: impl FnMut(&S) -> T + 'static,
        equality: impl FnMut(&T, &T) -> bool + 'static,
        notify: impl FnMut(&T) + 'static,
    ) -> Result<Selection<S, A, R, T>, StoreError<R::Error>>
    where
        T: Clone + 'static,
    {
        reject_callback_operation(true)?;
        let accepted = {
            let _guard = CallbackGuard::enter(CallbackPhase::Selection);
            selector(&self.inner.borrow().accepted)
        };
        self.observe_fallible(
            move |state| Ok(selector(state)),
            equality,
            notify,
            accepted,
            Some(owner),
        )
    }

    fn observe_fallible<T, Select, Equal, Notify>(
        &self,
        selector: Select,
        equality: Equal,
        notify: Notify,
        accepted: T,
        owner: Option<OwnerId>,
    ) -> Result<Selection<S, A, R, T>, StoreError<R::Error>>
    where
        T: Clone + 'static,
        Select: FnMut(&S) -> Result<T, String> + 'static,
        Equal: FnMut(&T, &T) -> bool + 'static,
        Notify: FnMut(&T) + 'static,
    {
        let selected = accepted.clone();
        let mut inner = self.inner.borrow_mut();
        if inner.retired {
            return Err(StoreError::RetiredStore { store: inner.id });
        }
        Self::ensure_registration_phase(&inner)?;
        if inner.public_observers.len() + inner.selectors.len() + inner.notifying_observers
            >= inner.limits.max_observers
        {
            return Err(StoreError::ObserverLimit {
                limit: inner.limits.max_observers,
            });
        }
        let id = inner.allocate_subscription();
        let activity = Rc::new(Cell::new(true));
        inner.selectors.insert(
            id,
            Box::new(Selector {
                accepted,
                pending: None,
                retired: None,
                changed: false,
                select: selector,
                equal: equality,
                notify,
                owner,
                _state: PhantomData,
            }),
        );
        inner
            .subscription_activity
            .insert(id, Rc::downgrade(&activity));
        if let Some(transaction) = &mut inner.transaction {
            transaction.new_selectors.push(id);
        }
        Ok(Selection {
            value: selected,
            subscription: Subscription::new(self, id, SubscriptionKind::Selector, activity),
        })
    }

    pub fn observe_eq<T>(
        &self,
        selector: impl FnMut(&S) -> T + 'static,
        notify: impl FnMut(&T) + 'static,
    ) -> Result<Selection<S, A, R, T>, StoreError<R::Error>>
    where
        T: Clone + PartialEq + 'static,
    {
        self.observe(selector, PartialEq::eq, notify)
    }

    pub fn observe_owner_eq<T>(
        &self,
        owner: OwnerId,
        selector: impl FnMut(&S) -> T + 'static,
        notify: impl FnMut(&T) + 'static,
    ) -> Result<Selection<S, A, R, T>, StoreError<R::Error>>
    where
        T: Clone + PartialEq + 'static,
    {
        self.observe_owner(owner, selector, PartialEq::eq, notify)
    }

    fn ensure_registration_phase(inner: &Inner<S, A, R>) -> Result<(), StoreError<R::Error>> {
        match inner.phase {
            Phase::Idle | Phase::Mutating => Ok(()),
            Phase::Prepared => Err(StoreError::WrongPhase("prepare acceptance")),
            Phase::Finalized => Err(StoreError::WrongPhase("finalize acceptance")),
            Phase::Notifying => Ok(()),
        }
    }

    fn dispatch_provisional(&self, action: &A) -> Result<DispatchResult, StoreError<R::Error>> {
        let mut inner = self.inner.borrow_mut();
        if inner.phase != Phase::Mutating {
            return Err(StoreError::TransactionUnavailable);
        }
        let dispatch_limit = inner.limits.max_dispatches_per_transaction;
        let transaction = inner
            .transaction
            .as_mut()
            .ok_or(StoreError::TransactionUnavailable)?;
        if transaction.dispatches >= dispatch_limit {
            return Err(StoreError::DispatchLimit {
                limit: dispatch_limit,
            });
        }
        transaction.dispatches += 1;
        inner.counters.attempted_dispatches = inner.counters.attempted_dispatches.saturating_add(1);

        let mut transaction = inner
            .transaction
            .take()
            .ok_or(StoreError::TransactionUnavailable)?;
        let callback_started = Instant::now();
        let reduced = catch_unwind(AssertUnwindSafe(|| {
            let _guard = CallbackGuard::enter(CallbackPhase::Reduction);
            inner
                .reducer
                .reduce(transaction.provisional.as_ref().unwrap(), action)
        }));
        transaction.callback_micros = transaction
            .callback_micros
            .saturating_add(callback_started.elapsed().as_micros());
        if transaction.callback_micros > u128::from(inner.limits.max_callback_micros) {
            inner.transaction = Some(transaction);
            return Err(StoreError::CallbackBudget {
                limit_micros: inner.limits.max_callback_micros,
            });
        }
        let reduction = match reduced {
            Ok(Ok(reduction)) => reduction,
            Ok(Err(error)) => {
                inner.transaction = Some(transaction);
                return Err(StoreError::Reducer(error));
            }
            Err(payload) => {
                inner.transaction = Some(transaction);
                drop(inner);
                resume_unwind(payload);
            }
        };
        let result = match reduction {
            Reduction::Unchanged => DispatchResult::Unchanged,
            Reduction::Changed(next) => {
                let state_bytes = next.rollback_bytes();
                if state_bytes > inner.limits.max_state_bytes {
                    inner.transaction = Some(transaction);
                    return Err(StoreError::StateLimit {
                        limit: inner.limits.max_state_bytes,
                    });
                }
                let accepted_bytes = inner.accepted.rollback_bytes();
                if state_bytes.saturating_add(accepted_bytes) > inner.limits.max_journal_bytes {
                    inner.transaction = Some(transaction);
                    return Err(StoreError::JournalLimit {
                        limit: inner.limits.max_journal_bytes,
                    });
                }
                transaction.provisional = Some(next);
                transaction.changed = true;
                DispatchResult::Changed
            }
        };
        inner.transaction = Some(transaction);
        Ok(result)
    }

    fn prepared_changed_owners(&self) -> Result<Vec<OwnerId>, StoreError<R::Error>> {
        let inner = self.inner.borrow();
        if inner.phase != Phase::Prepared {
            return Err(StoreError::WrongPhase("owner invalidation query"));
        }
        let mut owners = inner
            .selectors
            .values()
            .filter_map(|selector| selector.changed_owner())
            .collect::<Vec<_>>();
        owners.sort_unstable();
        owners.dedup();
        Ok(owners)
    }
}

impl<S, A, R> Inner<S, A, R>
where
    R: Reducer<S, A>,
{
    fn allocate_subscription(&mut self) -> SubscriptionId {
        let id = SubscriptionId(self.next_subscription);
        self.next_subscription = self
            .next_subscription
            .checked_add(1)
            .expect("subscription identity exhausted");
        id
    }

    fn unsubscribe(&mut self, id: SubscriptionId, kind: SubscriptionKind) -> bool {
        match (&mut self.transaction, kind) {
            (Some(transaction), SubscriptionKind::Public) => {
                if let Some(index) = transaction
                    .new_public
                    .iter()
                    .position(|candidate| *candidate == id)
                {
                    transaction.new_public.swap_remove(index);
                    self.subscription_activity.remove(&id);
                    return self.public_observers.remove(&id).is_some();
                }
                let Some(observer) = self.public_observers.remove(&id) else {
                    return false;
                };
                transaction.removed_public.insert(id, observer);
                true
            }
            (Some(transaction), SubscriptionKind::Selector) => {
                if let Some(index) = transaction
                    .new_selectors
                    .iter()
                    .position(|candidate| *candidate == id)
                {
                    transaction.new_selectors.swap_remove(index);
                    self.subscription_activity.remove(&id);
                    return self.selectors.remove(&id).is_some();
                }
                let Some(observer) = self.selectors.remove(&id) else {
                    return false;
                };
                transaction.removed_selectors.insert(id, observer);
                true
            }
            (None, SubscriptionKind::Public) => {
                self.subscription_activity.remove(&id);
                self.public_observers.remove(&id).is_some()
            }
            (None, SubscriptionKind::Selector) => {
                self.subscription_activity.remove(&id);
                self.selectors.remove(&id).is_some()
            }
        }
    }

    fn rollback_transaction(&mut self) {
        let Some(mut transaction) = self.transaction.take() else {
            return;
        };
        if let Some(reducer) = transaction.original_reducer.take() {
            self.reducer = reducer;
        }
        for id in transaction.new_public {
            self.public_observers.remove(&id);
            if let Some(activity) = self
                .subscription_activity
                .remove(&id)
                .and_then(|weak| weak.upgrade())
            {
                activity.set(false);
            }
        }
        for id in transaction.new_selectors {
            self.selectors.remove(&id);
            if let Some(activity) = self
                .subscription_activity
                .remove(&id)
                .and_then(|weak| weak.upgrade())
            {
                activity.set(false);
            }
        }
        let restored = transaction
            .removed_public
            .keys()
            .chain(transaction.removed_selectors.keys())
            .copied()
            .collect::<Vec<_>>();
        self.public_observers
            .append(&mut transaction.removed_public);
        self.selectors.append(&mut transaction.removed_selectors);
        for id in restored {
            if let Some(activity) = self.subscription_activity.get(&id).and_then(Weak::upgrade) {
                activity.set(true);
            } else {
                self.public_observers.remove(&id);
                self.selectors.remove(&id);
                self.subscription_activity.remove(&id);
            }
        }
    }
}

/// Stable typed read handle, analogous to React-Redux store access.
#[derive(Clone)]
pub struct StoreHandle<S, A, R>
where
    R: Reducer<S, A>,
{
    store: Store<S, A, R>,
}

impl<S, A, R> StoreHandle<S, A, R>
where
    S: RollbackSafe,
    A: 'static,
    R: Reducer<S, A>,
{
    pub fn id(&self) -> StoreId {
        self.store.id()
    }

    pub fn generation(&self) -> Generation {
        self.store.generation()
    }

    pub fn snapshot(&self) -> Result<S, StoreError<R::Error>> {
        reject_callback_operation(true)?;
        let inner = self.store.inner.borrow();
        if inner.retired {
            return Err(StoreError::RetiredStore { store: inner.id });
        }
        Ok(inner.transaction.as_ref().map_or_else(
            || inner.accepted.clone(),
            |transaction| transaction.provisional.as_ref().unwrap().clone(),
        ))
    }

    /// Owners whose accepted selections would change if this transaction is
    /// finalized. Available only after successful preparation.
    pub fn changed_owners(&self) -> Result<Vec<OwnerId>, StoreError<R::Error>> {
        self.store.prepared_changed_owners()
    }

    pub fn accepted_snapshot(&self) -> Result<S, StoreError<R::Error>> {
        let inner = self.store.inner.borrow();
        if inner.retired {
            return Err(StoreError::RetiredStore { store: inner.id });
        }
        Ok(inner.accepted.clone())
    }
}

/// Stable typed dispatch handle, analogous to React-Redux dispatch access.
#[derive(Clone)]
pub struct DispatchHandle<S, A, R>
where
    R: Reducer<S, A>,
{
    store: Store<S, A, R>,
}

impl<S, A, R> DispatchHandle<S, A, R>
where
    S: RollbackSafe,
    A: 'static,
    R: Reducer<S, A>,
{
    /// Dispatch immediately for a standalone store, or join an already active
    /// host transaction for a coordinated store.
    pub fn dispatch(&self, action: &A) -> Result<DispatchResult, StoreError<R::Error>> {
        reject_callback_operation(false)?;
        let inner = self.store.inner.borrow();
        if inner.retired {
            return Err(StoreError::RetiredStore { store: inner.id });
        }
        let phase = inner.phase;
        drop(inner);
        match phase {
            Phase::Idle => self.store.dispatch_immediate(action),
            Phase::Mutating => self.store.dispatch_provisional(action),
            Phase::Prepared => Err(StoreError::WrongPhase("prepare acceptance")),
            Phase::Finalized => Err(StoreError::WrongPhase("finalize acceptance")),
            Phase::Notifying => Err(StoreError::WrongPhase("observer delivery")),
        }
    }
}

/// Producer half of a bounded background-to-host action handoff.
pub struct ActionSender<A>(SyncSender<A>);

impl<A> Clone for ActionSender<A> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<A> ActionSender<A> {
    pub fn try_send(&self, action: A) -> Result<(), TrySendError<A>> {
        self.0.try_send(action)
    }
}

/// Host half of a bounded action handoff. Receiving never dispatches by itself;
/// the UI host must drain it inside an admitted transaction.
pub struct ActionReceiver<A>(Receiver<A>);

impl<A> ActionReceiver<A> {
    pub fn try_recv(&self) -> Result<A, TryRecvError> {
        self.0.try_recv()
    }
}

pub fn action_channel<A>(capacity: usize) -> (ActionSender<A>, ActionReceiver<A>) {
    let (sender, receiver) = sync_channel(capacity);
    (ActionSender(sender), ActionReceiver(receiver))
}

/// Whether one dispatch changed the provisional root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchResult {
    Unchanged,
    Changed,
}

/// Explicit store transaction. Dropping it before acceptance aborts it.
pub struct StoreTransaction<S, A, R>
where
    R: Reducer<S, A>,
{
    store: Store<S, A, R>,
    live: bool,
    retired_state: Option<S>,
    retired_transaction: Option<TransactionState<S, R>>,
}

impl<S, A, R> StoreTransaction<S, A, R>
where
    S: RollbackSafe,
    A: 'static,
    R: Reducer<S, A>,
{
    pub fn dispatch(&mut self, action: &A) -> Result<DispatchResult, StoreError<R::Error>> {
        self.store.dispatch_provisional(action)
    }

    /// Replace the reducer provisionally. Rejection restores the reducer that
    /// was active when this transaction began.
    pub fn replace_reducer(&mut self, reducer: R) -> Result<(), StoreError<R::Error>> {
        let mut inner = self.store.inner.borrow_mut();
        if inner.phase != Phase::Mutating {
            return Err(StoreError::WrongPhase("reducer replacement"));
        }
        let previous = std::mem::replace(&mut inner.reducer, reducer);
        let transaction = inner
            .transaction
            .as_mut()
            .ok_or(StoreError::TransactionUnavailable)?;
        if transaction.original_reducer.is_none() {
            transaction.original_reducer = Some(previous);
        }
        Ok(())
    }

    pub fn snapshot(&self) -> S {
        self.store.snapshot()
    }

    pub fn changed_owners(&self) -> Result<Vec<OwnerId>, StoreError<R::Error>> {
        self.store.prepared_changed_owners()
    }

    pub fn prepare_accept(&mut self) -> Result<(), StoreError<R::Error>> {
        let mut inner = self.store.inner.borrow_mut();
        if inner.phase != Phase::Mutating {
            return Err(StoreError::WrongPhase("prepare acceptance"));
        }
        let transaction = inner
            .transaction
            .as_ref()
            .ok_or(StoreError::TransactionUnavailable)?;
        let changed = transaction.changed;
        let prior_callback_micros = transaction.callback_micros;
        if changed && inner.generation.0 == u64::MAX {
            return Err(StoreError::GenerationExhausted);
        }
        let provisional = transaction.provisional.as_ref().unwrap().clone();
        let mut checks = 0_u64;
        let mut changes = 0_u64;
        let callback_started = Instant::now();
        if changed {
            let _guard = CallbackGuard::enter(CallbackPhase::Selection);
            for selector in inner.selectors.values_mut() {
                checks = checks.saturating_add(1);
                if selector
                    .prepare(&provisional)
                    .map_err(StoreError::Selector)?
                {
                    changes = changes.saturating_add(1);
                }
            }
        }
        let elapsed = callback_started.elapsed().as_micros();
        if prior_callback_micros.saturating_add(elapsed)
            > u128::from(inner.limits.max_callback_micros)
        {
            for selector in inner.selectors.values_mut() {
                selector.abort();
            }
            return Err(StoreError::CallbackBudget {
                limit_micros: inner.limits.max_callback_micros,
            });
        }
        inner.transaction.as_mut().unwrap().callback_micros =
            prior_callback_micros.saturating_add(elapsed);
        inner.counters.selector_checks = inner.counters.selector_checks.saturating_add(checks);
        inner.counters.selector_changes = inner.counters.selector_changes.saturating_add(changes);
        inner.phase = Phase::Prepared;
        Ok(())
    }

    /// Irrevocably switch the accepted state without invoking user callbacks.
    pub fn finalize_accept(&mut self) -> Result<(), StoreError<R::Error>> {
        let mut inner = self.store.inner.borrow_mut();
        if inner.phase != Phase::Prepared {
            return Err(StoreError::WrongPhase("finalize acceptance"));
        }
        let mut transaction = inner
            .transaction
            .take()
            .ok_or(StoreError::TransactionUnavailable)?;
        for id in transaction
            .removed_public
            .keys()
            .chain(transaction.removed_selectors.keys())
        {
            inner.subscription_activity.remove(id);
        }
        if transaction.changed {
            let next = transaction
                .provisional
                .take()
                .ok_or(StoreError::TransactionUnavailable)?;
            self.retired_state = Some(std::mem::replace(&mut inner.accepted, next));
            inner.generation.0 += 1;
            inner.counters.state_generations = inner.counters.state_generations.saturating_add(1);
        }
        inner.counters.accepted_dispatches = inner
            .counters
            .accepted_dispatches
            .saturating_add(transaction.dispatches as u64);
        inner.counters.accepted_transactions =
            inner.counters.accepted_transactions.saturating_add(1);
        inner.notify_public = transaction.dispatches > 0;
        for selector in inner.selectors.values_mut() {
            selector.accept();
        }
        self.retired_transaction = Some(transaction);
        inner.phase = Phase::Finalized;
        Ok(())
    }

    /// Deliver committed selector and public observer notifications.
    pub fn after_accept(&mut self) -> Result<(), StoreError<R::Error>> {
        let (selectors, public, snapshot) = {
            let mut inner = self.store.inner.borrow_mut();
            if inner.phase != Phase::Finalized {
                return Err(StoreError::WrongPhase("after acceptance"));
            }
            inner.phase = Phase::Notifying;
            let selectors = std::mem::take(&mut inner.selectors);
            let public = if inner.notify_public {
                std::mem::take(&mut inner.public_observers)
            } else {
                BTreeMap::new()
            };
            inner.notifying_observers = selectors.len().saturating_add(public.len());
            (selectors, public, inner.accepted.clone())
        };

        let mut deliveries = 0_u64;
        let mut panics = 0_usize;
        for (id, mut selector) in selectors {
            if catch_unwind(AssertUnwindSafe(|| {
                let _guard = CallbackGuard::enter(CallbackPhase::Notification);
                selector.notify()
            }))
            .is_err()
            {
                panics = panics.saturating_add(1);
            }
            let mut inner = self.store.inner.borrow_mut();
            inner.notifying_observers = inner.notifying_observers.saturating_sub(1);
            if inner
                .subscription_activity
                .get(&id)
                .and_then(Weak::upgrade)
                .is_some_and(|activity| activity.get())
            {
                inner.selectors.insert(id, selector);
            }
        }
        for (id, mut observer) in public {
            deliveries = deliveries.saturating_add(1);
            if catch_unwind(AssertUnwindSafe(|| {
                let _guard = CallbackGuard::enter(CallbackPhase::Notification);
                observer(&snapshot)
            }))
            .is_err()
            {
                panics = panics.saturating_add(1);
            }
            let mut inner = self.store.inner.borrow_mut();
            inner.notifying_observers = inner.notifying_observers.saturating_sub(1);
            if inner
                .subscription_activity
                .get(&id)
                .and_then(Weak::upgrade)
                .is_some_and(|activity| activity.get())
            {
                inner.public_observers.insert(id, observer);
            }
        }

        let mut deferred_panics = 0_usize;
        let mut retired = Vec::<Box<dyn Any>>::new();
        if let Some(state) = self.retired_state.take() {
            retired.push(Box::new(state));
        }
        if let Some(transaction) = self.retired_transaction.take() {
            retired.push(Box::new(transaction));
        }
        {
            let mut inner = self.store.inner.borrow_mut();
            for selector in inner.selectors.values_mut() {
                if let Some(value) = selector.take_retired() {
                    retired.push(value);
                }
            }
        }
        for value in retired {
            if catch_unwind(AssertUnwindSafe(|| drop(value))).is_err() {
                deferred_panics = deferred_panics.saturating_add(1);
            }
        }

        let mut inner = self.store.inner.borrow_mut();
        inner.notify_public = false;
        inner.notifying_observers = 0;
        inner.counters.observer_deliveries = inner
            .counters
            .observer_deliveries
            .saturating_add(deliveries);
        inner.phase = Phase::Idle;
        self.live = false;
        if deferred_panics > 0 {
            Err(StoreError::DeferredDropPanicked {
                count: deferred_panics,
            })
        } else if panics == 0 {
            Ok(())
        } else {
            Err(StoreError::ObserverPanicked { count: panics })
        }
    }

    pub fn abort(&mut self) -> Result<(), StoreError<R::Error>> {
        if !self.live {
            return Ok(());
        }
        let mut inner = self.store.inner.borrow_mut();
        if inner.phase == Phase::Finalized || inner.phase == Phase::Notifying {
            return Err(StoreError::WrongPhase("irreversible acceptance"));
        }
        for selector in inner.selectors.values_mut() {
            selector.abort();
        }
        inner.rollback_transaction();
        inner.notify_public = false;
        inner.phase = Phase::Idle;
        inner.counters.rejected_transactions =
            inner.counters.rejected_transactions.saturating_add(1);
        self.live = false;
        Ok(())
    }
}

impl<S, A, R> Drop for StoreTransaction<S, A, R>
where
    R: Reducer<S, A>,
{
    fn drop(&mut self) {
        if !self.live {
            return;
        }
        if let Ok(mut inner) = self.store.inner.try_borrow_mut()
            && inner.phase != Phase::Finalized
            && inner.phase != Phase::Notifying
        {
            for selector in inner.selectors.values_mut() {
                selector.abort();
            }
            inner.rollback_transaction();
            inner.notify_public = false;
            inner.phase = Phase::Idle;
            inner.counters.rejected_transactions =
                inner.counters.rejected_transactions.saturating_add(1);
        }
    }
}

trait ErasedStoreTransaction {
    fn prepare(&mut self) -> Result<Vec<OwnerId>, String>;
    fn finalize(&mut self) -> Result<(), String>;
    fn after_accept(&mut self) -> Result<(), String>;
    fn abort(&mut self);
}

impl<S, A, R> ErasedStoreTransaction for StoreTransaction<S, A, R>
where
    S: RollbackSafe,
    A: 'static,
    R: Reducer<S, A>,
    R::Error: fmt::Display,
{
    fn prepare(&mut self) -> Result<Vec<OwnerId>, String> {
        self.prepare_accept().map_err(|error| error.to_string())?;
        self.changed_owners().map_err(|error| error.to_string())
    }

    fn finalize(&mut self) -> Result<(), String> {
        self.finalize_accept().map_err(|error| error.to_string())
    }

    fn after_accept(&mut self) -> Result<(), String> {
        self.after_accept().map_err(|error| error.to_string())
    }

    fn abort(&mut self) {
        let _ = self.abort();
    }
}

/// Failure while coordinating heterogeneous stores under one host boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CoordinatorError {
    WrongPhase(&'static str),
    StoreLimit { limit: usize },
    Store(String),
    HostInvalidation(String),
}

impl fmt::Display for CoordinatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongPhase(phase) => write!(
                formatter,
                "coordinator operation is forbidden during {phase}"
            ),
            Self::StoreLimit { limit } => {
                write!(formatter, "coordinator exceeds {limit} stores")
            }
            Self::Store(error) => write!(formatter, "coordinated store failed: {error}"),
            Self::HostInvalidation(error) => write!(formatter, "host invalidation failed: {error}"),
        }
    }
}

/// Native renderer integration point for prepared selector invalidations.
///
/// Calls occur while every store remains rollbackable. A rejected owner or
/// surface candidate therefore aborts the complete coordinated transaction.
pub trait InvalidationSink {
    type Error: fmt::Display;

    /// Capture host invalidation state before a prepared owner batch is submitted.
    fn begin_invalidation(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn invalidate(&mut self, owner: OwnerId) -> Result<(), Self::Error>;

    /// Restore invalidation state if any owner in the batch is rejected.
    fn abort_invalidation(&mut self) {}
}

impl std::error::Error for CoordinatorError {}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum CoordinatorPhase {
    #[default]
    Mutating,
    Prepared,
    Finalized,
    Complete,
    Aborted,
}

/// Heterogeneous, single-thread transaction coordinator for native hosts.
///
/// Enlist every participating store before application callbacks. Dispatch
/// through each store's [`DispatchHandle`], prepare all stores, build and admit
/// native candidates for the returned owners, then finalize and notify.
#[derive(Default)]
pub struct TransactionCoordinator {
    participants: Vec<Box<dyn ErasedStoreTransaction>>,
    phase: CoordinatorPhase,
}

impl TransactionCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enlist<S, A, R>(&mut self, store: &Store<S, A, R>) -> Result<(), CoordinatorError>
    where
        S: RollbackSafe,
        A: 'static,
        R: Reducer<S, A>,
        R::Error: fmt::Display,
    {
        if self.phase != CoordinatorPhase::Mutating {
            return Err(CoordinatorError::WrongPhase("non-mutation phase"));
        }
        if self.participants.len() >= MAX_COORDINATED_STORES {
            return Err(CoordinatorError::StoreLimit {
                limit: MAX_COORDINATED_STORES,
            });
        }
        let transaction = store
            .begin_transaction()
            .map_err(|error| CoordinatorError::Store(error.to_string()))?;
        self.participants.push(Box::new(transaction));
        Ok(())
    }

    pub fn prepare_accept(&mut self) -> Result<Vec<OwnerId>, CoordinatorError> {
        if self.phase != CoordinatorPhase::Mutating {
            return Err(CoordinatorError::WrongPhase("prepare acceptance"));
        }
        let mut owners = Vec::new();
        for participant in &mut self.participants {
            match participant.prepare() {
                Ok(changed) => owners.extend(changed),
                Err(error) => {
                    for participant in &mut self.participants {
                        participant.abort();
                    }
                    self.phase = CoordinatorPhase::Aborted;
                    return Err(CoordinatorError::Store(error));
                }
            }
        }
        owners.sort_unstable();
        owners.dedup();
        self.phase = CoordinatorPhase::Prepared;
        Ok(owners)
    }

    /// Prepare every store and submit the deduplicated changed owners to a
    /// native host before any accepted root is switched.
    pub fn prepare_with_host(
        &mut self,
        host: &mut impl InvalidationSink,
    ) -> Result<Vec<OwnerId>, CoordinatorError> {
        let owners = self.prepare_accept()?;
        if let Err(error) = host.begin_invalidation() {
            for participant in &mut self.participants {
                participant.abort();
            }
            self.phase = CoordinatorPhase::Aborted;
            return Err(CoordinatorError::HostInvalidation(error.to_string()));
        }
        for owner in &owners {
            if let Err(error) = host.invalidate(*owner) {
                host.abort_invalidation();
                for participant in &mut self.participants {
                    participant.abort();
                }
                self.phase = CoordinatorPhase::Aborted;
                return Err(CoordinatorError::HostInvalidation(error.to_string()));
            }
        }
        Ok(owners)
    }

    /// Callback-free final switch after all native candidates are admitted.
    pub fn finalize_accept(&mut self) -> Result<(), CoordinatorError> {
        if self.phase != CoordinatorPhase::Prepared {
            return Err(CoordinatorError::WrongPhase("finalize acceptance"));
        }
        for participant in &mut self.participants {
            participant.finalize().map_err(CoordinatorError::Store)?;
        }
        self.phase = CoordinatorPhase::Finalized;
        Ok(())
    }

    pub fn after_accept(&mut self) -> Result<(), CoordinatorError> {
        if self.phase != CoordinatorPhase::Finalized {
            return Err(CoordinatorError::WrongPhase("after acceptance"));
        }
        let mut errors = Vec::new();
        for participant in &mut self.participants {
            if let Err(error) = participant.after_accept() {
                errors.push(error);
            }
        }
        self.phase = CoordinatorPhase::Complete;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(CoordinatorError::Store(errors.join("; ")))
        }
    }

    pub fn abort(&mut self) -> Result<(), CoordinatorError> {
        match self.phase {
            CoordinatorPhase::Mutating | CoordinatorPhase::Prepared => {
                for participant in &mut self.participants {
                    participant.abort();
                }
                self.phase = CoordinatorPhase::Aborted;
                Ok(())
            }
            CoordinatorPhase::Aborted => Ok(()),
            CoordinatorPhase::Finalized | CoordinatorPhase::Complete => {
                Err(CoordinatorError::WrongPhase("irreversible acceptance"))
            }
        }
    }
}

impl Drop for TransactionCoordinator {
    fn drop(&mut self) {
        if matches!(
            self.phase,
            CoordinatorPhase::Mutating | CoordinatorPhase::Prepared
        ) {
            for participant in &mut self.participants {
                participant.abort();
            }
            self.phase = CoordinatorPhase::Aborted;
        }
    }
}

#[derive(Clone, Copy)]
enum SubscriptionKind {
    Public,
    Selector,
}

/// Registration token. Dropping it unsubscribes while the store is idle.
pub struct Subscription<S, A, R>
where
    R: Reducer<S, A>,
{
    store: Weak<RefCell<Inner<S, A, R>>>,
    id: SubscriptionId,
    kind: SubscriptionKind,
    activity: Rc<Cell<bool>>,
}

impl<S, A, R> Subscription<S, A, R>
where
    R: Reducer<S, A>,
{
    fn new(
        store: &Store<S, A, R>,
        id: SubscriptionId,
        kind: SubscriptionKind,
        activity: Rc<Cell<bool>>,
    ) -> Self {
        Self {
            store: Rc::downgrade(&store.inner),
            id,
            kind,
            activity,
        }
    }

    pub fn id(&self) -> SubscriptionId {
        self.id
    }

    pub fn unsubscribe(&mut self) -> bool {
        if !self.activity.get() {
            return false;
        }
        let removed = self.store.upgrade().is_some_and(|inner| {
            let mut inner = inner.borrow_mut();
            inner.unsubscribe(self.id, self.kind)
        });
        self.activity.set(false);
        removed
    }
}

impl<S, A, R> Drop for Subscription<S, A, R>
where
    R: Reducer<S, A>,
{
    fn drop(&mut self) {
        self.unsubscribe();
    }
}

/// Initial selected value plus its lifetime token.
pub struct Selection<S, A, R, T>
where
    R: Reducer<S, A>,
{
    pub value: T,
    pub subscription: Subscription<S, A, R>,
}

type SharedSelector<S, T> = Rc<RefCell<Box<dyn FnMut(&S) -> T>>>;
type SharedEquality<T> = Rc<RefCell<Box<dyn FnMut(&T, &T) -> bool>>>;
type SharedSelectionNotify<T> = Rc<RefCell<Box<dyn FnMut(&T)>>>;

/// Typed key used by a heterogeneous [`StoreScope`].
#[derive(Debug, Eq, Hash, PartialEq)]
pub struct StoreKey<T: 'static> {
    name: &'static str,
    _type: PhantomData<fn() -> T>,
}

impl<T: 'static> Copy for StoreKey<T> {}

impl<T: 'static> Clone for StoreKey<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: 'static> StoreKey<T> {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            _type: PhantomData,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ErasedStoreKey {
    type_id: TypeId,
    name: &'static str,
}

/// Parent-linked native equivalent of a React-Redux Provider scope.
#[derive(Clone, Default)]
pub struct StoreScope {
    inner: Rc<ScopeInner>,
}

#[derive(Default)]
struct ScopeInner {
    parent: Option<StoreScope>,
    stores: RefCell<HashMap<ErasedStoreKey, Rc<dyn Any>>>,
    transaction_pending: Cell<bool>,
}

/// Failure while changing a native Provider-equivalent scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopeError {
    TransactionAlreadyPending,
    MutationDuringTransaction,
}

impl fmt::Display for ScopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TransactionAlreadyPending => {
                formatter.write_str("store scope transaction already pending")
            }
            Self::MutationDuringTransaction => {
                formatter.write_str("direct store scope mutation is forbidden during a transaction")
            }
        }
    }
}

impl std::error::Error for ScopeError {}

impl StoreScope {
    pub fn root() -> Self {
        Self::default()
    }

    pub fn child(&self) -> Self {
        Self {
            inner: Rc::new(ScopeInner {
                parent: Some(self.clone()),
                stores: RefCell::new(HashMap::new()),
                transaction_pending: Cell::new(false),
            }),
        }
    }

    pub fn provide<T: 'static>(
        &self,
        key: StoreKey<T>,
        store: T,
    ) -> Result<Option<Rc<T>>, ScopeError> {
        if self.inner.transaction_pending.get() {
            return Err(ScopeError::MutationDuringTransaction);
        }
        let erased = ErasedStoreKey {
            type_id: TypeId::of::<T>(),
            name: key.name,
        };
        Ok(self
            .inner
            .stores
            .borrow_mut()
            .insert(erased, Rc::new(store))
            .and_then(|old| old.downcast().ok()))
    }

    pub fn resolve<T: 'static>(&self, key: StoreKey<T>) -> Option<Rc<T>> {
        let erased = ErasedStoreKey {
            type_id: TypeId::of::<T>(),
            name: key.name,
        };
        let mut scope = Some(self);
        while let Some(current) = scope {
            if let Some(store) = current.inner.stores.borrow().get(&erased) {
                return Rc::clone(store).downcast().ok();
            }
            scope = current.inner.parent.as_ref();
        }
        None
    }

    /// Stage scope replacements without exposing them until acceptance.
    pub fn begin_transaction(&self) -> Result<ScopeTransaction, ScopeError> {
        if self.inner.transaction_pending.replace(true) {
            return Err(ScopeError::TransactionAlreadyPending);
        }
        Ok(ScopeTransaction {
            scope: self.clone(),
            provisional: self.inner.stores.borrow().clone(),
            live: true,
        })
    }

    /// Bind a native owner selection to the nearest scoped store incarnation.
    /// Call [`ScopedSelection::refresh_after_scope_accept`] after accepting a
    /// scope replacement; it retires the old registration and binds the new
    /// store identity before notifying the owner.
    pub fn observe_scoped_owner<S, A, R, T>(
        &self,
        key: StoreKey<Store<S, A, R>>,
        owner: OwnerId,
        selector: impl FnMut(&S) -> T + 'static,
        equality: impl FnMut(&T, &T) -> bool + 'static,
        notify: impl FnMut(&T) + 'static,
    ) -> Result<ScopedSelection<S, A, R, T>, ScopedSelectionError<R::Error>>
    where
        S: RollbackSafe,
        A: 'static,
        R: Reducer<S, A>,
        T: Clone + 'static,
    {
        ScopedSelection::new(self.clone(), key, owner, selector, equality, notify)
    }
}

/// Provisional Provider-equivalent topology for one scope.
pub struct ScopeTransaction {
    scope: StoreScope,
    provisional: HashMap<ErasedStoreKey, Rc<dyn Any>>,
    live: bool,
}

impl ScopeTransaction {
    pub fn provide<T: 'static>(&mut self, key: StoreKey<T>, store: T) -> Option<Rc<T>> {
        let erased = ErasedStoreKey {
            type_id: TypeId::of::<T>(),
            name: key.name,
        };
        self.provisional
            .insert(erased, Rc::new(store))
            .and_then(|old| old.downcast().ok())
    }

    pub fn remove<T: 'static>(&mut self, key: StoreKey<T>) -> Option<Rc<T>> {
        let erased = ErasedStoreKey {
            type_id: TypeId::of::<T>(),
            name: key.name,
        };
        self.provisional
            .remove(&erased)
            .and_then(|old| old.downcast().ok())
    }

    pub fn resolve<T: 'static>(&self, key: StoreKey<T>) -> Option<Rc<T>> {
        let erased = ErasedStoreKey {
            type_id: TypeId::of::<T>(),
            name: key.name,
        };
        if let Some(store) = self.provisional.get(&erased) {
            return Rc::clone(store).downcast().ok();
        }
        self.scope
            .inner
            .parent
            .as_ref()
            .and_then(|parent| parent.resolve(key))
    }

    pub fn accept(mut self) {
        *self.scope.inner.stores.borrow_mut() = std::mem::take(&mut self.provisional);
        self.scope.inner.transaction_pending.set(false);
        self.live = false;
    }

    pub fn abort(mut self) {
        self.scope.inner.transaction_pending.set(false);
        self.live = false;
    }
}

impl Drop for ScopeTransaction {
    fn drop(&mut self) {
        if self.live {
            self.scope.inner.transaction_pending.set(false);
        }
    }
}

/// Failure while resolving or registering a scoped native selection.
#[derive(Debug, PartialEq)]
pub enum ScopedSelectionError<E> {
    MissingStore { key: &'static str },
    Store(StoreError<E>),
}

impl<E: fmt::Display> fmt::Display for ScopedSelectionError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingStore { key } => write!(formatter, "scoped store {key:?} is unavailable"),
            Self::Store(error) => error.fmt(formatter),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for ScopedSelectionError<E> {}

/// Accepted owner binding to the nearest matching scoped store.
pub struct ScopedSelection<S: 'static, A: 'static, R, T>
where
    R: Reducer<S, A>,
{
    scope: StoreScope,
    key: StoreKey<Store<S, A, R>>,
    owner: OwnerId,
    store: Rc<Store<S, A, R>>,
    selection: Option<Selection<S, A, R, T>>,
    selected: Rc<RefCell<Option<T>>>,
    selector: SharedSelector<S, T>,
    equality: SharedEquality<T>,
    notify: SharedSelectionNotify<T>,
}

impl<S, A, R, T> ScopedSelection<S, A, R, T>
where
    S: RollbackSafe,
    A: 'static,
    R: Reducer<S, A>,
    T: Clone + 'static,
{
    fn new(
        scope: StoreScope,
        key: StoreKey<Store<S, A, R>>,
        owner: OwnerId,
        selector: impl FnMut(&S) -> T + 'static,
        equality: impl FnMut(&T, &T) -> bool + 'static,
        notify: impl FnMut(&T) + 'static,
    ) -> Result<Self, ScopedSelectionError<R::Error>> {
        let selector: SharedSelector<S, T> = Rc::new(RefCell::new(Box::new(selector)));
        let equality: SharedEquality<T> = Rc::new(RefCell::new(Box::new(equality)));
        let notify: SharedSelectionNotify<T> = Rc::new(RefCell::new(Box::new(notify)));
        let selected = Rc::new(RefCell::new(None));
        let store = scope
            .resolve(key)
            .ok_or(ScopedSelectionError::MissingStore { key: key.name })?;
        let selection = Self::register(&store, owner, &selector, &equality, &notify, &selected)
            .map_err(ScopedSelectionError::Store)?;
        *selected.borrow_mut() = Some(selection.value.clone());
        Ok(Self {
            scope,
            key,
            owner,
            store,
            selection: Some(selection),
            selected,
            selector,
            equality,
            notify,
        })
    }

    fn register(
        store: &Store<S, A, R>,
        owner: OwnerId,
        selector: &SharedSelector<S, T>,
        equality: &SharedEquality<T>,
        notify: &SharedSelectionNotify<T>,
        selected: &Rc<RefCell<Option<T>>>,
    ) -> Result<Selection<S, A, R, T>, StoreError<R::Error>> {
        let select = Rc::clone(selector);
        let equal = Rc::clone(equality);
        let output = Rc::clone(notify);
        let current = Rc::clone(selected);
        store.observe_owner(
            owner,
            move |state| (select.borrow_mut())(state),
            move |left, right| (equal.borrow_mut())(left, right),
            move |value| {
                *current.borrow_mut() = Some(value.clone());
                (output.borrow_mut())(value);
            },
        )
    }

    pub fn store_handle(&self) -> StoreHandle<S, A, R> {
        self.store.handle()
    }

    pub fn selected(&self) -> T {
        self.selected
            .borrow()
            .as_ref()
            .expect("scoped selection is initialized")
            .clone()
    }

    /// Re-resolve after scope acceptance. A changed store identity retires the
    /// old token, registers the same owner against the new store, and notifies
    /// even when the selected value compares equal because dispatch identity
    /// changed with the Provider-equivalent incarnation.
    pub fn refresh_after_scope_accept(&mut self) -> Result<bool, ScopedSelectionError<R::Error>> {
        let next_store = self
            .scope
            .resolve(self.key)
            .ok_or(ScopedSelectionError::MissingStore { key: self.key.name })?;
        if next_store.id() == self.store.id() {
            return Ok(false);
        }
        self.selection.take();
        let next = Self::register(
            &next_store,
            self.owner,
            &self.selector,
            &self.equality,
            &self.notify,
            &self.selected,
        )
        .map_err(ScopedSelectionError::Store)?;
        *self.selected.borrow_mut() = Some(next.value.clone());
        (self.notify.borrow_mut())(&next.value);
        self.store = next_store;
        self.selection = Some(next);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(Clone, Debug, PartialEq)]
    struct State {
        count: i32,
        label: String,
    }
    impl RollbackSafe for State {}

    #[derive(Clone, Copy, Debug)]
    enum Action {
        Add(i32),
        Rename,
    }

    struct RecordingHost {
        owners: Vec<OwnerId>,
        reject: Option<OwnerId>,
    }

    impl InvalidationSink for RecordingHost {
        type Error = &'static str;

        fn invalidate(&mut self, owner: OwnerId) -> Result<(), Self::Error> {
            self.owners.push(owner);
            if self.reject == Some(owner) {
                Err("candidate rejected")
            } else {
                Ok(())
            }
        }
    }

    fn store() -> Store<State, Action, impl Reducer<State, Action, Error = &'static str>> {
        Store::new(
            State {
                count: 0,
                label: "zero".into(),
            },
            |state: &State, action: &Action| {
                Ok(match action {
                    Action::Add(0) => Reduction::Unchanged,
                    Action::Add(by) => Reduction::Changed(State {
                        count: state.count + by,
                        label: state.label.clone(),
                    }),
                    Action::Rename => Reduction::Changed(State {
                        count: state.count,
                        label: "renamed".into(),
                    }),
                })
            },
        )
    }

    #[test]
    fn rejection_restores_state_and_suppresses_observers() {
        let store = store();
        let deliveries = Rc::new(Cell::new(0));
        let observed = Rc::clone(&deliveries);
        let _subscription = store
            .subscribe(move |_| observed.set(observed.get() + 1))
            .unwrap();
        {
            let mut transaction = store.begin_transaction().unwrap();
            transaction.dispatch(&Action::Add(3)).unwrap();
            assert_eq!(transaction.snapshot().count, 3);
        }
        assert_eq!(store.snapshot().count, 0);
        assert_eq!(deliveries.get(), 0);
        assert_eq!(store.counters().rejected_transactions, 1);
    }

    #[test]
    fn native_host_rejection_aborts_all_prepared_owner_invalidations() {
        let first = store();
        let second = store();
        let _first_selection = first
            .observe_owner_eq(OwnerId::new(10), |state| state.count, |_| {})
            .unwrap();
        let _second_selection = second
            .observe_owner_eq(OwnerId::new(20), |state| state.count, |_| {})
            .unwrap();
        let mut coordinator = TransactionCoordinator::new();
        coordinator.enlist(&first).unwrap();
        coordinator.enlist(&second).unwrap();
        first.dispatch_handle().dispatch(&Action::Add(1)).unwrap();
        second.dispatch_handle().dispatch(&Action::Add(2)).unwrap();

        let mut host = RecordingHost {
            owners: Vec::new(),
            reject: Some(OwnerId::new(20)),
        };
        assert_eq!(
            coordinator.prepare_with_host(&mut host),
            Err(CoordinatorError::HostInvalidation(
                "candidate rejected".into()
            ))
        );
        assert_eq!(host.owners, [OwnerId::new(10), OwnerId::new(20)]);
        assert_eq!(first.accepted_snapshot().count, 0);
        assert_eq!(second.accepted_snapshot().count, 0);
    }

    #[test]
    fn retired_store_handles_fail_and_observers_are_released() {
        let store = store();
        let read = store.handle();
        let dispatch = store.dispatch_handle();
        let subscription = store.subscribe(|_| {}).unwrap();
        let id = store.id();
        store.retire().unwrap();
        assert!(store.is_retired());
        assert!(!subscription.activity.get());
        assert_eq!(read.snapshot(), Err(StoreError::RetiredStore { store: id }));
        assert_eq!(
            dispatch.dispatch(&Action::Add(1)),
            Err(StoreError::RetiredStore { store: id })
        );
    }

    #[test]
    fn bounded_background_handoff_requires_host_drain() {
        let (sender, receiver) = action_channel(1);
        sender.try_send(Action::Add(3)).unwrap();
        assert!(matches!(
            sender.try_send(Action::Add(4)),
            Err(TrySendError::Full(Action::Add(4)))
        ));
        let store = store();
        assert_eq!(store.snapshot().count, 0);
        store
            .dispatch_immediate(&receiver.try_recv().unwrap())
            .unwrap();
        assert_eq!(store.snapshot().count, 3);
    }

    #[test]
    fn reducer_panic_preserves_transaction_journal_and_previous_reducer() {
        type TestStore = Store<i32, (), fn(&i32, &()) -> Result<Reduction<i32>, &'static str>>;
        fn old(state: &i32, _: &()) -> Result<Reduction<i32>, &'static str> {
            Ok(Reduction::Changed(state + 1))
        }
        fn panicking(_: &i32, _: &()) -> Result<Reduction<i32>, &'static str> {
            panic!("reducer panic")
        }
        let store = TestStore::new(0, old);
        let unwind = catch_unwind(AssertUnwindSafe(|| {
            let mut transaction = store.begin_transaction().unwrap();
            transaction.replace_reducer(panicking).unwrap();
            let _ = transaction.dispatch(&());
        }));
        assert!(unwind.is_err());
        store.dispatch_immediate(&()).unwrap();
        assert_eq!(store.snapshot(), 1);
    }

    #[test]
    fn prepared_observer_batch_survives_unsubscribe_and_allows_new_subscriptions() {
        let store = store();
        let later_slot = Rc::new(RefCell::new(None));
        let later_for_first = Rc::clone(&later_slot);
        let new_tokens = Rc::new(RefCell::new(Vec::new()));
        let new_tokens_for_first = Rc::clone(&new_tokens);
        let store_for_first = store.clone();
        let _first = store
            .subscribe(move |_| {
                later_for_first.borrow_mut().take();
                let token = store_for_first.subscribe(|_| {}).unwrap();
                new_tokens_for_first.borrow_mut().push(token);
            })
            .unwrap();
        let later_deliveries = Rc::new(Cell::new(0));
        let output = Rc::clone(&later_deliveries);
        *later_slot.borrow_mut() = Some(
            store
                .subscribe(move |_| output.set(output.get() + 1))
                .unwrap(),
        );

        store.dispatch_immediate(&Action::Add(1)).unwrap();
        assert_eq!(later_deliveries.get(), 1);
        assert_eq!(new_tokens.borrow().len(), 1);
    }

    #[test]
    fn destructor_panics_are_deferred_until_every_store_is_accepted() {
        static PANIC_ON_ZERO_DROP: AtomicBool = AtomicBool::new(false);
        #[derive(Clone)]
        struct DropState(i32);
        impl RollbackSafe for DropState {}
        impl Drop for DropState {
            fn drop(&mut self) {
                assert!(
                    !(self.0 == 0 && PANIC_ON_ZERO_DROP.load(Ordering::Relaxed)),
                    "accepted root destructor"
                );
            }
        }
        fn reducer(state: &DropState, by: &i32) -> Result<Reduction<DropState>, &'static str> {
            Ok(Reduction::Changed(DropState(state.0 + by)))
        }
        let first = Store::coordinated(DropState(0), reducer);
        let second = Store::coordinated(DropState(10), reducer);
        let mut coordinator = TransactionCoordinator::new();
        coordinator.enlist(&first).unwrap();
        coordinator.enlist(&second).unwrap();
        first.dispatch_handle().dispatch(&1).unwrap();
        second.dispatch_handle().dispatch(&1).unwrap();
        coordinator.prepare_accept().unwrap();
        PANIC_ON_ZERO_DROP.store(true, Ordering::Relaxed);
        coordinator.finalize_accept().unwrap();
        let result = coordinator.after_accept();
        PANIC_ON_ZERO_DROP.store(false, Ordering::Relaxed);

        assert!(matches!(result, Err(CoordinatorError::Store(_))));
        assert_eq!(first.accepted_snapshot().0, 1);
        assert_eq!(second.accepted_snapshot().0, 11);
        assert_eq!(first.generation().get(), 1);
        assert_eq!(second.generation().get(), 1);
    }

    #[test]
    fn prepare_finalize_and_after_accept_are_distinct() {
        let store = store();
        let deliveries = Rc::new(Cell::new(0));
        let observed = Rc::clone(&deliveries);
        let _subscription = store
            .subscribe(move |_| observed.set(observed.get() + 1))
            .unwrap();
        let mut transaction = store.begin_transaction().unwrap();
        transaction.dispatch(&Action::Add(2)).unwrap();
        transaction.prepare_accept().unwrap();
        assert_eq!(store.accepted_snapshot().count, 0);
        transaction.finalize_accept().unwrap();
        assert_eq!(store.accepted_snapshot().count, 2);
        assert_eq!(deliveries.get(), 0);
        transaction.after_accept().unwrap();
        assert_eq!(deliveries.get(), 1);
    }

    #[test]
    fn selector_notifies_only_when_selected_value_changes() {
        let store = store();
        let values = Rc::new(RefCell::new(Vec::new()));
        let output = Rc::clone(&values);
        let selection = store
            .observe_eq(
                |state| state.count,
                move |value| output.borrow_mut().push(*value),
            )
            .unwrap();
        assert_eq!(selection.value, 0);
        store.dispatch_immediate(&Action::Rename).unwrap();
        assert!(values.borrow().is_empty());
        store.dispatch_immediate(&Action::Add(1)).unwrap();
        assert_eq!(&*values.borrow(), &[1]);
    }

    #[test]
    fn nested_scopes_resolve_nearest_typed_store() {
        type Counter =
            Store<State, Action, fn(&State, &Action) -> Result<Reduction<State>, &'static str>>;
        const KEY: StoreKey<Counter> = StoreKey::new("counter");
        fn reducer(state: &State, _: &Action) -> Result<Reduction<State>, &'static str> {
            Ok(Reduction::Changed(state.clone()))
        }
        let root = StoreScope::root();
        root.provide(
            KEY,
            Store::new(
                State {
                    count: 1,
                    label: String::new(),
                },
                reducer,
            ),
        )
        .unwrap();
        let child = root.child();
        assert_eq!(child.resolve(KEY).unwrap().snapshot().count, 1);
        child
            .provide(
                KEY,
                Store::new(
                    State {
                        count: 2,
                        label: String::new(),
                    },
                    reducer,
                ),
            )
            .unwrap();
        assert_eq!(child.resolve(KEY).unwrap().snapshot().count, 2);
        assert_eq!(root.resolve(KEY).unwrap().snapshot().count, 1);
    }

    #[test]
    fn owner_invalidation_is_provisional_and_deduplicated() {
        let store = store();
        let first = store
            .observe_owner_eq(OwnerId::new(7), |state| state.count, |_| {})
            .unwrap();
        let second = store
            .observe_owner_eq(OwnerId::new(7), |state| state.label.clone(), |_| {})
            .unwrap();
        let mut transaction = store.begin_transaction().unwrap();
        transaction.dispatch(&Action::Add(1)).unwrap();
        transaction.prepare_accept().unwrap();
        assert_eq!(transaction.changed_owners().unwrap(), vec![OwnerId::new(7)]);
        transaction.abort().unwrap();
        assert_eq!(store.snapshot().count, 0);
        drop((first, second));
    }

    #[test]
    fn coordinator_prepares_every_store_before_callback_free_finalize() {
        let first = Store::coordinated(0_i32, |state: &i32, action: &i32| {
            Ok::<_, &'static str>(Reduction::Changed(state + action))
        });
        let second = Store::coordinated(10_i32, |state: &i32, action: &i32| {
            Ok::<_, &'static str>(Reduction::Changed(state + action))
        });
        let deliveries = Rc::new(RefCell::new(Vec::new()));
        let first_output = Rc::clone(&deliveries);
        let second_output = Rc::clone(&deliveries);
        let _first_subscription = first
            .subscribe(move |state| first_output.borrow_mut().push(("first", *state)))
            .unwrap();
        let _second_subscription = second
            .subscribe(move |state| second_output.borrow_mut().push(("second", *state)))
            .unwrap();

        let mut coordinator = TransactionCoordinator::new();
        coordinator.enlist(&first).unwrap();
        coordinator.enlist(&second).unwrap();
        first.dispatch_handle().dispatch(&2).unwrap();
        second.dispatch_handle().dispatch(&3).unwrap();
        assert!(coordinator.prepare_accept().unwrap().is_empty());
        assert_eq!(first.accepted_snapshot(), 0);
        assert_eq!(second.accepted_snapshot(), 10);
        coordinator.finalize_accept().unwrap();
        assert_eq!(first.accepted_snapshot(), 2);
        assert_eq!(second.accepted_snapshot(), 13);
        assert!(deliveries.borrow().is_empty());
        coordinator.after_accept().unwrap();
        assert_eq!(&*deliveries.borrow(), &[("first", 2), ("second", 13)]);
    }

    #[test]
    fn dropping_prepared_coordinator_rolls_every_store_back() {
        let first = Store::coordinated(0_i32, |state: &i32, action: &i32| {
            Ok::<_, &'static str>(Reduction::Changed(state + action))
        });
        let second = Store::coordinated(10_i32, |state: &i32, action: &i32| {
            Ok::<_, &'static str>(Reduction::Changed(state + action))
        });
        {
            let mut coordinator = TransactionCoordinator::new();
            coordinator.enlist(&first).unwrap();
            coordinator.enlist(&second).unwrap();
            first.dispatch_handle().dispatch(&2).unwrap();
            second.dispatch_handle().dispatch(&3).unwrap();
            coordinator.prepare_accept().unwrap();
        }
        assert_eq!(first.snapshot(), 0);
        assert_eq!(second.snapshot(), 10);
        assert_eq!(first.counters().rejected_transactions, 1);
        assert_eq!(second.counters().rejected_transactions, 1);
    }

    #[test]
    fn provisional_unsubscribe_reactivates_on_rollback() {
        let store = store();
        let mut subscription = store.subscribe(|_| {}).unwrap();
        let mut transaction = store.begin_transaction().unwrap();
        assert!(subscription.unsubscribe());
        transaction.abort().unwrap();
        assert!(subscription.unsubscribe());
    }

    #[test]
    fn dropped_provisional_subscription_is_not_leaked_by_rollback() {
        let store = store();
        let deliveries = Rc::new(Cell::new(0));
        let output = Rc::clone(&deliveries);
        let subscription = store
            .subscribe(move |_| output.set(output.get() + 1))
            .unwrap();
        let mut transaction = store.begin_transaction().unwrap();
        drop(subscription);
        transaction.abort().unwrap();
        store.dispatch_immediate(&Action::Add(1)).unwrap();
        assert_eq!(deliveries.get(), 0);
    }

    #[test]
    fn provisional_registration_is_retired_on_rollback() {
        let store = store();
        let deliveries = Rc::new(Cell::new(0));
        let output = Rc::clone(&deliveries);
        let mut transaction = store.begin_transaction().unwrap();
        let mut subscription = store
            .subscribe(move |_| output.set(output.get() + 1))
            .unwrap();
        transaction.abort().unwrap();
        assert!(!subscription.unsubscribe());
        store.dispatch_immediate(&Action::Add(1)).unwrap();
        assert_eq!(deliveries.get(), 0);
    }

    #[test]
    fn committed_observer_can_read_store_without_reentrant_borrow() {
        let store = store();
        let read = store.handle();
        let seen = Rc::new(Cell::new(0));
        let output = Rc::clone(&seen);
        let _subscription = store
            .subscribe(move |state| {
                assert_eq!(read.snapshot().unwrap(), *state);
                output.set(state.count);
            })
            .unwrap();
        store.dispatch_immediate(&Action::Add(4)).unwrap();
        assert_eq!(seen.get(), 4);
    }

    #[test]
    fn observer_panic_is_post_acceptance_and_does_not_skip_later_observers() {
        let store = store();
        let mut panicking = store.subscribe(|_| panic!("observer failure")).unwrap();
        let deliveries = Rc::new(Cell::new(0));
        let output = Rc::clone(&deliveries);
        let _healthy = store
            .subscribe(move |_| output.set(output.get() + 1))
            .unwrap();
        assert_eq!(
            store.dispatch_immediate(&Action::Add(1)),
            Err(StoreError::ObserverPanicked { count: 1 })
        );
        assert_eq!(store.accepted_snapshot().count, 1);
        assert_eq!(deliveries.get(), 1);
        assert!(panicking.unsubscribe());
        store.dispatch_immediate(&Action::Add(1)).unwrap();
        assert_eq!(store.accepted_snapshot().count, 2);
        assert_eq!(deliveries.get(), 2);
    }

    #[test]
    fn observer_phase_rejects_recursive_dispatch() {
        let store = store();
        let dispatch = store.dispatch_handle();
        let rejected = Rc::new(Cell::new(false));
        let output = Rc::clone(&rejected);
        let _subscription = store
            .subscribe(move |_| {
                output.set(matches!(
                    dispatch.dispatch(&Action::Add(1)),
                    Err(StoreError::WrongPhase("observer delivery"))
                ));
            })
            .unwrap();
        store.dispatch_immediate(&Action::Add(1)).unwrap();
        assert!(rejected.get());
        assert_eq!(store.accepted_snapshot().count, 1);
    }

    #[test]
    fn reducer_replacement_commits_or_rolls_back_with_transaction() {
        fn add(state: &i32, action: &i32) -> Result<Reduction<i32>, &'static str> {
            Ok(Reduction::Changed(state + action))
        }
        fn multiply(state: &i32, action: &i32) -> Result<Reduction<i32>, &'static str> {
            Ok(Reduction::Changed(state * action))
        }
        type IntegerStore = Store<i32, i32, fn(&i32, &i32) -> Result<Reduction<i32>, &'static str>>;
        let store: IntegerStore = Store::new(2_i32, add);
        {
            let mut transaction = store.begin_transaction().unwrap();
            transaction.replace_reducer(multiply).unwrap();
            transaction.dispatch(&3).unwrap();
        }
        assert_eq!(store.snapshot(), 2);
        store.dispatch_immediate(&3).unwrap();
        assert_eq!(store.snapshot(), 5);

        let mut transaction = store.begin_transaction().unwrap();
        transaction.replace_reducer(multiply).unwrap();
        transaction.prepare_accept().unwrap();
        transaction.finalize_accept().unwrap();
        transaction.after_accept().unwrap();
        store.dispatch_immediate(&3).unwrap();
        assert_eq!(store.snapshot(), 15);
    }

    #[test]
    fn scope_replacement_is_hidden_until_acceptance_and_rolls_back_on_drop() {
        const VALUE: StoreKey<i32> = StoreKey::new("value");
        let root = StoreScope::root();
        root.provide(VALUE, 1).unwrap();
        {
            let mut transaction = root.begin_transaction().unwrap();
            transaction.provide(VALUE, 2);
            assert_eq!(*transaction.resolve(VALUE).unwrap(), 2);
            assert_eq!(*root.resolve(VALUE).unwrap(), 1);
        }
        assert_eq!(*root.resolve(VALUE).unwrap(), 1);

        let mut transaction = root.begin_transaction().unwrap();
        transaction.provide(VALUE, 3);
        transaction.accept();
        assert_eq!(*root.resolve(VALUE).unwrap(), 3);
    }

    #[test]
    fn transactional_child_scope_still_resolves_parent_fallback() {
        const PRIMARY: StoreKey<i32> = StoreKey::new("primary");
        const SECONDARY: StoreKey<i32> = StoreKey::new("secondary");
        let root = StoreScope::root();
        root.provide(PRIMARY, 4).unwrap();
        let child = root.child();
        child.provide(SECONDARY, 8).unwrap();
        let mut transaction = child.begin_transaction().unwrap();
        transaction.remove(SECONDARY);
        assert!(transaction.resolve(SECONDARY).is_none());
        assert_eq!(*transaction.resolve(PRIMARY).unwrap(), 4);
        transaction.abort();
        assert_eq!(*child.resolve(SECONDARY).unwrap(), 8);
    }

    #[test]
    fn direct_scope_mutation_cannot_bypass_pending_journal() {
        const VALUE: StoreKey<i32> = StoreKey::new("scope-journal");
        let scope = StoreScope::root();
        scope.provide(VALUE, 1).unwrap();
        let mut transaction = scope.begin_transaction().unwrap();
        transaction.provide(VALUE, 2);
        assert_eq!(
            scope.provide(VALUE, 3),
            Err(ScopeError::MutationDuringTransaction)
        );
        transaction.abort();
        assert_eq!(*scope.resolve(VALUE).unwrap(), 1);
    }

    #[test]
    fn scoped_selection_rebinds_and_retires_old_store_registration() {
        fn add(state: &i32, action: &i32) -> Result<Reduction<i32>, &'static str> {
            Ok(Reduction::Changed(state + action))
        }
        type IntegerStore = Store<i32, i32, fn(&i32, &i32) -> Result<Reduction<i32>, &'static str>>;
        const STORE: StoreKey<IntegerStore> = StoreKey::new("scoped-counter");
        let scope = StoreScope::root();
        let first = Store::new(1, add as _);
        scope.provide(STORE, first.clone()).unwrap();
        let values = Rc::new(RefCell::new(Vec::new()));
        let output = Rc::clone(&values);
        let mut binding = scope
            .observe_scoped_owner(
                STORE,
                OwnerId::new(44),
                |value| *value,
                PartialEq::eq,
                move |value| output.borrow_mut().push(*value),
            )
            .unwrap();
        first.dispatch_immediate(&1).unwrap();
        assert_eq!(binding.selected(), 2);

        let second = Store::new(10, add as _);
        let mut transaction = scope.begin_transaction().unwrap();
        transaction.provide(STORE, second.clone());
        transaction.accept();
        assert!(binding.refresh_after_scope_accept().unwrap());
        assert_eq!(binding.selected(), 10);
        first.dispatch_immediate(&1).unwrap();
        second.dispatch_immediate(&1).unwrap();
        assert_eq!(&*values.borrow(), &[2, 10, 11]);
        assert_eq!(binding.store_handle().id(), second.id());
    }

    #[test]
    fn state_and_journal_limits_count_reserved_vector_capacity() {
        let mut state = Vec::<u8>::with_capacity(1_000_000);
        state.push(1);
        assert!(state.rollback_bytes() >= 1_000_000);
        let store = Store::with_limits(
            state,
            |state: &Vec<u8>, _: &()| Ok::<_, &'static str>(Reduction::Changed(state.clone())),
            StoreLimits {
                max_state_bytes: 1_024,
                max_journal_bytes: 2_048,
                ..StoreLimits::default()
            },
        );
        assert!(matches!(
            store.begin_transaction(),
            Err(StoreError::StateLimit { limit: 1_024 })
        ));
    }

    #[test]
    fn configured_limits_accept_the_boundary_and_reject_the_next_unit() {
        fn resize(_state: &Vec<u8>, size: &usize) -> Result<Reduction<Vec<u8>>, &'static str> {
            Ok(Reduction::Changed(vec![0; *size]))
        }
        let bytes_at_boundary = std::mem::size_of::<Vec<u8>>() + 8;
        let limits = StoreLimits {
            max_dispatches_per_transaction: 2,
            max_observers: 2,
            max_state_bytes: bytes_at_boundary,
            max_journal_bytes: bytes_at_boundary * 2,
            max_callback_micros: u64::MAX,
        };
        let store = Store::with_options(vec![0; 8], resize, StoreMode::Coordinated, limits);
        let _first = store.observe_eq(Vec::len, |_| {}).unwrap();
        let _second = store
            .observe_eq(|state| state.first().copied(), |_| {})
            .unwrap();
        assert!(matches!(
            store.observe_eq(Vec::capacity, |_| {}),
            Err(StoreError::ObserverLimit { limit: 2 })
        ));

        let mut transaction = store.begin_transaction().unwrap();
        transaction.dispatch(&8).unwrap();
        transaction.dispatch(&8).unwrap();
        assert_eq!(
            transaction.dispatch(&8),
            Err(StoreError::DispatchLimit { limit: 2 })
        );
        transaction.abort().unwrap();

        let mut transaction = store.begin_transaction().unwrap();
        transaction.dispatch(&8).unwrap();
        transaction.prepare_accept().unwrap();
        transaction.finalize_accept().unwrap();
        transaction.after_accept().unwrap();

        let mut transaction = store.begin_transaction().unwrap();
        assert_eq!(
            transaction.dispatch(&9),
            Err(StoreError::StateLimit {
                limit: bytes_at_boundary
            })
        );
        transaction.abort().unwrap();

        let journal_limited = Store::with_options(
            vec![0; 8],
            resize,
            StoreMode::Coordinated,
            StoreLimits {
                max_state_bytes: usize::MAX,
                max_journal_bytes: bytes_at_boundary * 2 - 1,
                ..limits
            },
        );
        assert_eq!(
            journal_limited.begin_transaction().map(|_| ()),
            Err(StoreError::JournalLimit {
                limit: bytes_at_boundary * 2 - 1
            })
        );
    }

    #[test]
    fn coordinator_accepts_sixty_four_stores_and_rejects_the_sixty_fifth() {
        fn reducer(state: &u8, _: &()) -> Result<Reduction<u8>, &'static str> {
            Ok(Reduction::Changed(state.wrapping_add(1)))
        }
        let stores = (0..=MAX_COORDINATED_STORES)
            .map(|_| Store::coordinated(0, reducer))
            .collect::<Vec<_>>();
        let mut coordinator = TransactionCoordinator::new();
        for store in &stores[..MAX_COORDINATED_STORES] {
            coordinator.enlist(store).unwrap();
        }
        assert_eq!(
            coordinator.enlist(&stores[MAX_COORDINATED_STORES]),
            Err(CoordinatorError::StoreLimit {
                limit: MAX_COORDINATED_STORES
            })
        );
        coordinator.abort().unwrap();
    }

    #[test]
    fn native_shared_semantic_trace_covers_burst_rollback_and_noop_notification() {
        #[derive(Clone)]
        enum TraceAction {
            Add(i32),
            Noop,
        }
        let store = Store::new(0_i32, |state: &i32, action: &TraceAction| {
            Ok::<_, &'static str>(match action {
                TraceAction::Add(by) => Reduction::Changed(state + by),
                TraceAction::Noop => Reduction::Unchanged,
            })
        });
        let notifications = Rc::new(Cell::new(0_u64));
        let output = Rc::clone(&notifications);
        let _subscription = store
            .subscribe(move |_| output.set(output.get().saturating_add(1)))
            .unwrap();
        let initial = store.snapshot();
        let mut burst = store.begin_transaction().unwrap();
        burst.dispatch(&TraceAction::Add(1)).unwrap();
        burst.dispatch(&TraceAction::Add(2)).unwrap();
        burst.prepare_accept().unwrap();
        burst.finalize_accept().unwrap();
        burst.after_accept().unwrap();
        let after_burst = store.snapshot();
        let notifications_after_burst = notifications.get();

        let mut rejected = store.begin_transaction().unwrap();
        rejected.dispatch(&TraceAction::Add(5)).unwrap();
        rejected.abort().unwrap();
        let after_reject = store.snapshot();
        let notifications_after_reject = notifications.get();

        store.dispatch_immediate(&TraceAction::Noop).unwrap();
        let trace = format!(
            "initial={initial}\nburst={after_burst}\nnotifications={notifications_after_burst}\nrejected={after_reject}\nnotifications_after_reject={notifications_after_reject}\nnoop={}\nnotifications_after_noop={}",
            store.snapshot(),
            notifications.get()
        );
        assert_eq!(
            trace,
            include_str!("../tests/fixtures/shared_semantic_trace.txt").trim()
        );
    }

    #[test]
    fn coordinator_delivers_every_store_after_one_observer_panics() {
        let first = Store::coordinated(0_i32, |state: &i32, action: &i32| {
            Ok::<_, &'static str>(Reduction::Changed(state + action))
        });
        let second = Store::coordinated(0_i32, |state: &i32, action: &i32| {
            Ok::<_, &'static str>(Reduction::Changed(state + action))
        });
        let _panicking = first.subscribe(|_| panic!("first failed")).unwrap();
        let delivered = Rc::new(Cell::new(false));
        let output = Rc::clone(&delivered);
        let _healthy = second.subscribe(move |_| output.set(true)).unwrap();
        let mut coordinator = TransactionCoordinator::new();
        coordinator.enlist(&first).unwrap();
        coordinator.enlist(&second).unwrap();
        first.dispatch_handle().dispatch(&1).unwrap();
        second.dispatch_handle().dispatch(&1).unwrap();
        coordinator.prepare_accept().unwrap();
        coordinator.finalize_accept().unwrap();
        assert!(matches!(
            coordinator.after_accept(),
            Err(CoordinatorError::Store(_))
        ));
        assert!(delivered.get());
        assert_eq!(first.accepted_snapshot(), 1);
        assert_eq!(second.accepted_snapshot(), 1);
    }

    #[test]
    fn observer_limit_counts_snapshot_entries_during_delivery() {
        let store = Store::with_limits(
            0_i32,
            |state: &i32, action: &i32| Ok::<_, &'static str>(Reduction::Changed(state + action)),
            StoreLimits {
                max_dispatches_per_transaction: 8,
                max_observers: 1,
                ..StoreLimits::default()
            },
        );
        let attempted = Rc::new(Cell::new(false));
        let rejected = Rc::new(Cell::new(false));
        let callback_store = store.clone();
        let callback_attempted = Rc::clone(&attempted);
        let callback_rejected = Rc::clone(&rejected);
        let _subscription = store
            .subscribe(move |_| {
                callback_attempted.set(true);
                callback_rejected.set(matches!(
                    callback_store.subscribe(|_| {}),
                    Err(StoreError::ObserverLimit { limit: 1 })
                ));
            })
            .unwrap();
        store.dispatch_immediate(&1).unwrap();
        assert!(attempted.get());
        assert!(rejected.get());
    }
}

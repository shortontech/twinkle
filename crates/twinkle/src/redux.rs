//! Native Twinkle ownership adapter for `twinkle-redux` selector invalidation.

use crate::UiId;
use std::{collections::HashMap, fmt};
use twinkle_redux::{InvalidationSink, OwnerId};

/// Bounded mapping from Redux selector owners to native Twinkle view owners.
///
/// Register this mapping when a native component observation is accepted, pass
/// it to `TransactionCoordinator::prepare_with_host`, validate/rebuild the
/// returned [`UiId`] candidates, then finalize the coordinator only if every
/// candidate is admitted.
pub struct ReduxUiOwners {
    owners: HashMap<OwnerId, UiId>,
    changed: Vec<UiId>,
    limit: usize,
    checkpoint: Option<(HashMap<OwnerId, UiId>, Vec<UiId>)>,
    invalidation_checkpoint: Option<Vec<UiId>>,
}

impl ReduxUiOwners {
    /// Default number of owner incarnations retained by one native surface.
    pub const DEFAULT_LIMIT: usize = 4_096;

    /// Create an empty owner map with the default finite limit.
    pub fn new() -> Self {
        Self::with_limit(Self::DEFAULT_LIMIT)
    }

    /// Create an owner map with an explicit finite limit.
    pub fn with_limit(limit: usize) -> Self {
        Self {
            owners: HashMap::new(),
            changed: Vec::new(),
            limit,
            checkpoint: None,
            invalidation_checkpoint: None,
        }
    }

    /// Capture owner bindings and dirty ownership before speculative native work.
    pub fn begin_transaction(&mut self) -> Result<(), ReduxUiError> {
        if self.checkpoint.is_some() {
            return Err(ReduxUiError::TransactionPending);
        }
        self.checkpoint = Some((self.owners.clone(), self.changed.clone()));
        Ok(())
    }

    /// Accept speculative bindings, retirements, and invalidations.
    pub fn accept_transaction(&mut self) -> Result<(), ReduxUiError> {
        self.checkpoint
            .take()
            .ok_or(ReduxUiError::TransactionUnavailable)?;
        self.invalidation_checkpoint = None;
        Ok(())
    }

    /// Restore the exact accepted owner topology and dirty queue.
    pub fn abort_transaction(&mut self) -> Result<(), ReduxUiError> {
        let (owners, changed) = self
            .checkpoint
            .take()
            .ok_or(ReduxUiError::TransactionUnavailable)?;
        self.owners = owners;
        self.changed = changed;
        self.invalidation_checkpoint = None;
        Ok(())
    }

    /// Bind one Redux observation incarnation to one native Twinkle owner.
    pub fn bind(&mut self, owner: OwnerId, ui: UiId) -> Result<Option<UiId>, ReduxUiError> {
        if !self.owners.contains_key(&owner) && self.owners.len() >= self.limit {
            return Err(ReduxUiError::OwnerLimit { limit: self.limit });
        }
        Ok(self.owners.insert(owner, ui))
    }

    /// Retire an accepted native owner and its future invalidation authority.
    pub fn retire(&mut self, owner: OwnerId) -> Option<UiId> {
        self.owners.remove(&owner)
    }

    /// Drain the deduplicated native owners changed by the latest preparation.
    pub fn take_changed(&mut self) -> Vec<UiId> {
        std::mem::take(&mut self.changed)
    }
}

impl Default for ReduxUiOwners {
    fn default() -> Self {
        Self::new()
    }
}

impl InvalidationSink for ReduxUiOwners {
    type Error = ReduxUiError;

    fn begin_invalidation(&mut self) -> Result<(), Self::Error> {
        self.invalidation_checkpoint = Some(self.changed.clone());
        Ok(())
    }

    fn invalidate(&mut self, owner: OwnerId) -> Result<(), Self::Error> {
        let ui = self
            .owners
            .get(&owner)
            .cloned()
            .ok_or(ReduxUiError::RetiredOwner { owner })?;
        if !self.changed.contains(&ui) {
            self.changed.push(ui);
        }
        Ok(())
    }

    fn abort_invalidation(&mut self) {
        if let Some(changed) = self.invalidation_checkpoint.take() {
            self.changed = changed;
        }
    }
}

/// Native owner registration or invalidation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReduxUiError {
    /// The surface reached its configured owner bound.
    OwnerLimit {
        /// Configured maximum number of native owners.
        limit: usize,
    },
    /// A prepared selection referred to an owner already retired by the UI.
    RetiredOwner {
        /// Redux owner incarnation that no longer has native authority.
        owner: OwnerId,
    },
    /// A native owner topology transaction is already active.
    TransactionPending,
    /// No native owner topology transaction is active.
    TransactionUnavailable,
}

impl fmt::Display for ReduxUiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OwnerLimit { limit } => write!(formatter, "Redux UI owner limit {limit} reached"),
            Self::RetiredOwner { owner } => {
                write!(formatter, "Redux UI owner {} is retired", owner.get())
            }
            Self::TransactionPending => {
                formatter.write_str("Redux UI owner transaction is pending")
            }
            Self::TransactionUnavailable => {
                formatter.write_str("Redux UI owner transaction is unavailable")
            }
        }
    }
}

impl std::error::Error for ReduxUiError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        FrameRequest, Rect, RetainedSurfaceChanges, RetainedSurfaceHost, RetainedSurfaceInput,
        RetainedSurfaceLifecycle, RetainedSurfaceLimits, RetainedSurfaceModel,
        RetainedSurfaceOutput, RetainedSurfaceRevisions, UiFrame, UiStateStore,
    };
    use twinkle_redux::{Reduction, RollbackSafe, Store, TransactionCoordinator};

    #[derive(Clone)]
    struct State(i32);
    impl RollbackSafe for State {}

    fn reducer(state: &State, by: &i32) -> Result<Reduction<State>, &'static str> {
        Ok(Reduction::Changed(State(state.0 + by)))
    }

    #[test]
    fn native_owner_mapping_is_bounded_deduplicated_and_retired() {
        let mut owners = ReduxUiOwners::with_limit(1);
        let owner = OwnerId::new(7);
        let ui = UiId::new("counter");
        owners.bind(owner, ui.clone()).unwrap();
        owners.invalidate(owner).unwrap();
        owners.invalidate(owner).unwrap();
        assert_eq!(owners.take_changed(), std::slice::from_ref(&ui));
        assert_eq!(owners.retire(owner), Some(ui));
        assert_eq!(
            owners.invalidate(owner),
            Err(ReduxUiError::RetiredOwner { owner })
        );
    }

    #[test]
    fn two_native_surfaces_join_one_store_acceptance_boundary() {
        let first = Store::coordinated(State(0), reducer);
        let second = Store::coordinated(State(10), reducer);
        let first_owner = OwnerId::new(1);
        let second_owner = OwnerId::new(2);
        let _first_selection = first
            .observe_owner_eq(first_owner, |state| state.0, |_| {})
            .unwrap();
        let _second_selection = second
            .observe_owner_eq(second_owner, |state| state.0, |_| {})
            .unwrap();
        let first_surface = UiId::new("surface-a/counter");
        let second_surface = UiId::new("surface-b/counter");
        let mut owners = ReduxUiOwners::new();
        owners.bind(first_owner, first_surface.clone()).unwrap();
        owners.bind(second_owner, second_surface.clone()).unwrap();

        let mut transaction = TransactionCoordinator::new();
        transaction.enlist(&first).unwrap();
        transaction.enlist(&second).unwrap();
        first.dispatch_handle().dispatch(&1).unwrap();
        second.dispatch_handle().dispatch(&2).unwrap();
        transaction.prepare_with_host(&mut owners).unwrap();
        assert_eq!(owners.take_changed(), [first_surface, second_surface]);
        transaction.finalize_accept().unwrap();
        transaction.after_accept().unwrap();
        assert_eq!(first.accepted_snapshot().0, 1);
        assert_eq!(second.accepted_snapshot().0, 12);
    }

    #[test]
    fn late_owner_failure_restores_dirty_queue_and_speculative_topology() {
        let first = Store::coordinated(State(0), reducer);
        let second = Store::coordinated(State(0), reducer);
        let first_owner = OwnerId::new(1);
        let missing_owner = OwnerId::new(2);
        let _first = first
            .observe_owner_eq(first_owner, |state| state.0, |_| {})
            .unwrap();
        let _second = second
            .observe_owner_eq(missing_owner, |state| state.0, |_| {})
            .unwrap();
        let accepted_ui = UiId::new("accepted");
        let speculative_ui = UiId::new("speculative");
        let mut owners = ReduxUiOwners::new();
        owners.bind(first_owner, accepted_ui.clone()).unwrap();
        owners.begin_transaction().unwrap();
        owners.bind(OwnerId::new(3), speculative_ui).unwrap();

        let mut transaction = TransactionCoordinator::new();
        transaction.enlist(&first).unwrap();
        transaction.enlist(&second).unwrap();
        first.dispatch_handle().dispatch(&1).unwrap();
        second.dispatch_handle().dispatch(&1).unwrap();
        assert!(transaction.prepare_with_host(&mut owners).is_err());
        assert!(owners.take_changed().is_empty());
        owners.abort_transaction().unwrap();
        assert_eq!(owners.retire(first_owner), Some(accepted_ui));
        assert_eq!(owners.retire(OwnerId::new(3)), None);
        assert_eq!(first.accepted_snapshot().0, 0);
        assert_eq!(second.accepted_snapshot().0, 0);
    }

    #[derive(Clone)]
    struct CounterSurface {
        reject_revision: Option<u64>,
    }

    impl RetainedSurfaceModel for CounterSurface {
        fn update(
            &mut self,
            input: RetainedSurfaceInput,
            _changes: RetainedSurfaceChanges,
        ) -> Result<RetainedSurfaceOutput, String> {
            if self.reject_revision == Some(input.revisions.content) {
                return Err("injected native candidate rejection".into());
            }
            Ok(RetainedSurfaceOutput {
                measured: input.bounds.size,
                ..RetainedSurfaceOutput::default()
            })
        }

        fn lifecycle(&mut self, _event: RetainedSurfaceLifecycle) {}

        fn retained_bytes(&self) -> usize {
            std::mem::size_of::<Self>()
        }
    }

    fn surface_input(revision: u64) -> RetainedSurfaceInput {
        RetainedSurfaceInput::new(
            Rect::new(0.0, 0.0, 80.0, 24.0),
            1.0,
            RetainedSurfaceRevisions {
                content: revision,
                ..RetainedSurfaceRevisions::default()
            },
        )
    }

    #[test]
    fn shared_store_and_real_native_surfaces_prepare_reject_and_finalize_atomically() {
        let store = Store::coordinated(State(0), reducer);
        let first_owner = OwnerId::new(21);
        let second_owner = OwnerId::new(22);
        let _first_selection = store
            .observe_owner_eq(first_owner, |state| state.0, |_| {})
            .unwrap();
        let _second_selection = store
            .observe_owner_eq(second_owner, |state| state.0, |_| {})
            .unwrap();
        let mut owners = ReduxUiOwners::new();
        owners.bind(first_owner, UiId::new("surface-a")).unwrap();
        owners.bind(second_owner, UiId::new("surface-b")).unwrap();

        let mut first_surface = RetainedSurfaceHost::new(
            CounterSurface {
                reject_revision: None,
            },
            RetainedSurfaceLimits::default(),
        )
        .unwrap();
        let mut second_surface = RetainedSurfaceHost::new(
            CounterSurface {
                reject_revision: Some(1),
            },
            RetainedSurfaceLimits::default(),
        )
        .unwrap();
        first_surface.update(surface_input(0)).unwrap();
        second_surface.update(surface_input(0)).unwrap();

        owners.begin_transaction().unwrap();
        let mut rejected = TransactionCoordinator::new();
        rejected.enlist(&store).unwrap();
        store.dispatch_handle().dispatch(&1).unwrap();
        rejected.prepare_with_host(&mut owners).unwrap();
        let mut first_candidate = first_surface.begin_candidate();
        let mut second_candidate = second_surface.begin_candidate();
        first_candidate.update(surface_input(1)).unwrap();
        assert!(second_candidate.update(surface_input(1)).is_err());
        rejected.abort().unwrap();
        owners.abort_transaction().unwrap();
        assert_eq!(store.accepted_snapshot().0, 0);
        assert_eq!(first_surface.snapshot().input.unwrap().revisions.content, 0);
        assert_eq!(
            second_surface.snapshot().input.unwrap().revisions.content,
            0
        );

        owners.begin_transaction().unwrap();
        let mut accepted = TransactionCoordinator::new();
        accepted.enlist(&store).unwrap();
        store.dispatch_handle().dispatch(&2).unwrap();
        accepted.prepare_with_host(&mut owners).unwrap();
        let mut first_candidate = first_surface.begin_candidate();
        let mut second_candidate = second_surface.begin_candidate();
        first_candidate.update(surface_input(2)).unwrap();
        second_candidate.update(surface_input(2)).unwrap();

        accepted.finalize_accept().unwrap();
        let retired_first = first_surface.finalize_candidate(first_candidate);
        let retired_second = second_surface.finalize_candidate(second_candidate);
        owners.accept_transaction().unwrap();
        accepted.after_accept().unwrap();
        drop((retired_first, retired_second));

        assert_eq!(store.accepted_snapshot().0, 2);
        assert_eq!(first_surface.snapshot().input.unwrap().revisions.content, 2);
        assert_eq!(
            second_surface.snapshot().input.unwrap().revisions.content,
            2
        );

        for (id, surface) in [
            ("surface-a", &first_surface),
            ("surface-b", &second_surface),
        ] {
            let element = surface.snapshot().try_element(id, |_| ()).unwrap();
            let mut state = UiStateStore::default();
            let _frame = UiFrame::resolve(
                element,
                FrameRequest::new(Rect::new(0.0, 0.0, 80.0, 24.0), &mut state),
            );
        }
    }
}

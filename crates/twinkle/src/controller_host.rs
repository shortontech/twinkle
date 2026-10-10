//! Explicit host ingress for controller execution. Native standalone input remains in Twinkle.
use crate::{
    ControllerAction, ControllerExecutionAuthority, ControllerExecutionBinding, ControllerFamily,
};

/// One host-validated controller delivery plus the independent authority used to revalidate it.
pub type HostedControllerDelivery = (
    Option<ControllerAction>,
    ControllerFamily,
    ControllerExecutionBinding,
    ControllerExecutionAuthority,
);

/// A trusted host provides bounded, nonblocking delivery and an independent execution oracle.
/// The engine still checks authority and pairing at dispatch. Implementations must never
/// derive the oracle by trusting fields in an unvalidated delivery.
pub trait HostedControllerSource {
    /// Reports whether the host currently owns a connected controller route.
    fn connected(&self) -> bool;
    /// Polls a bounded batch without blocking the native event loop.
    fn poll_actions(&mut self) -> Vec<HostedControllerDelivery>;
    /// Reports that bounded execution overflow reset the supplied binding.
    fn report_execution_overflow(&mut self, binding: ControllerExecutionBinding);
    /// Relinquishes host-side ownership and retires pending delivery.
    fn relinquish(&mut self);
}

/// Source selection is explicit; transport failure cannot switch hosted input to native polling.
pub enum ControllerSource {
    /// Discover physical controllers locally through the optional native backend.
    Standalone,
    /// Disable all controller ingress.
    Disabled,
    /// Receive already admitted events from a trusted host.
    Hosted(Box<dyn HostedControllerSource>),
}

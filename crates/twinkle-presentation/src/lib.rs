//! Native presentation adapters for Twinkle protocol and optional JSX content.
//!
//! This crate translates validated protocol component descriptions into `twinkle-ui` components.
//! Native Rust applications should depend on `twinkle-ui` directly. Hosts that accept declarative
//! packages use [`components`] and [`css`]; enable `jsx` only when the host deliberately embeds the
//! separate V8-based runtime. Capability authorization and service policy remain host-owned.

pub mod components;
pub mod css;
#[cfg(feature = "jsx")]
pub mod page;
pub mod virtual_source;

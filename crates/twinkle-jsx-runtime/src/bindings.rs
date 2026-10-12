//! Rust-owned, immutable initialization for bounded host extensions.
use crate::JsxRuntime;
use serde_json::Value;
use std::{cell::RefCell, rc::Rc};

pub type DataPublisher = fn(&mut JsxRuntime, &Value, &[&str]) -> Result<(), String>;
pub type NativeJsonCallback = Rc<RefCell<dyn FnMut(Value) -> Result<Value, String>>>;

#[derive(Default)]
pub struct HostBindings {
    pub(crate) bootstrap: String,
    pub(crate) publisher: Option<DataPublisher>,
    pub(crate) projections: Vec<String>,
    pub(crate) native_callbacks: Vec<(String, NativeJsonCallback)>,
    pub(crate) native_proxy_guards: Vec<String>,
}
impl HostBindings {
    pub const ABI_VERSION: u32 = 1;
    pub fn new(
        abi: u32,
        bootstrap: &str,
        publisher: DataPublisher,
        projections: &[&str],
    ) -> Result<Self, String> {
        if abi != Self::ABI_VERSION {
            return Err("unsupported host binding ABI".into());
        }
        if bootstrap.len() > 256 * 1024 || projections.len() > 128 {
            return Err("host bindings exceed limit".into());
        }
        let mut names = std::collections::BTreeSet::new();
        for name in projections {
            if name.is_empty() || name.len() > 64 || !names.insert(*name) {
                return Err("invalid or duplicate projection binding".into());
            }
        }
        Ok(Self {
            bootstrap: bootstrap.into(),
            publisher: Some(publisher),
            projections: projections.iter().map(|name| (*name).into()).collect(),
            native_callbacks: Vec::new(),
            native_proxy_guards: Vec::new(),
        })
    }

    /// Install one host-owned JSON callback before package initialization.
    ///
    /// The callback receives one JSON-compatible argument and returns one
    /// JSON-compatible result. It is deliberately unavailable to packages
    /// unless the trusted bootstrap exposes a bounded wrapper around its name.
    pub fn with_native_json_callback(
        mut self,
        name: &str,
        callback: NativeJsonCallback,
    ) -> Result<Self, String> {
        if name.is_empty()
            || name.len() > 64
            || !name.starts_with("__twinkle")
            || !name.chars().all(|character| {
                character.is_ascii_alphanumeric() || character == '_' || character == '$'
            })
            || self
                .native_callbacks
                .iter()
                .any(|(existing, _)| existing == name)
            || self.native_callbacks.len() >= 16
        {
            return Err("invalid or duplicate native callback binding".into());
        }
        self.native_callbacks.push((name.into(), callback));
        Ok(self)
    }

    /// Install a trusted one-argument predicate backed by V8's value kind
    /// inspection. It returns whether the argument is a Proxy without reading
    /// its prototype, properties, or traps.
    pub fn with_native_proxy_guard(mut self, name: &str) -> Result<Self, String> {
        if name.is_empty()
            || name.len() > 64
            || !name.starts_with("__twinkle")
            || !name.chars().all(|character| {
                character.is_ascii_alphanumeric() || character == '_' || character == '$'
            })
            || self
                .native_callbacks
                .iter()
                .any(|(existing, _)| existing == name)
            || self
                .native_proxy_guards
                .iter()
                .any(|existing| existing == name)
            || self.native_proxy_guards.len() >= 16
        {
            return Err("invalid or duplicate native proxy guard binding".into());
        }
        self.native_proxy_guards.push(name.into());
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn publisher(_: &mut JsxRuntime, _: &Value, _: &[&str]) -> Result<(), String> {
        Ok(())
    }
    #[test]
    fn registration_rejects_unsupported_abi_duplicate_names_and_bounds() {
        assert!(HostBindings::new(2, "", publisher, &[]).is_err());
        assert!(HostBindings::new(1, "", publisher, &["same", "same"]).is_err());
        assert!(HostBindings::new(1, "", publisher, &[""]).is_err());
        assert!(HostBindings::new(1, &"x".repeat(256 * 1024 + 1), publisher, &[]).is_err());
        assert!(HostBindings::new(1, "", publisher, &["valid"]).is_ok());
    }
    #[test]
    fn checkpoint_registration_is_sealed_before_package_initialization() {
        assert!(
            JsxRuntime::new(
                "__twinkleRegisterCheckpointParticipant({capture:()=>null,restore:()=>{}})",
                None
            )
            .is_err()
        );
    }

    #[test]
    fn native_json_callback_is_installed_before_package_initialization() {
        let calls = Rc::new(RefCell::new(0));
        let observed = Rc::clone(&calls);
        let callback: NativeJsonCallback = Rc::new(RefCell::new(move |value: Value| {
            *observed.borrow_mut() += 1;
            Ok(serde_json::json!({"next": value["value"].as_i64().unwrap() + 1}))
        }));
        let bindings = HostBindings::default()
            .with_native_json_callback("__twinkleTestNative", callback)
            .unwrap();
        let mut runtime = JsxRuntime::new_with_bindings(
            "globalThis.nativeResult=__twinkleTestNative({value:2})",
            None,
            &bindings,
        )
        .unwrap();
        assert_eq!(
            runtime
                .eval_json::<Value>("JSON.stringify(nativeResult)")
                .unwrap(),
            serde_json::json!({"next":3})
        );
        assert_eq!(*calls.borrow(), 1);
    }

    #[test]
    fn native_callback_names_are_bounded_and_reserved() {
        let callback: NativeJsonCallback = Rc::new(RefCell::new(|value: Value| Ok(value)));
        assert!(
            HostBindings::default()
                .with_native_json_callback("hostCall", Rc::clone(&callback))
                .is_err()
        );
        assert!(
            HostBindings::default()
                .with_native_json_callback("__twinkleCall", Rc::clone(&callback))
                .unwrap()
                .with_native_json_callback("__twinkleCall", callback)
                .is_err()
        );
    }
}

//! Generic multi-owner composition over explicit Rust-admitted contexts and links.
//!
//! Packages never exchange JavaScript functions, globals or handler indices.
//! The generic host invokes public components and contributions through opaque
//! Rust references, and validates native effects using their Rust-owned origin.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use twinkle_protocol::native_tree::{
    MAX_COMPONENT_NODES, MAX_TREE_DEPTH, MountPatchAuthority, bounded_json,
    composition_child_identity, diff_native_subtree, find_native_id, find_unique_native_prefix,
    is_action, namespace_native_metadata, native_slot_in_subtree, normalize_atomic_operations,
};

use serde_json::Value;

use crate::{
    JsxModuleGraph, JsxRuntime, NativePatchCounters, NativePatchEnvelope, NativePatchOperation,
    ScheduledPatch, ScheduledRender,
};

static NEXT_RUNTIME: AtomicU64 = AtomicU64::new(1);
static NEXT_SHARED_MOUNT: AtomicU64 = AtomicU64::new(1);
const MAX_MOUNTS: usize = 512;
const MAX_HANDLERS: usize = 4096;
const MAX_EFFECTS: usize = 1024;
const MAX_TRANSPORT_TOKEN: u64 = (1u64 << 53) - 1;

#[derive(Clone, Debug, PartialEq)]
pub struct AdmittedExport<O> {
    pub implemented_by: O,
    pub implementation: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct AdmittedContribution<O> {
    pub contributed_by: O,
    pub implementation: String,
    pub key: String,
    pub metadata: Value,
}
#[derive(Clone, Debug, PartialEq)]
pub struct AdmittedLinkPlan<O> {
    pub exports: BTreeMap<String, AdmittedExport<O>>,
    pub contributions: BTreeMap<String, Vec<AdmittedContribution<O>>>,
    pub owner_order: Vec<O>,
}
pub struct RegisteredSelection<O> {
    pub owner: O,
    pub implementation: String,
    pub key: String,
}
pub trait CompositionHost: Clone + std::fmt::Debug + Sized {
    type Owner: Clone + Ord + std::fmt::Debug;
    type State: Clone;
    fn publish_snapshot(
        &self,
        context: &mut OwnerRuntime<Self>,
        data: &Value,
    ) -> Result<(), String>;
    fn resolve_registered(
        &self,
        _: &BTreeMap<Self::Owner, OwnerRuntime<Self>>,
        _: &Value,
    ) -> Result<Option<RegisteredSelection<Self::Owner>>, String> {
        Ok(None)
    }
    fn register_mount(
        &self,
        context: &mut OwnerRuntime<Self>,
        mount: &str,
        implementation: &str,
    ) -> Result<(), String>;
    fn validate_entry(
        &self,
        context: &OwnerRuntime<Self>,
        implementation: &str,
    ) -> Result<(), String>;
}
#[derive(Clone, Debug)]
pub struct OrdinaryCompositionHost;
impl CompositionHost for OrdinaryCompositionHost {
    type Owner = String;
    type State = ();
    fn publish_snapshot(&self, _: &mut OwnerRuntime<Self>, _: &Value) -> Result<(), String> {
        Ok(())
    }
    fn register_mount(
        &self,
        context: &mut OwnerRuntime<Self>,
        mount: &str,
        implementation: &str,
    ) -> Result<(), String> {
        let selection = context
            .exports
            .get(implementation)
            .ok_or("component is not published by owner")?;
        context
            .runtime
            .borrow_mut()
            .register_component_mount(mount, selection)
    }
    fn validate_entry(
        &self,
        context: &OwnerRuntime<Self>,
        implementation: &str,
    ) -> Result<(), String> {
        context
            .exports
            .contains_key(implementation)
            .then_some(())
            .ok_or_else(|| "component is not published by owner".into())
    }
}
#[allow(deprecated)] // Rust minimum version compatibility.
fn allocate_identity(counter: &AtomicU64) -> Result<u64, String> {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
            (next != 0).then(|| next.checked_add(1).unwrap_or(0))
        })
        .map_err(|_| "composition incarnation exhausted".into())
}

#[derive(Clone, Debug)]
pub struct ComponentReference<H: CompositionHost> {
    runtime: u64,
    owner: H::Owner,
    implementation: String,
    incarnation: u64,
}
impl<H: CompositionHost> ComponentReference<H> {
    pub fn owner(&self) -> &H::Owner {
        &self.owner
    }
}

#[derive(Clone, Debug)]
pub struct ComponentMount {
    runtime: u64,
    id: u64,
}

#[derive(Clone, Debug)]
pub struct ComponentEventHandle<H: CompositionHost> {
    runtime: u64,
    mount: u64,
    generation: u64,
    action: u64,
    slot: String,
    owner: H::Owner,
}
impl<H: CompositionHost> ComponentEventHandle<H> {
    pub fn owner(&self) -> &H::Owner {
        &self.owner
    }
}

#[derive(Clone, Debug)]
pub struct OwnedComponentEffect<H: CompositionHost> {
    runtime: u64,
    owner: H::Owner,
    value: Value,
    incarnation: u64,
}
impl<H: CompositionHost> OwnedComponentEffect<H> {
    pub fn owner(&self) -> &H::Owner {
        &self.owner
    }
    pub fn value(&self) -> &Value {
        &self.value
    }
}

#[derive(Clone)]
pub struct RenderedComponent<H: CompositionHost> {
    pub node: Value,
    /// Node action indices address this host-owned table, not a JS runtime.
    pub events: BTreeMap<u64, ComponentEventHandle<H>>,
    generation: u64,
}

impl<H: CompositionHost> RenderedComponent<H> {
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

pub struct ScheduledComponentDispatch<H: CompositionHost> {
    pub rendered: Option<RenderedComponent<H>>,
    pub reconciliation_requested: bool,
    /// Composition callbacks may update another mount while draining effects.
    /// The expanded root must be rebuilt even when this mount itself was clean.
    pub requires_expansion: bool,
}

#[derive(Clone, Debug, Default)]
pub struct PackageReloadSignatures {
    /// Signatures for the graph which is currently accepted.
    pub previous: BTreeMap<String, String>,
    /// Signatures for the candidate graph. Keys are normalized module#export names.
    pub replacement: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct ReloadedMountPatch<H: CompositionHost> {
    pub mount: ComponentMount,
    pub patch: NativePatchEnvelope,
    pub events: BTreeMap<u64, ComponentEventHandle<H>>,
    pub generation: u64,
}

#[derive(Clone, Debug, Default)]
pub struct CompositionReload<H: CompositionHost> {
    pub preserved_packages: Vec<H::Owner>,
    pub restarted_packages: Vec<H::Owner>,
    pub patches: Vec<ReloadedMountPatch<H>>,
}

pub enum ScheduledExpandedBatch<H: CompositionHost, T> {
    Unchanged,
    Rendered {
        rendered: RenderedComponent<H>,
        validated: T,
        reconciliation_requested: bool,
    },
    Patched {
        patch: NativePatchEnvelope,
        events: BTreeMap<u64, ComponentEventHandle<H>>,
        generation: u64,
        validated: T,
        reconciliation_requested: bool,
    },
}

struct ExpansionState<H: CompositionHost> {
    root: u64,
    events: BTreeMap<u64, ComponentEventHandle<H>>,
    visited: std::collections::BTreeSet<String>,
    native_ids: std::collections::BTreeSet<String>,
    handler_slots: std::collections::BTreeSet<String>,
    next_event: u64,
}

struct PatchExpansion<H: CompositionHost> {
    retained: Value,
    dirty: std::collections::BTreeSet<u64>,
    rendered: BTreeMap<u64, RenderedComponent<H>>,
}

struct PatchedMountDispatch {
    patch: Option<NativePatchEnvelope>,
    dirty_components: Vec<String>,
    reconciliation_requested: bool,
}

type NativePatchValidator<'a, H, T> = dyn FnMut(&NativePatchEnvelope, &BTreeMap<u64, ComponentEventHandle<H>>, u64) -> Result<T, String>
    + 'a;

struct NativeRejection {
    owners: Vec<(u64, Vec<String>)>,
    message: String,
}

#[derive(Clone)]
struct CallbackGrant<H: CompositionHost> {
    receiver: u64,
    generation: u64,
    source: ComponentEventHandle<H>,
}

#[derive(Clone)]
struct OwnedChildGrant<H: CompositionHost> {
    receiver: u64,
    generation: u64,
    source: u64,
    source_generation: u64,
    node: Value,
    events: BTreeMap<u64, ComponentEventHandle<H>>,
}

#[derive(Clone)]
pub struct OwnerRuntime<H: CompositionHost> {
    pub runtime: std::rc::Rc<std::cell::RefCell<JsxRuntime>>,
    // Immutable trusted snapshot; transactions retain ownership, not a deep copy
    // of every inventory row. Updates replace the snapshot atomically.
    pub data: std::rc::Rc<Value>,
    pub exports: BTreeMap<String, String>,
    pub assets: BTreeMap<String, String>,
    pub incarnation: u64,
    pub source_digest: String,
    pub host_state: H::State,
}
/// An opaque package context retained by native lifecycle ownership. Importing
/// it into another composition host never copies executable code or grants.
#[derive(Clone)]
pub struct ProviderContext<H: CompositionHost> {
    owner: H::Owner,
    package: OwnerRuntime<H>,
}

impl<H: CompositionHost> ProviderContext<H> {
    pub fn owner(&self) -> &H::Owner {
        &self.owner
    }
    pub fn retained_context(&self) -> OwnerRuntime<H> {
        self.package.clone()
    }
}

impl<H: CompositionHost> OwnerRuntime<H> {
    pub fn admit(
        runtime: std::rc::Rc<std::cell::RefCell<JsxRuntime>>,
        host_state: H::State,
        data: std::rc::Rc<Value>,
        exports: BTreeMap<String, String>,
        assets: BTreeMap<String, String>,
        source_digest: String,
    ) -> Result<Self, String> {
        bounded_json(&data)?;
        if !data.is_object()
            || exports.len() > 1024
            || assets.len() > 1024
            || source_digest.len() > 512
        {
            return Err("owner context exceeds admission limits".into());
        }
        if exports.iter().chain(assets.iter()).any(|(key, value)| {
            key.is_empty() || key.len() > 512 || value.is_empty() || value.len() > 512
        }) {
            return Err("owner context contains invalid binding".into());
        }
        Ok(Self {
            runtime,
            host_state,
            data,
            exports,
            assets,
            incarnation: allocate_identity(&NEXT_RUNTIME)?,
            source_digest,
        })
    }
}

#[derive(Clone)]
struct MountState<H: CompositionHost> {
    reference: ComponentReference<H>,
    generation: u64,
    props: Value,
    surface: Option<Value>,
}

struct CompositionCheckpoint<H: CompositionHost> {
    mounts: BTreeMap<u64, MountState<H>>,
    effects: Vec<OwnedComponentEffect<H>>,
    nested_mounts: BTreeMap<(u64, String), ComponentMount>,
    callbacks: BTreeMap<u64, CallbackGrant<H>>,
    children: BTreeMap<u64, OwnedChildGrant<H>>,
    data: BTreeMap<H::Owner, std::rc::Rc<Value>>,
    scheduled_renders: BTreeMap<u64, RenderedComponent<H>>,
    patch_authority: BTreeMap<u64, MountPatchAuthority>,
}

/// One generic composition host, one context/module cache per package.
/// This API deliberately has no per-page or per-component JS host lifecycle.
/// Hosts retain responsibility for capability/value/revision checks on effects.
pub struct CompositionRuntime<H: CompositionHost> {
    id: u64,
    resolution: AdmittedLinkPlan<H::Owner>,
    extensions: H,
    packages: BTreeMap<H::Owner, OwnerRuntime<H>>,
    mounts: BTreeMap<u64, MountState<H>>,
    next_mount: u64,
    next_generation: u64,
    effects: Vec<OwnedComponentEffect<H>>,
    nested_mounts: BTreeMap<(u64, String), ComponentMount>,
    callbacks: BTreeMap<u64, CallbackGrant<H>>,
    children: BTreeMap<u64, OwnedChildGrant<H>>,
    next_callback: u64,
    callback_depth: usize,
    scheduled_renders: BTreeMap<u64, RenderedComponent<H>>,
    patch_authority: BTreeMap<u64, MountPatchAuthority>,
    checkpoint: Option<CompositionCheckpoint<H>>,
}

impl<H: CompositionHost> CompositionRuntime<H> {
    pub fn from_admitted_plan(
        resolution: AdmittedLinkPlan<H::Owner>,
        extensions: H,
        packages: BTreeMap<H::Owner, OwnerRuntime<H>>,
    ) -> Result<Self, String> {
        if packages.len() > 128 || resolution.exports.len() > 1024 {
            return Err("composition plan exceeds owner/export limits".into());
        }
        let mut incarnations = std::collections::BTreeSet::new();
        let mut contexts = std::collections::BTreeSet::new();
        for context in packages.values() {
            bounded_json(&context.data)?;
            if context.incarnation == 0
                || !incarnations.insert(context.incarnation)
                || !contexts.insert(std::rc::Rc::as_ptr(&context.runtime) as usize)
            {
                return Err("owner contexts share an incarnation or execution domain".into());
            }
        }
        for owner in &resolution.owner_order {
            if !packages.contains_key(owner) {
                return Err("composition plan names unknown owner".into());
            }
        }
        for (contract, entry) in &resolution.exports {
            if contract.is_empty() || contract.len() > 128 {
                return Err("invalid public component contract".into());
            }
            if !packages.contains_key(&entry.implemented_by) {
                return Err("export owner is not admitted".into());
            }
            if !packages[&entry.implemented_by]
                .exports
                .contains_key(&entry.implementation)
            {
                return Err("export implementation is not published by its owner".into());
            }
        }
        let mut catalog_bytes = 2usize;
        let mut contribution_count = 0usize;
        for (collection, entries) in &resolution.contributions {
            if collection.is_empty() || collection.len() > 128 {
                return Err("invalid contribution collection".into());
            }
            catalog_bytes = catalog_bytes
                .checked_add(collection.len() * 6 + 8)
                .ok_or("composition catalog size overflow")?;
            contribution_count = contribution_count
                .checked_add(entries.len())
                .ok_or("composition contribution count overflow")?;
            if contribution_count > 1024 {
                return Err("composition contribution limit exceeded".into());
            }
            for entry in entries {
                if !packages.contains_key(&entry.contributed_by) || !entry.metadata.is_object() {
                    return Err("contribution owner/metadata is invalid".into());
                }
                if !packages[&entry.contributed_by]
                    .exports
                    .contains_key(&entry.implementation)
                {
                    return Err("contribution implementation is not published by its owner".into());
                }
                bounded_json(&entry.metadata)?;
                if entry.key.is_empty() || entry.key.len() > 1024 {
                    return Err("invalid contribution key".into());
                }
                // Include conservative escaping/key-incarnation overhead before any
                // cloned catalog is formed or sent into an execution context.
                catalog_bytes = catalog_bytes
                    .checked_add(crate::encoded_json_len(&entry.metadata)?)
                    .and_then(|bytes| bytes.checked_add(entry.key.len() * 6 + 64))
                    .ok_or("composition catalog size overflow")?;
                if catalog_bytes > twinkle_protocol::native_tree::MAX_JSON_BYTES {
                    return Err("composition catalog exceeds size limit".into());
                }
            }
        }
        Ok(Self {
            id: allocate_identity(&NEXT_RUNTIME)?,
            resolution,
            extensions,
            packages,
            mounts: BTreeMap::new(),
            next_mount: 0,
            next_generation: 0,
            effects: Vec::new(),
            nested_mounts: BTreeMap::new(),
            callbacks: BTreeMap::new(),
            children: BTreeMap::new(),
            next_callback: 0,
            callback_depth: 0,
            scheduled_renders: BTreeMap::new(),
            patch_authority: BTreeMap::new(),
            checkpoint: None,
        })
    }

    fn advance_mount_generation_grants(
        &mut self,
        id: u64,
        previous_generation: u64,
        generation: u64,
    ) {
        for grant in self.callbacks.values_mut() {
            if grant.receiver == id && grant.generation == previous_generation {
                grant.generation = generation;
            }
            if grant.source.mount == id && grant.source.generation == previous_generation {
                grant.source.generation = generation;
            }
        }
        for grant in self.children.values_mut() {
            if grant.receiver == id && grant.generation == previous_generation {
                grant.generation = generation;
            }
            if grant.source == id && grant.source_generation == previous_generation {
                grant.source_generation = generation;
            }
        }
    }

    /// Validate every accepted root against an isolated replacement host.
    /// Cross-owner changes can affect roots whose own source is unchanged.
    /// Candidate effects are discarded; accepted contexts and grants are untouched.
    pub fn validate_replacement(&self, candidate: &mut Self) -> Result<(), String> {
        if self.checkpoint.is_some() || candidate.checkpoint.is_some() {
            return Err("cannot validate replacement during a composition transaction".into());
        }
        if !candidate.mounts.is_empty() {
            return Err("replacement validation requires an unmounted candidate".into());
        }
        for accepted in self.packages.values() {
            if candidate
                .packages
                .values()
                .any(|replacement| std::rc::Rc::ptr_eq(&accepted.runtime, &replacement.runtime))
            {
                return Err("replacement validation requires isolated execution domains".into());
            }
        }
        let nested = self
            .nested_mounts
            .values()
            .map(|mount| mount.id)
            .collect::<std::collections::BTreeSet<_>>();
        for (id, state) in &self.mounts {
            if nested.contains(id) {
                // Parent expansion supplies candidate-owned callback/child grants;
                // accepted nested transport props cannot cross into the candidate.
                continue;
            }
            let package = candidate
                .packages
                .get(&state.reference.owner)
                .ok_or("candidate removed a mounted owner")?;
            let reference = ComponentReference {
                runtime: candidate.id,
                owner: state.reference.owner.clone(),
                implementation: state.reference.implementation.clone(),
                incarnation: package.incarnation,
            };
            let mount = candidate.mount(&reference)?;
            let rendered = candidate.render_expanded(&mount, &state.props, |_| Ok(()));
            let cleanup = candidate.unmount(&mount);
            candidate.take_effects();
            rendered?;
            cleanup?;
        }
        Ok(())
    }

    /// Install a native-admitted replacement graph after isolated root preflight.
    /// Compatible owners retain their hook context; incompatible owners receive
    /// the candidate execution domain and all former event grants are retired.
    ///
    /// This preserves development reload semantics. Failures during installation
    /// or host registration are not transactional over arbitrary package globals;
    /// callers must not report such a failure as an unchanged accepted graph.
    pub fn reload_admitted(
        &mut self,
        mut candidate: Self,
        graphs: &BTreeMap<H::Owner, JsxModuleGraph>,
        signatures: &BTreeMap<H::Owner, PackageReloadSignatures>,
    ) -> Result<CompositionReload<H>, String>
    where
        H::State: PartialEq,
    {
        self.validate_replacement(&mut candidate)?;
        let changed = self
            .packages
            .keys()
            .chain(candidate.packages.keys())
            .filter(|owner| {
                self.packages
                    .get(*owner)
                    .zip(candidate.packages.get(*owner))
                    .is_none_or(|(old, new)| {
                        old.source_digest != new.source_digest
                            || old.host_state != new.host_state
                            || old.exports != new.exports
                            || old.assets != new.assets
                    })
            })
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let compatible = changed
            .iter()
            .filter(|owner| {
                signatures
                    .get(*owner)
                    .is_some_and(|set| set.previous == set.replacement)
                    && self
                        .packages
                        .get(*owner)
                        .zip(candidate.packages.get(*owner))
                        .is_some_and(|(old, new)| {
                            old.host_state == new.host_state
                                && old.exports == new.exports
                                && old.assets == new.assets
                        })
            })
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        for owner in &compatible {
            graphs
                .get(owner)
                .ok_or("compatible replacement module graph is unavailable")?
                .compile()?;
        }
        let nested = self
            .nested_mounts
            .values()
            .map(|mount| mount.id)
            .collect::<std::collections::BTreeSet<_>>();
        let roots = self
            .mounts
            .keys()
            .filter(|id| !nested.contains(id))
            .copied()
            .collect::<Vec<_>>();
        let mut result = CompositionReload {
            preserved_packages: Vec::new(),
            restarted_packages: Vec::new(),
            patches: Vec::new(),
        };
        if changed.is_empty() && candidate.resolution == self.resolution {
            return Ok(result);
        }
        // Unmount incompatible nested boundaries while their old domain is live.
        // Root handles stay stable and will be registered in replacement domains.
        let retire_nested = nested
            .iter()
            .filter(|id| {
                self.mounts.get(id).is_some_and(|state| {
                    changed.contains(&state.reference.owner)
                        && !compatible.contains(&state.reference.owner)
                })
            })
            .copied()
            .collect::<Vec<_>>();
        for id in retire_nested {
            if self.mounts.contains_key(&id) {
                self.unmount(&ComponentMount {
                    runtime: self.id,
                    id,
                })?;
            }
        }
        self.nested_mounts
            .retain(|_, mount| self.mounts.contains_key(&mount.id));
        for owner in &changed {
            if compatible.contains(owner) {
                self.packages
                    .get_mut(owner)
                    .unwrap()
                    .runtime
                    .borrow_mut()
                    .hot_install_modules(&graphs[owner], &signatures[owner].replacement)?;
                let replacement = candidate.packages.remove(owner).unwrap();
                let current = self.packages.get_mut(owner).unwrap();
                current.source_digest = replacement.source_digest;
                current.host_state = replacement.host_state;
                current.exports = replacement.exports;
                current.assets = replacement.assets;
                result.preserved_packages.push(owner.clone());
            } else {
                if let Some(old) = self.packages.get_mut(owner) {
                    for id in &roots {
                        if self.mounts[id].reference.owner == *owner {
                            old.runtime.borrow_mut().drop_surface(&surface(*id))?;
                        }
                    }
                }
                self.packages.remove(owner);
                if let Some(replacement) = candidate.packages.remove(owner) {
                    self.packages.insert(owner.clone(), replacement);
                    result.restarted_packages.push(owner.clone());
                }
            }
        }
        self.resolution = candidate.resolution;
        self.callbacks.clear();
        self.children.clear();
        self.effects
            .retain(|effect| !changed.contains(&effect.owner));
        self.scheduled_renders.clear();
        for id in roots {
            let state = self.mounts[&id].clone();
            let package = self
                .packages
                .get_mut(&state.reference.owner)
                .ok_or("replacement removed a mounted owner")?;
            if result.restarted_packages.contains(&state.reference.owner) {
                self.extensions.register_mount(
                    package,
                    &surface(id),
                    &state.reference.implementation,
                )?;
                self.mounts.get_mut(&id).unwrap().reference.incarnation = package.incarnation;
            }
            let mount = ComponentMount {
                runtime: self.id,
                id,
            };
            let rendered = self.render_expanded(&mount, &state.props, |_| Ok(()))?;
            result.patches.push(ReloadedMountPatch {
                mount,
                patch: NativePatchEnvelope {
                    version: 1,
                    operations: vec![NativePatchOperation::ReplaceSubtree {
                        target: "root".into(),
                        node: rendered.node,
                    }],
                    counters: NativePatchCounters {
                        nodes_visited: 1,
                        nodes_mutated: 1,
                        local_materializations: 1,
                        expansion_nodes: 1,
                        tree_bytes: 0,
                    },
                },
                events: rendered.events,
                generation: rendered.generation,
            });
        }
        self.publish_contribution_catalog()?;
        Ok(result)
    }

    /// Read admitted native metadata without exposing composition grants.
    pub fn owner_contexts(&self) -> impl Iterator<Item = (&H::Owner, &OwnerRuntime<H>)> {
        self.packages.iter()
    }

    /// Capture initialization requests from an owner before it has any mounts.
    /// Native hosts still validate every returned effect against their policy.
    pub fn collect_initial_owner_effects(&mut self, owner: &H::Owner) -> Result<(), String> {
        if !self.packages.contains_key(owner) {
            return Err("retired initialization owner".into());
        }
        if self
            .mounts
            .values()
            .any(|mount| &mount.reference.owner == owner)
        {
            return Err("initialization effects require an unmounted owner".into());
        }
        self.drain_effects(owner, 0, 0, false)
    }

    pub fn participating_owners(&self) -> impl Iterator<Item = &H::Owner> {
        self.resolution
            .owner_order
            .iter()
            .filter(|owner| self.packages.contains_key(*owner))
            .chain(
                self.packages
                    .keys()
                    .filter(|owner| !self.resolution.owner_order.contains(owner)),
            )
    }

    pub fn provider_context(&self, owner: &H::Owner) -> Result<ProviderContext<H>, String> {
        Ok(ProviderContext::<H> {
            owner: owner.clone(),
            package: self
                .packages
                .get(owner)
                .ok_or("retired package owner")?
                .clone(),
        })
    }

    /// Reconcile contribution links using contexts admitted by the native host.
    /// Existing owners cannot change code or bindings through this path. Removed
    /// owners remain live until native admission succeeds and the host retires them.
    /// The new catalog is published by the next production render, before guest
    /// component execution; this operation itself executes no guest code.
    pub fn sync_admitted_contributors(
        &mut self,
        resolution: AdmittedLinkPlan<H::Owner>,
        contexts: &BTreeMap<H::Owner, ProviderContext<H>>,
    ) -> Result<bool, String>
    where
        H::State: PartialEq,
    {
        if self.checkpoint.is_some() {
            return Err("cannot reconcile contributors during a composition transaction".into());
        }
        if resolution.exports != self.resolution.exports
            || resolution.owner_order != self.resolution.owner_order
        {
            return Err(
                "public component selections changed during contributor reconciliation".into(),
            );
        }
        let mut packages = self.packages.clone();
        let mut added = false;
        for (owner, context) in contexts {
            if context.owner != *owner {
                return Err("provider context identity mismatch".into());
            }
            if let Some(existing) = packages.get(owner) {
                let supplied = &context.package;
                if existing.source_digest != supplied.source_digest
                    || existing.host_state != supplied.host_state
                    || existing.exports != supplied.exports
                    || existing.assets != supplied.assets
                    || !std::rc::Rc::ptr_eq(&existing.runtime, &supplied.runtime)
                {
                    return Err("running contributor code or bindings changed".into());
                }
            } else {
                let mut supplied = context.package.clone();
                // A retained execution domain can return after retirement. Its
                // former reference must never regain authority in this host.
                supplied.incarnation = allocate_identity(&NEXT_RUNTIME)?;
                packages.insert(owner.clone(), supplied);
                added = true;
            }
        }
        // Reuse the complete bounded admission checks before changing live maps.
        // This constructor does not evaluate code or publish a guest catalog.
        let candidate = Self::from_admitted_plan(resolution, self.extensions.clone(), packages)?;
        let changed = added || candidate.resolution != self.resolution;
        self.resolution = candidate.resolution;
        self.packages = candidate.packages;
        Ok(changed)
    }

    fn contribution_catalog(&self) -> Value {
        let catalog = self
            .resolution
            .contributions
            .iter()
            .map(|(collection, entries)| {
                let entries = entries
                    .iter()
                    .filter(|entry| self.packages.contains_key(&entry.contributed_by))
                    .map(|entry| {
                        let mut metadata = entry.metadata.as_object().cloned().unwrap_or_default();
                        metadata.insert("key".into(), Value::String(self.contribution_key(entry)));
                        Value::Object(metadata)
                    })
                    .collect();
                (collection.clone(), Value::Array(entries))
            })
            .collect();
        Value::Object(catalog)
    }

    pub fn publish_contribution_catalog(&self) -> Result<(), String> {
        let catalog = self.contribution_catalog();
        for package in self.packages.values() {
            package
                .runtime
                .borrow_mut()
                .set_contribution_catalog(&catalog)?;
        }
        Ok(())
    }

    /// Trusted native adapters may share the owner context without creating a
    /// second package lifecycle. Guest code never receives this Rust reference.
    pub fn shared_owner_runtime(
        &self,
        owner: &H::Owner,
    ) -> Result<std::rc::Rc<std::cell::RefCell<JsxRuntime>>, String> {
        Ok(self
            .packages
            .get(owner)
            .ok_or("retired package owner")?
            .runtime
            .clone())
    }

    /// Service one live owner only after the complete composition transaction
    /// has finished. Host scheduling chooses owners fairly and admits any
    /// resulting component changes/effects through the normal validated path.
    pub fn service_owner_platform_tasks(&mut self, owner: &H::Owner) -> Result<usize, String> {
        if self.transaction_pending() {
            return Err("cannot service platform tasks during a composition transaction".into());
        }
        self.shared_owner_runtime(owner)?
            .borrow_mut()
            .service_platform_tasks()
    }

    /// Read native scheduling state without selecting a surface, executing
    /// JavaScript, allocating an owner list, or observing provisional work.
    pub fn platform_maintenance_pending(&self) -> bool {
        !self.transaction_pending()
            && self
                .packages
                .values()
                .any(|package| package.runtime.borrow().platform_maintenance_pending())
    }

    pub fn owner_platform_maintenance_pending(&self, owner: &H::Owner) -> Result<bool, String> {
        let package = self.packages.get(owner).ok_or("retired package owner")?;
        Ok(!self.transaction_pending() && package.runtime.borrow().platform_maintenance_pending())
    }

    /// Service at most one pending owner, resuming after the previous selection.
    /// The cursor is a hint only: owner retirement changes the live inventory.
    pub fn service_next_pending_platform_owner(
        &mut self,
        cursor: &mut usize,
    ) -> Result<Option<(H::Owner, usize)>, String> {
        if self.transaction_pending() {
            return Err("cannot service platform tasks during a composition transaction".into());
        }
        let owners = self.participating_owners().collect::<Vec<_>>();
        if owners.is_empty() {
            return Ok(None);
        }
        let start = *cursor % owners.len();
        let selected = (0..owners.len()).find_map(|offset| {
            let index = (start + offset) % owners.len();
            self.packages
                .get(owners[index])
                .filter(|package| package.runtime.borrow().platform_maintenance_pending())
                .map(|_| (index, owners[index].clone()))
        });
        let Some((index, owner)) = selected else {
            return Ok(None);
        };
        let next = (index + 1) % owners.len();
        let count = self.service_owner_platform_tasks(&owner)?;
        *cursor = next;
        Ok(Some((owner, count)))
    }

    pub fn mount_work_pending(&self, root: &ComponentMount) -> Result<bool, String> {
        if self.transaction_pending() {
            return Err("cannot query idle work during a composition transaction".into());
        }
        self.validate_mount(root)?;
        for id in std::iter::once(root.id).chain(
            self.nested_mounts
                .iter()
                .filter(|((owner, _), _)| *owner == root.id)
                .map(|(_, mount)| mount.id),
        ) {
            let mount = self.mounts.get(&id).ok_or("retired component mount")?;
            if self
                .shared_owner_runtime(&mount.reference.owner)?
                .borrow_mut()
                .surface_work_pending(&surface(id))?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn mount_count(&self) -> usize {
        self.mounts.len()
    }

    pub fn component(&self, contract: &str) -> Option<ComponentReference<H>> {
        self.resolution
            .exports
            .get(contract)
            .map(|export| ComponentReference::<H> {
                runtime: self.id,
                owner: export.implemented_by.clone(),
                implementation: export.implementation.clone(),
                incarnation: self
                    .packages
                    .get(&export.implemented_by)
                    .map_or(0, |p| p.incarnation),
            })
    }

    fn contribution_key(&self, entry: &AdmittedContribution<H::Owner>) -> String {
        format!(
            "{}/{}",
            entry.key,
            self.packages
                .get(&entry.contributed_by)
                .map_or(0, |package| package.incarnation)
        )
    }

    fn contribution(&self, key: &str) -> Option<ComponentReference<H>> {
        self.resolution
            .contributions
            .values()
            .flatten()
            .find(|entry| {
                self.contribution_key(entry) == key
                    && self.packages.contains_key(&entry.contributed_by)
            })
            .map(|entry| ComponentReference::<H> {
                runtime: self.id,
                owner: entry.contributed_by.clone(),
                implementation: entry.implementation.clone(),
                incarnation: self.packages[&entry.contributed_by].incarnation,
            })
    }

    pub fn contributions(&self, collection: &str) -> Vec<ComponentReference<H>> {
        self.resolution
            .contributions
            .get(collection)
            .into_iter()
            .flatten()
            .filter(|entry| self.packages.contains_key(&entry.contributed_by))
            .map(|entry| ComponentReference::<H> {
                runtime: self.id,
                owner: entry.contributed_by.clone(),
                implementation: entry.implementation.clone(),
                incarnation: self.packages[&entry.contributed_by].incarnation,
            })
            .collect()
    }

    pub fn mount(&mut self, reference: &ComponentReference<H>) -> Result<ComponentMount, String> {
        if reference.runtime != self.id
            || self
                .packages
                .get(&reference.owner)
                .is_none_or(|p| p.incarnation != reference.incarnation)
        {
            return Err("foreign or retired component reference".into());
        }
        if self.mounts.len() >= MAX_MOUNTS {
            return Err("too many component mounts".into());
        }
        // Shared provider contexts can serve several native composition hosts;
        // surface IDs must remain unique across them to keep hooks isolated.
        let id = allocate_identity(&NEXT_SHARED_MOUNT)?;
        self.next_mount = id;
        let package = self.packages.get_mut(&reference.owner).unwrap();
        self.extensions
            .register_mount(package, &surface(id), &reference.implementation)?;
        self.mounts.insert(
            id,
            MountState::<H> {
                reference: reference.clone(),
                generation: 0,
                props: serde_json::json!({}),
                surface: package.data.get("surface").cloned(),
            },
        );
        Ok(ComponentMount {
            runtime: self.id,
            id,
        })
    }

    /// Replace the host-owned observation attached to one native mount.
    pub fn update_mount_surface(
        &mut self,
        mount: &ComponentMount,
        surface: Value,
    ) -> Result<bool, String> {
        self.validate_mount(mount)?;
        let state = self
            .mounts
            .get_mut(&mount.id)
            .ok_or("retired component mount")?;
        if state.surface.as_ref() == Some(&surface) {
            return Ok(false);
        }
        state.surface = Some(surface);
        Ok(true)
    }

    /// A host that is about to perform a complete mount render may consume the
    /// store notification directly instead of carrying it into the incremental
    /// event patch queue.
    pub fn consume_mount_reconciliation(&mut self, mount: &ComponentMount) -> Result<(), String> {
        self.validate_mount(mount)?;
        let owner = self.mounts[&mount.id].reference.owner.clone();
        let package = self
            .packages
            .get_mut(&owner)
            .ok_or("retired component owner")?;
        let mut runtime = package.runtime.borrow_mut();
        runtime.select_surface(&surface(mount.id))?;
        runtime.consume_reconciliation()
    }

    /// Publish one root mount's current surface authority to every component
    /// ownership boundary on that native surface and report whether a hook
    /// selector actually observed a change.
    pub fn publish_mount_surface_authority(
        &mut self,
        root: &ComponentMount,
    ) -> Result<bool, String> {
        self.validate_mount(root)?;
        let root_surface = self.mounts[&root.id]
            .surface
            .clone()
            .unwrap_or_else(|| serde_json::json!({}));
        let mounts = std::iter::once(root.id)
            .chain(
                self.nested_mounts
                    .iter()
                    .filter(|((owner, _), _)| *owner == root.id)
                    .map(|(_, mount)| mount.id),
            )
            .collect::<Vec<_>>();
        let mut requested = false;
        for id in mounts {
            if id != root.id {
                self.mounts.get_mut(&id).unwrap().surface = Some(root_surface.clone());
            }
            let owner = self.mounts[&id].reference.owner.clone();
            let package = self
                .packages
                .get_mut(&owner)
                .ok_or("retired component owner")?;
            let mut runtime = package.runtime.borrow_mut();
            runtime.select_surface(&surface(id))?;
            let was_requested = runtime.reconciliation_requested()?;
            runtime.set_surface_store(&surface(id), &root_surface)?;
            let now_requested = runtime.reconciliation_requested()?;
            requested |= !was_requested && now_requested;
        }
        Ok(requested)
    }

    /// Begin a bounded transaction spanning every participating owner context.
    /// IDs remain monotonic so rolled-back mount/callback handles cannot be reused.
    pub fn begin_transaction(&mut self) -> Result<(), String> {
        if self.checkpoint.is_some() {
            return Err("composition transaction already pending".into());
        }
        let mut started = Vec::new();
        for (owner, package) in &self.packages {
            if let Err(error) = package.runtime.borrow_mut().begin_transaction() {
                for owner in started {
                    let _ = self.packages[&owner]
                        .runtime
                        .borrow_mut()
                        .finish_transaction(false);
                }
                return Err(error);
            }
            started.push(owner.clone());
        }
        self.checkpoint = Some(CompositionCheckpoint::<H> {
            mounts: self.mounts.clone(),
            effects: self.effects.clone(),
            nested_mounts: self.nested_mounts.clone(),
            callbacks: self.callbacks.clone(),
            children: self.children.clone(),
            data: self
                .packages
                .iter()
                .map(|(owner, package)| (owner.clone(), package.data.clone()))
                .collect(),
            scheduled_renders: self.scheduled_renders.clone(),
            patch_authority: self.patch_authority.clone(),
        });
        Ok(())
    }

    pub fn transaction_pending(&self) -> bool {
        self.checkpoint.is_some()
    }

    pub fn finish_transaction(&mut self, accepted: bool) -> Result<(), String> {
        let checkpoint = self
            .checkpoint
            .take()
            .ok_or("composition transaction is unavailable")?;
        let mut failure = None;
        for package in self.packages.values() {
            if let Err(error) = package.runtime.borrow_mut().finish_transaction(accepted) {
                failure = Some(error);
            }
        }
        if !accepted {
            self.mounts = checkpoint.mounts;
            self.effects = checkpoint.effects;
            self.nested_mounts = checkpoint.nested_mounts;
            self.callbacks = checkpoint.callbacks;
            self.children = checkpoint.children;
            for (owner, data) in checkpoint.data {
                if let Some(package) = self.packages.get_mut(&owner) {
                    package.data = data;
                }
            }
            self.scheduled_renders = checkpoint.scheduled_renders;
            self.patch_authority = checkpoint.patch_authority;
        }
        if let Some(error) = failure {
            self.effects.clear();
            let owners = self.packages.keys().cloned().collect::<Vec<_>>();
            for owner in owners {
                self.retire(&owner);
            }
            return Err(format!("composition checkpoint failed: {error}"));
        }
        Ok(())
    }

    fn transactional<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        if self.checkpoint.is_some() {
            return operation(self);
        }
        self.begin_transaction()?;
        let result = operation(self);
        self.finish_transaction(result.is_ok())?;
        result
    }

    pub fn render(
        &mut self,
        mount: &ComponentMount,
        props: &Value,
    ) -> Result<RenderedComponent<H>, String> {
        self.render_validated(mount, props, |_| Ok(()))
    }

    pub fn dispatch(
        &mut self,
        handle: &ComponentEventHandle<H>,
        value: &Value,
    ) -> Result<RenderedComponent<H>, String> {
        self.dispatch_validated(handle, value, |_| Ok(()))
    }

    /// Legacy/cold dispatch for a mount without admitted patch authority.
    /// Production retained mounts route through `render_mount_patched`; this
    /// path remains for initial admission, cold-oracle comparison, and newly
    /// discovered ownership boundaries. Referential no-ops still skip render.
    pub fn dispatch_scheduled(
        &mut self,
        handle: &ComponentEventHandle<H>,
        value: &Value,
    ) -> Result<ScheduledComponentDispatch<H>, String> {
        self.transactional(|host| {
            bounded_json(value)?;
            let mount = host
                .mounts
                .get(&handle.mount)
                .ok_or("retired component event")?;
            if handle.runtime != host.id
                || mount.generation != handle.generation
                || mount.reference.owner != handle.owner
            {
                return Err("foreign or stale component event".into());
            }
            host.render_mount_scheduled(handle.mount, serde_json::json!([[handle.action, value]]))
        })
    }

    /// Accept a production renderer's tree validation before committing hooks,
    /// handlers or effects. Rejection rolls back the existing render transaction.
    pub fn render_validated(
        &mut self,
        mount: &ComponentMount,
        props: &Value,
        validate: impl FnOnce(&Value) -> Result<(), String>,
    ) -> Result<RenderedComponent<H>, String> {
        self.transactional(|host| host.render_validated_inner(mount, props, validate))
    }

    fn render_validated_inner(
        &mut self,
        mount: &ComponentMount,
        props: &Value,
        validate: impl FnOnce(&Value) -> Result<(), String>,
    ) -> Result<RenderedComponent<H>, String> {
        bounded_json(props)?;
        if !props.is_object() {
            return Err("component props must be an object".into());
        }
        self.validate_mount(mount)?;
        let previous_props = std::mem::replace(
            &mut self.mounts.get_mut(&mount.id).unwrap().props,
            props.clone(),
        );
        let result = self.render_mount(mount.id, None, validate);
        if result.is_err()
            && let Some(state) = self.mounts.get_mut(&mount.id)
        {
            state.props = previous_props;
        }
        result
    }

    pub fn dispatch_validated(
        &mut self,
        handle: &ComponentEventHandle<H>,
        value: &Value,
        validate: impl FnOnce(&Value) -> Result<(), String>,
    ) -> Result<RenderedComponent<H>, String> {
        self.transactional(|host| host.dispatch_validated_inner(handle, value, validate))
    }

    fn dispatch_validated_inner(
        &mut self,
        handle: &ComponentEventHandle<H>,
        value: &Value,
        validate: impl FnOnce(&Value) -> Result<(), String>,
    ) -> Result<RenderedComponent<H>, String> {
        bounded_json(value)?;
        let mount = self
            .mounts
            .get(&handle.mount)
            .ok_or("retired component event")?;
        if handle.runtime != self.id
            || mount.generation != handle.generation
            || mount.reference.owner != handle.owner
        {
            return Err("foreign or stale component event".into());
        }
        self.render_mount(
            handle.mount,
            Some(serde_json::json!([[handle.action, value]])),
            validate,
        )
    }

    /// Expand public component mount requests through their owning contexts.
    /// Data props cross as bounded snapshots. Void callable props are routed
    /// through Rust-owned grants back to their originating surface/owner.
    pub fn render_expanded(
        &mut self,
        mount: &ComponentMount,
        props: &Value,
        validate: impl FnOnce(&Value) -> Result<(), String>,
    ) -> Result<RenderedComponent<H>, String> {
        self.render_expanded_validated(mount, props, validate)
            .map(|(rendered, ())| rendered)
    }

    pub fn render_expanded_validated<T>(
        &mut self,
        mount: &ComponentMount,
        props: &Value,
        validate: impl FnOnce(&Value) -> Result<T, String>,
    ) -> Result<(RenderedComponent<H>, T), String> {
        self.render_expanded_validated_with_generation(mount, props, |value, _| validate(value))
    }

    pub fn render_expanded_validated_with_generation<T>(
        &mut self,
        mount: &ComponentMount,
        props: &Value,
        validate: impl FnOnce(&Value, u64) -> Result<T, String>,
    ) -> Result<(RenderedComponent<H>, T), String> {
        self.transactional(|host| {
            host.validate_mount(mount)?;
            if let Some(surface) = host.mounts[&mount.id].surface.clone() {
                for package in host.packages.values_mut() {
                    if package.data.get("surface") == Some(&surface) {
                        continue;
                    }
                    std::rc::Rc::make_mut(&mut package.data)
                        .as_object_mut()
                        .ok_or("package snapshot must be an object")?
                        .insert("surface".into(), surface.clone());
                }
            }
            let rendered = host.render(mount, props)?;
            host.expand_rendered(mount.id, rendered, validate)
        })
    }

    pub fn dispatch_expanded(
        &mut self,
        root: &ComponentMount,
        handle: &ComponentEventHandle<H>,
        value: &Value,
        validate: impl FnOnce(&Value) -> Result<(), String>,
    ) -> Result<RenderedComponent<H>, String> {
        self.transactional(|host| {
            host.validate_mount(root)?;
            host.dispatch(handle, value)?;
            let props = host.mounts[&root.id].props.clone();
            host.render_expanded(root, &props, validate)
        })
    }

    /// Leave hooks/handlers/effects provisional until the native adapter has
    /// approved the entire effect batch. The adapter must finish_transaction.
    pub fn dispatch_expanded_pending(
        &mut self,
        root: &ComponentMount,
        handle: &ComponentEventHandle<H>,
        value: &Value,
        validate: impl FnOnce(&Value) -> Result<(), String>,
    ) -> Result<RenderedComponent<H>, String> {
        self.begin_transaction()?;
        let result = self.dispatch_expanded(root, handle, value, validate);
        if result.is_err() {
            self.finish_transaction(false)?;
        }
        result
    }

    /// Dispatch every handler emitted by one native UI transition against the
    /// render generation that produced it, then rebuild the expanded root once.
    /// Handles retain their package owner and mount, so a transition containing
    /// both a blur callback and a click cannot cross an ownership boundary.
    pub fn dispatch_expanded_batch_pending(
        &mut self,
        root: &ComponentMount,
        events: &[(ComponentEventHandle<H>, Value)],
        validate: impl FnOnce(&Value) -> Result<(), String>,
    ) -> Result<RenderedComponent<H>, String> {
        self.dispatch_expanded_batch_pending_validated(root, events, validate)
            .map(|(rendered, ())| rendered)
    }

    pub fn dispatch_expanded_batch_pending_validated<T>(
        &mut self,
        root: &ComponentMount,
        events: &[(ComponentEventHandle<H>, Value)],
        validate: impl FnOnce(&Value) -> Result<T, String>,
    ) -> Result<(RenderedComponent<H>, T), String> {
        self.begin_transaction()?;
        let result = (|| {
            self.validate_mount(root)?;
            for (handle, value) in events {
                bounded_json(value)?;
                let mount = self
                    .mounts
                    .get(&handle.mount)
                    .ok_or("retired component event")?;
                if handle.runtime != self.id
                    || mount.generation != handle.generation
                    || mount.reference.owner != handle.owner
                {
                    return Err("foreign or stale component event".into());
                }
            }

            let mut offset = 0;
            while offset < events.len() {
                let mount = events[offset].0.mount;
                let mut end = offset + 1;
                while end < events.len() && events[end].0.mount == mount {
                    end += 1;
                }
                let batch = events[offset..end]
                    .iter()
                    .map(|(handle, value)| serde_json::json!([handle.action, value]))
                    .collect::<Vec<_>>();
                self.render_mount(mount, Some(Value::Array(batch)), |_| Ok(()))?;
                offset = end;
            }

            let props = self.mounts[&root.id].props.clone();
            self.render_expanded_validated(root, &props, validate)
        })();
        if result.is_err() {
            self.finish_transaction(false)?;
        }
        result
    }

    /// Scheduled production bridge. A batch whose handlers do not change any
    /// hook value retains the caller's accepted expanded tree and event table.
    /// Every mount in an admitted expanded tree must retain component-owned
    /// typed patch authority. Initial and newly discovered boundary admission
    /// happens through `render_expanded`; losing authority afterwards is a
    /// contained error rather than permission to fall back to a complete tree.
    pub fn dispatch_expanded_batch_scheduled_pending_validated<T>(
        &mut self,
        root: &ComponentMount,
        events: &[(ComponentEventHandle<H>, Value)],
        accepted_events: &BTreeMap<u64, ComponentEventHandle<H>>,
        accepted_source: &Value,
        mut validate: impl FnMut(
            &NativePatchEnvelope,
            &BTreeMap<u64, ComponentEventHandle<H>>,
            u64,
        ) -> Result<T, String>,
    ) -> Result<ScheduledExpandedBatch<H, T>, String> {
        self.dispatch_expanded_batch_scheduled_pending_with_validator(
            root,
            events,
            accepted_events,
            accepted_source,
            &mut validate,
            true,
        )
    }

    /// Atomically publish one owner's host snapshot and reconcile only the
    /// mounted components whose versioned selectors observed a change. The
    /// accepted expanded tree remains native-owned and crosses this boundary
    /// solely as a typed patch; rejecting that patch restores both the package
    /// stores and the host snapshot.
    pub fn update_snapshot_and_reconcile_expanded_pending_validated<T>(
        &mut self,
        owner: &H::Owner,
        data: &Value,
        root: &ComponentMount,
        accepted_events: &BTreeMap<u64, ComponentEventHandle<H>>,
        accepted_source: &Value,
        mut validate: impl FnMut(
            &NativePatchEnvelope,
            &BTreeMap<u64, ComponentEventHandle<H>>,
            u64,
        ) -> Result<T, String>,
    ) -> Result<ScheduledExpandedBatch<H, T>, String> {
        self.begin_transaction()?;
        if let Err(error) = self.update_snapshot(owner, data) {
            self.finish_transaction(false)?;
            return Err(error);
        }
        self.dispatch_expanded_batch_scheduled_pending_with_validator(
            root,
            &[],
            accepted_events,
            accepted_source,
            &mut validate,
            false,
        )
    }

    fn dispatch_expanded_batch_scheduled_pending_with_validator<T>(
        &mut self,
        root: &ComponentMount,
        events: &[(ComponentEventHandle<H>, Value)],
        accepted_events: &BTreeMap<u64, ComponentEventHandle<H>>,
        accepted_source: &Value,
        validate: &mut NativePatchValidator<'_, H, T>,
        begin_transaction: bool,
    ) -> Result<ScheduledExpandedBatch<H, T>, String> {
        if begin_transaction {
            self.begin_transaction()?;
        } else if !self.transaction_pending() {
            return Err("composition transaction is unavailable".into());
        }
        let mut rejected_native = None;
        let result = (|| {
            self.validate_mount(root)?;
            let owned_mounts = std::iter::once(root.id)
                .chain(
                    self.nested_mounts
                        .iter()
                        .filter(|((owner, _), _)| *owner == root.id)
                        .map(|(_, mount)| mount.id),
                )
                .collect::<std::collections::BTreeSet<_>>();
            if let Some(mount) = owned_mounts
                .iter()
                .find(|mount| !self.patch_authority.contains_key(mount))
            {
                return Err(format!(
                    "admitted composition mount {mount} lost native patch authority"
                ));
            }
            for (handle, value) in events {
                bounded_json(value)?;
                let mount = self
                    .mounts
                    .get(&handle.mount)
                    .ok_or("retired component event")?;
                if handle.runtime != self.id
                    || !owned_mounts.contains(&handle.mount)
                    || mount.generation != handle.generation
                    || mount.reference.owner != handle.owner
                {
                    return Err("foreign or stale component event".into());
                }
            }

            let generations_before = self
                .mounts
                .iter()
                .filter(|(id, _)| owned_mounts.contains(id))
                .map(|(id, mount)| (*id, mount.generation))
                .collect::<BTreeMap<_, _>>();
            let mut rendered_mounts = BTreeMap::new();
            let mut patched_mounts = BTreeMap::new();
            let mut dirty_components = BTreeMap::new();
            let mut reconciliation_requested = false;
            if events.is_empty() {
                let mount_ids = owned_mounts.iter().copied().collect::<Vec<_>>();
                for mount in mount_ids {
                    let outcome = self.render_mount_patched(mount, Vec::new())?;
                    dirty_components.insert(mount, outcome.dirty_components);
                    if let Some(patch) = outcome.patch {
                        patched_mounts.insert(mount, patch);
                    }
                    rendered_mounts.append(&mut self.scheduled_renders);
                    reconciliation_requested |= outcome.reconciliation_requested;
                }
            }
            let mut offset = 0;
            while offset < events.len() {
                let mount = events[offset].0.mount;
                let mut end = offset + 1;
                while end < events.len() && events[end].0.mount == mount {
                    end += 1;
                }
                let batch = events[offset..end]
                    .iter()
                    .map(|(handle, value)| serde_json::json!([handle.slot, value]))
                    .collect::<Vec<_>>();
                let outcome = self.render_mount_slots_patched(mount, batch)?;
                dirty_components.insert(mount, outcome.dirty_components);
                if let Some(patch) = outcome.patch {
                    patched_mounts.insert(mount, patch);
                }
                rendered_mounts.append(&mut self.scheduled_renders);
                reconciliation_requested |= outcome.reconciliation_requested;
                offset = end;
            }

            let changed = self
                .mounts
                .iter()
                .filter(|(id, _)| owned_mounts.contains(id))
                .filter(|(id, mount)| generations_before.get(id) != Some(&mount.generation))
                .map(|(id, _)| *id)
                .collect::<Vec<_>>();
            if changed.is_empty() {
                return Ok(ScheduledExpandedBatch::Unchanged);
            }
            let dirty = changed
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>();
            let mut boundaries = changed
                .iter()
                .map(|mount_id| {
                    if *mount_id == root.id {
                        return Ok(("root".to_owned(), *mount_id, false));
                    }
                    self.nested_mounts
                        .iter()
                        .find(|((owner, _), mount)| *owner == root.id && mount.id == *mount_id)
                        .map(|((_, path), _)| (path.clone(), *mount_id, true))
                        .ok_or("dirty composition mount has no ownership boundary")
                })
                .collect::<Result<Vec<_>, _>>()?;
            boundaries.sort_by(|left, right| left.0.cmp(&right.0));
            boundaries = boundaries
                .clone()
                .into_iter()
                .filter(|candidate| {
                    !boundaries.iter().any(|ancestor| {
                        ancestor.0 != candidate.0
                            && candidate
                                .0
                                .strip_prefix(&ancestor.0)
                                .is_some_and(|suffix| suffix.starts_with('/'))
                    })
                })
                .collect();
            let mut retained_events = accepted_events
                .iter()
                .filter(|(_, handle)| self.mounts.contains_key(&handle.mount))
                .map(|(token, handle)| (*token, handle.clone()))
                .collect::<BTreeMap<_, _>>();
            for handle in retained_events.values_mut() {
                if patched_mounts.contains_key(&handle.mount) {
                    handle.generation = self.mounts[&handle.mount].generation;
                }
            }
            let next_event = retained_events
                .last_key_value()
                .map_or(0, |(token, _)| token.saturating_add(1));
            let mut expansion = ExpansionState::<H> {
                root: root.id,
                events: std::mem::take(&mut retained_events),
                visited: std::collections::BTreeSet::new(),
                native_ids: std::collections::BTreeSet::new(),
                handler_slots: std::collections::BTreeSet::new(),
                next_event,
            };
            let mut patch_expansion = Some(PatchExpansion::<H> {
                retained: accepted_source.clone(),
                dirty,
                rendered: rendered_mounts,
            });
            let mut operations = Vec::with_capacity(boundaries.len());
            let mut patch_nodes_visited = 0;
            let mut local_materializations = 0u64;
            let mut expansion_nodes = 0u64;
            let mut tree_bytes = 0u64;
            let mut generation = 0;
            let mut rebuilt_boundaries = std::collections::BTreeSet::new();
            for (boundary, mount_id, namespace) in &boundaries {
                if let Some(mut local_patch) = patched_mounts.remove(mount_id) {
                    generation = generation.max(self.mounts[mount_id].generation);
                    let structurally_expanded = self.expand_structural_patch_payloads(
                        root.id,
                        *mount_id,
                        boundary,
                        &mut local_patch,
                        &mut expansion,
                    )?;
                    let patch_owner = self.mounts[mount_id].reference.owner.clone();
                    let asset_aliases = &self.packages[&patch_owner].assets;
                    translate_package_patch(
                        local_patch,
                        if *namespace {
                            Some(boundary.as_str())
                        } else {
                            None
                        },
                        *mount_id,
                        &patch_owner,
                        self.id,
                        self.mounts[mount_id].generation,
                        &mut self.patch_authority,
                        &mut expansion,
                        &mut operations,
                        &mut patch_nodes_visited,
                        structurally_expanded,
                        asset_aliases,
                    )?;
                    continue;
                }
                local_materializations = local_materializations.saturating_add(1);
                let rendered = patch_expansion
                    .as_mut()
                    .unwrap()
                    .rendered
                    .remove(mount_id)
                    .ok_or("dirty composition mount omitted its scheduled render")?;
                tree_bytes = tree_bytes.saturating_add(
                    serde_json::to_vec(&rendered.node)
                        .map_err(|error| error.to_string())?
                        .len() as u64,
                );
                generation = generation.max(rendered.generation);
                let mut replacement = rendered.node;
                if *namespace {
                    namespace_native_metadata(&mut replacement, boundary, 0)?;
                }
                // This is a complete owner render, not a local patch. Rebuild
                // its live event set while retaining the slot-to-token index
                // long enough to preserve surviving native action identities.
                expansion
                    .events
                    .retain(|_, handle| handle.mount != *mount_id);
                replacement = self.expand_node(
                    boundary,
                    replacement,
                    &rendered.events,
                    *mount_id,
                    &mut expansion,
                    0,
                    &mut patch_expansion,
                )?;
                if let Some(authority) = self.patch_authority.get_mut(mount_id) {
                    authority.slots.retain(|slot, token| {
                        expansion
                            .events
                            .get(token)
                            .is_some_and(|handle| handle.mount == *mount_id && handle.slot == *slot)
                    });
                }
                rebuilt_boundaries.insert(boundary.clone());
                let target = replacement
                    .get("__nativeId")
                    .and_then(Value::as_str)
                    .ok_or("composition ownership boundary has no native identity")?
                    .to_owned();
                let previous = find_native_id(accepted_source, &target)
                    .ok_or("accepted ownership boundary target disappeared")?;
                diff_native_subtree(
                    previous,
                    &replacement,
                    &mut operations,
                    &mut patch_nodes_visited,
                )?;
                expansion_nodes = patch_nodes_visited;
            }
            // Expanded handler tokens are stable host authority. Both direct
            // patches and complete callback renders update their handle table;
            // the renderer does not need a slot mutation for the same token.
            operations.retain(|operation| {
                !matches!(operation, NativePatchOperation::ReplaceHandlerSlot { .. })
            });
            operations = normalize_atomic_operations(operations);
            let nodes_mutated = operations.len() as u64;
            let patch = NativePatchEnvelope {
                version: 1,
                operations,
                counters: NativePatchCounters {
                    nodes_visited: patch_nodes_visited,
                    nodes_mutated,
                    local_materializations,
                    expansion_nodes,
                    tree_bytes,
                },
            };
            // Expanding a dirty parent may re-render an owned child after the
            // initial dirty-generation scan (for example, transported native
            // children or callback props). Stable handler tokens remain native
            // authority, but their host handles must follow the mount's newly
            // provisional generation before this batch can be admitted.
            for handle in expansion.events.values_mut() {
                if let Some(mount) = self.mounts.get(&handle.mount) {
                    handle.generation = mount.generation;
                }
            }
            let application_started = Instant::now();
            let validation = validate(&patch, &expansion.events, generation);
            let application_micros = application_started
                .elapsed()
                .as_micros()
                .min(u128::from(u64::MAX)) as u64;
            self.report_mount_patch_application(root.id, application_micros, validation.is_ok())?;
            let validated = match validation {
                Ok(validated) => validated,
                Err(error) => {
                    rejected_native = Some(NativeRejection {
                        owners: boundaries
                            .iter()
                            .map(|(_, mount, _)| {
                                (*mount, dirty_components.remove(mount).unwrap_or_default())
                            })
                            .collect(),
                        message: error.clone(),
                    });
                    return Err(error);
                }
            };
            let removed = self
                .nested_mounts
                .keys()
                .filter(|(owner, path)| {
                    *owner == root.id
                        && rebuilt_boundaries.iter().any(|boundary| {
                            path != boundary
                                && path
                                    .strip_prefix(boundary)
                                    .is_some_and(|suffix| suffix.starts_with('/'))
                        })
                        && !expansion.visited.contains(path)
                })
                .cloned()
                .collect::<Vec<_>>();
            for key in removed {
                let mount = self.nested_mounts.remove(&key).unwrap();
                self.unmount(&mount)?;
            }
            Ok(ScheduledExpandedBatch::Patched {
                patch,
                events: expansion.events,
                generation,
                validated,
                reconciliation_requested,
            })
        })();
        if result.is_err() {
            self.finish_transaction(false)?;
            if let Some(rejection) = rejected_native {
                let mut captured = false;
                for (mount, component_owners) in rejection.owners {
                    let Some(state) = self.mounts.get(&mount) else {
                        continue;
                    };
                    let package = self
                        .packages
                        .get_mut(&state.reference.owner)
                        .ok_or("retired component owner")?;
                    let mut runtime = package.runtime.borrow_mut();
                    runtime.select_surface(&surface(mount))?;
                    captured |= !runtime
                        .capture_native_failure(&component_owners, &rejection.message)?
                        .is_empty();
                }
                if captured {
                    return self.dispatch_expanded_batch_scheduled_pending_with_validator(
                        root,
                        &[],
                        accepted_events,
                        accepted_source,
                        validate,
                        true,
                    );
                }
            }
        }
        result
    }

    fn report_mount_patch_application(
        &mut self,
        mount: u64,
        application_micros: u64,
        accepted: bool,
    ) -> Result<(), String> {
        let owner = self
            .mounts
            .get(&mount)
            .ok_or("retired component mount")?
            .reference
            .owner
            .clone();
        let package = self
            .packages
            .get_mut(&owner)
            .ok_or("retired component owner")?;
        let mut runtime = package.runtime.borrow_mut();
        runtime.select_surface(&surface(mount))?;
        runtime.report_typed_patch_apply(application_micros, accepted)
    }

    /// Reconcile state queued by passive effects after the preceding committed
    /// render. The empty event batch is intentional: it lets the owning mount's
    /// scheduler consume all queued hook updates once, then expands the root
    /// with the same generation and ownership checks as an input dispatch.
    pub fn reconcile_expanded_pending_validated<T>(
        &mut self,
        root: &ComponentMount,
        accepted_events: &BTreeMap<u64, ComponentEventHandle<H>>,
        accepted_source: &Value,
        validate: impl FnMut(
            &NativePatchEnvelope,
            &BTreeMap<u64, ComponentEventHandle<H>>,
            u64,
        ) -> Result<T, String>,
    ) -> Result<ScheduledExpandedBatch<H, T>, String> {
        let mut requested = false;
        let owned_mounts = std::iter::once(root.id)
            .chain(
                self.nested_mounts
                    .iter()
                    .filter(|((owner, _), _)| *owner == root.id)
                    .map(|(_, mount)| mount.id),
            )
            .collect::<std::collections::BTreeSet<_>>();
        for mount_id in owned_mounts {
            let mount = self
                .mounts
                .get(&mount_id)
                .ok_or("retired component mount")?;
            let package = self
                .packages
                .get(&mount.reference.owner)
                .ok_or("retired component owner")?;
            let mut runtime = package.runtime.borrow_mut();
            runtime.select_surface(&surface(mount_id))?;
            requested |= runtime.reconciliation_requested()?;
        }
        if !requested {
            self.begin_transaction()?;
            return Ok(ScheduledExpandedBatch::Unchanged);
        }
        self.dispatch_expanded_batch_scheduled_pending_validated(
            root,
            &[],
            accepted_events,
            accepted_source,
            validate,
        )
    }

    fn expand_rendered<T>(
        &mut self,
        root: u64,
        rendered: RenderedComponent<H>,
        validate: impl FnOnce(&Value, u64) -> Result<T, String>,
    ) -> Result<(RenderedComponent<H>, T), String> {
        let generation = rendered.generation;
        self.patch_authority.entry(root).or_default().paths.clear();
        let mut expansion = ExpansionState::<H> {
            root,
            events: BTreeMap::new(),
            visited: std::collections::BTreeSet::new(),
            native_ids: std::collections::BTreeSet::new(),
            handler_slots: std::collections::BTreeSet::new(),
            next_event: 0,
        };
        let node = self.expand_node(
            "root",
            rendered.node,
            &rendered.events,
            root,
            &mut expansion,
            0,
            &mut None,
        )?;
        bounded_json(&node)?;
        let validated = validate(&node, generation)?;
        let removed = self
            .nested_mounts
            .keys()
            .filter(|(owner, path)| *owner == root && !expansion.visited.contains(path))
            .cloned()
            .collect::<Vec<_>>();
        for key in removed {
            let mount = self.nested_mounts.remove(&key).unwrap();
            self.unmount(&mount)?;
        }
        self.rebuild_patch_authority(root, &node, &expansion.events)?;
        Ok((
            RenderedComponent::<H> {
                node,
                events: expansion.events,
                generation,
            },
            validated,
        ))
    }

    fn rebuild_patch_authority(
        &mut self,
        root: u64,
        node: &Value,
        events: &BTreeMap<u64, ComponentEventHandle<H>>,
    ) -> Result<(), String> {
        let prefixes = self
            .nested_mounts
            .iter()
            .filter(|((owner, _), _)| *owner == root)
            .map(|((_, path), mount)| (mount.id, format!("{path}::")))
            .collect::<BTreeMap<_, _>>();
        let mut authority = BTreeMap::<u64, MountPatchAuthority>::new();
        fn visit<H: CompositionHost>(
            value: &Value,
            events: &BTreeMap<u64, ComponentEventHandle<H>>,
            prefixes: &BTreeMap<u64, String>,
            authority: &mut BTreeMap<u64, MountPatchAuthority>,
        ) -> Result<(), String> {
            match value {
                Value::Object(object) => {
                    if let Some(slots) = object.get("__handlerSlots").and_then(Value::as_object) {
                        for (property, slot) in slots {
                            let token = object
                                .get(property)
                                .and_then(Value::as_u64)
                                .ok_or("accepted handler slot has no event token")?;
                            let handle = events
                                .get(&token)
                                .ok_or("accepted handler token has no authority")?;
                            let global_slot = slot
                                .as_str()
                                .ok_or("accepted handler slot is not a string")?;
                            let local_slot = prefixes
                                .get(&handle.mount)
                                .and_then(|prefix| global_slot.strip_prefix(prefix))
                                .unwrap_or(global_slot);
                            authority
                                .entry(handle.mount)
                                .or_default()
                                .slots
                                .insert(local_slot.to_owned(), token);
                        }
                    }
                    for value in object.values() {
                        visit(value, events, prefixes, authority)?;
                    }
                }
                Value::Array(values) => {
                    for value in values {
                        visit(value, events, prefixes, authority)?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        visit(node, events, &prefixes, &mut authority)?;
        // Reconstruct structural ownership from the accepted native tree.
        // A full render can replace a patch-created splice while retaining the
        // same nested mount. Keeping only the old incremental splice table
        // then leaves a later ReplaceSubtree unable to retire that mount.
        let nested_paths = self
            .nested_mounts
            .iter()
            .filter(|((owner, _), _)| *owner == root)
            .map(|((_, path), mount)| (path.clone(), mount.id))
            .collect::<Vec<_>>();
        fn record_containing_splices(
            value: &Value,
            prefix: &str,
            path: &str,
            splices: &mut BTreeMap<String, std::collections::BTreeSet<String>>,
        ) -> bool {
            let mut contains = value
                .get("__nativeId")
                .and_then(Value::as_str)
                .is_some_and(|id| id.starts_with(prefix));
            match value {
                Value::Object(object) => {
                    for child in object.values() {
                        contains |= record_containing_splices(child, prefix, path, splices);
                    }
                }
                Value::Array(values) => {
                    for child in values {
                        contains |= record_containing_splices(child, prefix, path, splices);
                    }
                }
                _ => {}
            }
            if contains && let Some(id) = value.get("__nativeId").and_then(Value::as_str) {
                splices
                    .entry(id.to_owned())
                    .or_default()
                    .insert(path.to_owned());
            }
            contains
        }
        fn owner_native_id(path: &str) -> Option<String> {
            let boundary = path.rsplit_once('/')?.0;
            let mut native = Vec::new();
            for segment in boundary.split('/') {
                if segment == "children" {
                    continue;
                }
                if let Some(encoded) = segment.strip_prefix('@') {
                    let bytes = encoded
                        .as_bytes()
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|pair| {
                            let text = std::str::from_utf8(pair).ok()?;
                            u8::from_str_radix(text, 16).ok()
                        })
                        .collect::<Option<Vec<_>>>()?;
                    let key = serde_json::from_slice::<Value>(&bytes).ok()?;
                    native.push(format!("@{}", key.as_str()?));
                } else {
                    native.push(segment.to_owned());
                }
            }
            Some(native.join("/"))
        }
        for (path, _) in &nested_paths {
            let prefix = format!("{path}::");
            let parent = nested_paths
                .iter()
                .filter(|(candidate, _)| {
                    path.strip_prefix(candidate)
                        .is_some_and(|suffix| suffix.starts_with('/'))
                })
                .max_by_key(|(candidate, _)| candidate.len())
                .map_or(root, |(_, mount)| *mount);
            record_containing_splices(
                node,
                &prefix,
                path,
                &mut authority.entry(parent).or_default().splices,
            );
            if let Some(identity) = owner_native_id(path) {
                authority
                    .entry(parent)
                    .or_default()
                    .splices
                    .entry(identity)
                    .or_default()
                    .insert(path.clone());
            }
        }
        // Patch identities belong to the unexpanded owner tree, so they need
        // not appear in the accepted expanded tree. Preserve those identities
        // while rebinding their paths to the currently mounted boundary with
        // the same public-component selection.
        if let Some(previous) = self.patch_authority.get(&root) {
            for (identity, old_paths) in &previous.splices {
                for old_path in old_paths {
                    let selection = old_path.rsplit('/').next().unwrap_or(old_path);
                    let mut matches = nested_paths
                        .iter()
                        .filter(|(path, _)| path.rsplit('/').next() == Some(selection));
                    if let Some((path, _)) = matches.next()
                        && matches.next().is_none()
                    {
                        authority
                            .entry(root)
                            .or_default()
                            .splices
                            .entry(identity.clone())
                            .or_default()
                            .insert(path.clone());
                    }
                }
            }
        }
        for mount in std::iter::once(root).chain(prefixes.keys().copied()) {
            let mut next = authority.remove(&mount).unwrap_or_default();
            if let Some(previous) = self.patch_authority.get_mut(&mount) {
                next.paths = std::mem::take(&mut previous.paths);
            }
            self.patch_authority.insert(mount, next);
        }
        Ok(())
    }

    fn expand_structural_patch_payloads(
        &mut self,
        root: u64,
        mount: u64,
        boundary: &str,
        patch: &mut NativePatchEnvelope,
        expansion: &mut ExpansionState<H>,
    ) -> Result<bool, String> {
        let retained_splices = self
            .patch_authority
            .get(&mount)
            .map(|authority| &authority.splices);
        let has_structural = patch.operations.iter().any(|operation| match operation {
            NativePatchOperation::InsertChild { node, .. } => contains_package_transport(node),
            NativePatchOperation::ReplaceSubtree { target, node } => {
                contains_package_transport(node)
                    || retained_splices.is_some_and(|splices| splices.contains_key(target))
            }
            NativePatchOperation::RemoveChild { child_id, .. } => {
                retained_splices.is_some_and(|splices| splices.contains_key(child_id))
            }
            _ => false,
        });
        if !has_structural {
            return Ok(false);
        }
        let mut source_events = expansion
            .events
            .values()
            .filter(|handle| handle.mount == mount)
            .map(|handle| (handle.action, handle.clone()))
            .collect::<BTreeMap<_, _>>();
        // Structural payloads contain package-local actions introduced by this
        // render, not just actions present in the previous expanded frame.
        // Bind those slots to the current native-owned mount before transporting
        // callback props into newly materialized package components.
        fn bind_payload_actions<H: CompositionHost>(
            value: &Value,
            template: &ComponentEventHandle<H>,
            events: &mut BTreeMap<u64, ComponentEventHandle<H>>,
            depth: usize,
        ) -> Result<(), String> {
            if depth > MAX_TREE_DEPTH {
                return Err("component tree exceeds depth limit".into());
            }
            match value {
                Value::Array(values) => {
                    for value in values {
                        bind_payload_actions(value, template, events, depth + 1)?;
                    }
                }
                Value::Object(object) => {
                    for (key, value) in object {
                        if key == "__handlerSlots" {
                            continue;
                        }
                        if is_action(key) && !value.is_null() {
                            let action = value
                                .as_u64()
                                .filter(|action| *action < MAX_HANDLERS as u64)
                                .ok_or("invalid component action")?;
                            let slot = object
                                .get("__handlerSlots")
                                .and_then(|slots| slots.get(key))
                                .and_then(Value::as_str)
                                .ok_or("component action has no stable handler slot")?;
                            let mut handle = template.clone();
                            handle.action = action;
                            handle.slot = slot.to_owned();
                            events.insert(action, handle);
                            if events.len() > MAX_HANDLERS {
                                return Err("component tree exceeds handler limit".into());
                            }
                        } else {
                            bind_payload_actions(value, template, events, depth + 1)?;
                        }
                    }
                }
                _ => {}
            }
            Ok(())
        }
        let state = &self.mounts[&mount];
        let template = ComponentEventHandle::<H> {
            runtime: self.id,
            mount,
            generation: state.generation,
            owner: state.reference.owner.clone(),
            action: 0,
            slot: String::new(),
        };
        for operation in &patch.operations {
            if let NativePatchOperation::InsertChild { node, .. }
            | NativePatchOperation::ReplaceSubtree { node, .. } = operation
            {
                bind_payload_actions(node, &template, &mut source_events, 0)?;
            }
        }
        // Retire the old subtree's slots before expansion installs replacement
        // bindings. Retiring them in translation afterwards would delete the
        // new callbacks as well, leaving live native nodes with dead tokens.
        for operation in &patch.operations {
            let target = match operation {
                NativePatchOperation::ReplaceSubtree { target, .. } => target,
                NativePatchOperation::RemoveChild { child_id, .. } => child_id,
                _ => continue,
            };
            if let Some(authority) = self.patch_authority.get_mut(&mount) {
                authority.slots.retain(|slot, token| {
                    let keep = !native_slot_in_subtree(slot, target);
                    if !keep {
                        expansion.events.remove(token);
                    }
                    keep
                });
            }
        }
        let mut expanded = false;
        for operation in &mut patch.operations {
            let (identity, node, is_insert, inserted_path) = match operation {
                NativePatchOperation::InsertChild {
                    parent,
                    child_id,
                    node,
                    index,
                    ..
                } => {
                    let path = self.patch_authority[&mount].paths.get(parent).map(|path| {
                        format!(
                            "{path}/children/{}",
                            composition_child_identity(node, *index)
                        )
                    });
                    (child_id.clone(), Some(node), true, path)
                }
                NativePatchOperation::ReplaceSubtree { target, node } => {
                    (target.clone(), Some(node), false, None)
                }
                NativePatchOperation::RemoveChild { child_id, .. } => {
                    if let Some(authority) = self.patch_authority.get_mut(&mount) {
                        authority.forget_paths(child_id);
                    }
                    if let Some(paths) = self
                        .patch_authority
                        .get_mut(&mount)
                        .and_then(|authority| authority.splices.remove(child_id))
                    {
                        for path in paths {
                            self.retire_composition_splice(root, &path)?;
                        }
                    }
                    continue;
                }
                _ => continue,
            };
            let Some(node) = node else { continue };
            let path = inserted_path
                .or_else(|| self.patch_authority[&mount].paths.get(&identity).cloned())
                .unwrap_or_else(|| format!("{boundary}/splice:{}", identity.replace('/', "%2f")));
            if is_insert && self.patch_authority[&mount].splices.contains_key(&identity) {
                return Err("duplicate retained composition splice".into());
            }
            self.patch_authority
                .get_mut(&mount)
                .unwrap()
                .forget_paths(&identity);
            let previous = if !is_insert {
                self.patch_authority
                    .get_mut(&mount)
                    .and_then(|authority| authority.splices.remove(&identity))
                    .unwrap_or_default()
            } else {
                Default::default()
            };
            self.patch_authority
                .entry(mount)
                .or_default()
                .splices
                .insert(identity, std::collections::BTreeSet::from([path.clone()]));
            if boundary != "root" {
                namespace_native_metadata(node, boundary, 0)?;
            }
            *node = self.expand_node(
                &path,
                std::mem::take(node),
                &source_events,
                mount,
                expansion,
                0,
                &mut None,
            )?;
            for old_path in previous {
                self.retire_unvisited_composition_splice(root, &old_path, &expansion.visited)?;
            }
            expanded = true;
        }
        Ok(expanded)
    }

    fn retire_composition_splice(&mut self, root: u64, path: &str) -> Result<(), String> {
        self.retire_unvisited_composition_splice(root, path, &Default::default())
    }

    fn retire_unvisited_composition_splice(
        &mut self,
        root: u64,
        path: &str,
        visited: &std::collections::BTreeSet<String>,
    ) -> Result<(), String> {
        let mut removed = self
            .nested_mounts
            .keys()
            .filter(|(owner, candidate)| {
                *owner == root
                    && !visited.contains(candidate)
                    && (candidate == path
                        || candidate
                            .strip_prefix(path)
                            .is_some_and(|suffix| suffix.starts_with('/')))
            })
            .cloned()
            .collect::<Vec<_>>();
        removed.sort_by_key(|entry| std::cmp::Reverse(entry.1.len()));
        for key in removed {
            if let Some(child) = self.nested_mounts.remove(&key) {
                self.unmount(&child)?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn expand_node(
        &mut self,
        path: &str,
        mut node: Value,
        source_events: &BTreeMap<u64, ComponentEventHandle<H>>,
        source_mount: u64,
        expansion: &mut ExpansionState<H>,
        depth: usize,
        patch: &mut Option<PatchExpansion<H>>,
    ) -> Result<Value, String> {
        if depth > 64 || expansion.events.len() > MAX_HANDLERS {
            return Err("composition expansion exceeds limits".into());
        }
        if let Some(id) = node.get("__nativeId").and_then(Value::as_str) {
            let local = id.rsplit_once("::").map_or(id, |(_, local)| local);
            let authority = self.patch_authority.entry(source_mount).or_default();
            authority.paths.insert(local.to_owned(), path.to_owned());
            if authority.paths.len() > MAX_COMPONENT_NODES {
                return Err("component path index exceeds node limit".into());
            }
        }
        if node.get("kind").and_then(Value::as_str) == Some("__packageChild") {
            let token = node
                .get("child")
                .and_then(Value::as_u64)
                .ok_or("invalid owned child token")?;
            let grant = self
                .children
                .get(&token)
                .ok_or("stale owned child")?
                .clone();
            if grant.receiver != source_mount
                || self.mounts[&source_mount].generation != grant.generation
                || self
                    .mounts
                    .get(&grant.source)
                    .is_none_or(|mount| mount.generation != grant.source_generation)
            {
                return Err("foreign or stale owned child".into());
            }
            let mut owned = grant.node;
            namespace_native_metadata(&mut owned, path, 0)?;
            return self.expand_node(
                path,
                owned,
                &grant.events,
                grant.source,
                expansion,
                depth + 1,
                patch,
            );
        }
        if node.get("kind").and_then(Value::as_str) == Some("__packageComponent") {
            let (selection, reference) =
                if let Some(contract) = node.get("contract").and_then(Value::as_str) {
                    (
                        format!("export:{contract}"),
                        self.component(contract)
                            .ok_or("unknown public component contract")?,
                    )
                } else if let Some(selection) =
                    self.extensions.resolve_registered(&self.packages, &node)?
                {
                    let incarnation = self
                        .packages
                        .get(&selection.owner)
                        .ok_or("retired registered component owner")?
                        .incarnation;
                    (
                        selection.key,
                        ComponentReference::<H> {
                            runtime: self.id,
                            owner: selection.owner,
                            implementation: selection.implementation,
                            incarnation,
                        },
                    )
                } else {
                    let key = node
                        .get("contribution")
                        .and_then(Value::as_str)
                        .ok_or("missing public component selection")?;
                    (
                        format!("contribution:{key}"),
                        self.contribution(key)
                            .ok_or("unknown public contribution")?,
                    )
                };
            let key = format!("{path}/{selection}");
            expansion.visited.insert(key.clone());
            let identity = (expansion.root, key.clone());
            let mount = if let Some(mount) = self.nested_mounts.get(&identity) {
                mount.clone()
            } else {
                let mount = self.mount(&reference)?;
                // Even a callback-free grouping component owns an admitted
                // native boundary. Structural insertion cannot rely on visiting
                // an action node to create its (possibly empty) patch index.
                self.patch_authority.entry(mount.id).or_default();
                self.nested_mounts.insert(identity, mount.clone());
                mount
            };
            // Public package components are ownership boundaries within the
            // same native surface. Give each nested mount the root mount's
            // current compositor authority so surface hooks never retain the
            // package snapshot that happened to exist when it was created.
            let root_surface = self.mounts[&expansion.root].surface.clone();
            let surface_changed = self.mounts[&mount.id].surface != root_surface;
            if surface_changed {
                self.mounts.get_mut(&mount.id).unwrap().surface = root_surface;
            }
            let props = node.get("props").ok_or("missing public component props")?;
            let can_reuse = patch.as_ref().is_some_and(|patch| {
                !surface_changed
                    && !patch.dirty.contains(&mount.id)
                    && !contains_owned_transport(props)
                    && self.mounts[&mount.id].props == *props
            });
            if can_reuse {
                let prefix = format!("{key}::");
                let retained =
                    find_unique_native_prefix(&patch.as_ref().unwrap().retained, &prefix)?
                        .ok_or("retained ownership boundary subtree disappeared")?
                        .clone();
                for (owner, retained_path) in self.nested_mounts.keys() {
                    if *owner == expansion.root
                        && (retained_path == &key
                            || retained_path
                                .strip_prefix(&key)
                                .is_some_and(|suffix| suffix.starts_with('/')))
                    {
                        expansion.visited.insert(retained_path.clone());
                    }
                }
                return Ok(retained);
            }
            self.callbacks.retain(|_, grant| grant.receiver != mount.id);
            self.children.retain(|_, grant| grant.receiver != mount.id);
            let props =
                self.transport_callback_props(props, source_events, &mount, source_mount, depth)?;
            let rendered = self.render(&mount, &props)?;
            self.patch_authority
                .entry(mount.id)
                .or_default()
                .paths
                .clear();
            // Replace this mount's event bindings, but preserve its hook state
            // and stable native slot tokens while expanding its new render.
            expansion
                .events
                .retain(|_, handle| handle.mount != mount.id);
            if let Some(patch) = patch {
                patch.rendered.remove(&mount.id);
            }
            let mut embedded = rendered.node;
            namespace_native_metadata(&mut embedded, &key, 0)?;
            return self.expand_node(
                &key,
                embedded,
                &rendered.events,
                mount.id,
                expansion,
                depth + 1,
                patch,
            );
        }
        if let Value::Object(object) = &mut node {
            validate_native_metadata(object, expansion)?;
            let owner = &self.mounts[&source_mount].reference.owner;
            for field in ["asset", "icon"] {
                if let Some(Value::String(name)) = object.get_mut(field) {
                    if name.starts_with("composition.asset.") {
                        return Err("reserved native asset reference".into());
                    }
                    if let Some(alias) = self.asset_key(owner, name) {
                        *name = alias.into();
                    }
                }
            }
        }
        match &mut node {
            Value::Array(values) => {
                for (index, value) in values.iter_mut().enumerate() {
                    let identity = composition_child_identity(value, index);
                    *value = self.expand_node(
                        &format!("{path}/{identity}"),
                        std::mem::take(value),
                        source_events,
                        source_mount,
                        expansion,
                        depth + 1,
                        patch,
                    )?;
                }
            }
            Value::Object(object) => {
                let retained_slots = object
                    .get("__handlerSlots")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                for (key, value) in object {
                    if key == "__handlerSlots" {
                        continue;
                    } else if is_action(key) && !value.is_null() {
                        let handle = source_events
                            .get(&value.as_u64().ok_or("invalid host event token")?)
                            .ok_or("unknown host event token")?
                            .clone();
                        let global_slot = retained_slots
                            .get(key)
                            .and_then(Value::as_str)
                            .ok_or("expanded action has no stable handler slot")?;
                        let prefix = self
                            .nested_mounts
                            .iter()
                            .find(|((owner, _), mount)| {
                                *owner == expansion.root && mount.id == source_mount
                            })
                            .map(|((_, path), _)| format!("{path}::"));
                        let local_slot = prefix
                            .as_deref()
                            .and_then(|prefix| global_slot.strip_prefix(prefix))
                            .unwrap_or(global_slot);
                        let retained_token = self
                            .patch_authority
                            .get(&source_mount)
                            .and_then(|authority| authority.slots.get(local_slot))
                            .copied()
                            .filter(|token| {
                                expansion.events.get(token).is_none_or(|previous| {
                                    previous.mount == source_mount && previous.slot == local_slot
                                })
                            });
                        let token = retained_token.unwrap_or(expansion.next_event);
                        expansion.next_event = expansion.next_event.max(
                            token
                                .checked_add(1)
                                .ok_or("expanded event identity exhausted")?,
                        );
                        expansion.events.insert(token, handle);
                        self.patch_authority
                            .entry(source_mount)
                            .or_default()
                            .slots
                            .insert(local_slot.to_owned(), token);
                        *value = Value::from(token);
                    } else {
                        *value = self.expand_node(
                            &format!("{path}/{key}"),
                            std::mem::take(value),
                            source_events,
                            source_mount,
                            expansion,
                            depth + 1,
                            patch,
                        )?;
                    }
                }
            }
            _ => {}
        }
        Ok(node)
    }

    fn transport_callback_props(
        &mut self,
        value: &Value,
        source_events: &BTreeMap<u64, ComponentEventHandle<H>>,
        receiver: &ComponentMount,
        source_mount: u64,
        depth: usize,
    ) -> Result<Value, String> {
        if depth > 64 {
            return Err("component prop depth exceeds limit".into());
        }
        match value {
            Value::Object(object) if object.contains_key("__ownedChild") => {
                if object.len() != 1 {
                    return Err("invalid owned child prop".into());
                }
                let node = object["__ownedChild"].clone();
                bounded_json(&node)?;
                if self.children.len() + self.callbacks.len() + self.children.len() >= MAX_HANDLERS
                    || self
                        .children
                        .values()
                        .map(|child| child.node.to_string().len())
                        .sum::<usize>()
                        + node.to_string().len()
                        > 4 * 1024 * 1024
                {
                    return Err("owned child props exceed limit".into());
                }
                self.next_callback = self
                    .next_callback
                    .checked_add(1)
                    .filter(|value| *value <= MAX_TRANSPORT_TOKEN)
                    .ok_or("child identity exhausted")?;
                let token = self.next_callback;
                self.children.insert(
                    token,
                    OwnedChildGrant::<H> {
                        receiver: receiver.id,
                        generation: self
                            .next_generation
                            .checked_add(1)
                            .ok_or("component generation exhausted")?,
                        source: source_mount,
                        source_generation: self.mounts[&source_mount].generation,
                        events: owned_child_events(&node, source_events)?,
                        node,
                    },
                );
                Ok(serde_json::json!({"__hostChild":token}))
            }
            Value::Object(object) if object.contains_key("__callbackAction") => {
                if object
                    .keys()
                    .any(|key| key != "__callbackAction" && key != "__handlerSlots")
                {
                    return Err("invalid callback prop".into());
                }
                let source = source_events
                    .get(
                        &object["__callbackAction"]
                            .as_u64()
                            .ok_or("invalid callback action")?,
                    )
                    .ok_or("unknown callback action")?
                    .clone();
                if self.callbacks.len() >= MAX_HANDLERS {
                    return Err("too many component callback props".into());
                }
                self.next_callback = self
                    .next_callback
                    .checked_add(1)
                    .filter(|value| *value <= MAX_TRANSPORT_TOKEN)
                    .ok_or("callback identity exhausted")?;
                let token = self.next_callback;
                self.callbacks.insert(
                    token,
                    CallbackGrant::<H> {
                        receiver: receiver.id,
                        generation: self
                            .next_generation
                            .checked_add(1)
                            .ok_or("component generation exhausted")?,
                        source,
                    },
                );
                Ok(serde_json::json!({"__hostCallback":token}))
            }
            Value::Object(object) => {
                if object.keys().any(|key| key.starts_with("__host")) {
                    return Err("forged callback prop".into());
                }
                object
                    .iter()
                    .map(|(key, value)| {
                        Ok((
                            key.clone(),
                            self.transport_callback_props(
                                value,
                                source_events,
                                receiver,
                                source_mount,
                                depth + 1,
                            )?,
                        ))
                    })
                    .collect::<Result<serde_json::Map<_, _>, String>>()
                    .map(Value::Object)
            }
            Value::Array(values) => values
                .iter()
                .map(|value| {
                    self.transport_callback_props(
                        value,
                        source_events,
                        receiver,
                        source_mount,
                        depth + 1,
                    )
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array),
            _ => Ok(value.clone()),
        }
    }

    /// Native resource alias for an asset declared by this exact live owner.
    pub fn asset_key(&self, owner: &H::Owner, name: &str) -> Option<&str> {
        self.packages
            .get(owner)?
            .assets
            .get(name)
            .map(String::as_str)
    }

    /// Return the host-owned data projection for an exact, live package owner.
    pub fn snapshot(&self, owner: &H::Owner) -> Result<&Value, String> {
        self.packages
            .get(owner)
            .map(|package| package.data.as_ref())
            .ok_or("retired snapshot owner".into())
    }

    pub fn update_snapshot(&mut self, owner: &H::Owner, data: &Value) -> Result<(), String> {
        bounded_json(data)?;
        if !data.is_object() {
            return Err("package snapshot must be an object".into());
        }
        let package = self
            .packages
            .get_mut(owner)
            .ok_or("retired snapshot owner")?;
        self.extensions.publish_snapshot(package, data)?;
        package.data = std::rc::Rc::new(data.clone());
        Ok(())
    }

    /// Check this immediately before the host validates and executes the effect.
    /// Outstanding envelopes from a retired or different host are stale.
    pub fn validate_effect(&self, effect: &OwnedComponentEffect<H>) -> Result<(), String> {
        if effect.runtime != self.id
            || self
                .packages
                .get(&effect.owner)
                .is_none_or(|p| p.incarnation != effect.incarnation)
        {
            return Err("foreign or retired component effect".into());
        }
        Ok(())
    }

    pub fn unmount(&mut self, mount: &ComponentMount) -> Result<(), String> {
        self.validate_mount(mount)?;
        let children = self
            .nested_mounts
            .keys()
            .filter(|(root, _)| *root == mount.id)
            .cloned()
            .collect::<Vec<_>>();
        for key in children {
            let child = self.nested_mounts.remove(&key).unwrap();
            self.unmount(&child)?;
        }
        let state = self.mounts.remove(&mount.id).unwrap();
        self.patch_authority.remove(&mount.id);
        self.callbacks
            .retain(|_, grant| grant.receiver != mount.id && grant.source.mount != mount.id);
        self.children
            .retain(|_, grant| grant.receiver != mount.id && grant.source != mount.id);
        self.packages
            .get_mut(&state.reference.owner)
            .unwrap()
            .runtime
            .borrow_mut()
            .drop_surface(&surface(mount.id))
    }

    /// Retirement invalidates all references/events and drops the entire owner
    /// context (hooks, handlers, subscriptions and executable registrations).
    pub fn retire(&mut self, owner: &H::Owner) {
        let mounts = self
            .mounts
            .iter()
            .filter(|(_, mount)| &mount.reference.owner == owner)
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in mounts {
            if self.mounts.contains_key(&id) {
                let _ = self.unmount(&ComponentMount {
                    runtime: self.id,
                    id,
                });
            }
        }
        self.packages.remove(owner);
        self.callbacks.retain(|_, grant| {
            &grant.source.owner != owner && self.mounts.contains_key(&grant.receiver)
        });
        self.children.retain(|_, grant| {
            self.mounts.contains_key(&grant.receiver) && self.mounts.contains_key(&grant.source)
        });
        self.nested_mounts
            .retain(|_, mount| self.mounts.contains_key(&mount.id));
        self.effects.retain(|effect| &effect.owner != owner);
        for entries in self.resolution.contributions.values_mut() {
            entries.retain(|entry| &entry.contributed_by != owner);
        }
        let _ = self.publish_contribution_catalog();
    }

    pub fn take_effects(&mut self) -> Vec<OwnedComponentEffect<H>> {
        // Once handed to native approval, a rejected batch is discarded rather
        // than resurrected from the checkpoint as a queued request.
        if let Some(checkpoint) = &mut self.checkpoint {
            checkpoint.effects.clear();
        }
        std::mem::take(&mut self.effects)
    }

    fn validate_mount(&self, mount: &ComponentMount) -> Result<(), String> {
        if mount.runtime != self.id || !self.mounts.contains_key(&mount.id) {
            return Err("foreign or retired component mount".into());
        }
        let reference = &self.mounts[&mount.id].reference;
        let package = self
            .packages
            .get(&reference.owner)
            .ok_or("retired component owner")?;
        self.extensions
            .validate_entry(package, &reference.implementation)?;
        Ok(())
    }

    fn render_mount(
        &mut self,
        id: u64,
        event: Option<Value>,
        validate: impl FnOnce(&Value) -> Result<(), String>,
    ) -> Result<RenderedComponent<H>, String> {
        let catalog = self.contribution_catalog();
        let state = &self.mounts[&id];
        let owner = state.reference.owner.clone();
        let surface_snapshot = state
            .surface
            .clone()
            .unwrap_or_else(|| serde_json::json!({}));
        let previous_generation = state.generation;
        let generation = self
            .next_generation
            .checked_add(1)
            .ok_or("component generation exhausted")?;
        self.next_generation = generation;
        let package = self
            .packages
            .get_mut(&owner)
            .ok_or("retired component owner")?;
        let mut runtime = package.runtime.borrow_mut();
        runtime.set_contribution_catalog(&catalog)?;
        runtime.select_surface(&surface(id))?;
        runtime.set_surface_store(&surface(id), &surface_snapshot)?;
        runtime.set_mount_data(&package.data, state.surface.as_ref(), &state.props)?;
        let result = runtime.render_native(event.as_ref(), |value| {
            bounded_json(value)?;
            let mut node = value.clone();
            let mut events = BTreeMap::new();
            rewrite_events(
                &mut node,
                self.id,
                id,
                generation,
                &owner,
                &mut events,
                &mut TreeBudget::default(),
                0,
            )?;
            validate(&node)?;
            Ok(RenderedComponent::<H> {
                node,
                events,
                generation,
            })
        });
        if event.is_some() {
            runtime.finish_event(result.is_ok())?;
        }
        let rendered = result?;
        drop(runtime);
        self.mounts.get_mut(&id).unwrap().generation = generation;
        self.drain_effects(&owner, id, previous_generation, event.is_some())?;
        Ok(rendered)
    }

    fn render_mount_scheduled(
        &mut self,
        id: u64,
        events: Value,
    ) -> Result<ScheduledComponentDispatch<H>, String> {
        let catalog = self.contribution_catalog();
        let state = &self.mounts[&id];
        let owner = state.reference.owner.clone();
        let surface_snapshot = state
            .surface
            .clone()
            .unwrap_or_else(|| serde_json::json!({}));
        let previous_generation = state.generation;
        let generation = self
            .next_generation
            .checked_add(1)
            .ok_or("component generation exhausted")?;
        let package = self
            .packages
            .get_mut(&owner)
            .ok_or("retired component owner")?;
        let mut runtime = package.runtime.borrow_mut();
        runtime.set_contribution_catalog(&catalog)?;
        runtime.select_surface(&surface(id))?;
        runtime.set_surface_store(&surface(id), &surface_snapshot)?;
        runtime.set_mount_data(&package.data, state.surface.as_ref(), &state.props)?;
        let expression = format!("__nickelDispatchBatchScheduled({events})");
        let outcome = runtime.dispatch_scheduled(&expression, |value| {
            bounded_json(value)?;
            let mut node = value.clone();
            let mut owned_events = BTreeMap::new();
            rewrite_events(
                &mut node,
                self.id,
                id,
                generation,
                &owner,
                &mut owned_events,
                &mut TreeBudget::default(),
                0,
            )?;
            Ok(RenderedComponent::<H> {
                node,
                events: owned_events,
                generation,
            })
        });
        runtime.finish_event(outcome.is_ok())?;
        let outcome = outcome?;
        drop(runtime);
        let (rendered, reconciliation_requested) = match outcome {
            ScheduledRender::Unchanged => (None, false),
            ScheduledRender::Rendered {
                value,
                reconciliation_requested,
                ..
            } => {
                self.next_generation = generation;
                self.mounts.get_mut(&id).unwrap().generation = generation;
                (Some(value), reconciliation_requested)
            }
        };
        let generation_before_effects = self.next_generation;
        self.drain_effects(&owner, id, previous_generation, true)?;
        if rendered.is_some() {
            self.advance_mount_generation_grants(id, previous_generation, generation);
        }
        Ok(ScheduledComponentDispatch::<H> {
            rendered,
            reconciliation_requested,
            requires_expansion: self.next_generation != generation_before_effects,
        })
    }

    fn render_mount_patched(
        &mut self,
        id: u64,
        events: Vec<Value>,
    ) -> Result<PatchedMountDispatch, String> {
        self.render_mount_patched_with(id, events, false)
    }

    fn render_mount_slots_patched(
        &mut self,
        id: u64,
        events: Vec<Value>,
    ) -> Result<PatchedMountDispatch, String> {
        self.render_mount_patched_with(id, events, true)
    }

    fn render_mount_patched_with(
        &mut self,
        id: u64,
        events: Vec<Value>,
        slots: bool,
    ) -> Result<PatchedMountDispatch, String> {
        let catalog = self.contribution_catalog();
        let state = &self.mounts[&id];
        let owner = state.reference.owner.clone();
        let surface_snapshot = state
            .surface
            .clone()
            .unwrap_or_else(|| serde_json::json!({}));
        let previous_generation = state.generation;
        let generation = self
            .next_generation
            .checked_add(1)
            .ok_or("component generation exhausted")?;
        let package = self
            .packages
            .get_mut(&owner)
            .ok_or("retired component owner")?;
        let mut runtime = package.runtime.borrow_mut();
        runtime.set_contribution_catalog(&catalog)?;
        runtime.select_surface(&surface(id))?;
        runtime.set_surface_store(&surface(id), &surface_snapshot)?;
        runtime.set_mount_data(&package.data, state.surface.as_ref(), &state.props)?;
        let outcome = if slots {
            runtime.dispatch_slots_patched(events)
        } else {
            runtime.dispatch_batch_patched(events, false)
        };
        // This accepts the package-local retained tree provisionally. The outer
        // composition checkpoint still restores it if native validation or any
        // sibling/callback render fails.
        runtime.finish_patch_render(outcome.is_ok())?;
        runtime.finish_event(outcome.is_ok())?;
        let outcome = outcome?;
        drop(runtime);
        let (patch, dirty_components, reconciliation_requested) = match outcome {
            ScheduledPatch::Unchanged => (None, Vec::new(), false),
            ScheduledPatch::Patched {
                patch,
                dirty_components,
                reconciliation_requested,
                ..
            } => {
                self.next_generation = generation;
                self.mounts.get_mut(&id).unwrap().generation = generation;
                (Some(patch), dirty_components, reconciliation_requested)
            }
        };
        self.drain_effects(&owner, id, previous_generation, true)?;
        if patch.is_some() {
            self.advance_mount_generation_grants(id, previous_generation, generation);
        }
        Ok(PatchedMountDispatch {
            patch,
            dirty_components,
            reconciliation_requested,
        })
    }

    fn drain_effects(
        &mut self,
        owner: &H::Owner,
        receiver: u64,
        generation: u64,
        from_event: bool,
    ) -> Result<(), String> {
        let effects = self
            .packages
            .get_mut(owner)
            .unwrap()
            .runtime
            .borrow_mut()
            .take_effects()?;
        if self.effects.len() + effects.len() > MAX_EFFECTS {
            return Err("too many composition effects".into());
        }
        let mut bytes = self
            .effects
            .iter()
            .map(|effect| effect.value.to_string().len())
            .sum::<usize>();
        for value in &effects {
            let size = value.to_string().len();
            if size > 64 * 1024 {
                return Err("composition effect exceeds size limit".into());
            }
            bytes = bytes
                .checked_add(size)
                .ok_or("composition effect size overflow")?;
            if bytes > 4 * 1024 * 1024 {
                return Err("composition effect queue exceeds size limit".into());
            }
        }
        let mut invocations: BTreeMap<u64, (ComponentEventHandle<H>, Vec<Value>)> = BTreeMap::new();
        for value in &effects {
            if value.get("type").and_then(Value::as_str) == Some("__compositionCallback") {
                if !from_event {
                    return Err("component callback invoked outside an event".into());
                }
                let token = value
                    .get("callback")
                    .and_then(Value::as_u64)
                    .ok_or("invalid callback token")?;
                let grant = self
                    .callbacks
                    .get(&token)
                    .ok_or("stale component callback")?;
                if grant.receiver != receiver || grant.generation != generation {
                    return Err("foreign component callback".into());
                }
                let args = value
                    .get("args")
                    .filter(|args| args.is_array())
                    .ok_or("invalid callback arguments")?;
                let source = &grant.source;
                let mount = self
                    .mounts
                    .get(&source.mount)
                    .ok_or("retired callback owner")?;
                if mount.generation != source.generation || mount.reference.owner != source.owner {
                    return Err("stale callback owner".into());
                }
                let group = invocations
                    .entry(source.mount)
                    .or_insert_with(|| (source.clone(), Vec::new()));
                group.1.push(serde_json::json!([source.action, args]));
            }
        }
        if effects.iter().any(|value| {
            value.get("type").and_then(Value::as_str) == Some("__compositionCallbackBoundary")
        }) && self.callback_depth == 0
        {
            return Err("unexpected component callback boundary".into());
        }
        if self.callback_depth >= 32 {
            return Err("component callback cycle exceeds limit".into());
        }
        self.callback_depth += 1;
        let result = (|| {
            let mut produced = BTreeMap::new();
            for (mount, (handle, events)) in invocations {
                if self
                    .mounts
                    .get(&mount)
                    .is_none_or(|state| state.generation != handle.generation)
                {
                    return Err("callback owner changed during invocation".into());
                }
                let count = events.len();
                let start = self.effects.len();
                let rendered = self.render_mount(mount, Some(Value::Array(events)), |_| Ok(()))?;
                self.scheduled_renders.insert(mount, rendered);
                let generated = self.effects.split_off(start);
                let mut groups = Vec::new();
                let mut current = Vec::new();
                for effect in generated {
                    if effect.value.get("type").and_then(Value::as_str)
                        == Some("__compositionCallbackBoundary")
                    {
                        groups.push(std::mem::take(&mut current));
                    } else {
                        current.push(effect);
                    }
                }
                if groups.len() != count {
                    return Err("invalid component callback boundaries".into());
                }
                if let Some(last) = groups.last_mut() {
                    last.extend(current);
                }
                produced.insert(mount, std::collections::VecDeque::from(groups));
            }
            for value in effects {
                if value.get("type").and_then(Value::as_str) == Some("__compositionCallback") {
                    let token = value["callback"].as_u64().unwrap();
                    let source_mount = self.callbacks[&token].source.mount;
                    self.effects.extend(
                        produced
                            .get_mut(&source_mount)
                            .and_then(|groups| groups.pop_front())
                            .ok_or("missing component callback effects")?,
                    );
                } else {
                    self.effects.push(OwnedComponentEffect::<H> {
                        runtime: self.id,
                        owner: owner.clone(),
                        incarnation: self.packages[owner].incarnation,
                        value,
                    });
                }
            }
            if self.effects.len() > MAX_EFFECTS
                || self
                    .effects
                    .iter()
                    .map(|effect| effect.value.to_string().len())
                    .sum::<usize>()
                    > 4 * 1024 * 1024
            {
                return Err("composition callback effects exceed queue limit".into());
            }
            Ok(())
        })();
        self.callback_depth -= 1;
        result
    }
}

fn owned_child_events<H: CompositionHost>(
    node: &Value,
    source: &BTreeMap<u64, ComponentEventHandle<H>>,
) -> Result<BTreeMap<u64, ComponentEventHandle<H>>, String> {
    fn collect(node: &Value, tokens: &mut std::collections::BTreeSet<u64>) {
        match node {
            Value::Object(object) => {
                for (key, value) in object {
                    if is_action(key) {
                        if let Some(token) = value.as_u64() {
                            tokens.insert(token);
                        }
                    } else {
                        collect(value, tokens);
                    }
                }
            }
            Value::Array(values) => {
                for value in values {
                    collect(value, tokens);
                }
            }
            _ => {}
        }
    }
    let mut tokens = std::collections::BTreeSet::new();
    collect(node, &mut tokens);
    tokens
        .into_iter()
        .map(|token| {
            Ok((
                token,
                source
                    .get(&token)
                    .ok_or("unknown owned child action")?
                    .clone(),
            ))
        })
        .collect()
}

fn contains_owned_transport(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            object.contains_key("__callbackAction")
                || object.contains_key("__ownedChild")
                || object.values().any(contains_owned_transport)
        }
        Value::Array(values) => values.iter().any(contains_owned_transport),
        _ => false,
    }
}

fn contains_package_transport(value: &Value) -> bool {
    if matches!(
        value.get("kind").and_then(Value::as_str),
        Some("__packageComponent" | "__packageChild")
    ) {
        return true;
    }
    match value {
        Value::Object(object) => object.values().any(contains_package_transport),
        Value::Array(values) => values.iter().any(contains_package_transport),
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn translate_package_patch<H: CompositionHost>(
    patch: NativePatchEnvelope,
    namespace: Option<&str>,
    mount: u64,
    owner: &H::Owner,
    runtime: u64,
    generation: u64,
    authorities: &mut BTreeMap<u64, MountPatchAuthority>,
    expansion: &mut ExpansionState<H>,
    output: &mut Vec<NativePatchOperation>,
    visited: &mut u64,
    payloads_expanded: bool,
    asset_aliases: &BTreeMap<String, String>,
) -> Result<(), String> {
    fn rewrite_asset_value(
        property: &str,
        value: &mut Value,
        asset_aliases: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        if matches!(property, "asset" | "icon")
            && let Value::String(name) = value
        {
            if name.starts_with("composition.asset.") {
                return Err("reserved native asset reference".into());
            }
            if let Some(alias) = asset_aliases.get(name) {
                *name = alias.clone();
            }
        }
        Ok(())
    }

    fn namespaced(namespace: Option<&str>, value: String) -> String {
        namespace.map_or(value.clone(), |prefix| format!("{prefix}::{value}"))
    }
    fn rewrite_payload<H: CompositionHost>(
        value: &mut Value,
        namespace: Option<&str>,
        mount: u64,
        owner: &H::Owner,
        runtime: u64,
        generation: u64,
        authority: &mut MountPatchAuthority,
        expansion: &mut ExpansionState<H>,
        asset_aliases: &BTreeMap<String, String>,
        depth: usize,
    ) -> Result<(), String> {
        if depth > MAX_TREE_DEPTH {
            return Err("native patch payload exceeds depth limit".into());
        }
        if matches!(
            value.get("kind").and_then(Value::as_str),
            Some("__packageComponent" | "__packageChild")
        ) {
            return Err("structural package transport requires a retained ownership splice".into());
        }
        match value {
            Value::Array(values) => {
                for value in values {
                    rewrite_payload(
                        value,
                        namespace,
                        mount,
                        owner,
                        runtime,
                        generation,
                        authority,
                        expansion,
                        asset_aliases,
                        depth + 1,
                    )?;
                }
            }
            Value::Object(object) => {
                let slots = object
                    .get("__handlerSlots")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                for (property, value) in object.iter_mut() {
                    if property == "__handlerSlots" {
                        continue;
                    }
                    if is_action(property) && !value.is_null() {
                        let action = value.as_u64().ok_or("invalid patched component action")?;
                        let slot = slots
                            .get(property)
                            .and_then(Value::as_str)
                            .ok_or("patched action has no stable handler slot")?
                            .to_owned();
                        let token = expansion.next_event;
                        expansion.next_event = token
                            .checked_add(1)
                            .ok_or("expanded event identity exhausted")?;
                        expansion.events.insert(
                            token,
                            ComponentEventHandle::<H> {
                                runtime,
                                mount,
                                generation,
                                action,
                                slot: slot.clone(),
                                owner: owner.clone(),
                            },
                        );
                        authority.slots.insert(slot, token);
                        *value = Value::from(token);
                    } else {
                        rewrite_asset_value(property, value, asset_aliases)?;
                        rewrite_payload(
                            value,
                            namespace,
                            mount,
                            owner,
                            runtime,
                            generation,
                            authority,
                            expansion,
                            asset_aliases,
                            depth + 1,
                        )?;
                    }
                }
                if let Some(prefix) = namespace {
                    if let Some(id) = object
                        .get_mut("__nativeId")
                        .and_then(|value| value.as_str())
                        .map(str::to_owned)
                    {
                        object.insert(
                            "__nativeId".into(),
                            Value::String(format!("{prefix}::{id}")),
                        );
                    }
                    if let Some(slots) = object
                        .get_mut("__handlerSlots")
                        .and_then(Value::as_object_mut)
                    {
                        for slot in slots.values_mut() {
                            let local = slot.as_str().ok_or("invalid handler slot")?;
                            *slot = Value::String(format!("{prefix}::{local}"));
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    let authority = authorities.entry(mount).or_default();
    for operation in patch.operations {
        *visited = visited.saturating_add(1);
        match operation {
            NativePatchOperation::SetPrimitive {
                target,
                property,
                mut value,
            } => {
                rewrite_asset_value(&property, &mut value, asset_aliases)?;
                output.push(NativePatchOperation::SetPrimitive {
                    target: namespaced(namespace, target),
                    property,
                    value,
                });
            }
            NativePatchOperation::ReplaceHandlerSlot { slot, action } => {
                let token = *authority
                    .slots
                    .get(&slot)
                    .ok_or("patched handler slot has no retained authority")?;
                expansion.events.insert(
                    token,
                    ComponentEventHandle::<H> {
                        runtime,
                        mount,
                        generation,
                        action: action as u64,
                        slot,
                        owner: owner.clone(),
                    },
                );
                // The renderer already addresses this stable expanded token.
                // Only its host-owned action authority changes.
            }
            NativePatchOperation::InsertChild {
                parent,
                key,
                child_id,
                index,
                mut node,
            } => {
                if !payloads_expanded {
                    let path = authority
                        .paths
                        .get(&parent)
                        .map(|path| {
                            format!(
                                "{path}/children/{}",
                                composition_child_identity(&node, index)
                            )
                        })
                        .unwrap_or_else(|| {
                            format!(
                                "{}/splice:{}",
                                namespace.unwrap_or("root"),
                                child_id.replace('/', "%2f")
                            )
                        });
                    authority.record_paths(&node, &path, 0)?;
                    rewrite_payload(
                        &mut node,
                        namespace,
                        mount,
                        owner,
                        runtime,
                        generation,
                        authority,
                        expansion,
                        asset_aliases,
                        0,
                    )?;
                }
                output.push(NativePatchOperation::InsertChild {
                    parent: namespaced(namespace, parent),
                    key,
                    child_id: namespaced(namespace, child_id),
                    index,
                    node,
                });
            }
            NativePatchOperation::RemoveChild {
                parent,
                key,
                child_id,
                index,
            } => {
                authority.forget_paths(&child_id);
                if !payloads_expanded {
                    authority.slots.retain(|slot, token| {
                        let keep = !native_slot_in_subtree(slot, &child_id);
                        if !keep {
                            expansion.events.remove(token);
                        }
                        keep
                    });
                }
                output.push(NativePatchOperation::RemoveChild {
                    parent: namespaced(namespace, parent),
                    key,
                    child_id: namespaced(namespace, child_id),
                    index,
                });
            }
            NativePatchOperation::MoveChild {
                parent,
                key,
                child_id,
                from,
                to,
            } => output.push(NativePatchOperation::MoveChild {
                parent: namespaced(namespace, parent),
                key,
                child_id: namespaced(namespace, child_id),
                from,
                to,
            }),
            NativePatchOperation::ReplaceSubtree { target, mut node } => {
                if !payloads_expanded {
                    let path = authority.paths.get(&target).cloned().unwrap_or_else(|| {
                        format!(
                            "{}/splice:{}",
                            namespace.unwrap_or("root"),
                            target.replace('/', "%2f")
                        )
                    });
                    authority.forget_paths(&target);
                    authority.record_paths(&node, &path, 0)?;
                    authority.slots.retain(|slot, token| {
                        let keep = !native_slot_in_subtree(slot, &target);
                        if !keep {
                            expansion.events.remove(token);
                        }
                        keep
                    });
                }
                if !payloads_expanded {
                    rewrite_payload(
                        &mut node,
                        namespace,
                        mount,
                        owner,
                        runtime,
                        generation,
                        authority,
                        expansion,
                        asset_aliases,
                        0,
                    )?;
                }
                output.push(NativePatchOperation::ReplaceSubtree {
                    target: namespaced(namespace, target),
                    node,
                });
            }
        }
    }
    Ok(())
}

fn surface(id: u64) -> String {
    format!("composition-{id}")
}
fn validate_native_metadata<H: CompositionHost>(
    object: &serde_json::Map<String, Value>,
    expansion: &mut ExpansionState<H>,
) -> Result<(), String> {
    let Some(id) = object.get("__nativeId") else {
        if object.contains_key("__handlerSlots") {
            return Err("native handler slots require a node identity".into());
        }
        return Ok(());
    };
    let id = id.as_str().ok_or("native node identity must be a string")?;
    if id.is_empty() || id.len() > 1024 || !expansion.native_ids.insert(id.to_owned()) {
        return Err("invalid or duplicate native node identity".into());
    }
    if let Some(slots) = object.get("__handlerSlots") {
        let slots = slots
            .as_object()
            .ok_or("native handler slots must be an object")?;
        for slot in slots.values() {
            let slot = slot
                .as_str()
                .ok_or("native handler slot must be a string")?;
            if slot.is_empty()
                || slot.len() > 1280
                || !slot.starts_with(id.split("::").next().unwrap_or(id))
                || !expansion.handler_slots.insert(slot.to_owned())
            {
                return Err("invalid or duplicate native handler slot".into());
            }
        }
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
fn rewrite_events<H: CompositionHost>(
    value: &mut Value,
    runtime: u64,
    mount: u64,
    generation: u64,
    owner: &H::Owner,
    events: &mut BTreeMap<u64, ComponentEventHandle<H>>,
    budget: &mut TreeBudget,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_TREE_DEPTH {
        return Err("component tree exceeds depth limit".into());
    }
    if value
        .as_object()
        .is_some_and(|object| object.get("kind").and_then(Value::as_str).is_some())
    {
        budget.nodes += 1;
    }
    if budget.nodes > MAX_COMPONENT_NODES {
        return Err("component tree exceeds node limit".into());
    }
    match value {
        Value::Array(values) => {
            for value in values {
                rewrite_events(
                    value,
                    runtime,
                    mount,
                    generation,
                    owner,
                    events,
                    budget,
                    depth + 1,
                )?;
            }
        }
        Value::Object(object) => {
            let slots = object
                .get("__handlerSlots")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            for (key, value) in object {
                if key == "__handlerSlots" {
                    continue;
                } else if is_action(key) && !value.is_null() {
                    let action = value
                        .as_u64()
                        .filter(|action| *action < MAX_HANDLERS as u64)
                        .ok_or("invalid component action")?;
                    let slot = slots
                        .get(key)
                        .and_then(Value::as_str)
                        .ok_or("component action has no stable handler slot")?
                        .to_owned();
                    if events.len() >= MAX_HANDLERS {
                        return Err("component tree exceeds handler limit".into());
                    }
                    let token = events.len() as u64;
                    events.insert(
                        token,
                        ComponentEventHandle::<H> {
                            runtime,
                            mount,
                            generation,
                            action,
                            slot,
                            owner: owner.clone(),
                        },
                    );
                    *value = Value::from(token);
                } else {
                    rewrite_events(
                        value,
                        runtime,
                        mount,
                        generation,
                        owner,
                        events,
                        budget,
                        depth + 1,
                    )?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

#[derive(Default)]
struct TreeBudget {
    nodes: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{JsxModuleGraph, ModuleSource};
    use serde_json::json;

    fn graph(source: &str) -> JsxModuleGraph {
        JsxModuleGraph::new(
            "main.js",
            [ModuleSource {
                path: "main.js",
                source,
            }],
        )
        .unwrap()
        .with_public_exports(&BTreeMap::from([("entry".into(), "main.js#View".into())]))
        .unwrap()
        .with_component_bridge(json!({}), json!({}))
    }

    fn context(source: &str) -> OwnerRuntime<OrdinaryCompositionHost> {
        let graph = graph(source);
        OwnerRuntime::admit(
            std::rc::Rc::new(std::cell::RefCell::new(
                JsxRuntime::new_modules(&graph, None).unwrap(),
            )),
            (),
            std::rc::Rc::new(json!({})),
            BTreeMap::from([("entry".into(), "entry".into())]),
            BTreeMap::new(),
            "fixture".into(),
        )
        .unwrap()
    }

    fn composed() -> CompositionRuntime<OrdinaryCompositionHost> {
        let parent = context(
            "globalThis.secret='parent'; export function View(){return h(Column,null,h(twinkle.component('widget'),{label:'child'}))}",
        );
        let child = context(
            "globalThis.secret='provider'; export function View({label}){const [count,setCount]=useState(0);return h(Button,{onClick:()=>{setCount(value=>value+1);twinkle.request({type:'selected',source:secret})}},label+':'+secret+':'+count)}",
        );
        CompositionRuntime::from_admitted_plan(
            AdmittedLinkPlan {
                exports: BTreeMap::from([
                    (
                        "root".into(),
                        AdmittedExport {
                            implemented_by: "parent".into(),
                            implementation: "entry".into(),
                        },
                    ),
                    (
                        "widget".into(),
                        AdmittedExport {
                            implemented_by: "provider".into(),
                            implementation: "entry".into(),
                        },
                    ),
                ]),
                contributions: BTreeMap::new(),
                owner_order: vec!["parent".into(), "provider".into()],
            },
            OrdinaryCompositionHost,
            BTreeMap::from([("parent".into(), parent), ("provider".into(), child)]),
        )
        .unwrap()
    }

    #[test]
    fn initialization_requests_keep_native_owner_and_require_an_unmounted_domain() {
        let mut host = composed();
        host.packages.insert("provider".into(), context(
            "twinkle.request({type:'initialized'}); export function View(){return h(Text,null,'ready')}"
        ));
        let owner = "provider".to_owned();
        host.collect_initial_owner_effects(&owner).unwrap();
        let effects = host.take_effects();
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].owner(), &owner);
        assert_eq!(effects[0].value()["type"], "initialized");
        host.validate_effect(&effects[0]).unwrap();
        host.mount(&host.component("widget").unwrap()).unwrap();
        assert!(host.collect_initial_owner_effects(&owner).is_err());
    }

    #[test]
    fn rejected_reload_preflight_keeps_accepted_events_and_provider_state_live() {
        let mut accepted = composed();
        let mount = accepted
            .mount(&accepted.component("root").unwrap())
            .unwrap();
        let initial = accepted
            .render_expanded(&mount, &json!({}), |_| Ok(()))
            .unwrap();
        let event = initial.events.values().next().unwrap().clone();
        let mut candidate = composed();
        let mut replacement =
            context("export function View(){throw new Error('rejected provider')}");
        replacement.source_digest = "changed".into();
        candidate.packages.insert("provider".into(), replacement);
        assert!(
            accepted
                .reload_admitted(candidate, &BTreeMap::new(), &BTreeMap::new())
                .is_err()
        );
        assert!(accepted.dispatch(&event, &Value::Null).is_ok());
        let updated = accepted
            .render_expanded(&mount, &json!({}), |_| Ok(()))
            .unwrap();
        assert!(updated.node.to_string().contains("provider:1"));
    }

    #[test]
    fn reload_changed_provider_updates_unchanged_root_and_retires_old_events() {
        let mut accepted = composed();
        let mount = accepted
            .mount(&accepted.component("root").unwrap())
            .unwrap();
        let rendered = accepted
            .render_expanded(&mount, &json!({}), |_| Ok(()))
            .unwrap();
        let stale = rendered.events.values().next().unwrap().clone();
        let reference = accepted.component("widget").unwrap();
        let mut candidate = composed();
        let mut replacement =
            context("export function View(){return h(Text,null,'replacement provider')}");
        replacement.source_digest = "changed".into();
        candidate.packages.insert("provider".into(), replacement);
        let result = accepted
            .reload_admitted(candidate, &BTreeMap::new(), &BTreeMap::new())
            .unwrap();
        assert_eq!(result.restarted_packages, vec!["provider"]);
        assert_eq!(result.patches.len(), 1);
        assert!(
            format!("{:?}", result.patches[0].patch.operations).contains("replacement provider")
        );
        assert!(accepted.dispatch(&stale, &Value::Null).is_err());
        assert!(accepted.mount(&reference).is_err());
        assert!(
            accepted
                .render_expanded(&mount, &json!({}), |_| Ok(()))
                .is_ok()
        );
    }

    #[test]
    fn compatible_reload_preserves_hooks_and_changes_implementation() {
        let mut accepted = composed();
        let mount = accepted
            .mount(&accepted.component("widget").unwrap())
            .unwrap();
        let rendered = accepted
            .render_expanded(&mount, &json!({"label":"child"}), |_| Ok(()))
            .unwrap();
        let event = rendered.events.values().next().unwrap().clone();
        accepted.dispatch(&event, &Value::Null).unwrap();
        let source = "export function View({label}){const [count,setCount]=useState(0);return h(Button,{onClick:()=>setCount(value=>value+1)},'updated:'+count)}";
        let mut candidate = composed();
        let mut replacement = context(source);
        replacement.source_digest = "changed".into();
        candidate.packages.insert("provider".into(), replacement);
        let signatures = BTreeMap::from([("main.js#View".into(), "state".into())]);
        let result = accepted
            .reload_admitted(
                candidate,
                &BTreeMap::from([("provider".into(), graph(source))]),
                &BTreeMap::from([(
                    "provider".into(),
                    PackageReloadSignatures {
                        previous: signatures.clone(),
                        replacement: signatures,
                    },
                )]),
            )
            .unwrap();
        assert_eq!(result.preserved_packages, vec!["provider"]);
        assert!(format!("{:?}", result.patches[0].patch.operations).contains("updated:1"));
        assert!(accepted.dispatch(&event, &Value::Null).is_err());
    }

    #[test]
    fn contributor_reimport_does_not_revive_retired_reference() {
        let mut host = composed();
        let owner = "provider".to_owned();
        let old = host.component("widget").unwrap();
        let retained = host.provider_context(&owner).unwrap();
        let plan = host.resolution.clone();
        host.retire(&owner);
        assert!(
            host.sync_admitted_contributors(plan, &BTreeMap::from([(owner.clone(), retained)]))
                .unwrap()
        );
        assert!(host.mount(&old).is_err());
        let fresh = host.component("widget").unwrap();
        assert!(host.mount(&fresh).is_ok());
    }

    #[test]
    fn contributor_sync_rejects_invalid_links_without_changing_live_catalog() {
        let mut host = composed();
        let original = host.resolution.clone();
        let mut invalid = original.clone();
        invalid.contributions.insert(
            "items".into(),
            vec![AdmittedContribution {
                contributed_by: "missing".into(),
                implementation: "entry".into(),
                key: "missing-item".into(),
                metadata: json!({}),
            }],
        );
        assert!(
            host.sync_admitted_contributors(invalid, &BTreeMap::new())
                .is_err()
        );
        assert_eq!(host.resolution, original);
        let mount = host.mount(&host.component("root").unwrap()).unwrap();
        assert!(host.render_expanded(&mount, &json!({}), |_| Ok(())).is_ok());
    }

    #[test]
    fn replacement_preflight_rejects_shared_execution_domains() {
        let accepted = composed();
        let mut candidate = CompositionRuntime::from_admitted_plan(
            accepted.resolution.clone(),
            OrdinaryCompositionHost,
            accepted.packages.clone(),
        )
        .unwrap();
        assert!(
            accepted
                .validate_replacement(&mut candidate)
                .unwrap_err()
                .contains("isolated execution domains")
        );
        assert_eq!(candidate.mount_count(), 0);
    }

    #[test]
    fn replacement_checks_changed_provider_without_mutating_accepted_root() {
        let mut accepted = composed();
        let mount = accepted
            .mount(&accepted.component("root").unwrap())
            .unwrap();
        let before = accepted
            .render_expanded(&mount, &json!({}), |_| Ok(()))
            .unwrap();
        let mut candidate = composed();
        candidate.packages.insert(
            "provider".into(),
            context("export function View(){throw new Error('broken replacement provider')}"),
        );
        assert!(accepted.validate_replacement(&mut candidate).is_err());
        assert_eq!(candidate.mount_count(), 0);
        assert!(candidate.take_effects().is_empty());
        let after = accepted
            .render_expanded(&mount, &json!({}), |_| Ok(()))
            .unwrap();
        assert_eq!(before.node, after.node);
    }

    #[test]
    fn ordinary_composition_retains_provider_context_and_effect_origin() {
        let mut host = composed();
        let mount = host.mount(&host.component("root").unwrap()).unwrap();
        let initial = host
            .render_expanded(&mount, &json!({}), |_| Ok(()))
            .unwrap();
        assert!(initial.node.to_string().contains("child:provider:0"));
        let event = initial.events.values().next().unwrap().clone();
        assert_eq!(event.owner(), "provider");
        let changed = host
            .dispatch_expanded(&mount, &event, &Value::Null, |_| Ok(()))
            .unwrap();
        assert!(changed.node.to_string().contains("child:provider:1"));
        let effects = host.take_effects();
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].owner(), "provider");
        assert_eq!(
            effects[0].value(),
            &json!({"type":"selected","source":"provider"})
        );
        host.validate_effect(&effects[0]).unwrap();
        host.unmount(&mount).unwrap();
        assert!(host.dispatch(&event, &Value::Null).is_err());
        host.retire(&"provider".into());
        assert!(host.validate_effect(&effects[0]).is_err());
    }

    #[test]
    fn rejected_cross_owner_native_commit_discards_state_and_effects() {
        let mut host = composed();
        let mount = host.mount(&host.component("root").unwrap()).unwrap();
        let initial = host
            .render_expanded(&mount, &json!({}), |_| Ok(()))
            .unwrap();
        let event = initial.events.values().next().unwrap().clone();
        assert!(
            host.dispatch_expanded(&mount, &event, &Value::Null, |_| Err(
                "native admission rejected".into()
            ))
            .is_err()
        );
        assert!(host.take_effects().is_empty());
        let restored = host
            .render_expanded(&mount, &json!({}), |_| Ok(()))
            .unwrap();
        assert!(restored.node.to_string().contains("child:provider:0"));
    }

    #[test]
    fn shared_provider_context_never_accepts_another_hosts_handle() {
        let mut first = composed();
        let retained = first.provider_context(&"provider".into()).unwrap();
        let mut second = CompositionRuntime::from_admitted_plan(
            AdmittedLinkPlan {
                exports: BTreeMap::from([(
                    "root".into(),
                    AdmittedExport {
                        implemented_by: "provider".into(),
                        implementation: "entry".into(),
                    },
                )]),
                contributions: BTreeMap::new(),
                owner_order: vec!["provider".into()],
            },
            OrdinaryCompositionHost,
            BTreeMap::from([("provider".into(), retained.retained_context())]),
        )
        .unwrap();
        let reference = first.component("widget").unwrap();
        assert!(second.mount(&reference).is_err());
        let mount = first.mount(&first.component("root").unwrap()).unwrap();
        let initial = first
            .render_expanded(&mount, &json!({}), |_| Ok(()))
            .unwrap();
        assert!(
            second
                .dispatch(initial.events.values().next().unwrap(), &Value::Null)
                .is_err()
        );
    }

    #[test]
    fn composition_identity_exhaustion_never_reuses_an_incarnation() {
        let counter = AtomicU64::new(u64::MAX - 1);
        assert_eq!(allocate_identity(&counter).unwrap(), u64::MAX - 1);
        assert_eq!(allocate_identity(&counter).unwrap(), u64::MAX);
        assert!(allocate_identity(&counter).is_err());
        assert!(allocate_identity(&counter).is_err());
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn two_owners_cannot_share_the_same_execution_domain() {
        let original = context("export function View(){return h(Text,null,'ok')}");
        let mut other = original.clone();
        other.incarnation = allocate_identity(&NEXT_RUNTIME).unwrap();
        assert!(
            CompositionRuntime::from_admitted_plan(
                AdmittedLinkPlan {
                    exports: BTreeMap::new(),
                    contributions: BTreeMap::new(),
                    owner_order: vec!["first".into(), "second".into()],
                },
                OrdinaryCompositionHost,
                BTreeMap::from([("first".into(), original), ("second".into(), other)])
            )
            .is_err()
        );
    }
    #[test]
    fn ordinary_contributions_use_only_host_admitted_metadata_and_provider_code() {
        let parent = context(
            "export function View(){return h(Row,null,...twinkle.contributions('widgets').map(entry=>h(entry.component,{key:entry.key,label:entry.id})))}",
        );
        let child = context("export function View({label}){return h(Text,null,label+':provider')}");
        let mut host = CompositionRuntime::from_admitted_plan(
            AdmittedLinkPlan {
                exports: BTreeMap::from([(
                    "root".into(),
                    AdmittedExport {
                        implemented_by: "parent".into(),
                        implementation: "entry".into(),
                    },
                )]),
                contributions: BTreeMap::from([(
                    "widgets".into(),
                    vec![AdmittedContribution {
                        contributed_by: "provider".into(),
                        implementation: "entry".into(),
                        key: "widgets/provider/extra".into(),
                        metadata: json!({"id":"extra"}),
                    }],
                )]),
                owner_order: vec!["parent".into(), "provider".into()],
            },
            OrdinaryCompositionHost,
            BTreeMap::from([("parent".into(), parent), ("provider".into(), child)]),
        )
        .unwrap();
        let mount = host.mount(&host.component("root").unwrap()).unwrap();
        let rendered = host
            .render_expanded(&mount, &json!({}), |_| Ok(()))
            .unwrap();
        assert!(rendered.node.to_string().contains("extra:provider"));
        assert!(host.take_effects().is_empty());
    }

    #[test]
    fn combined_contribution_metadata_is_bounded_before_catalog_cloning() {
        let provider = context("export function View(){return h(Text,null,'provider')}");
        let metadata = json!({"text":"x".repeat(twinkle_protocol::native_tree::MAX_JSON_BYTES/2)});
        let result = CompositionRuntime::from_admitted_plan(
            AdmittedLinkPlan {
                exports: BTreeMap::new(),
                owner_order: vec!["provider".into()],
                contributions: BTreeMap::from([(
                    "widgets".into(),
                    (0..2)
                        .map(|index| AdmittedContribution {
                            contributed_by: "provider".into(),
                            implementation: "entry".into(),
                            key: format!("widgets/{index}"),
                            metadata: metadata.clone(),
                        })
                        .collect(),
                )]),
            },
            OrdinaryCompositionHost,
            BTreeMap::from([("provider".into(), provider)]),
        );
        assert!(matches!(result, Err(error) if error=="composition catalog exceeds size limit"));
    }
    #[test]
    fn callback_transport_stops_before_javascript_can_round_an_identity() {
        let mut host = composed();
        let mount = host.mount(&host.component("root").unwrap()).unwrap();
        let rendered = host
            .render_expanded(&mount, &json!({}), |_| Ok(()))
            .unwrap();
        let (&action, event) = rendered.events.iter().next().unwrap();
        let source_mount = event.mount;
        host.next_callback = MAX_TRANSPORT_TOKEN - 1;
        let payload = json!({"__callbackAction":action});
        let admitted = host
            .transport_callback_props(&payload, &rendered.events, &mount, source_mount, 0)
            .unwrap();
        assert_eq!(admitted, json!({"__hostCallback":MAX_TRANSPORT_TOKEN}));
        let count = host.callbacks.len();
        assert!(
            host.transport_callback_props(&payload, &rendered.events, &mount, source_mount, 0)
                .is_err()
        );
        assert_eq!(host.next_callback, MAX_TRANSPORT_TOKEN);
        assert_eq!(host.callbacks.len(), count);
        assert!(host.take_effects().is_empty());
    }
}

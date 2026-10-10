# JSX API ownership

This inventory defines the boundary between Twinkle's generic JSX runtime and an
application host. The runtime supplies presentation and composition mechanics;
the host supplies its own schemas, authority and effects.

| Contract | Implementation owner | Boundary |
| --- | --- | --- |
| Native element/property vocabulary, style values and admission | `twinkle-presentation`, `twinkle` | Bounded tree requests; native layout, input and hit testing remain Rust-owned |
| `h`, Fragment, state/reducer/ref/context/memo/callback/effect hooks | `twinkle-jsx-runtime` bootstrap | Generic transaction, identity and lifecycle semantics |
| `twinkle.data`, `twinkle.request`, `twinkle.component` | Generic bootstrap | Ordinary host data, queued effect requests and admitted export lookup; no action dispatcher is installed implicitly |
| Locale, theme, reduced motion and surface hooks | Generic bootstrap | Effective presentation observations; no saved-preference mutation |
| `TwinkleStores`, `useSyncExternalStore`, `useHostCapability` | Generic bootstrap | Branded subscriptions, selected values, explicit host vocabulary and generation checks |
| Checkpoint traversal, native trees, hooks, subscriptions, effects and rollback | Generic bootstrap/runtime | Host domains participate through bounded capture/restore hooks; registration seals before package initialization |
| `HostBindings`, `DataPublisher`, projection reuse | Generic `bindings.rs`/runtime | Versioned immutable Rust-owned initialization; bounded names and duplicate rejection |
| Module normalization, limits, Oxc preparation/cache and relative linking | Generic `modules.rs` | Package discovery and admission policy belong to the host |
| Composition mounts, callback/child grants, effect origins and reconciliation | Generic `composition.rs` | The host admits link plans, resolves application identities and performs protected operations |
| Ambient declarations | `types/twinkle.d.ts` | Hosts compose application-specific capabilities, protocols and service declarations over the neutral vocabulary |
| Standalone TSX preview and controller exercise | Optional `twinkle-workbench` JSX preview and generic TSX/CSS fixture | Exercises native semantic, keyboard and controller navigation, keyed state, selected stores and rollback |

`JsxRuntime::register_component_mount` admits public exports and per-mount hooks.
A conformance test exercises two mounts sharing loaded code while retaining
independent state, native callback dispatch and state across reselection. Unknown
and empty selections are rejected.

The composition manager operates over `AdmittedLinkPlan`, `OwnerRuntime` and
explicit `CompositionHost` hooks. Its ordinary host uses unit metadata. Module
preparation supplies the generic component and contribution client without
assuming an application catalog or service schema.

`reload_admitted` receives an isolated admitted candidate, prepared module graphs
and compatibility signatures. It preflights complete root expansion, preserves
compatible hook contexts, replaces incompatible provider contexts, retires old
grants and emits native root replacement patches. Contributor synchronization
validates bounded link plans without executing guest code and gives re-imported
contexts fresh local authority incarnations.

Development reload is not an atomic transaction over arbitrary package globals.
If installation or native registration fails after preflight, accepted contexts
may already have changed. Hosts that require transactional reload must provide a
recovery protocol around these hooks.

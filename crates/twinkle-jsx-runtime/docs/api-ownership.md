# JSX API ownership during extraction

This inventory records implemented ownership and remaining boundaries. It does not certify extraction completion.

| Contract | Implementation owner | Boundary |
| --- | --- | --- |
| Native element/property vocabulary, style values and admission | `twinkle-presentation`, `twinkle` | Bounded tree requests; native layout, input and hit testing remain Rust-owned |
| `h`, Fragment, state/reducer/ref/context/memo/callback/effect hooks | `twinkle-jsx-runtime` bootstrap | Generic transaction, identity and lifecycle semantics |
| `twinkle.data`, `twinkle.request`, `twinkle.component` | Generic bootstrap | Ordinary host data, queued effect requests and admitted export lookup; no action dispatcher is installed implicitly |
| Locale, theme, reduced motion and surface hooks | Generic bootstrap | Effective presentation observations; no saved-preference mutation |
| `TwinkleStores`, `useSyncExternalStore`, `useHostCapability` | Generic bootstrap | Branded subscriptions, selected values, explicit host vocabulary and generation checks |
| Windows, applications, notification, workspace, output topology, preview and menu stores/hooks | `nickel-jsx-host` bootstrap and `stores.rs` | Nickel schemas and filtered snapshots; host output resolver adapts topology for generic surface observations |
| `nickel` service clients and protected operation request schemas | Nickel host bootstrap | Grant-filtered service availability and bounded requests; Rust revalidates authority before executing effects |
| `NickelStores`, `useCapability` | Nickel host bootstrap | Compatibility names composed over generic machinery; no standalone Nickel capability union |
| Settings registration, metadata, pages, values, invocation and retirement | Nickel bootstrap and `settings.rs` | Nickel validates provider identity, registry membership, types and bounds |
| Checkpoint traversal, native trees, hooks, subscriptions, effects and rollback | Generic bootstrap/runtime | Host domains participate through bounded capture/restore hooks; registration seals before package initialization |
| `HostBindings`, `DataPublisher`, projection reuse | Generic `bindings.rs`/runtime | Versioned immutable Rust-owned initialization; bounded bootstrap/projection names, duplicate rejection; caches know registered names rather than desktop schemas |
| Default data-to-desktop-store adaptation and appearance configuration projection | Nickel `stores.rs` | Explicit registered publisher; standalone data does not imply desktop grants or shell schemas |
| Module normalization, limits, Oxc preparation/cache and relative linking | Generic `modules.rs` | No installed catalog discovery |
| Catalog resolution, package identities, provider ownership, Settings integration | Nickel composition modules | Admitted downstream policy and authority |
| Composition client prelude and Settings aliases | Nickel `composition_bindings.js` / `module_bindings.rs` | Generic module preparation accepts bounded shared/per-module host preludes; component proxy mechanics remain generic |
| Generic ambient declarations and host declaration composition | `types/twinkle.d.ts`, downstream `nickel-plugin.d.ts` | Neutral native vocabulary, generic hooks and store types; Nickel composes service/grant/Settings declarations and compatibility aliases |
| Standalone TSX preview and controller exercise | Optional `twinkle-workbench` JSX preview and generic TSX/CSS fixture | Production native semantic/keyboard/controller navigation, keyed state, selected stores and rollback pass; physical gamepad/Windows acceptance remains unverified |

The runtime and presentation unit fixtures use generic application APIs. Desktop schema, service, provider and shipped package fixtures live in Nickel's host tests. Desktop release-admission workloads moved to `nickel-jsx-host/tests/release_admission.rs`; Generic keyed insert/remove/reorder and leaf hook/reducer workloads live in `twinkle-jsx-runtime/tests/release_admission.rs`; desktop store/lifecycle workloads remain downstream. No release timing or memory baseline has been rerun by this move.

Internal `__nickel` names and stable `useId` spellings remain implementation compatibility identities. They grant no host authority and do not imply an installed shell service. Renaming them is not required to remove dependency direction; public standalone globals are checked separately.

Native composition subtree diffing, patch normalization, namespaces and retained path accounting now live in `twinkle-protocol::native_tree`, with generic regressions upstream. The remaining multi-owner mount/callback/transaction manager still resides in Nickel's mixed composition runtime and must be separated from catalog/grant policy before 0275 completion. Dependency closure independence does not prove this ownership boundary.

`JsxRuntime::register_component_mount` and its bootstrap entry are generic, using admitted public exports and per-mount hooks without a Nickel bootstrap. The former registered-page switch belongs to Nickel's `SettingsRuntimeExt`. A standalone conformance test exercises two mounts sharing loaded code, independent state, native callback dispatch, retained state on reselection, and rejected unknown/empty selections. This supplies the mount primitive for the remaining generic multi-owner manager extraction.

The generic multi-owner manager is now implemented in `composition.rs` over `AdmittedLinkPlan`, `OwnerRuntime`, and explicit `CompositionHost` hooks. Its ordinary host uses unit metadata and no Nickel bindings. Mounts, callback/child grants, effect origins, native patch reconciliation, checkpoint rollback and retirement are generic. Module preparation owns the portable component/contribution client and declares contribution metadata without a desktop schema. Eight manager regressions and all 103 runtime tests pass; independent workspace compilation and formatting pass. Nickel's active legacy manager, catalog reload, and provider synchronization still require adapter integration and replacement before extraction sign-off.

The generic composition manager also owns `reload_admitted`: it receives an
isolated native-admitted candidate, prepared module graphs and compatibility
signatures. It preflights complete root expansion, preserves compatible hook
contexts, replaces incompatible provider contexts, retires old grants and emits
native root replacement patches. It does not resolve Nickel catalogs or inspect
capability/Settings schemas. Contributor synchronization validates bounded link
plans without executing guest code and gives re-imported contexts fresh local
authority incarnations.

Development reload is not an atomic transaction over arbitrary package globals.
An installation or native registration failure after successful preflight may
have changed accepted contexts. Nickel's active spec 0279 tracks the recovery
protocol needed before such errors can be reported as preserving the accepted
graph. The production Nickel manager adapter remains an integration step.

//! Optional `twinkle/redux` module backed by native `twinkle-redux` stores.
//!
//! This crate is intentionally separate: native Rust users depend only on
//! `twinkle-redux` and do not compile Oxc or V8.

use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    ops::{Deref, DerefMut},
    rc::Rc,
};
use twinkle_jsx_runtime::{
    HostBindings, JsxModuleGraph, JsxRuntime, ModuleSource, NativeJsonCallback,
};
use twinkle_redux::{CoordinatorError, Reduction, RollbackSafe, Store, TransactionCoordinator};

const MAX_STORES: usize = 32;
const NATIVE_CALLBACK: &str = "__twinkleReduxNative";
const PROXY_GUARD: &str = "__twinkleReduxIsProxy";
const BUILTIN_MODULE: &str = "twinkle/redux";
const BUILTIN_SOURCE: &str = "module.exports = globalThis.__twinkleReduxExports;";
const BOOTSTRAP: &str = include_str!("bootstrap.js");

#[derive(Clone, Debug)]
struct RootHandle(u64);
impl RollbackSafe for RootHandle {}

#[derive(Clone, Debug)]
struct ReplaceAction {
    root: u64,
    changed: bool,
}

type JsonReducer = fn(&RootHandle, &ReplaceAction) -> Result<Reduction<RootHandle>, &'static str>;
type JsonStore = Store<RootHandle, ReplaceAction, JsonReducer>;

fn replace_reducer(
    _state: &RootHandle,
    action: &ReplaceAction,
) -> Result<Reduction<RootHandle>, &'static str> {
    if action.changed {
        Ok(Reduction::Changed(RootHandle(action.root)))
    } else {
        Ok(Reduction::Unchanged)
    }
}

#[derive(Default)]
struct Registry {
    stores: BTreeMap<u64, JsonStore>,
    next_store: u64,
    coordinator: Option<TransactionCoordinator>,
    prepared: bool,
    provisional_stores: Vec<u64>,
}

impl Registry {
    fn call(&mut self, request: Value) -> Result<Value, String> {
        let request = request
            .as_object()
            .ok_or("Redux native request must be an object")?;
        let operation = request
            .get("op")
            .and_then(Value::as_str)
            .ok_or("Redux native request requires an operation")?;
        match operation {
            "authorize" => {
                if self.coordinator.is_none() || self.prepared {
                    return Err("Redux mutation requires an active mutation turn".into());
                }
                Ok(Value::Null)
            }
            "create" => {
                if self.stores.len() >= MAX_STORES {
                    return Err(format!("Redux adapter exceeds {MAX_STORES} stores"));
                }
                self.next_store = self
                    .next_store
                    .checked_add(1)
                    .ok_or("Redux store identity exhausted")?;
                let id = self.next_store;
                let root = request
                    .get("root")
                    .and_then(Value::as_u64)
                    .ok_or("Redux store requires a root handle")?;
                self.stores.insert(
                    id,
                    Store::coordinated(RootHandle(root), replace_reducer as JsonReducer),
                );
                if let Some(coordinator) = &mut self.coordinator {
                    if let Err(error) = coordinator.enlist(&self.stores[&id]) {
                        self.stores.remove(&id);
                        return Err(error.to_string());
                    }
                    self.provisional_stores.push(id);
                }
                Ok(json!({"store":id}))
            }
            "get" => {
                let store = self.store(request)?;
                Ok(json!({"root":store.snapshot().0,"generation":store.generation().get()}))
            }
            "dispatch" => {
                if self.coordinator.is_none() {
                    return Err("Redux dispatch requires an active JSX transaction".into());
                }
                let store = self.store(request)?;
                let action = ReplaceAction {
                    root: request
                        .get("root")
                        .and_then(Value::as_u64)
                        .ok_or("Redux dispatch requires a root handle")?,
                    changed: request
                        .get("changed")
                        .and_then(Value::as_bool)
                        .ok_or("Redux dispatch requires changed")?,
                };
                store
                    .dispatch_handle()
                    .dispatch(&action)
                    .map_err(|error| error.to_string())?;
                Ok(json!({"generation":store.generation().get()}))
            }
            _ => Err(format!("unsupported Redux native operation {operation:?}")),
        }
    }

    fn store(&self, request: &serde_json::Map<String, Value>) -> Result<&JsonStore, String> {
        let id = request
            .get("store")
            .and_then(Value::as_u64)
            .ok_or("Redux native request requires a store")?;
        self.stores.get(&id).ok_or("unknown Redux store".into())
    }

    fn begin(&mut self) -> Result<(), String> {
        if self.coordinator.is_some() {
            return Err("Redux adapter transaction already pending".into());
        }
        let mut coordinator = TransactionCoordinator::new();
        for store in self.stores.values() {
            coordinator
                .enlist(store)
                .map_err(|error| error.to_string())?;
        }
        self.coordinator = Some(coordinator);
        self.prepared = false;
        self.provisional_stores.clear();
        Ok(())
    }

    fn prepare(&mut self) -> Result<(), String> {
        if self.prepared {
            return Err("Redux adapter transaction already prepared".into());
        }
        self.coordinator
            .as_mut()
            .ok_or("Redux adapter transaction is unavailable")?
            .prepare_accept()
            .map_err(|error| error.to_string())?;
        self.prepared = true;
        Ok(())
    }

    fn finalize(&mut self) -> Result<(), String> {
        if !self.prepared {
            return Err("Redux adapter transaction is not prepared".into());
        }
        self.coordinator
            .as_mut()
            .ok_or("Redux adapter transaction is unavailable")?
            .finalize_accept()
            .map_err(|error| error.to_string())
    }

    fn after_accept(&mut self) -> Result<(), String> {
        let result = self
            .coordinator
            .as_mut()
            .ok_or("Redux adapter transaction is unavailable")?
            .after_accept()
            .map_err(|error| error.to_string());
        self.coordinator = None;
        self.prepared = false;
        self.provisional_stores.clear();
        result
    }

    fn abort(&mut self) -> Result<(), String> {
        let result = if let Some(coordinator) = &mut self.coordinator {
            coordinator
                .abort()
                .map_err(|error: CoordinatorError| error.to_string())
        } else {
            Ok(())
        };
        self.coordinator = None;
        self.prepared = false;
        for id in self.provisional_stores.drain(..) {
            if let Some(store) = self.stores.remove(&id) {
                let _ = store.retire();
            }
        }
        result
    }
}

/// JSX runtime plus the native Redux acceptance participant.
pub struct ReduxJsxRuntime {
    runtime: JsxRuntime,
    registry: Rc<RefCell<Registry>>,
    prepared: bool,
    finalized: bool,
}

impl ReduxJsxRuntime {
    pub fn graph<'a>(
        entry: &str,
        modules: impl IntoIterator<Item = ModuleSource<'a>>,
    ) -> Result<JsxModuleGraph, String> {
        JsxModuleGraph::new_with_builtin_modules(entry, modules, [(BUILTIN_MODULE, BUILTIN_SOURCE)])
    }

    pub fn new(graph: &JsxModuleGraph, data: Option<&str>) -> Result<Self, String> {
        let registry = Rc::new(RefCell::new(Registry::default()));
        let callback_registry = Rc::clone(&registry);
        let callback: NativeJsonCallback = Rc::new(RefCell::new(move |request| {
            callback_registry.borrow_mut().call(request)
        }));
        let bindings =
            HostBindings::new(HostBindings::ABI_VERSION, BOOTSTRAP, |_, _, _| Ok(()), &[])?
                .with_native_json_callback(NATIVE_CALLBACK, callback)?
                .with_native_proxy_guard(PROXY_GUARD)?;
        let runtime = JsxRuntime::new_modules_with_bindings(graph, data, &bindings)?;
        Ok(Self {
            runtime,
            registry,
            prepared: false,
            finalized: false,
        })
    }

    pub fn begin_transaction(&mut self) -> Result<(), String> {
        self.registry.borrow_mut().begin()?;
        if let Err(error) = self.runtime.begin_transaction() {
            let _ = self.registry.borrow_mut().abort();
            return Err(error);
        }
        self.prepared = false;
        self.finalized = false;
        Ok(())
    }

    /// Start the fresh admitted turn that releases passive effects from the
    /// preceding acceptance. Render and validate any requested reconciliation,
    /// then use the normal prepare/finalize/after sequence on this turn.
    pub fn begin_effect_turn(&mut self) -> Result<(), String> {
        if !self.runtime.after_transaction_work_pending()? {
            return Err("Redux JSX lifecycle turn is not pending".into());
        }
        self.registry.borrow_mut().begin()?;
        if let Err(error) = self.runtime.begin_after_transaction() {
            let _ = self.registry.borrow_mut().abort();
            return Err(error);
        }
        self.prepared = false;
        self.finalized = false;
        Ok(())
    }

    pub fn effect_turn_pending(&mut self) -> Result<bool, String> {
        self.runtime.after_transaction_work_pending()
    }

    pub fn prepare_accept(&mut self) -> Result<(), String> {
        if let Err(error) = self.runtime.eval("__twinkleReduxPrepareAccept()") {
            let _ = self.registry.borrow_mut().abort();
            let _ = self.runtime.finish_transaction(false);
            return Err(error);
        }
        if let Err(error) = self.runtime.prepare_transaction() {
            let _ = self.registry.borrow_mut().abort();
            let _ = self.runtime.finish_transaction(false);
            return Err(error);
        }
        if let Err(error) = self.registry.borrow_mut().prepare() {
            let _ = self.runtime.finish_transaction(false);
            return Err(error);
        }
        self.prepared = true;
        Ok(())
    }

    pub fn finalize_accept(&mut self) -> Result<(), String> {
        if !self.prepared {
            return Err("Redux JSX transaction is not prepared".into());
        }
        // Both participants are prepared before either irreversible switch.
        // JSX acceptance is now a checked, callback-free operation; native
        // store finalization is likewise callback-free.
        if let Err(error) = self.runtime.finalize_transaction() {
            return Err(format!(
                "prepared JSX finalization failed terminally: {error}"
            ));
        }
        if let Err(error) = self.registry.borrow_mut().finalize() {
            return Err(format!(
                "JSX finalized but prepared Redux finalization failed terminally: {error}"
            ));
        }
        self.finalized = true;
        Ok(())
    }

    pub fn after_accept(&mut self) -> Result<(), String> {
        if !self.finalized {
            return Err("Redux JSX transaction is not finalized".into());
        }
        let native = self.registry.borrow_mut().after_accept();
        let package = self.runtime.eval("__twinkleReduxAfterAccept()");
        self.prepared = false;
        self.finalized = false;
        native?;
        package?;
        if !self.runtime.after_transaction_work_pending()? {
            self.runtime.after_transaction_accept()?;
        }
        Ok(())
    }

    pub fn abort(&mut self) -> Result<(), String> {
        if self.finalized {
            return Err("Redux JSX transaction is already finalized".into());
        }
        let runtime = self.runtime.finish_transaction(false);
        let native = self.registry.borrow_mut().abort();
        self.prepared = false;
        runtime?;
        native
    }
}

impl Deref for ReduxJsxRuntime {
    type Target = JsxRuntime;

    fn deref(&self) -> &Self::Target {
        &self.runtime
    }
}

impl DerefMut for ReduxJsxRuntime {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.runtime
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORACLE_EXPECTED: &str = include_str!("../tests/oracle/expected.json");

    fn runtime() -> ReduxJsxRuntime {
        let graph = ReduxJsxRuntime::graph(
            "main.jsx",
            [ModuleSource {
                path: "main.jsx",
                source: r#"
import { createStore } from 'twinkle/redux';
const store = createStore((state = {count:0}, action) => action.type === 'add' ? {count:state.count + action.value} : state);
globalThis.testStore = store;
globalThis.notifications = 0;
store.subscribe(() => notifications++);
export default function App() { return <Text>{String(store.getState().count)}</Text>; }
"#,
            }],
        )
        .unwrap();
        ReduxJsxRuntime::new(&graph, None).unwrap()
    }

    #[test]
    fn native_store_commits_and_rolls_back_with_jsx_transaction() {
        let mut runtime = runtime();
        let _: Value = runtime
            .render("__twinkleRender()", |value| Ok(value.clone()))
            .unwrap();
        assert_eq!(
            runtime
                .eval_json::<Value>("JSON.stringify(testStore.getState())")
                .unwrap(),
            json!({"count":0})
        );

        runtime.begin_transaction().unwrap();
        runtime
            .eval("testStore.dispatch({type:'add',value:2})")
            .unwrap();
        assert_eq!(
            runtime
                .eval_json::<Value>("JSON.stringify(testStore.getState())")
                .unwrap(),
            json!({"count":2})
        );
        assert_eq!(runtime.eval_json::<u64>("notifications").unwrap(), 0);
        runtime.prepare_accept().unwrap();
        runtime.finalize_accept().unwrap();
        assert_eq!(runtime.eval_json::<u64>("notifications").unwrap(), 0);
        runtime.after_accept().unwrap();
        assert_eq!(runtime.eval_json::<u64>("notifications").unwrap(), 1);

        runtime.begin_transaction().unwrap();
        runtime
            .eval("testStore.dispatch({type:'add',value:3})")
            .unwrap();
        assert_eq!(
            runtime
                .eval_json::<Value>("JSON.stringify(testStore.getState())")
                .unwrap(),
            json!({"count":5})
        );
        runtime.abort().unwrap();
        assert_eq!(
            runtime
                .eval_json::<Value>("JSON.stringify(testStore.getState())")
                .unwrap(),
            json!({"count":2})
        );
        assert_eq!(runtime.eval_json::<u64>("notifications").unwrap(), 1);
    }

    #[test]
    fn adapter_does_not_admit_upstream_bare_packages() {
        assert!(
            ReduxJsxRuntime::graph(
                "main.jsx",
                [ModuleSource {
                    path: "main.jsx",
                    source: "import { createStore } from 'redux';\nexport default function App(){return null;}",
                }],
            )
            .is_err()
        );
    }

    #[test]
    fn adapter_rejects_unsupported_options_and_matches_object_is_shallow_equality() {
        let mut runtime = runtime();
        let error = runtime
            .eval("__twinkleReduxExports.configureStore({reducer:(s=0)=>s,middleware:[]})")
            .unwrap_err();
        assert!(error.contains("does not support option middleware"));
        assert!(
            runtime
                .eval_json::<bool>("__twinkleReduxExports.shallowEqual({x:NaN},{x:NaN})")
                .unwrap()
        );
        assert!(
            !runtime
                .eval_json::<bool>("__twinkleReduxExports.shallowEqual({x:0},{x:-0})")
                .unwrap()
        );
    }

    #[test]
    fn provider_selector_dispatch_and_slice_helpers_render_through_native_store() {
        let graph = ReduxJsxRuntime::graph(
            "main.jsx",
            [ModuleSource {
                path: "main.jsx",
                source: r#"
import { configureStore, createSlice, Provider, useDispatch, useSelector } from 'twinkle/redux';
const counter = createSlice({name:'counter',initialState:{count:0},reducers:{add:(state,action)=>({count:state.count+action.payload})}});
const store = configureStore({reducer:counter.reducer});
globalThis.sliceStore = store;
globalThis.add = counter.actions.add;
function Counter() {
    const count = useSelector(state => state.count);
    const dispatch = useDispatch();
    return <Button onClick={() => dispatch(add(1))}>{String(count)}</Button>;
}
export default function App() { return <Provider store={store}><Counter /></Provider>; }
"#,
            }],
        )
        .unwrap();
        let mut runtime = ReduxJsxRuntime::new(&graph, None).unwrap();
        let first: Value = runtime
            .render("__twinkleRender()", |value| Ok(value.clone()))
            .unwrap();
        assert!(first.to_string().contains("\"0\""));

        runtime.begin_transaction().unwrap();
        runtime.eval("sliceStore.dispatch(add(1))").unwrap();
        let second: Value = runtime
            .render("__twinkleRender()", |value| Ok(value.clone()))
            .unwrap();
        assert!(second.to_string().contains("\"1\""));
        runtime.prepare_accept().unwrap();
        runtime.finalize_accept().unwrap();
        runtime.after_accept().unwrap();
        assert_eq!(
            runtime
                .eval_json::<Value>("JSON.stringify(sliceStore.getState())")
                .unwrap(),
            json!({"count":1})
        );
    }

    #[test]
    fn admitted_store_and_toolkit_helpers_match_pinned_oracle_trace() {
        let mut runtime = runtime();
        runtime
            .eval(
                r#"
const oracleAdd = __twinkleReduxExports.createAction('counter/add');
const oracleReducer = (state={count:0,label:'zero'},action) => action.type === oracleAdd.type ? {...state,count:state.count+action.payload} : state;
const oracleStore = __twinkleReduxExports.createStore(oracleReducer);
const oracleInitial = oracleStore.getState();
const oracleCombined = __twinkleReduxExports.combineReducers({counter:oracleReducer,stable:(state=7)=>state});
const oracleCombinedInitial = oracleCombined(undefined,{type:'@@trace/init'});
const oracleSlice = __twinkleReduxExports.createSlice({name:'value',initialState:1,reducers:{replace:(_state,action)=>action.payload}});
const oracleToolkit = __twinkleReduxExports.configureStore({reducer:oracleSlice.reducer});
"#,
            )
            .unwrap();
        runtime.begin_transaction().unwrap();
        let actual: Value = runtime
            .eval_json(
                r#"JSON.stringify((() => {
const ignored=oracleStore.dispatch({type:'ignored'});
const ignoredRootShared=oracleStore.getState()===oracleInitial;
const dispatched=oracleStore.dispatch(oracleAdd(2));
const combinedIgnored=oracleCombined(oracleCombinedInitial,{type:'ignored'});
oracleToolkit.dispatch(oracleSlice.actions.replace(9));
return {redux:{initial:oracleInitial,ignored,ignoredRootShared,dispatched,final:oracleStore.getState(),combinedIgnoredRootShared:combinedIgnored===oracleCombinedInitial},toolkit:{action:oracleAdd(3),actionType:oracleAdd.type,sliceName:oracleSlice.name,sliceAction:oracleSlice.actions.replace(9),final:oracleToolkit.getState()}};
})())"#,
            )
            .unwrap();
        let expected: Value = serde_json::from_str(ORACLE_EXPECTED).unwrap();
        assert_eq!(actual, expected);
        runtime.abort().unwrap();
    }

    #[test]
    fn state_graph_never_crosses_json_and_preserves_supported_javascript_identity() {
        let graph = ReduxJsxRuntime::graph(
            "main.jsx",
            [ModuleSource {
                path: "main.jsx",
                source: r#"
import { createStore } from 'twinkle/redux';
let getterCalls=0;
const bad={}; Object.defineProperty(bad,'value',{get(){getterCalls++;return 1;}});
try { createStore((_state={})=>_state,bad); } catch (error) { globalThis.graphError=String(error); }
globalThis.getterCalls=()=>getterCalls;
let proxyTraps=0;
const proxy=new Proxy({},{getPrototypeOf(){proxyTraps++;return Object.prototype;},ownKeys(){proxyTraps++;return [];}});
try { createStore((_state={})=>_state,proxy); } catch (error) { globalThis.proxyError=String(error); }
globalThis.proxyTraps=()=>proxyTraps;
const shared={value:1};
const initial={shared,alias:shared,hole:[,undefined],nan:NaN,negativeZero:-0}; initial.self=initial;
globalThis.graphStore=createStore((state,action)=>action.type==='replace'?action.payload:state,initial);
globalThis.acceptedGraph=initial;
export default function App(){return <Text>graph</Text>;}
"#,
            }],
        )
        .unwrap();
        let mut runtime = ReduxJsxRuntime::new(&graph, None).unwrap();
        assert_eq!(runtime.eval_json::<u64>("getterCalls()").unwrap(), 0);
        assert!(
            runtime
                .eval_json::<bool>("graphError.includes('accessor')")
                .unwrap()
        );
        assert_eq!(runtime.eval_json::<u64>("proxyTraps()").unwrap(), 0);
        assert!(
            runtime
                .eval_json::<bool>("proxyError.includes('Proxy')")
                .unwrap()
        );
        assert!(runtime
            .eval_json::<bool>("graphStore.getState().self===graphStore.getState() && graphStore.getState().shared===graphStore.getState().alias && !(0 in graphStore.getState().hole) && Object.is(graphStore.getState().negativeZero,-0) && Number.isNaN(graphStore.getState().nan) && Object.isFrozen(graphStore.getState())")
            .unwrap());

        runtime.begin_transaction().unwrap();
        runtime
            .eval("const next={value:2};next.self=next;graphStore.dispatch({type:'replace',payload:next})")
            .unwrap();
        runtime.abort().unwrap();
        assert!(runtime
            .eval_json::<bool>("graphStore.getState()===acceptedGraph && graphStore.getState().self===acceptedGraph")
            .unwrap());
    }

    #[test]
    fn stores_created_in_admitted_turn_commit_or_retire_with_the_outer_transaction() {
        let mut runtime = runtime();
        runtime.begin_transaction().unwrap();
        runtime
            .eval("globalThis.rejectedStore=__twinkleReduxExports.createStore((state=0,action)=>action.type==='add'?state+1:state)")
            .unwrap();
        runtime.abort().unwrap();

        runtime.begin_transaction().unwrap();
        let rejected = runtime
            .eval("rejectedStore.dispatch({type:'add'})")
            .unwrap_err();
        assert!(rejected.contains("unknown Redux store"));
        runtime.abort().unwrap();

        runtime.begin_transaction().unwrap();
        runtime
            .eval("globalThis.acceptedStore=__twinkleReduxExports.createStore((state=0,action)=>action.type==='add'?state+1:state)")
            .unwrap();
        runtime.prepare_accept().unwrap();
        runtime.finalize_accept().unwrap();
        runtime.after_accept().unwrap();

        runtime.begin_transaction().unwrap();
        runtime
            .eval("acceptedStore.dispatch({type:'add'})")
            .unwrap();
        runtime.prepare_accept().unwrap();
        runtime.finalize_accept().unwrap();
        runtime.after_accept().unwrap();
        assert_eq!(
            runtime
                .eval_json::<u64>("acceptedStore.getState()")
                .unwrap(),
            1
        );
    }

    #[test]
    fn aborted_unsubscribe_reactivates_the_same_javascript_handle() {
        let mut runtime = runtime();
        runtime
            .eval(
                "globalThis.extraHits=0;globalThis.stopExtra=testStore.subscribe(()=>extraHits++)",
            )
            .unwrap();
        runtime.begin_transaction().unwrap();
        runtime.eval("stopExtra()").unwrap();
        runtime.abort().unwrap();

        runtime.begin_transaction().unwrap();
        runtime
            .eval("testStore.dispatch({type:'add',value:1})")
            .unwrap();
        runtime.prepare_accept().unwrap();
        runtime.finalize_accept().unwrap();
        runtime.after_accept().unwrap();
        assert_eq!(runtime.eval_json::<u64>("extraHits").unwrap(), 1);

        runtime.eval("stopExtra()").unwrap();
        runtime.begin_transaction().unwrap();
        runtime
            .eval("testStore.dispatch({type:'add',value:1})")
            .unwrap();
        runtime.prepare_accept().unwrap();
        runtime.finalize_accept().unwrap();
        runtime.after_accept().unwrap();
        assert_eq!(runtime.eval_json::<u64>("extraHits").unwrap(), 1);
    }

    #[test]
    fn rejected_dispatch_runs_no_reducer_or_proxy_trap_and_listener_errors_do_not_skip() {
        let mut runtime = runtime();
        runtime
            .eval(
                "globalThis.reducerCalls=0;globalThis.guardedStore=__twinkleReduxExports.createStore((state=0,action)=>{reducerCalls++;return action.type==='add'?state+1:state});globalThis.reducerBaseline=reducerCalls",
            )
            .unwrap();
        assert!(
            runtime
                .eval("guardedStore.dispatch({type:'add'})")
                .unwrap_err()
                .contains("active mutation turn")
        );
        assert!(
            runtime
                .eval_json::<bool>("reducerCalls===reducerBaseline")
                .unwrap()
        );

        runtime.begin_transaction().unwrap();
        runtime
            .eval("globalThis.actionTraps=0;const actionProxy=new Proxy({type:'add'},{getPrototypeOf(){actionTraps++;return Object.prototype;}});try{guardedStore.dispatch(actionProxy)}catch(error){globalThis.actionProxyError=String(error)}")
            .unwrap();
        assert_eq!(runtime.eval_json::<u64>("actionTraps").unwrap(), 0);
        assert!(
            runtime
                .eval_json::<bool>("actionProxyError.includes('plain object')")
                .unwrap()
        );
        runtime.abort().unwrap();

        runtime
            .eval("globalThis.laterListenerRuns=0;guardedStore.subscribe(()=>{throw Error('listener')});guardedStore.subscribe(()=>laterListenerRuns++)")
            .unwrap();
        runtime.begin_transaction().unwrap();
        runtime.eval("guardedStore.dispatch({type:'add'})").unwrap();
        runtime.prepare_accept().unwrap();
        runtime.finalize_accept().unwrap();
        assert!(runtime.after_accept().unwrap_err().contains("subscriber"));
        assert_eq!(runtime.eval_json::<u64>("laterListenerRuns").unwrap(), 1);
    }

    #[test]
    fn state_admission_rejects_mutable_hook_alias_before_freezing() {
        let graph = ReduxJsxRuntime::graph(
            "main.jsx",
            [ModuleSource {
                path: "main.jsx",
                source: r#"
import {createStore} from 'twinkle/redux';
globalThis.aliasStore=createStore((state={},action)=>action.type==='replace'?action.payload:state);
export default function App(){const value=useRef({count:1});globalThis.hookAlias=value.current;return <Text>alias</Text>;}
"#,
            }],
        )
        .unwrap();
        let mut runtime = ReduxJsxRuntime::new(&graph, None).unwrap();
        let _: Value = runtime
            .render("__twinkleRender()", |value| Ok(value.clone()))
            .unwrap();
        runtime.begin_transaction().unwrap();
        let error = runtime
            .eval("aliasStore.dispatch({type:'replace',payload:hookAlias})")
            .unwrap_err();
        assert!(error.contains("aliases mutable checkpoint-owned state"));
        assert!(
            !runtime
                .eval_json::<bool>("Object.isFrozen(hookAlias)")
                .unwrap()
        );
        runtime.abort().unwrap();
    }

    #[test]
    fn selector_work_is_coalesced_to_the_final_root_of_a_dispatch_burst() {
        let graph = ReduxJsxRuntime::graph(
            "main.jsx",
            [ModuleSource {
                path: "main.jsx",
                source: r#"
import {createStore,Provider,useSelector} from 'twinkle/redux';
globalThis.burstStore=createStore((state={count:0},action)=>action.type==='add'?{count:state.count+action.by}:state);
globalThis.selectorCalls=0;
function Counter(){const count=useSelector(state=>{selectorCalls++;return state.count});return <Text>{String(count)}</Text>;}
export default function App(){return <Provider store={burstStore}><Counter/></Provider>;}
"#,
            }],
        )
        .unwrap();
        let mut runtime = ReduxJsxRuntime::new(&graph, None).unwrap();
        let _: Value = runtime
            .render("__twinkleRender()", |value| Ok(value.clone()))
            .unwrap();
        let baseline = runtime.eval_json::<u64>("selectorCalls").unwrap();
        runtime.begin_transaction().unwrap();
        runtime
            .eval("for(let i=0;i<100;i++)burstStore.dispatch({type:'add',by:i===99?-99:1})")
            .unwrap();
        let _: Value = runtime
            .render("__twinkleRender()", |value| Ok(value.clone()))
            .unwrap();
        let calls = runtime.eval_json::<u64>("selectorCalls").unwrap() - baseline;
        assert!(calls <= 3, "selector ran {calls} times for one burst");
        runtime.prepare_accept().unwrap();
        runtime.finalize_accept().unwrap();
        runtime.after_accept().unwrap();
    }

    #[test]
    fn accepted_effect_dispatch_runs_in_a_fresh_transaction() {
        let graph = ReduxJsxRuntime::graph(
            "main.jsx",
            [ModuleSource {
                path: "main.jsx",
                source: r#"
import {createStore,Provider,useSelector} from 'twinkle/redux';
globalThis.sourceStore=createStore((state=0,action)=>action.type==='add'?state+1:state);
globalThis.effectStore=createStore((state=0,action)=>action.type==='effect'?state+1:state);
function Child(){
  const value=useSelector(state=>state);
  useEffect(()=>{if(value===1)effectStore.dispatch({type:'effect'});},[value]);
  return <Text>{String(value)}</Text>;
}
export default function App(){return <Provider store={sourceStore}><Child/></Provider>;}
"#,
            }],
        )
        .unwrap();
        let mut runtime = ReduxJsxRuntime::new(&graph, None).unwrap();
        let _: Value = runtime
            .render("__twinkleRender()", |value| Ok(value.clone()))
            .unwrap();
        runtime.begin_transaction().unwrap();
        runtime.eval("sourceStore.dispatch({type:'add'})").unwrap();
        let _: Value = runtime
            .render("__twinkleRender()", |value| Ok(value.clone()))
            .unwrap();
        runtime.prepare_accept().unwrap();
        runtime.finalize_accept().unwrap();
        runtime.after_accept().unwrap();
        assert!(runtime.effect_turn_pending().unwrap());
        assert_eq!(
            runtime.eval_json::<u64>("effectStore.getState()").unwrap(),
            0
        );

        runtime.begin_effect_turn().unwrap();
        assert_eq!(
            runtime.eval_json::<u64>("effectStore.getState()").unwrap(),
            1
        );
        let _: Value = runtime
            .render("__twinkleRender()", |value| Ok(value.clone()))
            .unwrap();
        runtime.prepare_accept().unwrap();
        runtime.finalize_accept().unwrap();
        runtime.after_accept().unwrap();
        assert!(!runtime.effect_turn_pending().unwrap());
        assert_eq!(
            runtime.eval_json::<u64>("effectStore.getState()").unwrap(),
            1
        );
    }

    #[test]
    fn promise_jobs_cannot_dispatch_with_outer_transaction_authority() {
        let mut runtime = runtime();
        runtime.begin_transaction().unwrap();
        runtime.eval("globalThis.lateError='';globalThis.lateCreateError='';Promise.resolve().then(()=>{try{testStore.dispatch({type:'add',value:1})}catch(error){lateError=String(error)}try{__twinkleReduxExports.createStore((s=0)=>s)}catch(error){lateCreateError=String(error)}})").unwrap();
        assert_eq!(
            runtime
                .eval_json::<u64>("testStore.getState().count")
                .unwrap(),
            0
        );
        assert!(
            runtime
                .eval_json::<bool>("lateError.includes('synchronous host mutation authority')")
                .unwrap()
        );
        assert!(
            runtime
                .eval_json::<bool>(
                    "lateCreateError.includes('synchronous host mutation authority')"
                )
                .unwrap()
        );
        runtime.abort().unwrap();
    }

    #[test]
    fn redux_only_event_invalidates_before_scheduler_dirty_check() {
        let graph = ReduxJsxRuntime::graph("main.jsx", [ModuleSource { path:"main.jsx", source:r#"
import {createStore,Provider,useDispatch,useSelector} from 'twinkle/redux';
globalThis.eventStore=createStore((state=0,action)=>action.type==='add'?state+1:state);
function Counter(){const value=useSelector(state=>state);const dispatch=useDispatch();return <Button onClick={()=>dispatch({type:'add'})}>{String(value)}</Button>}
export default function App(){return <Provider store={eventStore}><Counter/></Provider>}
"# }]).unwrap();
        let mut runtime = ReduxJsxRuntime::new(&graph, None).unwrap();
        let first: Value = runtime
            .render("__twinkleRender()", |value| Ok(value.clone()))
            .unwrap();
        let action = first["action"].as_u64().unwrap();
        runtime.begin_transaction().unwrap();
        let outcome = runtime
            .dispatch_scheduled(
                &format!("__twinkleDispatchBatchScheduled([[{action},null]])"),
                |value| Ok(value.clone()),
            )
            .unwrap();
        assert!(matches!(
            outcome,
            twinkle_jsx_runtime::ScheduledRender::Rendered { .. }
        ));
        runtime.finish_event(true).unwrap();
        runtime.prepare_accept().unwrap();
        runtime.finalize_accept().unwrap();
        runtime.after_accept().unwrap();
        assert_eq!(
            runtime.eval_json::<u64>("eventStore.getState()").unwrap(),
            1
        );
    }

    #[test]
    fn parent_removal_precedes_stale_child_selector() {
        let graph = ReduxJsxRuntime::graph("main.jsx", [ModuleSource { path:"main.jsx", source:r#"
import {createStore,Provider,useSelector} from 'twinkle/redux';
globalThis.entityStore=createStore((state={show:true,items:{x:{name:'X'}}},action)=>action.type==='hide'?{show:false,items:{}}:state);
function Child(){const name=useSelector(state=>state.items.x.name);return <Text>{name}</Text>}
function Parent(){const show=useSelector(state=>state.show);return show?<Child/>:<Text>hidden</Text>}
export default function App(){return <Provider store={entityStore}><Parent/></Provider>}
"# }]).unwrap();
        let mut runtime = ReduxJsxRuntime::new(&graph, None).unwrap();
        let _: Value = runtime
            .render("__twinkleRender()", |value| Ok(value.clone()))
            .unwrap();
        runtime.begin_transaction().unwrap();
        runtime.eval("entityStore.dispatch({type:'hide'})").unwrap();
        let node: Value = runtime
            .render("__twinkleRender()", |value| Ok(value.clone()))
            .unwrap();
        assert!(node.to_string().contains("hidden"));
        runtime.prepare_accept().unwrap();
        runtime.finalize_accept().unwrap();
        runtime.after_accept().unwrap();
    }

    #[test]
    fn reducer_cannot_read_or_subscribe_to_another_store() {
        let mut runtime = runtime();
        runtime.eval("globalThis.other=__twinkleReduxExports.createStore((s=0)=>s);globalThis.phaseErrors=[];globalThis.phaseStore=__twinkleReduxExports.createStore((s=0,a)=>{if(a.type==='probe'){for(const f of [()=>other.getState(),()=>other.subscribe(()=>{}),()=>__twinkleReduxExports.createStore((x=0)=>x)])try{f()}catch(e){phaseErrors.push(String(e))}}return s})").unwrap();
        runtime.begin_transaction().unwrap();
        runtime.eval("phaseStore.dispatch({type:'probe'})").unwrap();
        assert_eq!(runtime.eval_json::<u64>("phaseErrors.length").unwrap(), 3);
        assert!(
            runtime
                .eval_json::<bool>("phaseErrors.every(error=>error.includes('reduction'))")
                .unwrap()
        );
        runtime.abort().unwrap();
    }

    #[test]
    fn parent_selector_across_stores_precedes_stale_child_selector() {
        let graph = ReduxJsxRuntime::graph("main.jsx", [ModuleSource { path:"main.jsx", source:r#"
import {createStore,Provider,useSelector} from 'twinkle/redux';
globalThis.parentStore=createStore((s=true,a)=>a.type==='hide'?false:s);
globalThis.childStore=createStore((s={x:{name:'X'}},a)=>a.type==='remove'?{}:s);
function Child(){const name=useSelector(s=>s.x.name);return <Text>{name}</Text>}
function Parent(){const show=useSelector(s=>s);return <Provider store={childStore}>{show?<Child/>:<Text>hidden</Text>}</Provider>}
export default function App(){return <Provider store={parentStore}><Parent/></Provider>}
"# }]).unwrap();
        let mut runtime = ReduxJsxRuntime::new(&graph, None).unwrap();
        let _: Value = runtime
            .render("__twinkleRender()", |value| Ok(value.clone()))
            .unwrap();
        runtime.begin_transaction().unwrap();
        runtime
            .eval("childStore.dispatch({type:'remove'});parentStore.dispatch({type:'hide'})")
            .unwrap();
        let node: Value = runtime
            .render("__twinkleRender()", |value| Ok(value.clone()))
            .unwrap();
        assert!(node.to_string().contains("hidden"));
        runtime.prepare_accept().unwrap();
        runtime.finalize_accept().unwrap();
        runtime.after_accept().unwrap();
    }

    #[test]
    fn render_time_selector_cannot_mutate_a_store_registry() {
        let graph = ReduxJsxRuntime::graph("main.jsx", [ModuleSource { path:"main.jsx", source:r#"
import {createStore,Provider,useSelector} from 'twinkle/redux';
const store=createStore((s=0)=>s);globalThis.selectorSubscribed=false;
function Child(){useSelector(state=>{try{store.subscribe(()=>{});selectorSubscribed=true}catch(_error){}return state});return <Text>x</Text>}
export default function App(){return <Provider store={store}><Child/></Provider>}
"# }]).unwrap();
        let mut runtime = ReduxJsxRuntime::new(&graph, None).unwrap();
        let _: Value = runtime
            .render("__twinkleRender()", |value| Ok(value.clone()))
            .unwrap();
        assert!(!runtime.eval_json::<bool>("selectorSubscribed").unwrap());
    }

    #[test]
    fn adapter_matches_native_shared_semantic_and_recovery_trace() {
        let mut runtime = runtime();
        runtime.eval("globalThis.traceNotifications=0;globalThis.traceStore=__twinkleReduxExports.createStore((state=0,action)=>action.type==='add'?state+action.by:state);traceStore.subscribe(()=>traceNotifications++)").unwrap();
        let initial = runtime.eval_json::<i32>("traceStore.getState()").unwrap();
        runtime.begin_transaction().unwrap();
        runtime
            .eval("traceStore.dispatch({type:'add',by:1});traceStore.dispatch({type:'add',by:2})")
            .unwrap();
        runtime.prepare_accept().unwrap();
        runtime.finalize_accept().unwrap();
        runtime.after_accept().unwrap();
        let burst = runtime.eval_json::<i32>("traceStore.getState()").unwrap();
        let notifications = runtime.eval_json::<u64>("traceNotifications").unwrap();

        runtime.begin_transaction().unwrap();
        runtime
            .eval("traceStore.dispatch({type:'add',by:5})")
            .unwrap();
        runtime.abort().unwrap();
        let rejected = runtime.eval_json::<i32>("traceStore.getState()").unwrap();
        let notifications_after_reject = runtime.eval_json::<u64>("traceNotifications").unwrap();

        runtime.begin_transaction().unwrap();
        runtime.eval("traceStore.dispatch({type:'noop'})").unwrap();
        runtime.prepare_accept().unwrap();
        runtime.finalize_accept().unwrap();
        runtime.after_accept().unwrap();
        let noop = runtime.eval_json::<i32>("traceStore.getState()").unwrap();
        let notifications_after_noop = runtime.eval_json::<u64>("traceNotifications").unwrap();
        let trace = format!(
            "initial={initial}\nburst={burst}\nnotifications={notifications}\nrejected={rejected}\nnotifications_after_reject={notifications_after_reject}\nnoop={noop}\nnotifications_after_noop={notifications_after_noop}"
        );
        assert_eq!(
            trace,
            include_str!("../../twinkle-redux/tests/fixtures/shared_semantic_trace.txt").trim()
        );
    }

    #[test]
    fn adapter_limits_accept_the_boundary_and_reject_the_next_unit() {
        let mut graph_runtime = runtime();
        graph_runtime.begin_transaction().unwrap();
        graph_runtime
            .eval(
                "globalThis.nest=n=>{let value=0;for(let i=0;i<n;i++)value=[value];return value};\
                 globalThis.depth128=__twinkleReduxExports.createStore(s=>s,nest(129));\
                 globalThis.depthError='';try{__twinkleReduxExports.createStore(s=>s,nest(130))}catch(error){depthError=String(error)};\
                 globalThis.stringLimit=__twinkleReduxExports.createStore(s=>s,'x'.repeat(1024*1024));\
                 globalThis.stringError='';try{__twinkleReduxExports.createStore(s=>s,'x'.repeat(1024*1024+1))}catch(error){stringError=String(error)}",
            )
            .unwrap();
        assert!(
            graph_runtime
                .eval_json::<bool>("depth128.getState()!==undefined && depthError.includes('graph limit') && stringLimit.getState().length===1024*1024 && stringError.includes('string budget')")
                .unwrap()
        );
        graph_runtime.abort().unwrap();

        let mut store_runtime = runtime();
        store_runtime.begin_transaction().unwrap();
        store_runtime
            .eval(
                "globalThis.boundaryStores=[];for(let i=0;i<31;i++)boundaryStores.push(__twinkleReduxExports.createStore(s=>s,0));\
                 globalThis.storeLimitError='';try{__twinkleReduxExports.createStore(s=>s,0)}catch(error){storeLimitError=String(error)}",
            )
            .unwrap();
        assert!(
            store_runtime
                .eval_json::<bool>(
                    "boundaryStores.length===31 && storeLimitError.includes('32 stores')"
                )
                .unwrap()
        );
        store_runtime.abort().unwrap();

        let mut observer_runtime = runtime();
        observer_runtime.begin_transaction().unwrap();
        observer_runtime
            .eval(
                "globalThis.observerStore=__twinkleReduxExports.createStore(s=>s,0);\
                 globalThis.unsubscribers=[];for(let i=0;i<4096;i++)unsubscribers.push(observerStore.subscribe(()=>{}));\
                 globalThis.observerLimitError='';try{observerStore.subscribe(()=>{})}catch(error){observerLimitError=String(error)}",
            )
            .unwrap();
        assert!(
            observer_runtime
                .eval_json::<bool>(
                    "unsubscribers.length===4096 && observerLimitError.includes('4096 observers')"
                )
                .unwrap()
        );
        observer_runtime.abort().unwrap();
    }
}

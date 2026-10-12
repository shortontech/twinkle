const __twinkleReduxStores = new Map();
const __twinkleReduxMaxObservers = 4096;
const __twinkleReduxPendingPublic = new Set();
const __twinkleReduxPendingSelectors = new Set();
const __twinkleReduxNotificationBatches = new Map();
let __twinkleReduxCallbackPhase = 'idle';
let __twinkleReduxSynchronousAuthority = 0;
let __twinkleReduxNextRoot = 1;
function __twinkleReduxAllocateRoot() {
    if (__twinkleReduxNextRoot >= Number.MAX_SAFE_INTEGER) throw Error('Redux root handle exhausted');
    return __twinkleReduxNextRoot++;
}

function __twinkleReduxPlainObject(value, label) {
    if (!value || typeof value !== 'object' || __twinkleReduxIsProxy(value) || Array.isArray(value)
        || (Object.getPrototypeOf(value) !== Object.prototype
            && Object.getPrototypeOf(value) !== null))
        throw TypeError(`${label} must be a plain object`);
    return value;
}
function __twinkleReduxExactKeys(value, allowed, label) {
    for (const key of Object.keys(value))
        if (!allowed.includes(key)) throw TypeError(`${label} does not support option ${key}`);
}
function __twinkleReduxAdmitGraph(root, label, freeze) {
    const seen = new Set();
    const admitted = [];
    let nodes = 0;
    let properties = 0;
    let units = 0;
    function visit(value, depth) {
        const kind = typeof value;
        if (value === null || kind === 'undefined' || kind === 'boolean') return;
        if (kind === 'number') return;
        if (kind === 'string') {
            units += value.length;
            if (units > 1024 * 1024) throw RangeError(`${label} exceeds string budget`);
            return;
        }
        if (kind !== 'object') throw TypeError(`${label} contains unsupported ${kind}`);
        if (seen.has(value)) return;
        if (depth > 128 || ++nodes > 16384) throw RangeError(`${label} exceeds graph limit`);
        if (__twinkleReduxIsProxy(value)) throw TypeError(`${label} contains a Proxy`);
        const prototype = Object.getPrototypeOf(value);
        if (!Array.isArray(value) && prototype !== Object.prototype && prototype !== null)
            throw TypeError(`${label} contains an exotic object`);
        if (freeze && __compositionCheckpoint?.graph.has(value) && !Object.isFrozen(value))
            throw TypeError(`${label} aliases mutable checkpoint-owned state`);
        seen.add(value); admitted.push(value);
        for (const key of Reflect.ownKeys(value)) {
            if (typeof key === 'symbol') throw TypeError(`${label} contains a symbol key`);
            if (++properties > 65536) throw RangeError(`${label} exceeds property limit`);
            units += key.length;
            if (units > 1024 * 1024) throw RangeError(`${label} exceeds string budget`);
            const descriptor = Object.getOwnPropertyDescriptor(value,key);
            if (!descriptor || !('value' in descriptor)) throw TypeError(`${label} contains an accessor`);
            if (Array.isArray(value) && key === 'length') continue;
            visit(descriptor.value,depth+1);
        }
    }
    visit(root,0);
    if (freeze) for (let index=admitted.length-1;index>=0;index--) Object.freeze(admitted[index]);
    return root;
}

function __twinkleReduxCreateStore(reducer, preloadedState) {
    if (__twinkleReduxSynchronousAuthority <= 0) throw Error('Redux store creation requires synchronous host mutation authority');
    if (typeof reducer !== 'function') throw TypeError('Redux reducer must be a function');
    if (__currentComponent !== null) throw Error('Redux store creation is forbidden during render');
    if (__twinkleReduxCallbackPhase !== 'idle') throw Error(`Redux store creation is forbidden during ${__twinkleReduxCallbackPhase}`);
    __twinkleReduxCallbackPhase='reduction';
    try { preloadedState = reducer(preloadedState, {type:'@@twinkle-redux/init'}); }
    finally { __twinkleReduxCallbackPhase='idle'; }
    if (preloadedState === undefined) throw TypeError('Redux reducer returned undefined initial state');
    __twinkleReduxAdmitGraph(preloadedState, 'Redux initial state', true);
    const initialRoot=__twinkleReduxAllocateRoot();
    const roots=new Map([[initialRoot,preloadedState]]);
    const native = __twinkleReduxNative({op:'create',root:initialRoot});
    const listeners = new Set();
    const selectors = new Set();
    const store = {
        getState() {
            if (__twinkleReduxCallbackPhase !== 'idle') throw Error(`Redux state reads are forbidden during ${__twinkleReduxCallbackPhase}`);
            return store.__getStateUnchecked();
        },
        __getStateUnchecked() {
            const handle=__twinkleReduxNative({op:'get',store:native.store}).root;
            if (!roots.has(handle)) throw Error('Redux native root handle is unavailable');
            return roots.get(handle);
        },
        dispatch(action) {
            if (__twinkleReduxSynchronousAuthority <= 0) throw Error('Redux dispatch requires synchronous host mutation authority');
            if (__currentComponent !== null) throw Error('Redux dispatch is forbidden during render');
            if (__twinkleReduxCallbackPhase !== 'idle') throw Error(`Redux dispatch is forbidden during ${__twinkleReduxCallbackPhase}`);
            __twinkleReduxNative({op:'authorize'});
            __twinkleReduxPlainObject(action, 'Redux action');
            __twinkleReduxAdmitGraph(action, 'Redux action', false);
            const actionType=Object.getOwnPropertyDescriptor(action,'type')?.value;
            if (typeof actionType !== 'string' || !actionType) throw TypeError('Redux action needs a nonempty string type');
            const previous = store.getState();
            let next;
            __twinkleReduxCallbackPhase='reduction';
            try { next = reducer(previous, action); }
            finally { __twinkleReduxCallbackPhase='idle'; }
            if (next === undefined) throw TypeError('Redux reducer returned undefined');
            __twinkleReduxAdmitGraph(next, 'Redux state', true);
            const changed=next !== previous;
            const root=changed?__twinkleReduxAllocateRoot():__twinkleReduxNative({op:'get',store:native.store}).root;
            if (changed) roots.set(root,next);
            try { __twinkleReduxNative({op:'dispatch', store:native.store, root, changed}); }
            catch (error) { if (changed) roots.delete(root); throw error; }
            __twinkleReduxPendingSelectors.add(store);
            __twinkleReduxPendingPublic.add(store);
            return action;
        },
        subscribe(listener) {
            if (__twinkleReduxCallbackPhase !== 'idle') throw Error(`Redux subscription is forbidden during ${__twinkleReduxCallbackPhase}`);
            if (typeof listener !== 'function') throw TypeError('Redux subscriber must be a function');
            if (listeners.size+selectors.size>=__twinkleReduxMaxObservers) throw RangeError(`Redux store exceeds ${__twinkleReduxMaxObservers} observers`);
            const record={listener,active:true};
            listeners.add(record);
            return () => { if (record.active) { record.active=false; listeners.delete(record); } };
        },
        replaceReducer(next) {
            if (__twinkleReduxSynchronousAuthority <= 0) throw Error('Redux reducer replacement requires synchronous host mutation authority');
            if (__currentComponent !== null) throw Error('Redux reducer replacement is forbidden during render');
            if (__twinkleReduxCallbackPhase !== 'idle') throw Error(`Redux reducer replacement is forbidden during ${__twinkleReduxCallbackPhase}`);
            __twinkleReduxNative({op:'authorize'});
            if (typeof next !== 'function') throw TypeError('Redux reducer must be a function');
            reducer = next;
            store.dispatch({type:'@@twinkle-redux/replace'});
        },
        __subscribeSelector(listener, owner) {
            if (listeners.size+selectors.size>=__twinkleReduxMaxObservers) throw RangeError(`Redux store exceeds ${__twinkleReduxMaxObservers} observers`);
            const record={listener,owner}; selectors.add(record); return () => selectors.delete(record);
        },
        __selectors() { return selectors; },
        __snapshotListeners() { return Array.from(listeners); },
        __notifyPublic(batch) {
            const errors=[];
            for (const record of batch) try { record.listener(); } catch (error) { errors.push(error); }
            return errors;
        },
        __compactRoots() {
            const current=__twinkleReduxNative({op:'get',store:native.store}).root;
            for (const root of Array.from(roots.keys())) if (root!==current) roots.delete(root);
        },
        __snapshot() { return {reducer,roots:Array.from(roots),listeners:Array.from(listeners), selectors:Array.from(selectors)}; },
        __restore(snapshot,original=value=>value) {
            reducer=snapshot.reducer;
            roots.clear(); for (const [id,value] of snapshot.roots) roots.set(id,original(value));
            const restored=snapshot.listeners.map(original);
            for (const record of listeners) if (!restored.includes(record)) record.active=false;
            listeners.clear();
            for (const record of restored) { record.active=true; listeners.add(record); }
            selectors.clear(); for (const listener of snapshot.selectors) selectors.add(listener);
        }
    };
    __twinkleReduxStores.set(native.store, store);
    return Object.freeze(store);
}

function __twinkleReduxAfterAccept() {
    const pending = Array.from(__twinkleReduxNotificationBatches);
    __twinkleReduxNotificationBatches.clear();
    __twinkleReduxPendingPublic.clear();
    const errors=[];
    for (const [store,listeners] of pending) {
        store.__compactRoots();
        errors.push(...store.__notifyPublic(listeners));
    }
    if (errors.length) throw new AggregateError(errors,`${errors.length} Redux subscriber(s) failed after acceptance`);
}
function __twinkleReduxFlushSelectors() {
    const pending=Array.from(__twinkleReduxPendingSelectors);
    __twinkleReduxPendingSelectors.clear();
    __twinkleReduxCallbackPhase='selection';
    try {
        const records=[];
        for (const store of pending) for (const record of store.__selectors()) records.push(record);
        records.sort((left,right)=>(left.owner?.split('/').length??0)-(right.owner?.split('/').length??0));
        for (const record of records) {
            const blocked=record.owner && Array.from(__dirtyComponents).some(path =>
                record.owner !== path && record.owner.startsWith(`${path}/`));
            if (blocked) continue;
            record.listener();
        }
    } finally { __twinkleReduxCallbackPhase='idle'; }
}
function __twinkleReduxPrepareAccept() {
    __twinkleReduxFlushSelectors();
    if (__twinkleReduxNotificationBatches.size) throw Error('Redux notification batch already prepared');
    for (const store of __twinkleReduxPendingPublic)
        __twinkleReduxNotificationBatches.set(store,store.__snapshotListeners());
}

__twinkleRegisterCheckpointParticipant({
    capture() {
        return {stores:Array.from(__twinkleReduxStores, ([id,store]) => [id,store.__snapshot()]),
            pending:Array.from(__twinkleReduxPendingPublic),
            pendingSelectors:Array.from(__twinkleReduxPendingSelectors),
            batches:Array.from(__twinkleReduxNotificationBatches,([store,listeners])=>[store,listeners.slice()])};
    },
    restore(snapshot,original) {
        const retained=new Set(snapshot.stores.map(([id])=>id));
        for (const id of Array.from(__twinkleReduxStores.keys())) if (!retained.has(id)) __twinkleReduxStores.delete(id);
        for (const [id,state] of snapshot.stores) __twinkleReduxStores.get(id)?.__restore(state,original);
        __twinkleReduxPendingPublic.clear();
        for (const store of snapshot.pending) __twinkleReduxPendingPublic.add(store);
        __twinkleReduxPendingSelectors.clear();
        for (const store of snapshot.pendingSelectors) __twinkleReduxPendingSelectors.add(store);
        __twinkleReduxNotificationBatches.clear();
        for (const [store,listeners] of snapshot.batches) __twinkleReduxNotificationBatches.set(store,listeners);
    }
});

function __twinkleReduxCombineReducers(reducers) {
    __twinkleReduxPlainObject(reducers, 'Reducer map');
    const entries = Object.entries(reducers);
    if (!entries.length || entries.some(([,reducer]) => typeof reducer !== 'function'))
        throw TypeError('Reducer map needs functions');
    return (state = {}, action) => {
        let changed = false;
        const next = {};
        for (const [key,reducer] of entries) {
            const value = reducer(state[key], action);
            if (value === undefined) throw TypeError(`Reducer ${key} returned undefined`);
            next[key] = value; changed ||= value !== state[key];
        }
        return changed || Object.keys(state).length !== entries.length ? next : state;
    };
}

const __twinkleReduxContext = createContext(null);
function __twinkleReduxProvider({store,children}) {
    if (!store || typeof store.dispatch !== 'function') throw TypeError('Provider requires a Redux store');
    return h(__twinkleReduxContext.Provider, {value:store}, ...(Array.isArray(children)?children:[children]));
}
function __twinkleReduxUseStore() {
    const store = useContext(__twinkleReduxContext);
    if (!store) throw Error('Redux store is unavailable; add Provider');
    return store;
}
function __twinkleReduxUseDispatch() { return __twinkleReduxUseStore().dispatch; }
function __twinkleReduxRunSelection(callback) {
    const previous=__twinkleReduxCallbackPhase;
    __twinkleReduxCallbackPhase='selection';
    try { return callback(); } finally { __twinkleReduxCallbackPhase=previous; }
}
function __twinkleReduxUseSelector(selector, equality = (left,right) => left === right) {
    if (equality && typeof equality === 'object') {
        __twinkleReduxExactKeys(equality, ['equalityFn'], 'useSelector options');
        equality = equality.equalityFn;
    }
    if (typeof selector !== 'function' || typeof equality !== 'function') throw TypeError('useSelector needs selector and equality functions');
    const store = __twinkleReduxUseStore();
    const owner = __currentComponent;
    const selectorRef = useRef(selector); selectorRef.current = selector;
    const equalityRef = useRef(equality); equalityRef.current = equality;
    const [selected,setSelected] = useState(() => __twinkleReduxRunSelection(()=>selector(store.__getStateUnchecked())));
    const current = __twinkleReduxRunSelection(()=>selector(store.__getStateUnchecked()));
    useEffect(() => {
        const check=()=>{
            const next = __twinkleReduxRunSelection(()=>selectorRef.current(store.__getStateUnchecked()));
            setSelected(previous => __twinkleReduxRunSelection(()=>equalityRef.current(previous,next)) ? previous : next);
        };
        const unsubscribe=store.__subscribeSelector(check,owner);
        check();
        return unsubscribe;
    }, [store]);
    return __twinkleReduxRunSelection(()=>equality(selected,current)) ? selected : current;
}
function __twinkleReduxCreateAction(type) {
    if (typeof type !== 'string' || !type) throw TypeError('Action type must be a nonempty string');
    const creator = payload => ({type,payload}); creator.type=type; creator.toString=()=>type; return creator;
}
function __twinkleReduxCreateSlice(options) {
    __twinkleReduxPlainObject(options, 'Slice options');
    __twinkleReduxExactKeys(options, ['name','initialState','reducers'], 'createSlice');
    const {name,initialState,reducers} = options;
    if (typeof name !== 'string' || !name) throw TypeError('Slice name must be a nonempty string');
    __twinkleReduxPlainObject(reducers, 'Slice reducers');
    const actions = {};
    const cases = new Map();
    for (const [key,caseReducer] of Object.entries(reducers)) {
        if (typeof caseReducer !== 'function') throw TypeError(`Slice reducer ${key} must be a function`);
        const type = `${name}/${key}`;
        actions[key] = __twinkleReduxCreateAction(type);
        cases.set(type,caseReducer);
    }
    return Object.freeze({name,actions:Object.freeze(actions),reducer(state=initialState,action) {
        const caseReducer=cases.get(action?.type);
        if (!caseReducer) return state;
        const next=caseReducer(state,action);
        if (next === undefined) throw TypeError('Twinkle slice reducers must return next state; mutation-style Immer recipes are unsupported');
        return next;
    }});
}

__twinkleReduxUseStore.withTypes = () => __twinkleReduxUseStore;
__twinkleReduxUseDispatch.withTypes = () => __twinkleReduxUseDispatch;
__twinkleReduxUseSelector.withTypes = () => __twinkleReduxUseSelector;

globalThis.__twinkleReduxAfterAccept = __twinkleReduxAfterAccept;
globalThis.__twinkleReduxPrepareAccept = __twinkleReduxPrepareAccept;
Object.defineProperty(globalThis,'__twinkleBeforeRender',{value:__twinkleReduxFlushSelectors,writable:false,configurable:false});
Object.defineProperty(globalThis,'__twinkleExternalWorkPending',{value:()=>__twinkleReduxPendingSelectors.size>0,writable:false,configurable:false});
Object.defineProperty(globalThis,'__twinkleBeginSynchronousHostCall',{value:()=>{__twinkleReduxSynchronousAuthority++;},writable:false,configurable:false});
Object.defineProperty(globalThis,'__twinkleEndSynchronousHostCall',{value:()=>{if (__twinkleReduxSynchronousAuthority>0) __twinkleReduxSynchronousAuthority--;},writable:false,configurable:false});
globalThis.__twinkleReduxExports = Object.freeze({
    createStore:__twinkleReduxCreateStore,
    configureStore(options={}) {
        __twinkleReduxPlainObject(options, 'configureStore options');
        __twinkleReduxExactKeys(options, ['reducer','preloadedState'], 'configureStore');
        let {reducer,preloadedState} = options;
        if (reducer && typeof reducer === 'object') reducer=__twinkleReduxCombineReducers(reducer);
        return __twinkleReduxCreateStore(reducer,preloadedState);
    },
    combineReducers:__twinkleReduxCombineReducers,
    createAction:__twinkleReduxCreateAction,
    createSlice:__twinkleReduxCreateSlice,
    Provider:__twinkleReduxProvider,
    useStore:__twinkleReduxUseStore,
    useDispatch:__twinkleReduxUseDispatch,
    useSelector:__twinkleReduxUseSelector,
    shallowEqual(left,right) {
        const same = (a,b) => a === b ? a !== 0 || 1/a === 1/b : a !== a && b !== b;
        if (same(left,right)) return true;
        if (!left || !right || typeof left !== 'object' || typeof right !== 'object') return false;
        const keys=Object.keys(left); if(keys.length!==Object.keys(right).length)return false;
        return keys.every(key=>Object.prototype.hasOwnProperty.call(right,key)&&same(left[key],right[key]));
    }
});

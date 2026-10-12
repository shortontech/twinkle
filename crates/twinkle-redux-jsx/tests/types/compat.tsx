/// <reference path="../../../twinkle-jsx-runtime/types/twinkle.d.ts" />
/// <reference path="../../types/twinkle-redux.d.ts" />

import {
    configureStore, createAction, createSlice, Provider,
    useDispatch, useSelector, useStore,
} from "twinkle/redux";

const add = createAction<number>("counter/add");
const slice = createSlice({
    name: "counter",
    initialState: { count: 0 },
    reducers: {
        replace: (_state: { count: number }, action: { type: string; payload: number }) => ({ count: action.payload }),
    },
});
const store = configureStore({ reducer: slice.reducer });
const mapped = configureStore({ reducer: { counter: slice.reducer } });
const mappedCount: number = mapped.getState().counter.count;
void mappedCount;
type Root = ReturnType<typeof store.getState>;
type Dispatch = typeof store.dispatch;
const useAppSelector = useSelector.withTypes<Root>();
const useAppDispatch = useDispatch.withTypes<Dispatch>();
const useAppStore = useStore.withTypes<typeof store>();

function Child() {
    const count: number = useAppSelector(state => state.count, { equalityFn: Object.is });
    const dispatch = useAppDispatch();
    dispatch(add(count));
    dispatch(slice.actions.replace(count));
    const sameStore: typeof store = useAppStore();
    return <Text>{String(sameStore.getState().count)}</Text>;
}

export function App() {
    return <Provider store={store}><Child /></Provider>;
}

// @ts-expect-error middleware is deliberately unsupported
configureStore({ reducer: slice.reducer, middleware: [] });
// @ts-expect-error mutation-style recipes returning void are unsupported
createSlice({ name: "bad", initialState: { count: 0 }, reducers: { add(state: { count: number }) { state.count++; } } });

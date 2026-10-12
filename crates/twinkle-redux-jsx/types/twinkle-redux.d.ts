declare module "twinkle/redux" {
    export interface AnyAction { type: string }
    export interface Store<S, A extends AnyAction = AnyAction> {
        getState(): S;
        dispatch(action: A): A;
        subscribe(listener: () => void): () => void;
        replaceReducer(reducer: Reducer<S, A>): void;
    }
    export type Reducer<S, A extends AnyAction = AnyAction> =
        (state: S | undefined, action: A) => S;
    export function createStore<S, A extends AnyAction>(
        reducer: Reducer<S, A>, preloadedState?: S
    ): Store<S, A>;
    export type ReducerMap = Record<string, Reducer<any, any>>;
    export type StateFromReducers<R extends ReducerMap> = {
        [K in keyof R]: R[K] extends Reducer<infer S, any> ? S : never
    };
    export type ActionFromReducers<R extends ReducerMap> = Parameters<R[keyof R]>[1];
    export function combineReducers<R extends ReducerMap>(reducers: R): Reducer<StateFromReducers<R>, ActionFromReducers<R>>;
    export function configureStore<R extends ReducerMap>(options: {
        reducer: R;
        preloadedState?: StateFromReducers<R>;
    }): Store<StateFromReducers<R>, ActionFromReducers<R>>;
    export function configureStore<S, A extends AnyAction = AnyAction>(options: {
        reducer: Reducer<S, A>;
        preloadedState?: S;
    }): Store<S, A>;
    export interface ActionCreator<P = unknown> {
        (payload: P): { type: string; payload: P };
        readonly type: string;
        toString(): string;
    }
    export function createAction<P = unknown>(type: string): ActionCreator<P>;
    export type SliceCaseReducers<S> = Record<string, (state: S, action: any) => S>;
    export type ActionForCase<F> = F extends (state: any, action: infer A) => any
        ? A extends { payload: infer P } ? ActionCreator<P> : ActionCreator<void>
        : never;
    export function createSlice<S, R extends SliceCaseReducers<S>>(options: {
        name: string;
        initialState: S;
        reducers: R;
    }): { readonly name: string; readonly reducer: Reducer<S>; readonly actions: Readonly<{ [K in keyof R]: ActionForCase<R[K]> }> };
    export function Provider<S, A extends AnyAction>(props: {
        store: Store<S, A>;
        children?: TwinkleChild;
    }): JSX.Element;
    export interface StoreHook {
        <S, A extends AnyAction = AnyAction>(): Store<S, A>;
        withTypes<T extends Store<any, any>>(): () => T;
    }
    export interface DispatchHook {
        <A extends AnyAction = AnyAction>(): (action: A) => A;
        withTypes<D extends (action: any) => any>(): () => D;
    }
    export const useStore: StoreHook;
    export const useDispatch: DispatchHook;
    export interface SelectorHook {
        <S, T>(selector: (state: S) => T, equality?: ((left: T, right: T) => boolean) | { equalityFn: (left: T, right: T) => boolean }): T;
        withTypes<S>(): <T>(selector: (state: S) => T, equality?: ((left: T, right: T) => boolean) | { equalityFn: (left: T, right: T) => boolean }) => T;
    }
    export const useSelector: SelectorHook;
    export function shallowEqual(left: unknown, right: unknown): boolean;
}

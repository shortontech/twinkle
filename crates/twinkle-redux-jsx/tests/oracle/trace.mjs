import { createStore, combineReducers } from 'redux';
import { configureStore, createAction, createSlice } from '@reduxjs/toolkit';

const add = createAction('counter/add');
const reducer = (state = { count: 0, label: 'zero' }, action) =>
  action.type === add.type ? { ...state, count: state.count + action.payload } : state;
const store = createStore(reducer);
const initial = store.getState();
const ignored = store.dispatch({ type: 'ignored' });
const ignoredRootShared = store.getState() === initial;
const dispatched = store.dispatch(add(2));

const combined = combineReducers({ counter: reducer, stable: (state = 7) => state });
const combinedInitial = combined(undefined, { type: '@@trace/init' });
const combinedIgnored = combined(combinedInitial, { type: 'ignored' });

const slice = createSlice({
  name: 'value',
  initialState: 1,
  reducers: { replace: (_state, action) => action.payload }
});
const toolkit = configureStore({ reducer: slice.reducer, middleware: () => [] });
toolkit.dispatch(slice.actions.replace(9));

console.log(JSON.stringify({
  redux: {
    initial,
    ignored,
    ignoredRootShared,
    dispatched,
    final: store.getState(),
    combinedIgnoredRootShared: combinedIgnored === combinedInitial
  },
  toolkit: {
    action: add(3),
    actionType: add.type,
    sliceName: slice.name,
    sliceAction: slice.actions.replace(9),
    final: toolkit.getState()
  }
}));

use serde_json::Value;
use std::{hint::black_box, time::Instant};
use twinkle_jsx_runtime::ModuleSource;
use twinkle_redux_jsx::ReduxJsxRuntime;

fn runtime() -> ReduxJsxRuntime {
    let graph = ReduxJsxRuntime::graph(
        "main.jsx",
        [ModuleSource {
            path: "main.jsx",
            source: r#"
import {createStore,Provider,useSelector} from 'twinkle/redux';
globalThis.measuredStore=createStore((state={count:0,other:0},action)=>{
  if(action.type==='add')return {count:state.count+action.by,other:state.other};
  if(action.type==='other')return {count:state.count,other:state.other+1};
  return state;
});
function Counter(){const count=useSelector(state=>state.count);return <Text>{String(count)}</Text>;}
export default function App(){return <Provider store={measuredStore}><Counter/></Provider>;}
"#,
        }],
    )
    .unwrap();
    let mut runtime = ReduxJsxRuntime::new(&graph, None).unwrap();
    let _: Value = runtime
        .render("__twinkleRender()", |value| Ok(value.clone()))
        .unwrap();
    runtime
}

fn measure(mut operation: impl FnMut(), samples: usize) -> (u128, u128) {
    let mut micros = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        operation();
        micros.push(start.elapsed().as_micros());
    }
    micros.sort_unstable();
    (micros[samples / 2], micros[samples * 95 / 100])
}

fn turn(runtime: &mut ReduxJsxRuntime, source: &str) {
    runtime.begin_transaction().unwrap();
    runtime.eval(source).unwrap();
    let value: Value = runtime
        .render("__twinkleRender()", |value| Ok(value.clone()))
        .unwrap();
    black_box(value);
    runtime.prepare_accept().unwrap();
    runtime.finalize_accept().unwrap();
    runtime.after_accept().unwrap();
    assert!(!runtime.effect_turn_pending().unwrap());
}

fn main() {
    println!("scenario,median_us,p95_us");
    let mut noop = runtime();
    let result = measure(
        || turn(&mut noop, "measuredStore.dispatch({type:'noop'})"),
        1_000,
    );
    println!("adapter_noop,{},{}", result.0, result.1);

    let mut leaf = runtime();
    let result = measure(
        || turn(&mut leaf, "measuredStore.dispatch({type:'add',by:1})"),
        1_000,
    );
    println!("adapter_one_leaf,{},{}", result.0, result.1);

    let mut unrelated = runtime();
    let result = measure(
        || turn(&mut unrelated, "measuredStore.dispatch({type:'other'})"),
        1_000,
    );
    println!("adapter_unrelated_slice,{},{}", result.0, result.1);

    let mut burst = runtime();
    let result = measure(
        || {
            turn(
                &mut burst,
                "for(let i=0;i<100;i++)measuredStore.dispatch({type:'add',by:1})",
            )
        },
        250,
    );
    println!("adapter_hundred_dispatch_burst,{},{}", result.0, result.1);
}

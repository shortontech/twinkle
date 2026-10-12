//! Optional bridge scopes, excluded from normal builds and timing runs.
#[cfg(feature = "pipeline-attribution")]
mod enabled {
    use std::{
        cell::Cell,
        fs::{File, OpenOptions},
        io::{BufWriter, Write},
        sync::{Mutex, OnceLock},
        time::Instant,
    };
    static OUTPUT: OnceLock<Option<Mutex<BufWriter<File>>>> = OnceLock::new();
    thread_local! {
        static INPUT_BYTES: Cell<Option<[u64; 2]>> = const { Cell::new(None) };
    }
    pub(crate) struct InputScope {
        previous: Option<[u64; 2]>,
    }
    impl InputScope {
        pub(crate) fn new(active: bool) -> Self {
            Self {
                previous: INPUT_BYTES.replace(active.then_some([0, 0])),
            }
        }
        pub(crate) fn counts(&self) -> [u64; 2] {
            INPUT_BYTES.get().unwrap_or([0, 0])
        }
    }
    impl Drop for InputScope {
        fn drop(&mut self) {
            INPUT_BYTES.set(self.previous);
        }
    }
    pub(crate) fn input_string(text: &str, key: bool) {
        if let Some(mut bytes) = INPUT_BYTES.get() {
            let index = if key { 0 } else { 1 };
            bytes[index] = bytes[index].saturating_add(text.len() as u64);
            INPUT_BYTES.set(Some(bytes));
        }
    }
    fn output() -> Option<&'static Mutex<BufWriter<File>>> {
        OUTPUT
            .get_or_init(|| {
                let path = std::env::var_os("TWINKLE_JSX_TRACE")?;
                match OpenOptions::new().create_new(true).write(true).open(path) {
                    Ok(file) => Some(Mutex::new(BufWriter::new(file))),
                    Err(error) => {
                        eprintln!("JSX attribution output failed: {error}");
                        None
                    }
                }
            })
            .as_ref()
    }
    pub(crate) fn clock() -> Option<Instant> {
        output().map(|_| Instant::now())
    }
    pub(crate) fn row(value: impl FnOnce() -> serde_json::Value) {
        let Some(output) = output() else {
            return;
        };
        if let Ok(mut file) = output.lock() {
            let result = writeln!(file, "{}", value()).and_then(|_| file.flush());
            if let Err(error) = result {
                eprintln!("JSX attribution write failed: {error}");
            }
        }
    }
    #[cfg(test)]
    mod tests {
        use super::{InputScope, input_string};
        #[test]
        fn input_bytes_are_utf8_and_nested_scopes_restore_the_caller() {
            let outer = InputScope::new(true);
            input_string("é", true);
            input_string("🦀", false);
            assert_eq!(outer.counts(), [2, 4]);
            {
                let disabled = InputScope::new(false);
                input_string("ignored", true);
                assert_eq!(disabled.counts(), [0, 0]);
            }
            {
                let inner = InputScope::new(true);
                input_string("界", false);
                assert_eq!(inner.counts(), [0, 3]);
            }
            assert_eq!(outer.counts(), [2, 4]);
            drop(outer);
            input_string("outside", false);
            assert_eq!(InputScope::new(false).counts(), [0, 0]);
        }
    }
}
#[cfg(feature = "pipeline-attribution")]
pub(crate) use enabled::{InputScope, input_string};
pub(crate) fn clock() -> Option<std::time::Instant> {
    #[cfg(feature = "pipeline-attribution")]
    return enabled::clock();
    #[cfg(not(feature = "pipeline-attribution"))]
    None
}
pub(crate) fn elapsed(start: Option<std::time::Instant>) -> Option<f64> {
    start.map(|s| s.elapsed().as_secs_f64() * 1e6)
}
#[cfg(feature = "pipeline-attribution")]
pub(crate) fn configured() -> bool {
    enabled::clock().is_some()
}
#[cfg(feature = "pipeline-attribution")]
pub(crate) fn timestamp_us() -> f64 {
    static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    ORIGIN
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64()
        * 1e6
}
#[cfg(feature = "pipeline-attribution")]
pub(crate) fn serialization(micros: f64, units: f64) {
    enabled::row(
        || serde_json::json!({"event":"json_serialization","pid":std::process::id(),"serviceUs":micros,"outputUtf16Units":units,"scope":"JavaScript JSON.stringify of render envelope only; object construction and patch rendering precede this scope. Nested in v8_call JavaScript time. Opt-in clock callback overhead included; output logging follows the measured interval. UTF16 length is not UTF8 bytes."}),
    );
}
#[cfg(feature = "pipeline-attribution")]
pub(crate) fn bootstrap(source: &str) -> String {
    let helper = r#"
function __twinkleAttributionStringify(value) {
    const start = __twinkleAttributionClock();
    const text = JSON.stringify(value);
    const elapsed = __twinkleAttributionClock() - start;
    __twinkleAttributionSerialization(elapsed, text.length);
    return text;
}
"#;
    format!(
        "{helper}{}",
        source.replace(
            "return JSON.stringify({rendered:",
            "return __twinkleAttributionStringify({rendered:"
        )
    )
}
pub(crate) fn call(
    name: &str,
    conversion: Option<f64>,
    copy: Option<f64>,
    javascript: u64,
    microtask: u64,
    checkpoint: bool,
    values: u64,
    bytes: usize,
    input_bytes: [u64; 2],
) {
    #[cfg(feature = "pipeline-attribution")]
    enabled::row(
        || serde_json::json!({"event":"v8_call","call":name,"pid":std::process::id(),"argumentConversionUs":conversion,"javascriptCallUs":javascript,"microtaskCheckpointUs":microtask,"checkpointSkipped":!checkpoint,"resultStringCopyUs":copy,"argumentValues":values,"argumentKeyUtf8Bytes":input_bytes[0],"argumentStringUtf8Bytes":input_bytes[1],"resultUtf8Bytes":bytes,"scope":"Conversion creates V8 values directly, with no serialized JSON input wire. Input key/string counters measure UTF8 content passed to V8 string creation, not allocated V8 storage. No separate input traversal; opt-in thread-local counting inside conversion. JavaScript call may include JSON serialization, reconciliation and bookkeeping. Result copy is V8 string conversion plus UTF8 Rust copy. Parse recorded separately. Existing bridge aggregate includes other setup/teardown, not just conversion. Not additive to enclosing calls."}),
    );
    #[cfg(not(feature = "pipeline-attribution"))]
    let _ = (
        name,
        conversion,
        copy,
        javascript,
        microtask,
        checkpoint,
        values,
        bytes,
        input_bytes,
    );
}
pub(crate) fn parse(name: &str, micros: u64, bytes: usize) {
    #[cfg(feature = "pipeline-attribution")]
    enabled::row(
        || serde_json::json!({"event":"json_parse","call":name,"pid":std::process::id(),"serviceUs":micros,"inputUtf8Bytes":bytes,"scope":"Rust serde_json decode of returned JSON text; distinct from V8 UTF8 result copy"}),
    );
    #[cfg(not(feature = "pipeline-attribution"))]
    let _ = (name, micros, bytes);
}

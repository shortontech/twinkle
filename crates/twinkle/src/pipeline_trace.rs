//! Opt-in attribution output. Disabled builds perform no clocks, allocations,
//! per-frame file writes or resource walks. Backend identity is a separate
//! startup/transition-only opt-in report available to normal builds.

#[cfg(feature = "pipeline-attribution")]
mod enabled {
    use std::{
        fs::{File, OpenOptions},
        io::{BufWriter, Write},
        sync::{Mutex, OnceLock},
        time::Instant,
    };

    struct Trace {
        writer: BufWriter<File>,
        origin: Instant,
        sequence: u64,
    }
    static TRACE: OnceLock<Option<Mutex<Trace>>> = OnceLock::new();
    static ALLOCATOR_COUNTS: OnceLock<fn() -> Option<[u64; 3]>> = OnceLock::new();

    pub fn install_allocator_counter(counter: fn() -> Option<[u64; 3]>) {
        let _ = ALLOCATOR_COUNTS.set(counter);
    }

    pub(crate) fn allocator_counts() -> Option<[u64; 3]> {
        ALLOCATOR_COUNTS.get().and_then(|counter| counter())
    }

    fn trace() -> Option<&'static Mutex<Trace>> {
        TRACE
            .get_or_init(|| {
                let path = std::env::var_os("TWINKLE_PIPELINE_TRACE")?;
                match OpenOptions::new().write(true).create_new(true).open(path) {
                    Ok(file) => Some(Mutex::new(Trace {
                        writer: BufWriter::new(file),
                        origin: Instant::now(),
                        sequence: 0,
                    })),
                    Err(error) => {
                        eprintln!("cannot create TWINKLE_PIPELINE_TRACE: {error}");
                        None
                    }
                }
            })
            .as_ref()
    }

    pub(crate) fn is_enabled() -> bool {
        trace().is_some()
    }

    pub(crate) fn record(mut value: serde_json::Value) {
        let Some(trace) = trace() else { return };
        let mut trace = trace
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        value["sequence"] = trace.sequence.into();
        value["elapsed_us"] = (trace.origin.elapsed().as_micros() as u64).into();
        value["pid"] = std::process::id().into();
        trace.sequence += 1;
        if let Err(error) = serde_json::to_writer(&mut trace.writer, &value)
            .map_err(std::io::Error::other)
            .and_then(|()| trace.writer.write_all(b"\n"))
            .and_then(|()| trace.writer.flush())
        {
            eprintln!("cannot write TWINKLE_PIPELINE_TRACE: {error}");
        }
    }
}

#[inline]
pub(crate) fn clock() -> Option<std::time::Instant> {
    #[cfg(feature = "pipeline-attribution")]
    if enabled::is_enabled() {
        return Some(std::time::Instant::now());
    }
    None
}

#[inline]
pub(crate) fn micros(start: Option<std::time::Instant>) -> Option<u64> {
    start.map(|start| start.elapsed().as_micros() as u64)
}

#[cfg(feature = "pipeline-attribution")]
pub(crate) use enabled::{is_enabled, record};

#[doc(hidden)]
pub fn install_allocator_counter(counter: fn() -> Option<[u64; 3]>) {
    #[cfg(feature = "pipeline-attribution")]
    enabled::install_allocator_counter(counter);
    #[cfg(not(feature = "pipeline-attribution"))]
    let _ = counter;
}

#[inline]
pub(crate) fn allocator_counts() -> Option<[u64; 3]> {
    #[cfg(feature = "pipeline-attribution")]
    return enabled::allocator_counts();
    #[cfg(not(feature = "pipeline-attribution"))]
    None
}

macro_rules! event {
    ($($value:tt)*) => {
        #[cfg(feature = "pipeline-attribution")]
        if crate::pipeline_trace::is_enabled() {
            crate::pipeline_trace::record(serde_json::json!($($value)*));
        }
    };
}
pub(crate) use event;

// Thread-local counters avoid attributing another thread's traversal to a frame.
// All updates and guards are compiled out of normal builds.
#[cfg(feature = "pipeline-attribution")]
mod work {
    use std::cell::Cell;
    thread_local! {
        static COUNTS: Cell<[u64; 12]> = const { Cell::new([0; 12]) };
        static DEPTH: Cell<u32> = const { Cell::new(0) };
    }
    pub(crate) fn add(index: usize, amount: u64) {
        if DEPTH.get() != 0 {
            COUNTS.with(|counts| {
                let mut value = counts.get();
                value[index] += amount;
                counts.set(value);
            });
        }
    }
    pub(crate) struct Scope {
        name: &'static str,
        start: Option<[u64; 12]>,
    }
    impl Scope {
        pub(crate) fn new(name: &'static str) -> Self {
            let start = super::is_enabled().then(|| {
                DEPTH.set(DEPTH.get() + 1);
                COUNTS.get()
            });
            Self { name, start }
        }
    }
    impl Drop for Scope {
        fn drop(&mut self) {
            let Some(start) = self.start else { return };
            let end = COUNTS.get();
            DEPTH.set(DEPTH.get() - 1);
            let delta: [u64; 12] = std::array::from_fn(|i| end[i] - start[i]);
            super::record(
                serde_json::json!({"event":"native_frame_work","scope":self.name,"layoutElementVisits":delta[0],"recursiveMeasurements":delta[1],"emissionElementVisits":delta[2],"transientStateVisits":delta[3],"idConstructions":delta[4],"idConstructedUtf8Bytes":delta[5],"canonicalMeasurementRequests":delta[6],"overlayMeasurementRequests":delta[7],"measurementOverlayValueReads":delta[8],"measurementChildIdConstructions":delta[9],"measurementChildIdUtf8Bytes":delta[10],"effectiveChildStyleReads":delta[11],"boundary":"Thread-local inclusive counts during frame resolution; visits count function entries including repeated work and reuse checks. IDs count new identity allocations, not Arc clones; string bytes are logical content, not allocator capacity. Measurement overlay reads count value-table access attempts, not hits. Other traversals are not included. Nested scopes overlap."}),
            );
        }
    }
}
#[cfg(feature = "pipeline-attribution")]
pub(crate) use work::{Scope as WorkScope, add as add_work};
macro_rules! count_work {
    ($index:expr, $amount:expr) => {
        #[cfg(feature = "pipeline-attribution")]
        crate::pipeline_trace::add_work($index, $amount);
    };
}
pub(crate) use count_work;

// One emission clock for each outer recursive call, not one clock per node.
#[cfg(feature = "pipeline-attribution")]
pub(crate) struct EmissionScope {
    name: &'static str,
    start: Option<std::time::Instant>,
}
#[cfg(feature = "pipeline-attribution")]
thread_local! {
    static EMISSION_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}
#[cfg(feature = "pipeline-attribution")]
impl EmissionScope {
    pub(crate) fn new(name: &'static str) -> Self {
        let start = (EMISSION_DEPTH.get() == 0).then(clock).flatten();
        EMISSION_DEPTH.set(EMISSION_DEPTH.get() + 1);
        Self { name, start }
    }
}
#[cfg(feature = "pipeline-attribution")]
impl Drop for EmissionScope {
    fn drop(&mut self) {
        EMISSION_DEPTH.set(EMISSION_DEPTH.get() - 1);
        if let Some(start) = self.start {
            record(
                serde_json::json!({"event":"native_emission","scope":self.name,
                "service_us":start.elapsed().as_micros() as u64,
                "boundary":"Outermost recursive emission call; includes reuse checks and fragment output. Nested in native layout/frame resolution. Accessibility geometry has a separate scope. Ancillary post-emission overlay/scrollbar bookkeeping outside these calls remains in enclosing layout; do not sum stage medians."}),
            );
        }
    }
}

/// Startup/transition identity only; no per-frame clocks or resource walks.
pub(crate) fn backend_identity(event: &str, detail: impl FnOnce() -> String) {
    use std::io::Write;
    let Some(path) = std::env::var_os("TWINKLE_BACKEND_REPORT") else {
        return;
    };
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| writeln!(file, "{}\t{}\t{:?}", std::process::id(), event, detail()));
    if let Err(error) = result {
        eprintln!("backend identity report failed: {error}");
    }
}

//! Optional benchmark stage scopes. Enclosing host times include output I/O.
use std::sync::OnceLock;
static OUTPUT: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
pub(super) fn measure<T>(stage: &str, f: impl FnOnce() -> T) -> T {
    let path = OUTPUT.get_or_init(|| {
        if cfg!(feature = "pipeline-attribution") {
            std::env::var_os("TWINKLE_BENCH_JSX_STAGES").map(Into::into)
        } else {
            None
        }
    });
    let Some(path) = path else {
        return f();
    };
    let allocations = super::allocator::counts();
    let epoch_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0);
    let start = std::time::Instant::now();
    let value = f();
    let skipped = stage.ends_with("_skipped");
    let micros = if skipped {
        0.0
    } else {
        start.elapsed().as_secs_f64() * 1e6
    };
    let observed_delta = allocations
        .zip(super::allocator::counts())
        .map(|(a, b)| [b[0] - a[0], b[1] - a[1], b[2] - a[2]]);
    // A skipped scope executes an empty closure. Global allocator counters can
    // still advance on another thread; preserve that observation separately.
    let delta = if skipped {
        observed_delta.map(|_| [0, 0, 0])
    } else {
        observed_delta
    };
    use std::io::Write;
    let row = serde_json::json!({"event":"jsx_stage","stage":stage,"skipped":skipped,"startedEpochUs":epoch_us,"pid":std::process::id(),"serviceUs":micros,"allocationDeltaCallsFreesRequestedBytes":delta,"observedProcessAllocationDeltaCallsFreesRequestedBytes":observed_delta,"boundary":"CPU scope; enclosing stages may include this scope and earlier trace writes; not additive or compositor timing. Active allocation deltas are process-wide and may include concurrent threads. Skipped stages have zero work; concurrent allocator observations are retained separately. Null means allocator instrumentation unavailable."});
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        Ok(mut file) => {
            if let Err(error) = writeln!(file, "{}", row) {
                eprintln!("JSX stage output failed: {error}");
            }
        }
        Err(error) => eprintln!("JSX stage output failed: {error}"),
    }
    value
}

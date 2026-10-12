//! Frozen live comparison workloads for spec 0012.
#[path = "pipeline_workloads/allocator.rs"]
mod allocator;
#[path = "pipeline_workloads/blocks.rs"]
mod blocks;
#[path = "pipeline_workloads/fixtures.rs"]
mod fixtures;
#[path = "pipeline_workloads/replay.rs"]
mod replay;
#[path = "pipeline_workloads/stages.rs"]
mod stages;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "pipeline-allocator")]
    twinkle::install_pipeline_allocator_counter(allocator::counts);
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return twinkle::run(blocks::Blocks::new()?);
    }
    if args.first().map(String::as_str) == Some("--fixture") {
        let name = args.get(1).ok_or("missing fixture name")?;
        let output = args
            .iter()
            .position(|s| s == "--output")
            .and_then(|i| args.get(i + 1))
            .ok_or("missing --output")?;
        if args.iter().any(|s| s == "--headless") {
            return fixtures::headless(name, output.into());
        }
        return twinkle::run_with_adapter(
            fixtures::fixture(name)?,
            fixtures::Capture::new(name.clone(), output.into()),
        );
    }
    if args == ["--validate"] {
        return blocks::original_entrypoint();
    }
    if args.first().map(String::as_str) == Some("--replay") {
        let trace: serde_json::Value =
            serde_json::from_slice(&std::fs::read(args.get(1).ok_or("missing trace path")?)?)?;
        let output = args
            .iter()
            .position(|s| s == "--output")
            .and_then(|i| args.get(i + 1))
            .ok_or("missing --output path")?;
        let adapter =
            replay::Replay::new(trace, output.into(), args.iter().any(|s| s == "--states"))?;
        if args.iter().any(|s| s == "--headless") {
            return adapter.headless();
        }
        return twinkle::run_with_adapter(replay::ReplayBlocks::new()?, adapter);
    }
    Err("usage: twinkle-pipeline-bench [--validate | --fixture NAME --output DIRECTORY [--headless] | --replay TRACE --output DIRECTORY [--headless] [--states]]".into())
}

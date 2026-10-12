//! Allocation-instrumented wrapper around the canonical chart admission harness.

extern crate twinkle as twinkle_ui;

#[path = "pipeline_workloads/allocator.rs"]
#[allow(dead_code)]
mod allocator;
#[path = "../../../twinkle-charts/examples/measure.rs"]
#[allow(dead_code)]
mod measure;

fn main() -> Result<(), twinkle_charts::ChartError> {
    twinkle::install_pipeline_allocator_counter(allocator::counts);
    measure::run_with_allocation_counter(allocator::counts)
}

//! Spec 0021's frame, guard and attribution fixtures. Timing and attribution builds are separate.
use super::*;
use std::time::Instant;
#[path = "measurement_allocator.rs"]
mod allocator;

fn fixture(branches: usize, dense: bool, revision: usize, structural: bool) -> Element<()> {
    let mut root = Row::new()
        .gap(2.0)
        .children((0..branches).map(|index| {
            let branch = Container::new().id(if structural {
                format!("branch-{index}-{revision}")
            } else {
                format!("branch-{index}")
            });
            let paints = [
                InteractionPaint::default(),
                InteractionPaint {
                    width: Some(Length::Px(96.0)),
                    height: Some(Length::Px(48.0)),
                    ..Default::default()
                },
                InteractionPaint::default(),
            ];
            let branch = if dense {
                branch.part_interaction_paints(paints)
            } else if index == 0 {
                branch.message(()).interaction_paints(paints)
            } else {
                branch
            };
            let branch = branch
                .child(
                    Column::new()
                        .child(
                            Text::new(format!("branch {index} primary measurement text"))
                                .wrap(true),
                        )
                        .child(Text::new("secondary measurement text").wrap(true)),
                )
                .into_element();
            branch
        }))
        .into_element();
    if dense {
        root = root.message(());
    }
    root
}

#[test]
#[ignore = "serialized spec 0021 release admission and separate attribution fixture"]
fn measurement_boundary_native_frame_release_admission() {
    crate::install_pipeline_allocator_counter(allocator::counts);
    let case = std::env::var("TWINKLE_MEASUREMENT_CASE").unwrap_or_else(|_| "sparse-cold".into());
    assert!(
        [
            "sparse-cold",
            "sparse-retained",
            "tiny",
            "dense",
            "structural"
        ]
        .contains(&case.as_str())
    );
    let branches = if case == "tiny" { 1 } else { 200 };
    let dense = case == "dense";
    let structural = case == "structural";
    let retained = case != "sparse-cold" && !structural;
    let iterations = 200;
    let bounds = Rect::new(0.0, 0.0, 800.0, 720.0);
    let mut state = UiStateStore::default();
    if !structural {
        state.set_pressed(Some(UiId::from(if dense {
            "root"
        } else {
            "root/branch-0"
        })));
    }
    let mut previous = UiFrame::resolve(
        fixture(branches, dense, 0, structural),
        FrameRequest::new(bounds, &mut state),
    );
    for revision in 1..=10 {
        let declaration = fixture(branches, dense, revision, structural);
        previous = if retained {
            UiFrame::resolve_against_reusing(
                declaration,
                FrameRequest::new(bounds, &mut state),
                &mut previous,
            )
        } else {
            UiFrame::resolve(declaration, FrameRequest::new(bounds, &mut state))
        };
    }
    let mut native = Vec::with_capacity(iterations);
    let mut complete = Vec::with_capacity(iterations);
    let mut allocations = [0u64; 3];
    for iteration in 0..iterations {
        let before = allocator::counts();
        let started = Instant::now();
        let declaration = fixture(branches, dense, iteration + 11, structural);
        let native_started = Instant::now();
        let frame = if retained {
            UiFrame::resolve_against_reusing(
                declaration,
                FrameRequest::new(bounds, &mut state),
                &mut previous,
            )
        } else {
            UiFrame::resolve(declaration, FrameRequest::new(bounds, &mut state))
        };
        native.push(native_started.elapsed().as_secs_f64() * 1_000_000.0);
        complete.push(started.elapsed().as_secs_f64() * 1_000_000.0);
        if let (Some(before), Some(after)) = (before, allocator::counts()) {
            for index in 0..3 {
                allocations[index] += after[index] - before[index];
            }
        }
        std::hint::black_box(frame.resolved_layout().nodes().len());
        previous = frame;
    }
    let ownership = allocator::snapshot();
    let native_total: f64 = native.iter().sum();
    let complete_total: f64 = complete.iter().sum();
    native.sort_by(f64::total_cmp);
    complete.sort_by(f64::total_cmp);
    let nodes = previous.resolved_layout().nodes().len();
    assert_eq!(nodes, branches * 4 + 1);
    let cold = UiFrame::resolve(
        fixture(branches, dense, iterations + 10, structural),
        FrameRequest::new(bounds, &mut state.clone()),
    );
    assert_eq!(previous.commands(), cold.commands());
    assert_eq!(previous.resolved_layout(), cold.resolved_layout());
    eprintln!(
        "measurement_boundary_admission={}",
        serde_json::json!({
            "case": case, "nodes": nodes, "warmups": 10, "iterations": iterations,
            "nativeFrame": {"p50_us":native[iterations/2], "p95_us":native[iterations*95/100-1], "total_us":native_total},
            "completeUpdate": {"p50_us":complete[iterations/2], "p95_us":complete[iterations*95/100-1], "total_us":complete_total},
            "allocationCalls": allocations[0], "deallocationCalls": allocations[1], "requestedBytes": allocations[2],
            "allocatorEnabled": allocator::counts().is_some(), "processOwnership": ownership,
            "scope": "Complete update includes declaration construction. Native frame excludes it. Neither includes a JSX producer. Allocation deltas cover measured calls; process peak includes initialization/warmups and global font caches. Allocator bookkeeping/fragmentation and foreign allocations are excluded."
        })
    );
}

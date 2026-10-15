//! Headless spec 0022 admission through the ordinary, cfg(test)-free UI library.
#[path = "pipeline_workloads/allocator.rs"]
mod allocator;

use std::time::Instant;
use twinkle::backend::PaintCommand;
use twinkle::{
    Align, AnyView, Application, Column, ComponentBuilderExt, Container, Dropdown,
    DropdownPartStyle, FrameRequest, HostBatch, HostEvent, InteractionPaint, Length, Point, Rect,
    Slider, Text, UiEvent, UiFrame, UiHost, UiId, UiStateStore, View, ViewContext,
};

const BASE: u32 = 0xff223344;
const ACTIVE: u32 = 0xffcc1122;

#[derive(Clone)]
struct Controls {
    family: String,
    scenario: String,
    branches: usize,
    options: usize,
    revision: usize,
}

impl Controls {
    fn control(&self, id: String) -> AnyView<()> {
        let paint = InteractionPaint {
            background: Some(ACTIVE),
            font_size: (self.scenario == "font").then_some(22.0),
            ..Default::default()
        };
        let part = DropdownPartStyle {
            width: 120.0,
            height: 24.0,
            font_size: 14.0,
            foreground: Some(0xffffffff),
            background: Some(BASE),
            interaction_paints: [paint; 3],
            ..Default::default()
        };
        let frame = DropdownPartStyle {
            foreground: Some(0xffffffff),
            ..Default::default()
        };
        if self.family == "slider" {
            AnyView::new(
                Slider::new((), 0.5)
                    .parts(
                        part,
                        part,
                        DropdownPartStyle {
                            width: 16.0,
                            height: 16.0,
                            ..part
                        },
                    )
                    .css_frame(frame)
                    .id(id)
                    .width_length(Length::Auto)
                    .height_length(Length::Auto)
                    .shrink(0.0),
            )
        } else {
            let inline = self.scenario == "inline" || self.scenario == "many-options";
            AnyView::new(
                Dropdown::new(
                    (),
                    "Selected label",
                    (0..self.options).map(|i| (format!("Option {i}"), ())),
                )
                .overlay(!inline)
                .open_generation(u64::from(inline))
                .parts(
                    DropdownPartStyle {
                        width: if self.scenario == "font" { 0.0 } else { 120.0 },
                        ..part
                    },
                    part,
                    DropdownPartStyle {
                        width: 16.0,
                        ..part
                    },
                )
                .css_frame(frame)
                .id(id)
                .width_length(Length::Auto)
                .height_length(Length::Auto)
                .shrink(0.0),
            )
        }
    }

    fn fixture(&self) -> AnyView<()> {
        let mut children = vec![self.control("control".into())];
        if self.scenario == "dense" {
            children.extend((0..63).map(|i| self.control(format!("dense-{i}"))));
        }
        children.extend((0..self.branches).map(|i| {
            AnyView::new(
                Container::new()
                    .id(if self.scenario == "structural" {
                        format!("branch-{i}-{}", self.revision)
                    } else {
                        format!("branch-{i}")
                    })
                    .child(
                        Column::new()
                            .child(
                                Text::new(format!("Unrelated primary measurement text {i}"))
                                    .wrap(true),
                            )
                            .child(Text::new("Unrelated secondary measurement text").wrap(true)),
                    ),
            )
        }));
        AnyView::new(Column::new().align_items(Align::Start).children(children))
    }
}

impl Application for Controls {
    type Message = ();
    fn update(&mut self, _: ()) {}
    fn view(&self, _: ViewContext) -> impl View<()> {
        self.fixture()
    }
}

fn distribution(mut samples: Vec<f64>) -> serde_json::Value {
    let total: f64 = samples.iter().sum();
    samples.sort_by(f64::total_cmp);
    let n = samples.len();
    serde_json::json!({"p50_us":samples[(n*50).div_ceil(100)-1],
        "p95_us":samples[(n*95).div_ceil(100)-1],"total_us":total})
}

fn visible_active(commands: &[PaintCommand]) -> bool {
    commands.iter().any(|command| {
        matches!(command,
        PaintCommand::Fill {color,..} | PaintCommand::RoundedFill {color,..} if *color == ACTIVE)
    })
}

fn main() {
    twinkle::install_pipeline_allocator_counter(allocator::counts);
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let arg = |key: &str, default: &str| {
        args.iter()
            .position(|a| a == key)
            .and_then(|i| args.get(i + 1))
            .cloned()
            .unwrap_or_else(|| default.into())
    };
    let family = arg("--family", "slider");
    let mode = arg("--mode", "retained");
    let scenario = arg("--scenario", "paint");
    assert!(["slider", "dropdown"].contains(&family.as_str()));
    assert!(["cold", "retained"].contains(&mode.as_str()));
    assert!(
        [
            "paint",
            "font",
            "inline",
            "many-options",
            "dense",
            "tiny",
            "structural"
        ]
        .contains(&scenario.as_str())
    );
    let iterations: usize = arg("--iterations", "200").parse().unwrap();
    assert!(iterations >= 20);
    let config = Controls {
        family: family.clone(),
        scenario: scenario.clone(),
        branches: if scenario == "tiny" { 0 } else { 200 },
        options: if scenario == "many-options" { 1024 } else { 8 },
        revision: 0,
    };
    let bounds = Rect::new(0.0, 0.0, 800.0, 720.0);
    let mut state = UiStateStore::default();
    let mut host = (mode == "retained").then(|| UiHost::new(config.clone(), 800, 720));
    let mut complete = Vec::with_capacity(iterations);
    let mut native = Vec::with_capacity(iterations);
    let mut layout = Vec::with_capacity(iterations);
    let mut allocation_delta = [0u64; 3];
    let mut active_frames = 0;
    let mut node_count = 0;
    let mut command_count = 0;
    let mut active_control = None;
    let mut inactive_control = None;
    for sample in 0..iterations + 10 {
        let active = scenario != "structural" && sample % 2 == 0;
        let before = allocator::counts();
        let started = Instant::now();
        let (native_us, layout_us) = if mode == "cold" {
            state.set_hovered(active.then(|| UiId::from("root/control")));
            let mut view = config.clone();
            view.revision = sample;
            let declaration = view.fixture();
            let native_started = Instant::now();
            let frame = UiFrame::resolve(declaration, FrameRequest::new(bounds, &mut state));
            let elapsed = native_started.elapsed().as_secs_f64() * 1e6;
            assert_eq!(
                visible_active(frame.commands()),
                active,
                "paint transition must be visible"
            );
            let control = &frame.resolved_layout().nodes()[1];
            let dimensions = [
                control.preferred.width,
                control.preferred.height,
                control.allocated.size.width,
                control.allocated.size.height,
            ];
            if active {
                active_control = Some(dimensions);
            } else {
                inactive_control = Some(dimensions);
            }
            node_count = frame.resolved_layout().nodes().len();
            command_count = frame.commands().len();
            (elapsed, elapsed)
        } else {
            let host = host.as_mut().unwrap();
            // Apply a native-only interaction transaction first, matching the
            // host's production invalidation/retained-paint path, then time a
            // full application publication with that accepted transient state.
            let control = host.resolved_layout().nodes()[1].allocated;
            let point = Point {
                x: control.origin.x + (control.size.width / 2.0).min(4.0),
                y: control.origin.y + (control.size.height / 2.0).min(4.0),
            };
            host.step(HostBatch {
                events: vec![HostEvent::Ui(UiEvent::PointerMoved(if active {
                    point
                } else {
                    Point { x: -1.0, y: -1.0 }
                }))],
                ..Default::default()
            });
            host.application_mut().revision = sample;
            let native_started = Instant::now();
            let outcome = host.step(HostBatch {
                application_changed: true,
                ..Default::default()
            });
            let elapsed = native_started.elapsed().as_secs_f64() * 1e6;
            assert!(
                outcome.telemetry.rebuilt,
                "full native publication must execute"
            );
            assert_eq!(
                visible_active(host.commands()),
                active,
                "paint transition must be visible"
            );
            let control = &host.resolved_layout().nodes()[1];
            let dimensions = [
                control.preferred.width,
                control.preferred.height,
                control.allocated.size.width,
                control.allocated.size.height,
            ];
            if active {
                active_control = Some(dimensions);
            } else {
                inactive_control = Some(dimensions);
            }
            node_count = host.resolved_layout().nodes().len();
            command_count = host.commands().len();
            (elapsed, outcome.telemetry.layout_us as f64)
        };
        let complete_us = started.elapsed().as_secs_f64() * 1e6;
        let after = allocator::counts();
        if sample >= 10 {
            native.push(native_us);
            layout.push(layout_us);
            complete.push(complete_us);
            active_frames += usize::from(active);
            if let (Some(before), Some(after)) = (before, after) {
                for i in 0..3 {
                    allocation_delta[i] += after[i] - before[i];
                }
            }
        }
    }
    let ownership = allocator::snapshot();
    let raw_samples =
        serde_json::json!({"nativeFrame":native,"completeUpdate":complete,"layout":layout});
    println!(
        "compound_admission={}",
        serde_json::json!({
            "schema":1,"family":family,"mode":mode,"scenario":scenario,
            "iterations":iterations,"warmups":10,"activeFrames":active_frames,
            "viewport":[800,720],"nodeCount":node_count,"commandCount":command_count,
        "activeControl":active_control,"inactiveControl":inactive_control,
            "nativeFrame":distribution(native),"completeUpdate":distribution(complete),"layout":distribution(layout),
            "allocatorEnabled":allocator::counts().is_some(),"allocationCalls":allocation_delta[0],
            "deallocationCalls":allocation_delta[1],"requestedBytes":allocation_delta[2],"processOwnership":ownership,"rawSamplesUs":raw_samples,
            "harnessCfgTest":cfg!(test),"scope":"Ordinary library, no cfg(test). Cold native/layout boundary is complete UiFrame resolution. Retained native is forced host publication, layout is host telemetry. Complete retained update additionally includes the preceding interaction transaction. Do not sum nested stages. Normal timing must disable attribution/counting allocator."
        })
    );
}

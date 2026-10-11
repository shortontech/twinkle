#![cfg(feature = "jsx")]
//! Ignored release-profile admission workload for a Blocks-shaped JSX application.
//!
//! Run with:
//! `cargo test --release -p twinkle-workbench --features jsx --test blocks_admission -- --ignored --nocapture`

use std::time::{Duration, Instant};

use serde_json::json;
use twinkle::{Application, HostBatch, HostTelemetry, UiHost, UiId, View, ViewContext};
use twinkle_jsx_runtime::{JsxModuleGraph, JsxRuntime, ModuleSource, ScheduledPatch};
use twinkle_presentation::{
    components::{PluginImages, PluginMessage, RetainedPanelTree, render_retained_panel},
    css::StyleSheet,
};
use twinkle_protocol::SurfaceBounds;

const WIDTH: usize = 10;
const HEIGHT: usize = 20;
const CELL_COUNT: usize = WIDTH * HEIGHT;
const WARMUP_ITERATIONS: usize = 10;
const MEASURED_ITERATIONS: usize = 1_000;

const SOURCE: &str = r#"
    import './fixture.css';
    const Cell = memo(function Cell({color}) {
        return <Div className={color ? 'cell filled ' + color : 'cell'} />;
    });
    export default function App() {
        const board = twinkle.data.board || [];
        return <Window id="main" width={800} height={720} title="Blocks admission" background={0xff101820}>
            <Column className="app">
                <Row className="topbar">
                    <Text className="brand">Blocks admission</Text>
                    <Text>{'Score ' + twinkle.data.score + '  ·  Lines ' + twinkle.data.lines + '  ·  Level ' + twinkle.data.level}</Text>
                </Row>
                <Row className="play">
                    <Div className="board" role="group" aria-label="Game board">{board.map((color, index) => <Cell key={index} color={color} />)}</Div>
                    <Column className="side">
                        <Text className="status">In play</Text>
                        <Text>Next</Text>
                        <Div className="next-piece"><Div className="next-block" /></Div>
                        <Text>Controls</Text>
                        <Text className="hint">Event-driven controls</Text>
                    </Column>
                </Row>
            </Column>
        </Window>;
    }
"#;

const CSS: &str = r#"
    :root { --ink: #eaf2f8; --panel: #1d2a35; --accent: #58d6c5; }
    .app { width: 100%; height: 100%; padding: 18px; gap: 16px; background: #101820; color: var(--ink); }
    .topbar { align-items: center; gap: 14px; padding: 10px; background: var(--panel); border-radius: 8px; }
    .brand { font-size: 22px; flex-grow: 1; }
    .play { gap: 18px; justify-content: center; align-items: start; }
    .board { display: grid; grid-template-columns: repeat(10, 28px); gap: 2px; padding: 8px; background: #263845; border: 2px solid #496274; border-radius: 8px; }
    .cell { width: 28px; height: 28px; background: #17242d; border: 2px solid transparent; border-radius: 3px; }
    .cell.filled { background: var(--accent); border-color: #d3fff8; }
    .cell.c2 { background: #b388ff; border-color: #efe2ff; }
    .side { width: 210px; gap: 9px; padding: 12px; background: var(--panel); border-radius: 8px; }
    .status { padding: 8px; background: #15232d; border-radius: 5px; }
    .next-piece { width: 90px; height: 68px; padding: 12px; background: #15232d; }
    .next-block { width: 30px; height: 30px; background: #b388ff; border: 2px solid #f3eaff; }
    .hint { color: #a9bdc9; }
"#;

const RICH_FORM_SOURCE: &str = r#"
    import './fixture.css';
    const FieldRow = memo(function FieldRow({field, value}) {
        return <Column className="field-row">
            <Text className="label">{field.label}</Text>
            <TextField className={field.multiline ? 'field notes' : 'field'} id={field.id}
                value={value} placeholder={field.placeholder} accessibilityLabel={field.label}
                onChange={() => {}} onFocus={() => {}} onBlur={() => {}} />
        </Column>;
    });
    export default function App() {
        const data = twinkle.data;
        return <Window id="main" width={800} height={720} title="Accessible form admission" background={0xff182028}>
            <Column className="shell">
                <Text className="heading">Account details</Text>
                <Text className="description">A representative accessible form with retained controls and scrolling.</Text>
                <ScrollView id="form-scroll" className="form-scroll" height={500}>
                    <Column className="form" role="group" aria-label="Account details form">
                        {data.fields.map(field => <FieldRow key={field.id} field={field} value={data.values[field.id]} />)}
                        <Checkbox id="updates" checked={data.updates} label="Receive product updates" accessibilityLabel="Receive product updates" onChange={() => {}} />
                        <Slider id="volume" value={data.volume} min={0} max={100} step={1} accessibilityLabel="Notification volume" onChange={() => {}} />
                        <Select id="region" value={data.region} accessibilityLabel="Region" onClick={() => {}}>
                            <Option id="americas" onClick={() => {}}>Americas</Option>
                            <Option id="europe" onClick={() => {}}>Europe</Option>
                            <Option id="asia" onClick={() => {}}>Asia Pacific</Option>
                        </Select>
                        <Row className="actions">
                            <Button id="cancel" className="button" accessibilityLabel="Discard account changes"
                                onClick={() => {}} onFocus={() => {}} onBlur={() => {}} onContextMenu={() => {}}>Cancel</Button>
                            <Button id="save" className="button primary" accessibilityLabel="Save account changes"
                                onClick={() => {}} onFocus={() => {}} onBlur={() => {}} onContextMenu={() => {}}>Save changes</Button>
                        </Row>
                    </Column>
                </ScrollView>
                <Text className="status">{'Revision ' + data.revision + ': ' + data.status}</Text>
            </Column>
        </Window>;
    }
"#;

const RICH_FORM_CSS: &str = r#"
    :root { --ink: #e8eef2; --muted: #aebbc5; --panel: #22303b; --accent: #58d6c5; }
    .shell { width: 100%; height: 100%; padding: 20px; gap: 10px; background: #182028; color: var(--ink); }
    .heading { font-size: 24px; line-height: 30px; }
    .description { color: var(--muted); }
    .form-scroll { width: 100%; height: 500px; padding: 12px; background: var(--panel); border-radius: 8px; }
    .form { width: 100%; gap: 12px; }
    .field-row { width: 100%; gap: 4px; }
    .label { font-size: 14px; color: var(--muted); }
    .field { width: 100%; min-height: 36px; padding: 8px; background: #15212a; border: 1px solid #4d6475; border-radius: 5px; }
    .notes { min-height: 84px; }
    .actions { gap: 10px; justify-content: flex-end; }
    .button { padding: 9px; background: #304655; border-radius: 5px; }
    .primary { background: var(--accent); color: #10201d; }
    .status { padding: 8px; background: #15212a; border-radius: 5px; }
    scrollbar-track { width: 10px; background: #18242d; }
    scrollbar-thumb { width: 8px; background: #668092; border-radius: 4px; }
"#;

#[derive(Default)]
struct StageSample {
    snapshot: Duration,
    patch_reconciliation: Duration,
    patch_admission: Duration,
}

struct JsxAdmission {
    runtime: JsxRuntime,
    accepted: RetainedPanelTree,
    bounds: SurfaceBounds,
    stylesheet: StyleSheet,
    images: PluginImages,
    sample: StageSample,
    changed_view_ids: Option<Vec<UiId>>,
    title: &'static str,
}

impl JsxAdmission {
    fn new(
        source: &'static str,
        css: &'static str,
        initial: serde_json::Value,
        title: &'static str,
    ) -> Result<Self, String> {
        let graph = JsxModuleGraph::new(
            "fixture.tsx",
            [
                ModuleSource {
                    path: "fixture.tsx",
                    source,
                },
                ModuleSource {
                    path: "fixture.css",
                    source: css,
                },
            ],
        )?;
        let stylesheet = StyleSheet::compile(&graph.stylesheet()?)?;
        let bounds = SurfaceBounds::from_json(
            r#"{"surfaces":[{"id":"main","kind":"window","width":800,"height":720}]}"#,
        )?;
        let mut runtime = JsxRuntime::new_modules(&graph, None)?;
        runtime.set_data_value(initial)?;
        let accepted =
            render_retained_panel(&mut runtime, &bounds, Some("main"), "__twinkleRender()", 1)?;
        accepted.validate_candidate(accepted.source(), &bounds, Some("main"), &stylesheet)?;
        Ok(Self {
            runtime,
            accepted,
            bounds,
            stylesheet,
            images: PluginImages::default(),
            sample: StageSample::default(),
            changed_view_ids: None,
            title,
        })
    }

    fn advance(&mut self, snapshot: serde_json::Value) -> Result<(), String> {
        let started = Instant::now();
        self.sample.snapshot = started.elapsed();

        self.runtime.begin_transaction()?;
        let started = Instant::now();
        let outcome = self.runtime.reconcile_data_patched(snapshot);
        self.sample.patch_reconciliation = started.elapsed();
        let mut changed_view_ids = None;
        let result: Result<RetainedPanelTree, String> = (|| {
            let started = Instant::now();
            let mut candidate = self.accepted.clone();
            match outcome? {
                ScheduledPatch::Unchanged => {}
                ScheduledPatch::Patched {
                    patch,
                    transport_bytes,
                    ..
                } => {
                    changed_view_ids = candidate
                        .changed_element_ids(&patch)
                        .map(|ids| ids.into_iter().map(UiId::from).collect());
                    let generation = candidate.generation() + 1;
                    candidate = candidate
                        .apply_patch_owned(
                            &patch,
                            &self.bounds,
                            Some("main"),
                            &self.stylesheet,
                            generation,
                            transport_bytes,
                        )?
                        .0;
                    self.runtime.finish_patch_render(true)?;
                }
            }
            self.sample.patch_admission = started.elapsed();
            if !self.runtime.take_effects()?.is_empty() {
                return Err("snapshot reconciliation emitted an effect".into());
            }
            Ok(candidate)
        })();
        self.runtime.finish_transaction(result.is_ok())?;
        self.accepted = result?;
        self.changed_view_ids = changed_view_ids;
        Ok(())
    }
}

impl Application for JsxAdmission {
    type Message = PluginMessage;

    fn update(&mut self, _message: Self::Message) {}

    fn view(&self, _context: ViewContext) -> impl View<Self::Message> {
        self.accepted.node().view(&self.images, &self.stylesheet)
    }

    fn title(&self) -> &str {
        self.title
    }

    fn take_changed_view_ids(&mut self) -> Option<Vec<UiId>> {
        self.changed_view_ids.take()
    }
}

fn blocks_snapshot(row: usize) -> serde_json::Value {
    let mut board = vec![""; CELL_COUNT];
    let row = row.min(HEIGHT - 2);
    for (x, y) in [(4, row), (3, row + 1), (4, row + 1), (5, row + 1)] {
        board[y * WIDTH + x] = "c2";
    }
    json!({
        "board": board,
        "score": row * 10,
        "lines": row / 2,
        "level": 1 + row / 10,
    })
}

fn rich_form_snapshot(revision: usize) -> serde_json::Value {
    json!({
        "revision": revision,
        "status": if revision.is_multiple_of(2) { "Saved" } else { "Editing" },
        "updates": true,
        "volume": 40 + revision % 20,
        "region": "Americas",
        "fields": [
            {"id": "display-name", "label": "Display name", "placeholder": "Ada Lovelace", "multiline": false},
            {"id": "email", "label": "Email address", "placeholder": "ada@example.test", "multiline": false},
            {"id": "organization", "label": "Organization", "placeholder": "Analytical Engines", "multiline": false},
            {"id": "role", "label": "Role", "placeholder": "Engineer", "multiline": false},
            {"id": "city", "label": "City", "placeholder": "London", "multiline": false},
            {"id": "timezone", "label": "Time zone", "placeholder": "Europe/London", "multiline": false},
            {"id": "website", "label": "Website", "placeholder": "https://example.test", "multiline": false},
            {"id": "notes", "label": "Profile notes", "placeholder": "Notes", "multiline": true}
        ],
        "values": {
            "display-name": format!("Ada Lovelace {revision}"),
            "email": "ada@example.test",
            "organization": "Analytical Engines",
            "role": "Engineer",
            "city": "London",
            "timezone": "Europe/London",
            "website": "https://example.test",
            "notes": "First line of profile notes.\nA second line exercises multiline content."
        }
    })
}

#[derive(Default)]
struct Samples {
    snapshot: Vec<Duration>,
    patch_reconciliation: Vec<Duration>,
    patch_admission: Vec<Duration>,
    native_view: Vec<Duration>,
    native_layout: Vec<Duration>,
    native_frame: Vec<Duration>,
    complete: Vec<Duration>,
    last_telemetry: HostTelemetry,
}

impl Samples {
    fn record(
        &mut self,
        sample: &StageSample,
        native_view: Duration,
        native_layout: Duration,
        native_frame: Duration,
        complete: Duration,
        telemetry: HostTelemetry,
    ) {
        self.snapshot.push(sample.snapshot);
        self.patch_reconciliation.push(sample.patch_reconciliation);
        self.patch_admission.push(sample.patch_admission);
        self.native_view.push(native_view);
        self.native_layout.push(native_layout);
        self.native_frame.push(native_frame);
        self.complete.push(complete);
        self.last_telemetry = telemetry;
    }
}

fn percentile(samples: &[Duration], percentile: usize) -> Duration {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let rank = sorted.len().saturating_mul(percentile).div_ceil(100);
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

fn micros(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000_000.0
}

#[test]
#[ignore = "release-profile Blocks-shaped end-to-end admission workload"]
fn blocks_shaped_jsx_updates_emit_end_to_end_release_distribution() {
    let mut host = UiHost::new(
        JsxAdmission::new(SOURCE, CSS, blocks_snapshot(0), "Blocks admission").expect("fixture"),
        800,
        720,
    );
    let mut row = 0;
    for _ in 0..WARMUP_ITERATIONS {
        row = (row + 1) % (HEIGHT - 1);
        host.application_mut()
            .advance(blocks_snapshot(row))
            .expect("warmup update");
        host.step(HostBatch {
            application_changed: true,
            ..HostBatch::default()
        });
    }

    let mut samples = Samples::default();
    let workload_started = Instant::now();
    for _ in 0..MEASURED_ITERATIONS {
        row = (row + 1) % (HEIGHT - 1);
        let complete_started = Instant::now();
        host.application_mut()
            .advance(blocks_snapshot(row))
            .expect("measured update");
        let native_started = Instant::now();
        let outcome = host.step(HostBatch {
            application_changed: true,
            ..HostBatch::default()
        });
        let native_frame = native_started.elapsed();
        let complete = complete_started.elapsed();
        samples.record(
            &host.application().sample,
            Duration::from_micros(outcome.telemetry.paint_list_us),
            Duration::from_micros(outcome.telemetry.layout_us),
            native_frame,
            complete,
            outcome.telemetry,
        );
        assert_eq!(host.semantic_nodes().len(), 7);
    }
    let workload_elapsed = workload_started.elapsed();
    let average_tick = workload_elapsed / MEASURED_ITERATIONS as u32;
    let ticks_per_second = MEASURED_ITERATIONS as f64 / workload_elapsed.as_secs_f64();
    let resources = host.inspect().resources;
    let accessibility_snapshot_nodes = host.accessibility_nodes().len();
    let resources_after_accessibility = host.inspect().resources;

    let report = json!({
        "schema": 1,
        "suite": "blocks_end_to_end",
        "workload": "200_cell_external_data_republish",
        "metadata": {
            "cells": CELL_COUNT,
            "changedCellsPerTick": 6,
            "changedCellsAtWrap": 8,
            "iterations": MEASURED_ITERATIONS,
            "warmupIterations": WARMUP_ITERATIONS,
            "profile": "release",
            "accessibilitySnapshotNodes": accessibility_snapshot_nodes,
            "accessibilityProjectionBytesAfterSnapshot": resources_after_accessibility.accessibility_projection_bytes,
            "estimatedOwnedBytesAfterSnapshot": resources_after_accessibility.estimated_owned_bytes,
            "lastFrameWork": {
                "nodesMeasured": samples.last_telemetry.nodes_measured,
                "nodesPlaced": samples.last_telemetry.nodes_placed,
                "resolvedNodesReused": samples.last_telemetry.resolved_nodes_reused,
                "paintFragmentsRebuilt": samples.last_telemetry.paint_fragments_rebuilt,
                "paintFragmentsReused": samples.last_telemetry.paint_fragments_reused,
                "semanticNodesRebuilt": samples.last_telemetry.semantic_nodes_rebuilt,
                "semanticNodesReused": samples.last_telemetry.semantic_nodes_reused,
            },
            "lastFrameResources": {
                "nodes": resources.node_count,
                "declarationTreeBytes": resources.declaration_tree_bytes,
                "declarationAllocations": resources.declaration_allocation_count,
                "uiIdSharedBytes": resources.ui_id_shared_storage_bytes,
                "uiIdSharedAllocations": resources.ui_id_shared_allocation_count,
                "declarationEventSidecars": resources.declaration_event_sidecar_count,
                "declarationEventSidecarBytes": resources.declaration_event_sidecar_bytes,
                "declarationKindSidecars": resources.declaration_kind_sidecar_count,
                "declarationKindSidecarBytes": resources.declaration_kind_sidecar_bytes,
                "declarationStyleSidecars": resources.declaration_style_sidecar_count,
                "declarationStyleSidecarBytes": resources.declaration_style_sidecar_bytes,
                "declarationNavigationSidecars": resources.declaration_navigation_sidecar_count,
                "declarationNavigationSidecarBytes": resources.declaration_navigation_sidecar_bytes,
                "transientDeclarationNodesCloned": resources.transient_declaration_nodes_cloned,
                "transientStyleOverlayNodes": resources.transient_style_overlay_count,
                "transientKindOverlayNodes": resources.transient_kind_overlay_count,
                "resolvedSidecars": resources.resolved_sidecar_count,
                "resolvedSidecarBytes": resources.resolved_sidecar_bytes,
                "retainedMetadataSidecars": resources.retained_metadata_sidecar_count,
                "retainedMetadataSidecarBytes": resources.retained_metadata_sidecar_bytes,
                "accessibilityProjectionBytes": resources.accessibility_projection_bytes,
                "estimatedOwnedBytes": resources.estimated_owned_bytes,
            },
        },
        "summary": {
            "ticks": MEASURED_ITERATIONS,
            "total_us": micros(workload_elapsed),
            "average_tick_us": micros(average_tick),
            "ticks_per_second": ticks_per_second,
        },
        "timings": {
            "snapshot": { "p50_us": micros(percentile(&samples.snapshot, 50)), "p95_us": micros(percentile(&samples.snapshot, 95)) },
            "patchReconciliation": { "p50_us": micros(percentile(&samples.patch_reconciliation, 50)), "p95_us": micros(percentile(&samples.patch_reconciliation, 95)) },
            "patchAdmission": { "p50_us": micros(percentile(&samples.patch_admission, 50)), "p95_us": micros(percentile(&samples.patch_admission, 95)) },
            "nativeView": { "p50_us": micros(percentile(&samples.native_view, 50)), "p95_us": micros(percentile(&samples.native_view, 95)) },
            "nativeLayout": { "p50_us": micros(percentile(&samples.native_layout, 50)), "p95_us": micros(percentile(&samples.native_layout, 95)) },
            "nativeFrame": { "p50_us": micros(percentile(&samples.native_frame, 50)), "p95_us": micros(percentile(&samples.native_frame, 95)) },
            "complete": { "p50_us": micros(percentile(&samples.complete, 50)), "p95_us": micros(percentile(&samples.complete, 95)), "p99_us": micros(percentile(&samples.complete, 99)) },
        }
    });
    println!(
        "blocks_ticks={} total_ms={:.3} average_tick_us={:.3} ticks_per_second={:.1}",
        MEASURED_ITERATIONS,
        workload_elapsed.as_secs_f64() * 1_000.0,
        micros(average_tick),
        ticks_per_second,
    );
    println!("twinkle_release_admission={report}");
}

#[test]
#[ignore = "release-profile rich accessible form admission workload"]
fn rich_accessible_form_updates_emit_end_to_end_release_distribution() {
    let mut host = UiHost::new(
        JsxAdmission::new(
            RICH_FORM_SOURCE,
            RICH_FORM_CSS,
            rich_form_snapshot(0),
            "Accessible form admission",
        )
        .expect("fixture"),
        800,
        720,
    );
    for revision in 1..=WARMUP_ITERATIONS {
        host.application_mut()
            .advance(rich_form_snapshot(revision))
            .expect("warmup update");
        host.step(HostBatch {
            application_changed: true,
            ..HostBatch::default()
        });
    }

    let mut samples = Samples::default();
    let workload_started = Instant::now();
    for iteration in 0..MEASURED_ITERATIONS {
        let revision = WARMUP_ITERATIONS + iteration + 1;
        let complete_started = Instant::now();
        host.application_mut()
            .advance(rich_form_snapshot(revision))
            .expect("measured update");
        let native_started = Instant::now();
        let outcome = host.step(HostBatch {
            application_changed: true,
            ..HostBatch::default()
        });
        let native_frame = native_started.elapsed();
        let complete = complete_started.elapsed();
        samples.record(
            &host.application().sample,
            Duration::from_micros(outcome.telemetry.paint_list_us),
            Duration::from_micros(outcome.telemetry.layout_us),
            native_frame,
            complete,
            outcome.telemetry,
        );
        assert!(host.semantic_nodes().len() >= 20);
    }
    let workload_elapsed = workload_started.elapsed();
    let average_tick = workload_elapsed / MEASURED_ITERATIONS as u32;
    let ticks_per_second = MEASURED_ITERATIONS as f64 / workload_elapsed.as_secs_f64();
    let resources = host.inspect().resources;
    let accessibility_snapshot_nodes = host.accessibility_nodes().len();
    let resources_after_accessibility = host.inspect().resources;

    let report = json!({
        "schema": 1,
        "suite": "rich_accessible_form_end_to_end",
        "workload": "eight_fields_controls_scroll_dense_handlers_external_republish",
        "metadata": {
            "fields": 8,
            "iterations": MEASURED_ITERATIONS,
            "warmupIterations": WARMUP_ITERATIONS,
            "profile": "release",
            "semanticNodes": host.semantic_nodes().len(),
            "accessibilitySnapshotNodes": accessibility_snapshot_nodes,
            "accessibilityProjectionBytesAfterSnapshot": resources_after_accessibility.accessibility_projection_bytes,
            "estimatedOwnedBytesAfterSnapshot": resources_after_accessibility.estimated_owned_bytes,
            "lastFrameWork": {
                "nodesMeasured": samples.last_telemetry.nodes_measured,
                "nodesPlaced": samples.last_telemetry.nodes_placed,
                "resolvedNodesReused": samples.last_telemetry.resolved_nodes_reused,
                "paintFragmentsRebuilt": samples.last_telemetry.paint_fragments_rebuilt,
                "paintFragmentsReused": samples.last_telemetry.paint_fragments_reused,
                "semanticNodesRebuilt": samples.last_telemetry.semantic_nodes_rebuilt,
                "semanticNodesReused": samples.last_telemetry.semantic_nodes_reused,
            },
            "lastFrameResources": {
                "nodes": resources.node_count,
                "declarationTreeBytes": resources.declaration_tree_bytes,
                "declarationAllocations": resources.declaration_allocation_count,
                "uiIdSharedBytes": resources.ui_id_shared_storage_bytes,
                "uiIdSharedAllocations": resources.ui_id_shared_allocation_count,
                "declarationEventSidecars": resources.declaration_event_sidecar_count,
                "declarationEventSidecarBytes": resources.declaration_event_sidecar_bytes,
                "declarationKindSidecars": resources.declaration_kind_sidecar_count,
                "declarationKindSidecarBytes": resources.declaration_kind_sidecar_bytes,
                "declarationStyleSidecars": resources.declaration_style_sidecar_count,
                "declarationStyleSidecarBytes": resources.declaration_style_sidecar_bytes,
                "declarationNavigationSidecars": resources.declaration_navigation_sidecar_count,
                "declarationNavigationSidecarBytes": resources.declaration_navigation_sidecar_bytes,
                "transientDeclarationNodesCloned": resources.transient_declaration_nodes_cloned,
                "transientStyleJournalNodes": resources.transient_style_overlay_count,
                "transientKindJournalNodes": resources.transient_kind_overlay_count,
                "resolvedSidecars": resources.resolved_sidecar_count,
                "resolvedSidecarBytes": resources.resolved_sidecar_bytes,
                "retainedMetadataSidecars": resources.retained_metadata_sidecar_count,
                "retainedMetadataSidecarBytes": resources.retained_metadata_sidecar_bytes,
                "accessibilityProjectionBytes": resources.accessibility_projection_bytes,
                "estimatedOwnedBytes": resources.estimated_owned_bytes,
            },
        },
        "summary": {
            "ticks": MEASURED_ITERATIONS,
            "total_us": micros(workload_elapsed),
            "average_tick_us": micros(average_tick),
            "ticks_per_second": ticks_per_second,
        },
        "timings": {
            "snapshot": { "p50_us": micros(percentile(&samples.snapshot, 50)), "p95_us": micros(percentile(&samples.snapshot, 95)) },
            "patchReconciliation": { "p50_us": micros(percentile(&samples.patch_reconciliation, 50)), "p95_us": micros(percentile(&samples.patch_reconciliation, 95)) },
            "patchAdmission": { "p50_us": micros(percentile(&samples.patch_admission, 50)), "p95_us": micros(percentile(&samples.patch_admission, 95)) },
            "nativeView": { "p50_us": micros(percentile(&samples.native_view, 50)), "p95_us": micros(percentile(&samples.native_view, 95)) },
            "nativeLayout": { "p50_us": micros(percentile(&samples.native_layout, 50)), "p95_us": micros(percentile(&samples.native_layout, 95)) },
            "nativeFrame": { "p50_us": micros(percentile(&samples.native_frame, 50)), "p95_us": micros(percentile(&samples.native_frame, 95)) },
            "complete": { "p50_us": micros(percentile(&samples.complete, 50)), "p95_us": micros(percentile(&samples.complete, 95)), "p99_us": micros(percentile(&samples.complete, 99)) },
        }
    });
    println!(
        "rich_form_ticks={} total_ms={:.3} average_tick_us={:.3} ticks_per_second={:.1}",
        MEASURED_ITERATIONS,
        workload_elapsed.as_secs_f64() * 1_000.0,
        micros(average_tick),
        ticks_per_second,
    );
    println!("twinkle_release_admission={report}");
}

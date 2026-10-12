//! Frozen admission scenes copied without removing controls or semantic attributes.
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};
use twinkle::{
    AdapterOutcome, Application, HostAdapter, HostServices, InputModality, Point, UiEvent, UiHost,
    UiId, View, ViewContext,
};
use twinkle_jsx_runtime::{JsxModuleGraph, JsxRuntime, ModuleSource};
use twinkle_presentation::{
    components::{PluginImages, PluginMessage, RetainedPanelTree, render_retained_panel},
    css::StyleSheet,
};
use twinkle_protocol::SurfaceBounds;
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

const PRIMITIVES_SOURCE: &str = r#"
    import './fixture.css';
    const Primitive = memo(function Primitive({index}) {
        return index === 0
            ? <Button className="primitive" accessibilityLabel="Sparse repaint probe" onClick={() => {}} />
            : <Div className="primitive" />;
    });
    export default function App() {
        return <Window id="main" width={800} height={720} title="Primitive scaling fixture" background={0xff101820}>
            <Div className="primitive-grid">{twinkle.data.items.map(index => <Primitive key={index} index={index} />)}</Div>
        </Window>;
    }
"#;

const PRIMITIVES_CSS: &str = r#"
    .primitive-grid { display: grid; grid-template-columns: repeat(32, 8px); gap: 1px; }
    .primitive { width: 8px; height: 8px; background: #58d6c5; }
    button.primitive:hover { background: #b388ff; }
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
                <Button id="sparse-probe" className="button" accessibilityLabel="Sparse repaint probe"
                    onClick={() => {}}>Sparse repaint probe</Button>
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
    text-field.field:hover { background: #1d303c; }
    .notes { min-height: 84px; }
    .actions { gap: 10px; justify-content: flex-end; }
    .button { padding: 9px; background: #304655; border-radius: 5px; }
    button.button:hover { background: #3d596b; }
    .primary { background: var(--accent); color: #10201d; }
    .status { padding: 8px; background: #15212a; border-radius: 5px; }
    scrollbar-track { width: 10px; background: #18242d; }
    scrollbar-thumb { width: 8px; background: #668092; border-radius: 4px; }
"#;

pub(super) struct JsxAdmission {
    runtime: JsxRuntime,
    accepted: RetainedPanelTree,
    stylesheet: StyleSheet,
    images: PluginImages,
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
            stylesheet,
            images: PluginImages::default(),
            changed_view_ids: None,
            title,
        })
    }
}

impl Application for JsxAdmission {
    type Message = PluginMessage;

    fn update(&mut self, _message: Self::Message) {}

    fn view(&self, _context: ViewContext) -> impl View<Self::Message> {
        self.accepted.node().view(&self.images, &self.stylesheet)
    }

    fn initial_size(&self) -> (u32, u32) {
        (800, 720)
    }

    fn title(&self) -> &str {
        self.title
    }

    fn take_changed_view_ids(&mut self) -> Option<Vec<UiId>> {
        self.changed_view_ids.take()
    }
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

pub(super) fn headless(name: &str, output: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(&output)?;
    let path = output.join("fixture.json");
    if path.exists() {
        return Err("refusing to overwrite fixture report".into());
    }
    let mut host = UiHost::new(fixture(name)?, 800, 720);
    let r = host.inspect().resources;
    let d = host.application_mut().runtime.runtime_diagnostics()?;
    fs::write(
        path,
        serde_json::to_vec_pretty(&json!({
            "fixture":name,"nodeCount":r.node_count,"retainedNodes":r.retained_node_count,
            "paintPrimitives":r.paint_primitive_count,"accessibilityNodes":r.accessibility_node_count,
            "expandedOwnedBytes":r.estimated_owned_bytes,"legacySubtotalBytes":r.estimated_retained_bytes,
            "jsxRuntime":d,"logicalViewport":[800,720],"backend":"headless",
            "collectionPolicy":"natural; no forced GC",
            "scope":"Initial unchanged static admission and native resolved frame only; no GPU/software presentation, compositor, or live process timing claim"
        }))?,
    )?;
    Ok(())
}

pub(super) fn fixture(name: &str) -> Result<JsxAdmission, String> {
    match name {
        "empty" => JsxAdmission::new(
            "export default function App(){return <Window id=\"main\" width={800} height={720} title=\"Empty baseline\" background={0xff101820}/>;}",
            "",
            json!({}),
            "Empty baseline",
        ),
        "rich-form" => JsxAdmission::new(
            RICH_FORM_SOURCE,
            RICH_FORM_CSS,
            rich_form_snapshot(0),
            "Accessible form admission",
        ),
        "primitives-64" | "primitives-256" | "primitives-1024" | "primitives-4096" => {
            let count: usize = name
                .strip_prefix("primitives-")
                .expect("matched primitive fixture")
                .parse()
                .map_err(|_| "invalid primitive count")?;
            JsxAdmission::new(
                PRIMITIVES_SOURCE,
                PRIMITIVES_CSS,
                // The window and interactive probe contribute two primitives;
                // keep the reported display-list count exact for this fixture.
                json!({"items": (0..count.saturating_sub(2)).collect::<Vec<_>>() }),
                "Primitive scaling fixture",
            )
        }
        _ => {
            let count: usize = name
                .strip_prefix("cells-")
                .ok_or("unknown fixture")?
                .parse()
                .map_err(|_| "invalid cells")?;
            if ![50, 64, 200, 256, 800, 1_024, 4_096].contains(&count) {
                return Err("unsupported cell count".into());
            }
            let board: Vec<&str> = (0..count)
                .map(|i| if i % 50 == 0 { "c2" } else { "" })
                .collect();
            JsxAdmission::new(
                SOURCE,
                CSS,
                json!({"board":board,"score":0,"lines":0,"level":1}),
                match count {
                    50 => "Twinkle scaling fixture: 50 cells (static)",
                    64 => "Twinkle scaling fixture: 64 cells (static)",
                    200 => "Twinkle scaling fixture: 200 cells (static)",
                    256 => "Twinkle scaling fixture: 256 cells (static)",
                    800 => "Twinkle scaling fixture: 800 cells (static)",
                    1_024 => "Twinkle scaling fixture: 1024 cells (static)",
                    _ => "Twinkle scaling fixture: 4096 cells (static)",
                },
            )
        }
    }
}
pub(super) struct Capture {
    output: PathBuf,
    deadline: Option<Instant>,
    name: String,
    finished: bool,
    sampler_summary: Option<PathBuf>,
    identical_rebuilds_remaining: usize,
    full_rebuilds_remaining: usize,
    identical_rebuild_interval: Duration,
    sparse_pointer_updates_remaining: usize,
    sparse_pointer_inside: bool,
}
impl Capture {
    pub fn new(name: String, output: PathBuf) -> Self {
        Self {
            output,
            deadline: None,
            name,
            finished: false,
            sampler_summary: std::env::var_os("TWINKLE_BENCH_SAMPLER_READY")
                .map(PathBuf::from)
                .and_then(|path| path.parent().map(|parent| parent.join("summary.json"))),
            identical_rebuilds_remaining: std::env::var("TWINKLE_BENCH_IDENTICAL_REBUILDS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            full_rebuilds_remaining: std::env::var("TWINKLE_BENCH_FULL_REBUILDS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            identical_rebuild_interval: Duration::from_millis(
                std::env::var("TWINKLE_BENCH_IDENTICAL_REBUILD_INTERVAL_MS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(16),
            ),
            sparse_pointer_updates_remaining: std::env::var("TWINKLE_BENCH_SPARSE_POINTER_UPDATES")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            sparse_pointer_inside: false,
        }
    }
}
impl HostAdapter<JsxAdmission> for Capture {
    fn started(
        &mut self,
        host: &mut UiHost<JsxAdmission>,
        services: HostServices<'_>,
    ) -> Result<AdapterOutcome, Box<dyn std::error::Error>> {
        fs::create_dir_all(&self.output)?;
        if self.sparse_pointer_updates_remaining > 0 {
            host.adopt_input_modality(InputModality::Pointer);
        }
        if self.output.join("fixture.json").exists() {
            return Err("refusing to overwrite fixture report".into());
        }
        self.deadline = Some(
            Instant::now()
                + if self.sampler_summary.is_some()
                    || self.identical_rebuilds_remaining > 0
                    || self.full_rebuilds_remaining > 0
                    || self.sparse_pointer_updates_remaining > 0
                {
                    Duration::from_millis(100)
                } else {
                    Duration::from_secs(13)
                },
        );
        let size = services.window().surface_size();
        fs::write(
            self.output.join("ready.json"),
            serde_json::to_vec(
                &json!({"physicalViewport":[size.width,size.height],"scale":services.window().scale_factor(),"focused":services.window().has_focus(),"visible":services.window().is_visible()}),
            )?,
        )?;
        Ok(AdapterOutcome::default())
    }
    fn next_deadline(&self, _: Instant) -> Option<Instant> {
        if self.finished { None } else { self.deadline }
    }
    fn poll(
        &mut self,
        host: &mut UiHost<JsxAdmission>,
        services: HostServices<'_>,
    ) -> Result<AdapterOutcome, Box<dyn std::error::Error>> {
        if self.full_rebuilds_remaining > 0 {
            self.full_rebuilds_remaining -= 1;
            self.deadline = Some(Instant::now() + self.identical_rebuild_interval);
            return Ok(AdapterOutcome::changed());
        }
        if self.identical_rebuilds_remaining > 0 {
            self.identical_rebuilds_remaining -= 1;
            self.deadline = Some(Instant::now() + self.identical_rebuild_interval);
            return Ok(AdapterOutcome::present());
        }
        if self.sparse_pointer_updates_remaining > 0 {
            self.sparse_pointer_updates_remaining -= 1;
            self.sparse_pointer_inside = !self.sparse_pointer_inside;
            let point = if self.sparse_pointer_inside {
                let node = host
                    .semantic_nodes()
                    .into_iter()
                    .find(|node| {
                        node.name.as_deref() == Some("Sparse repaint probe")
                            && node.bounds.origin.x >= 0.0
                            && node.bounds.origin.y >= 0.0
                            && node.bounds.origin.x + node.bounds.size.width <= 800.0
                            && node.bounds.origin.y + node.bounds.size.height <= 720.0
                    })
                    .ok_or("sparse pointer fixture target missing")?;
                Point {
                    x: node.bounds.origin.x + node.bounds.size.width * 0.5,
                    y: node.bounds.origin.y + node.bounds.size.height * 0.5,
                }
            } else {
                Point { x: -10.0, y: -10.0 }
            };
            let _ = host.handle_event(UiEvent::PointerMoved(point));
            self.deadline = Some(Instant::now() + self.identical_rebuild_interval);
            return Ok(AdapterOutcome::default());
        }
        // Endpoint ownership walks belong after the process timing window.
        // Waiting checks only file existence; no frame/V8 inspection per poll.
        if self
            .sampler_summary
            .as_ref()
            .is_some_and(|path| !path.exists())
        {
            self.deadline = Some(Instant::now() + Duration::from_millis(100));
            return Ok(AdapterOutcome::default());
        }
        let r = host.inspect().resources;
        let d = host.application_mut().runtime.runtime_diagnostics()?;
        fs::write(
            self.output.join("fixture.json"),
            serde_json::to_vec_pretty(
                &json!({"fixture":self.name,"nodeCount":r.node_count,"retainedNodes":r.retained_node_count,"paintPrimitives":r.paint_primitive_count,"accessibilityNodes":r.accessibility_node_count,"expandedOwnedBytes":r.estimated_owned_bytes,"legacySubtotalBytes":r.estimated_retained_bytes,"scale":services.window().scale_factor(),"focused":services.window().has_focus(),"visible":services.window().is_visible(),"jsxRuntime":d,"collectionPolicy":"natural; no forced GC","scope":"static admission scene; detailed diagnostic snapshot after sampler summary exists, or after 13 seconds without sampler; process total not additive to frame/V8 categories"}),
            )?,
        )?;
        self.finished = true;
        Ok(AdapterOutcome::default())
    }
}

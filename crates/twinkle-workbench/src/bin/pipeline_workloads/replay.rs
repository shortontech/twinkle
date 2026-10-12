//! Fixed-clock input replay. Runtime scheduling is disabled only in this wrapper;
//! each trace poll delegates to the frozen application's production poll method.
use super::allocator;
use super::blocks::Blocks;
use serde_json::{Value, json};
use std::io::Write;
use std::{
    error::Error,
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use twinkle::{
    AdapterOutcome, Application, ApplicationControllerEvent, ApplicationControllerInput,
    ControllerButton, ControllerFamily, ControllerId, ControllerInputOutcome, ControllerSnapshot,
    HostAdapter, HostBatch, HostEvent, HostServices, KeyEdge, SemanticAction, Shortcut,
    ShortcutOutcome, UiEvent, UiHost, UiId, View, ViewContext,
};
use twinkle_presentation::components::PluginMessage;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub(super) struct ReplayBlocks {
    pub inner: Blocks,
    focused: bool,
    visible: bool,
}
impl ReplayBlocks {
    pub fn new() -> Result<Self> {
        Ok(Self {
            inner: Blocks::new()?,
            focused: true,
            visible: true,
        })
    }
    fn state(&self, input: &ControllerSnapshot) -> Value {
        let mut state = self.inner.benchmark_state();
        state["focused"] = self.focused.into();
        state["visible"] = self.visible.into();
        state["held"] = json!({"left": input.button_down(&ControllerButton::DPadLeft),
            "right": input.button_down(&ControllerButton::DPadRight), "down": input.button_down(&ControllerButton::DPadDown)});
        state
    }
}
impl Application for ReplayBlocks {
    type Message = PluginMessage;
    fn update(&mut self, m: PluginMessage) {
        self.inner.update(m);
    }
    fn view(&self, c: ViewContext) -> impl View<PluginMessage> {
        self.inner.view(c)
    }
    fn title(&self) -> &str {
        "Twinkle Blocks — fixed trace"
    }
    fn initial_size(&self) -> (u32, u32) {
        (800, 720)
    }
    fn controller_input(&mut self, e: ApplicationControllerEvent) -> ControllerInputOutcome {
        self.inner.controller_input(e)
    }
    fn shortcut_outcome(&mut self, s: Shortcut) -> ShortcutOutcome {
        self.inner.shortcut_outcome(s)
    }
    fn take_changed_view_ids(&mut self) -> Option<Vec<UiId>> {
        self.inner.take_changed_view_ids()
    }
    fn poll_controller(&mut self, c: &ControllerSnapshot) -> bool {
        self.focused && self.visible && self.inner.poll_controller(c)
    }
    // No autonomous model clock. Native caret/animation and presentation retain
    // their own runtime paths; frozen events supply every application poll.
    fn poll_interval_controller(&self, _: &ControllerSnapshot) -> Option<Duration> {
        None
    }
}

pub(super) struct Replay {
    trace: Value,
    output: PathBuf,
    pub warmup: Duration,
    origin: Option<Instant>,
    sampler_ready: Option<PathBuf>,
    alignment: Option<Value>,
    waiting_deadline: Instant,
    index: usize,
    records: Vec<Value>,
    collect_states: bool,
    finished: bool,
    ownership: bool,
    native_window_context: Option<Value>,
    exit_after_report: bool,
}
impl Replay {
    pub fn new(trace: Value, output: PathBuf, collect_states: bool) -> Result<Self> {
        if std::env::var_os("TWINKLE_BENCH_OWNERSHIP").is_some()
            && !cfg!(feature = "pipeline-allocator")
        {
            return Err("ownership runs require pipeline-allocator feature".into());
        }
        fs::create_dir_all(&output)?;
        if output.join("replay.json").exists() {
            return Err("refusing to overwrite a replay".into());
        }
        let count = trace["steps"].as_array().ok_or("trace has no steps")?.len();
        if count == 0 || count > 100_000 {
            return Err("trace step count outside bounds".into());
        }
        let mut prior = 0;
        for step in trace["steps"].as_array().unwrap() {
            let at = step["atMs"].as_u64().ok_or("invalid trace timestamp")?;
            if at < prior || at > 600_000 {
                return Err("trace is unordered or too long".into());
            }
            prior = at;
        }
        Ok(Self {
            trace,
            output,
            warmup: Duration::from_secs(3),
            origin: None,
            sampler_ready: std::env::var_os("TWINKLE_BENCH_SAMPLER_READY").map(PathBuf::from),
            alignment: None,
            waiting_deadline: Instant::now(),
            index: 0,
            records: Vec::with_capacity(count),
            collect_states,
            finished: false,
            ownership: std::env::var_os("TWINKLE_BENCH_OWNERSHIP").is_some(),
            native_window_context: None,
            exit_after_report: std::env::var_os("TWINKLE_BENCH_EXIT_AFTER_REPORT").is_some(),
        })
    }
    fn neutralize(host: &mut UiHost<ReplayBlocks>, order: usize) {
        host.handle_application_controller_input(ApplicationControllerEvent {
            device: ControllerId(1),
            family: ControllerFamily::Xbox,
            backend_order: order as u64,
            produced_unix_ms: order as u64,
            input: ApplicationControllerInput::RouteNeutralized,
        });
    }
    pub fn step(
        &mut self,
        host: &mut UiHost<ReplayBlocks>,
        index: usize,
        actual_ms: f64,
    ) -> Result<()> {
        let step = &self.trace["steps"][index];
        let kind = step["kind"].as_str().ok_or("trace step lacks kind")?;
        // Semantic lookup and optional observer snapshots are outside the service
        // timer. They are disclosed harness work, not counted as native dispatch.
        let semantic = if kind == "click" {
            let name = match step["target"].as_str() {
                Some("restart") => "New game",
                Some("continue") => "Continue",
                _ => return Err("unknown click target".into()),
            };
            Some(
                host.semantic_nodes()
                    .into_iter()
                    .find(|node| node.name.as_deref() == Some(name))
                    .ok_or("semantic target missing")?
                    .id,
            )
        } else {
            None
        };
        let before_observer_started = Instant::now();
        let before = self
            .collect_states
            .then(|| host.application().state(host.controller_snapshot()));
        let before_observer_us = before_observer_started.elapsed().as_secs_f64() * 1e6;
        let generation_before = host.resolved_frame_generation();
        let accepted_before = host.application().inner.benchmark_generation();
        let allocation_before = if self.ownership {
            allocator::counts()
        } else {
            None
        };
        let started = Instant::now();
        let attempts_before = host.application().inner.benchmark_attempts();
        match kind {
            "poll" => {
                host.step(HostBatch {
                    events: vec![HostEvent::Poll],
                    ..HostBatch::default()
                });
            }
            "button" => {
                let button = match step["button"].as_str() {
                    Some("left") => ControllerButton::DPadLeft,
                    Some("right") => ControllerButton::DPadRight,
                    Some("down") => ControllerButton::DPadDown,
                    Some("rotate") => ControllerButton::South,
                    Some("drop") => ControllerButton::East,
                    _ => return Err("unknown controller button".into()),
                };
                let edge = if step["pressed"].as_bool().ok_or("missing button edge")? {
                    KeyEdge::Pressed
                } else {
                    KeyEdge::Released
                };
                let outcome =
                    host.handle_application_controller_input(ApplicationControllerEvent {
                        device: ControllerId(1),
                        family: ControllerFamily::Xbox,
                        backend_order: index as u64 + 1,
                        produced_unix_ms: step["atMs"].as_u64().unwrap(),
                        input: ApplicationControllerInput::Button {
                            button,
                            edge,
                            repeat: false,
                        },
                    });
                let _ = outcome;
            }
            "menu" => {
                host.step(HostBatch {
                    events: vec![HostEvent::Shortcut(Shortcut::Escape)],
                    ..HostBatch::default()
                });
            }
            "click" => {
                host.perform_semantic_action(
                    semantic.unwrap(),
                    SemanticAction::Invoke(twinkle::ActionKind::Activate),
                );
            }
            "focus" | "visibility" => {
                let value = step["value"].as_bool().ok_or("missing focus/visibility")?;
                if kind == "focus" {
                    host.application_mut().focused = value;
                    host.step(HostBatch {
                        window_focused: Some(value),
                        ..HostBatch::default()
                    });
                } else {
                    host.application_mut().visible = value;
                    if !value {
                        host.handle_event(UiEvent::Suspended);
                    }
                }
                if !value {
                    Self::neutralize(host, index + 1);
                }
            }
            "observe" => {}
            _ => return Err(format!("unsupported trace kind {kind}").into()),
        }
        let allocation_after = if self.ownership {
            allocator::counts()
        } else {
            None
        };
        let allocation_delta = allocation_before
            .zip(allocation_after)
            .map(|(a, b)| [b[0] - a[0], b[1] - a[1], b[2] - a[2]]);
        let service_us = started.elapsed().as_secs_f64() * 1e6;
        let observation_started = Instant::now();
        let state = self
            .collect_states
            .then(|| host.application().state(host.controller_snapshot()));
        if self.ownership && (kind == "observe" || step["label"] == "cycle-restart") {
            let allocation = allocator::snapshot();
            let resources = host_resources(host);
            let diagnostics = host.application_mut().inner.benchmark_diagnostics()?;
            let retained = diagnostics["retained"].clone();
            let engine = diagnostics["engine"].clone();
            drop(diagnostics);
            let row = json!({"index":index,"label":step["label"],"cycle":step["cycle"],"allocator":allocation,"resources":resources,"retainedJsx":retained,"v8":engine,"harnessStoredSteps":self.records.len(),"policy":"streamed attribution; no forced GC; observers outside service timing"});
            let mut f = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.output.join("ownership.jsonl"))?;
            writeln!(f, "{}", row)?;
        }
        let observation_us = observation_started.elapsed().as_secs_f64() * 1e6;
        self.records.push(json!({"index":index,"atMs":step["atMs"],"label":step["label"],"kind":kind,
            "allocationDeltaCallsFreesRequestedBytes":allocation_delta,"actualMs":actual_ms,"serviceUs":service_us,"observerUs":before_observer_us+observation_us,
            "poll":kind=="poll","attemptedPublication":host.application().inner.benchmark_attempts()>attempts_before,
            "publicationAttempts":host.application().inner.benchmark_attempts()-attempts_before,
            "acceptedGenerationBefore":accepted_before,"acceptedGenerationAfter":host.application().inner.benchmark_generation(),
            "resolvedGenerationBefore":generation_before,"resolvedGenerationAfter":host.resolved_frame_generation(),
            "observableChanged": before.as_ref().zip(state.as_ref()).map(|(before,after)|before["presentation"]!=after["presentation"]),
            "state":state}));
        if self.ownership {
            let mut f = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.output.join("steps.jsonl"))?;
            writeln!(f, "{}", self.records.last().unwrap())?;
            self.records.clear();
        }
        Ok(())
    }
    pub fn finish(&mut self, host: &mut UiHost<ReplayBlocks>) -> Result<()> {
        let resources = self.collect_states.then(|| host.inspect().resources);
        let diagnostics = if self.collect_states {
            Some(host.application_mut().inner.benchmark_diagnostics()?)
        } else {
            None
        };
        let report = json!({"schema":1,"status":"complete","implementation":"twinkle","trace":self.trace,
            "streamedSteps":self.ownership,"totalTraceSteps":self.trace["steps"].as_array().unwrap().len(),"allocator":allocator::snapshot(),"alignment":self.alignment,"steps":self.records,"finalState":host.application().state(host.controller_snapshot()),
            "nativeResources":resources.map(|resources| json!({
                "nodeCount":resources.node_count,"retainedNodes":resources.retained_node_count,
                "paintPrimitives":resources.paint_primitive_count,"accessibilityNodes":resources.accessibility_node_count,
                "legacySubtotalBytes":resources.estimated_retained_bytes,"expandedOwnedBytes":resources.estimated_owned_bytes,
                "peakOverlapBytes":resources.peak_overlapping_generation_bytes,"buildScratchBytes":resources.retained_build_scratch_bytes,
                "declarationBytes":resources.declaration_tree_bytes,"sharedUiIdBytes":resources.ui_id_shared_storage_bytes,
                "resolvedBytes":resources.resolved_storage_bytes,"retainedArenaBytes":resources.retained_arena_bytes,
                "retainedIdIndexBytes":resources.retained_id_index_bytes,"hashTableOverheadBytes":resources.hash_table_overhead_bytes,
                "paintStorageBytes":resources.paint_storage_bytes,"interactionBytes":resources.interaction_storage_bytes,
                "indexBytes":resources.index_storage_bytes,"accessibilityBytes":resources.accessibility_storage_bytes
            })),"jsxRuntime":diagnostics,
            "logicalViewport":[800,720],"scale":self.native_window_context.as_ref().and_then(|window| window.get("scale")).cloned().unwrap_or(json!(1)),"nativeWindow":self.native_window_context,
            "resourceSnapshotBoundary":"Detailed end ownership snapshot only in state/attribution runs; normal runs retain startup node counts captured before warmup and perform no end diagnostic heap walk.",
            "collectionPolicy":"natural; no forced GC or V8 diagnostic heap snapshot",
            "timingBoundary":"serviceUs includes canonical host dispatch through resolved native frame; excludes semantic lookup, observer, serialization, GPU presentation and compositor display",
            "presentationTimestamp":null});
        fs::write(
            self.output.join("replay.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        self.finished = true;
        Ok(())
    }
    pub fn headless(mut self) -> Result<()> {
        let mut host = UiHost::new(ReplayBlocks::new()?, 800, 720);
        for index in 0..self.trace["steps"].as_array().unwrap().len() {
            self.step(&mut host, index, 0.0)?;
        }
        self.finish(&mut host)
    }
}
impl HostAdapter<ReplayBlocks> for Replay {
    fn started(
        &mut self,
        _host: &mut UiHost<ReplayBlocks>,
        _services: HostServices<'_>,
    ) -> Result<AdapterOutcome> {
        if self.sampler_ready.is_none() {
            self.origin = Some(Instant::now() + self.warmup);
        }
        let size = _services.window().surface_size();
        self.native_window_context = Some(window_context(&_services));
        let initial = host_resources(_host);
        fs::write(
            self.output.join("ready.json"),
            serde_json::to_vec(
                &json!({"ready":true,"physicalViewport":[size.width,size.height],"nativeWindow":self.native_window_context,"initialResources":initial}),
            )?,
        )?;
        Ok(AdapterOutcome::default())
    }
    fn next_deadline(&self, _now: Instant) -> Option<Instant> {
        if self.finished {
            return (self.exit_after_report && self.sampler_ready.is_some())
                .then_some(self.waiting_deadline);
        }
        if self.origin.is_none() {
            return Some(self.waiting_deadline);
        }
        self.origin.map(|origin| {
            origin
                + Duration::from_millis(
                    if self.index < self.trace["steps"].as_array().unwrap().len() {
                        self.trace["steps"][self.index]["atMs"].as_u64().unwrap()
                    } else {
                        self.trace["durationMs"].as_u64().unwrap()
                    },
                )
        })
    }
    fn poll(
        &mut self,
        host: &mut UiHost<ReplayBlocks>,
        _services: HostServices<'_>,
    ) -> Result<AdapterOutcome> {
        if self.finished && self.exit_after_report {
            let sampling_complete = self.sampler_ready.as_ref().is_none_or(|path| {
                path.parent()
                    .is_some_and(|parent| parent.join("summary.json").exists())
            });
            if sampling_complete {
                return Ok(AdapterOutcome::exit());
            }
            self.waiting_deadline = Instant::now() + Duration::from_millis(100);
            return Ok(AdapterOutcome::default());
        }
        if self.origin.is_none() {
            let path = self.sampler_ready.as_ref().ok_or("missing sampler clock")?;
            if !path.exists() {
                self.waiting_deadline = Instant::now() + Duration::from_millis(10);
                return Ok(AdapterOutcome::default());
            }
            let clock: Value = serde_json::from_slice(&fs::read(path)?)?;
            let epoch_ms = clock["samplingStartEpochMs"]
                .as_f64()
                .ok_or("invalid sampler epoch")?
                + clock["warmupMs"].as_f64().ok_or("invalid sampler warmup")?;
            let wall_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64() * 1000.0;
            if epoch_ms <= wall_ms {
                return Err("sampler origin elapsed before native window ready".into());
            }
            self.origin =
                Some(Instant::now() + Duration::from_secs_f64((epoch_ms - wall_ms) / 1000.0));
            self.alignment =
                Some(json!({"sampler":clock,"originEpochMs":epoch_ms,"readyEpochMs":wall_ms}));
        }
        let origin = self.origin.unwrap();
        let now = Instant::now();
        while self.index < self.trace["steps"].as_array().unwrap().len()
            && now
                >= origin
                    + Duration::from_millis(
                        self.trace["steps"][self.index]["atMs"].as_u64().unwrap(),
                    )
        {
            self.step(
                host,
                self.index,
                now.saturating_duration_since(origin).as_secs_f64() * 1000.0,
            )?;
            self.index += 1;
        }
        if self.index == self.trace["steps"].as_array().unwrap().len()
            && now
                >= origin
                    + Duration::from_millis(
                        self.trace["durationMs"]
                            .as_u64()
                            .ok_or("missing duration")?,
                    )
        {
            self.native_window_context = Some(window_context(&_services));
            self.finish(host)?;
            self.waiting_deadline = Instant::now() + Duration::from_millis(100);
            return Ok(if self.exit_after_report && self.sampler_ready.is_none() {
                AdapterOutcome::exit()
            } else {
                AdapterOutcome::default()
            });
        }
        Ok(AdapterOutcome::default())
    }
}

fn window_context(services: &HostServices<'_>) -> Value {
    let window = services.window();
    let size = window.surface_size();
    json!({"physicalViewport":[size.width,size.height],"scale":window.scale_factor(),
        "focused":window.has_focus(),"visible":window.is_visible(),
        "boundary":"Native window query at startup/completion, separate from virtual focus/visibility flags in the controlled input trace. Visibility may be unavailable; occlusion and physical monitor display timing are unavailable."})
}

fn host_resources(host: &UiHost<ReplayBlocks>) -> Value {
    let r = host.inspect().resources;
    json!({"nodeCount":r.node_count,"retainedNodes":r.retained_node_count,"paintPrimitives":r.paint_primitive_count,
        "legacySubtotalBytes":r.estimated_retained_bytes,"expandedOwnedBytes":r.estimated_owned_bytes})
}

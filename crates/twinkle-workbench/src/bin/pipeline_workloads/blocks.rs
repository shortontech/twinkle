// Frozen live Blocks workload for spec 0012. See provenance.json.
use serde_json::{Value, json};
use std::time::Duration;
use twinkle as twinkle_ui;
use twinkle_jsx_runtime::{JsxModuleGraph, JsxRuntime, ModuleSource, ScheduledPatch};
use twinkle_presentation::{
    components::{PluginImages, PluginMessage, RetainedPanelTree, render_retained_panel},
    css::StyleSheet,
};
use twinkle_protocol::SurfaceBounds;
use twinkle_ui::{
    Application, ApplicationControllerEvent, ApplicationControllerInput, ControllerAxis,
    ControllerButton, ControllerFamily, ControllerId, ControllerInputOutcome, ControllerSnapshot,
    KeyEdge, Shortcut, ShortcutOutcome, UiId, View, ViewContext,
};

const W: usize = 10;
const H: usize = 20;
const SHAPES: [[(i32, i32); 4]; 7] = [
    [(0, 0), (1, 0), (0, 1), (1, 1)],
    [(0, 0), (1, 0), (2, 0), (3, 0)],
    [(0, 0), (1, 0), (2, 0), (1, 1)],
    [(0, 0), (0, 1), (1, 1), (2, 1)],
    [(2, 0), (0, 1), (1, 1), (2, 1)],
    [(1, 0), (2, 0), (0, 1), (1, 1)],
    [(0, 0), (1, 0), (1, 1), (2, 1)],
];
const COLORS: [&str; 7] = [
    "#f4d35e", "#4cc9f0", "#b388ff", "#80ed99", "#ff9f1c", "#ff5d8f", "#72efdd",
];

#[derive(Clone)]
struct Game {
    cells: Vec<Option<u8>>,
    shape: u8,
    x: i32,
    y: i32,
    rotation: u8,
    score: u32,
    lines: u32,
    next: u8,
    bag: [u8; 7],
    bag_index: usize,
    random_state: u32,
    paused: bool,
    over: bool,
}

impl Default for Game {
    fn default() -> Self {
        Self {
            cells: vec![None; W * H],
            shape: 1,
            x: 3,
            y: 0,
            rotation: 0,
            score: 0,
            lines: 0,
            next: 2,
            bag: [6, 5, 0, 3, 4, 1, 2],
            bag_index: 0,
            random_state: 0x6d2b_79f5,
            paused: false,
            over: false,
        }
    }
}

impl Game {
    fn draw_piece(&mut self) -> u8 {
        if self.bag_index == self.bag.len() {
            self.bag = [0, 1, 2, 3, 4, 5, 6];
            for index in (1..self.bag.len()).rev() {
                // A tiny deterministic xorshift generator is sufficient here:
                // the bag supplies the distribution guarantee, while this only
                // chooses its permutation without adding an application dependency.
                self.random_state ^= self.random_state << 13;
                self.random_state ^= self.random_state >> 17;
                self.random_state ^= self.random_state << 5;
                let other = self.random_state as usize % (index + 1);
                self.bag.swap(index, other);
            }
            self.bag_index = 0;
        }
        let piece = self.bag[self.bag_index];
        self.bag_index += 1;
        piece
    }

    fn shape_coords(shape: u8, rotation: u8) -> [(i32, i32); 4] {
        if shape == 0 {
            return SHAPES[0];
        }
        if shape == 1 {
            return match rotation % 4 {
                0 => [(0, 1), (1, 1), (2, 1), (3, 1)],
                1 => [(2, 0), (2, 1), (2, 2), (2, 3)],
                2 => [(0, 2), (1, 2), (2, 2), (3, 2)],
                _ => [(1, 0), (1, 1), (1, 2), (1, 3)],
            };
        }
        SHAPES[shape as usize].map(|(x, y)| match rotation % 4 {
            0 => (x, y),
            1 => (2 - y, x),
            2 => (2 - x, 2 - y),
            _ => (y, 2 - x),
        })
    }

    fn coords(&self) -> [(i32, i32); 4] {
        Self::shape_coords(self.shape, self.rotation)
    }

    fn fits(&self, x: i32, y: i32, rotation: u8) -> bool {
        Self::shape_coords(self.shape, rotation)
            .iter()
            .all(|&(px, py)| {
                let (cx, cy) = (x + px, y + py);
                cx >= 0
                    && cx < W as i32
                    && cy < H as i32
                    && (cy < 0 || self.cells[cy as usize * W + cx as usize].is_none())
            })
    }

    fn lock_piece(&mut self) {
        for (px, py) in self.coords() {
            let (x, y) = (self.x + px, self.y + py);
            if y >= 0 && y < H as i32 && x >= 0 && x < W as i32 {
                self.cells[y as usize * W + x as usize] = Some(self.shape);
            }
        }
        let mut kept = Vec::with_capacity(H * W);
        let mut cleared = 0;
        for row in self.cells.chunks_exact(W) {
            if row.iter().all(Option::is_some) {
                cleared += 1;
            } else {
                kept.extend_from_slice(row);
            }
        }
        for _ in 0..cleared {
            kept.splice(0..0, std::iter::repeat_n(None, W));
        }
        self.cells = kept;
        self.lines += cleared as u32;
        self.score += [0, 100, 300, 500, 800][cleared.min(4)] * (self.level());
        self.shape = self.next;
        self.next = self.draw_piece();
        self.x = 3;
        self.y = 0;
        self.rotation = 0;
        self.over = !self.fits(self.x, self.y, self.rotation);
    }

    fn level(&self) -> u32 {
        1 + self.lines / 10
    }

    fn gravity_interval(&self) -> Duration {
        Duration::from_millis(
            700_u64
                .saturating_sub(u64::from(self.level() - 1) * 55)
                .max(90),
        )
    }

    fn soft_drop_interval(&self) -> Duration {
        Duration::from_millis((self.gravity_interval().as_millis() as u64 / 12).max(25))
    }

    fn gravity_tick(&mut self) {
        if self.fits(self.x, self.y + 1, self.rotation) {
            self.y += 1;
        } else {
            self.lock_piece();
        }
    }

    fn act(&mut self, action: &str) {
        if action == "restart" {
            *self = Self {
                paused: false,
                ..Self::default()
            };
            return;
        }
        if action == "toggle" {
            if !self.over {
                self.paused = !self.paused;
            }
            return;
        }
        if self.paused || self.over {
            return;
        }
        match action {
            "left" if self.fits(self.x - 1, self.y, self.rotation) => self.x -= 1,
            "right" if self.fits(self.x + 1, self.y, self.rotation) => self.x += 1,
            "rotate" => {
                if self.shape == 0 {
                    return;
                }
                let next = self.rotation.wrapping_add(1);
                if let Some(kick) = [0, -1, 1, -2, 2]
                    .into_iter()
                    .find(|kick| self.fits(self.x + kick, self.y, next))
                {
                    self.x += kick;
                    self.rotation = next;
                }
            }
            "down" => {
                if self.fits(self.x, self.y + 1, self.rotation) {
                    self.y += 1;
                    self.score += 1;
                } else {
                    self.lock_piece();
                }
            }
            "drop" => {
                let mut distance = 0;
                while self.fits(self.x, self.y + 1, self.rotation) {
                    self.y += 1;
                    distance += 1;
                }
                self.score += distance * 2;
                self.lock_piece();
            }
            _ => {}
        }
    }

    fn snapshot(&self, menu_open: bool) -> Value {
        let mut board: Vec<Option<u8>> = self.cells.clone();
        for (px, py) in self.coords() {
            let (x, y) = (self.x + px, self.y + py);
            if y >= 0 && y < H as i32 && x >= 0 && x < W as i32 {
                board[y as usize * W + x as usize] = Some(self.shape);
            }
        }
        json!({
            "board": board.iter().map(|cell| cell.map(|n| format!("c{n}")).unwrap_or_default()).collect::<Vec<_>>(),
            "next": COLORS[self.next as usize], "score": self.score, "lines": self.lines,
            "level": self.level(), "paused": self.paused, "over": self.over,
            "menuOpen": menu_open,
            "message": if self.over { "Stack topped out" } else if self.paused { "Ready — choose Start" } else { "In play" }
        })
    }
}

pub(super) struct Blocks {
    runtime: JsxRuntime,
    accepted: RetainedPanelTree,
    bounds: SurfaceBounds,
    stylesheet: StyleSheet,
    images: PluginImages,
    game: Game,
    menu_open: bool,
    axis_x: i8,
    gravity_elapsed: Duration,
    horizontal_direction: i8,
    horizontal_elapsed: Duration,
    changed_view_ids: Option<Vec<UiId>>,
    benchmark_publication_attempts: u64,
}

impl Blocks {
    pub(super) fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let graph = JsxModuleGraph::new(
            "app.tsx",
            [
                ModuleSource {
                    path: "app.tsx",
                    source: include_str!("app.tsx"),
                },
                ModuleSource {
                    path: "app.css",
                    source: include_str!("app.css"),
                },
            ],
        )?;
        let stylesheet = StyleSheet::compile(&graph.stylesheet()?)?;
        let bounds = SurfaceBounds::from_json(
            r#"{"surfaces":[{"id":"main","kind":"window","width":800,"height":720}]}"#,
        )?;
        let mut runtime = JsxRuntime::new_modules(&graph, None)?;
        runtime.set_data_value(Game::default().snapshot(false))?;
        let accepted =
            render_retained_panel(&mut runtime, &bounds, Some("main"), "__twinkleRender()", 1)?;
        accepted.validate_candidate(accepted.source(), &bounds, Some("main"), &stylesheet)?;
        Ok(Self {
            runtime,
            accepted,
            bounds,
            stylesheet,
            images: Default::default(),
            game: Game::default(),
            menu_open: false,
            axis_x: 0,
            gravity_elapsed: Duration::ZERO,
            horizontal_direction: 0,
            horizontal_elapsed: Duration::ZERO,
            changed_view_ids: None,
            benchmark_publication_attempts: 0,
        })
    }

    pub(super) fn benchmark_state(&self) -> Value {
        json!({
            "presentation": self.game.snapshot(self.menu_open),
            "model": {"cells": self.game.cells, "shape": self.game.shape, "x": self.game.x,
                "y": self.game.y, "rotation": self.game.rotation, "next": self.game.next,
                "score": self.game.score, "lines": self.game.lines, "paused": self.game.paused, "over": self.game.over},
            "bookkeeping": {"axisX": self.axis_x, "gravityElapsedMs": self.gravity_elapsed.as_millis(),
                "horizontalDirection": self.horizontal_direction, "horizontalElapsedMs": self.horizontal_elapsed.as_millis()}
        })
    }

    pub(super) fn benchmark_generation(&self) -> u64 {
        self.accepted.generation()
    }
    pub(super) fn benchmark_attempts(&self) -> u64 {
        self.benchmark_publication_attempts
    }

    pub(super) fn benchmark_diagnostics(&mut self) -> Result<Value, String> {
        self.runtime.runtime_diagnostics()
    }

    fn render(&mut self) -> Result<(), String> {
        self.benchmark_publication_attempts += 1;
        super::stages::measure("transaction_checkpoint_begin", || {
            self.runtime.begin_transaction()
        })?;
        let result: Result<RetainedPanelTree, String> = (|| {
            let mut candidate =
                super::stages::measure("retained_candidate_clone", || self.accepted.clone());
            self.changed_view_ids = None;
            let snapshot =
                super::stages::measure("rust_snapshot", || self.game.snapshot(self.menu_open));
            match super::stages::measure("bridge_and_jsx_reconciliation", || {
                self.runtime.reconcile_data_patched(snapshot)
            })? {
                ScheduledPatch::Unchanged => {
                    super::stages::measure("native_patch_admission_skipped", || ());
                    super::stages::measure("patch_commit_bookkeeping_skipped", || ());
                }
                ScheduledPatch::Patched {
                    patch,
                    transport_bytes,
                    ..
                } => {
                    self.changed_view_ids =
                        super::stages::measure("changed_id_construction", || {
                            candidate
                                .changed_element_ids(&patch)
                                .map(|ids| ids.into_iter().map(UiId::from).collect())
                        });
                    let generation = candidate.generation() + 1;
                    candidate = super::stages::measure("native_patch_admission", || {
                        candidate.apply_patch_owned(
                            &patch,
                            &self.bounds,
                            Some("main"),
                            &self.stylesheet,
                            generation,
                            transport_bytes,
                        )
                    })?
                    .0;
                    super::stages::measure("patch_commit_bookkeeping", || {
                        self.runtime.finish_patch_render(true)
                    })?;
                }
            }
            if !super::stages::measure("effects_drain", || self.runtime.take_effects())?.is_empty()
            {
                return Err("external game-state reconciliation emitted an effect".into());
            }
            Ok(candidate)
        })();
        super::stages::measure("transaction_checkpoint_finish", || {
            self.runtime.finish_transaction(result.is_ok())
        })?;
        self.accepted = result?;
        Ok(())
    }

    fn dispatch(&mut self, action: usize) -> Result<(), String> {
        super::stages::measure("semantic_dispatch_complete", || self.dispatch_inner(action))
    }
    fn dispatch_inner(&mut self, action: usize) -> Result<(), String> {
        super::stages::measure("dispatch_checkpoint_begin", || {
            self.runtime.begin_transaction()
        })?;
        let result: Result<(RetainedPanelTree, Option<String>), String> = (|| {
            let mut candidate = super::stages::measure("dispatch_retained_candidate_clone", || {
                self.accepted.clone()
            });
            if let ScheduledPatch::Patched {
                patch,
                transport_bytes,
                ..
            } = super::stages::measure("dispatch_bridge_jsx_and_patch_parse", || {
                self.runtime
                    .dispatch_batch_patched(vec![json!([action, null])], false)
            })? {
                let generation = candidate.generation() + 1;
                candidate = super::stages::measure("dispatch_native_patch_admission", || {
                    candidate.apply_patch_owned(
                        &patch,
                        &self.bounds,
                        Some("main"),
                        &self.stylesheet,
                        generation,
                        transport_bytes,
                    )
                })?
                .0;
                super::stages::measure("dispatch_patch_commit", || {
                    self.runtime.finish_patch_render(true)
                })?;
            } else {
                super::stages::measure("dispatch_native_patch_admission_skipped", || ());
                super::stages::measure("dispatch_patch_commit_skipped", || ());
            }
            let effects =
                super::stages::measure("dispatch_effects_drain", || self.runtime.take_effects())?;
            if effects.len() > 1 {
                return Err("expected at most one game action".into());
            }
            let requested: Option<String> = effects
                .first()
                .map(|effect| -> Result<String, String> {
                    if effect.as_object().is_none_or(|object| object.len() != 1) {
                        return Err("game action contains unexpected properties".into());
                    }
                    effect["action"]
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| "invalid game action".into())
                })
                .transpose()?;
            super::stages::measure("dispatch_event_finish", || self.runtime.finish_event(true))?;
            Ok((candidate, requested))
        })();
        super::stages::measure("dispatch_checkpoint_finish", || {
            self.runtime.finish_transaction(result.is_ok())
        })?;
        let (candidate, action) = result?;
        self.accepted = candidate;
        if let Some(action) = action {
            if action == "menu" {
                self.menu_open = !self.menu_open;
                self.axis_x = 0;
                self.horizontal_direction = 0;
                self.horizontal_elapsed = Duration::ZERO;
            } else if action == "restart" {
                self.menu_open = false;
                self.game.act(&action);
                self.axis_x = 0;
                self.gravity_elapsed = Duration::ZERO;
                self.horizontal_direction = 0;
                self.horizontal_elapsed = Duration::ZERO;
            }
        }
        self.render()
    }
}

impl Application for Blocks {
    type Message = PluginMessage;
    fn update(&mut self, message: PluginMessage) {
        let action = match message {
            PluginMessage::Click(a)
            | PluginMessage::Context(a)
            | PluginMessage::Text(a, _)
            | PluginMessage::Value(a, _) => Some(a),
            PluginMessage::Button { click, .. } => Some(click),
            PluginMessage::Scroll | PluginMessage::Drag(..) | PluginMessage::Drop(..) => None,
        };
        if let Some(action) = action {
            if let Err(error) = self.dispatch(action) {
                eprintln!("Game action rejected: {error}");
            }
        }
    }

    fn controller_input(&mut self, event: ApplicationControllerEvent) -> ControllerInputOutcome {
        use ApplicationControllerInput as Input;
        if matches!(&event.input, Input::RouteNeutralized | Input::Disconnected) {
            self.axis_x = 0;
        }
        if let Input::Button {
            button: ControllerButton::Start,
            edge: KeyEdge::Pressed,
            repeat,
        } = event.input
        {
            if repeat {
                return ControllerInputOutcome::handled(false);
            }
            if self.game.over {
                return ControllerInputOutcome::handled(false);
            }
            self.menu_open = !self.menu_open;
            self.axis_x = 0;
            self.horizontal_direction = 0;
            self.horizontal_elapsed = Duration::ZERO;
            if let Err(error) = self.render() {
                eprintln!("Game-menu render failed: {error}");
                return ControllerInputOutcome::handled(false);
            }
            return ControllerInputOutcome::handled(true);
        }
        if let Input::Button {
            button: ControllerButton::DPadDown,
            edge,
            repeat,
        } = event.input
        {
            let gameplay_active = !self.menu_open && !self.game.paused && !self.game.over;
            let changed = gameplay_active && edge == KeyEdge::Pressed && !repeat;
            if changed {
                self.game.act("down");
                if let Err(error) = self.render() {
                    eprintln!("Controller action failed: {error}");
                }
            }
            return if gameplay_active {
                ControllerInputOutcome::handled(changed)
            } else {
                ControllerInputOutcome::unhandled()
            };
        }
        if self.menu_open || self.game.paused || self.game.over {
            return ControllerInputOutcome::unhandled();
        }
        let (action, changed) = match event.input {
            Input::Button {
                button,
                edge: KeyEdge::Pressed,
                repeat,
            } => {
                let action = match button {
                    ControllerButton::DPadLeft => Some("left"),
                    ControllerButton::DPadRight => Some("right"),
                    ControllerButton::DPadDown => Some("down"),
                    ControllerButton::DPadUp | ControllerButton::South => Some("rotate"),
                    ControllerButton::East => Some("drop"),
                    ControllerButton::West => Some("down"),
                    _ => None,
                };
                if repeat && !matches!(action, Some("left" | "right" | "down")) {
                    return ControllerInputOutcome::handled(false);
                }
                (action, action.is_some())
            }
            Input::Axis {
                axis: ControllerAxis::LeftX,
                value,
            } => {
                let direction = if value < -0.65 {
                    -1
                } else if value > 0.65 {
                    1
                } else {
                    0
                };
                let action = (direction != 0 && direction != self.axis_x)
                    .then_some(if direction < 0 { "left" } else { "right" });
                self.axis_x = direction;
                (action, action.is_some())
            }
            Input::Axis {
                axis: ControllerAxis::LeftY,
                value,
            } => {
                let _ = value;
                return ControllerInputOutcome::handled(false);
            }
            Input::RouteNeutralized | Input::Disconnected => {
                return ControllerInputOutcome::handled(false);
            }
            _ => return ControllerInputOutcome::unhandled(),
        };
        if let Some(action) = action {
            self.game.act(action);
            if let Err(error) = self.render() {
                eprintln!("Controller action failed: {error}");
            }
        }
        ControllerInputOutcome::handled(changed)
    }

    fn shortcut_outcome(&mut self, shortcut: Shortcut) -> ShortcutOutcome {
        if shortcut == Shortcut::Escape {
            if self.game.over {
                return ShortcutOutcome::from_changed(false);
            }
            self.menu_open = !self.menu_open;
            self.axis_x = 0;
            self.horizontal_direction = 0;
            self.horizontal_elapsed = Duration::ZERO;
            let changed = self.render().is_ok();
            return ShortcutOutcome::from_changed(changed);
        }
        if self.menu_open {
            return ShortcutOutcome::from_changed(false);
        }
        let action = match shortcut {
            Shortcut::NavigateUp => Some("rotate"),
            Shortcut::NavigateDown => Some("down"),
            Shortcut::Back => Some("left"),
            Shortcut::Forward => Some("right"),
            Shortcut::Submit => Some("drop"),
            Shortcut::Reload => Some("restart"),
            _ => None,
        };
        if let Some(action) = action {
            if (self.game.paused || self.game.over) && action != "restart" {
                return ShortcutOutcome::from_changed(false);
            }
            self.game.act(action);
            let _ = self.render();
            ShortcutOutcome::from_changed(true)
        } else {
            ShortcutOutcome::from_changed(false)
        }
    }

    fn poll_interval(&self) -> Option<Duration> {
        (!self.game.paused && !self.game.over && !self.menu_open)
            .then(|| self.game.gravity_interval())
    }

    fn poll_interval_controller(&self, input: &ControllerSnapshot) -> Option<Duration> {
        (!self.game.paused && !self.game.over && !self.menu_open).then(|| {
            let mut interval = self.game.gravity_interval();
            if input.button_down(&ControllerButton::DPadDown)
                || input.axis(&ControllerAxis::LeftY) > 0.65
            {
                interval = interval.min(self.game.soft_drop_interval());
            }
            if input.button_down(&ControllerButton::DPadLeft)
                || input.button_down(&ControllerButton::DPadRight)
                || input.axis(&ControllerAxis::LeftX).abs() > 0.65
            {
                interval = interval.min(Duration::from_millis(50));
            }
            interval
        })
    }

    fn poll_controller(&mut self, input: &ControllerSnapshot) -> bool {
        if self.game.paused || self.game.over || self.menu_open {
            return false;
        }
        let interval = self
            .poll_interval_controller(input)
            .unwrap_or_else(|| self.game.gravity_interval());
        let soft_drop = input.button_down(&ControllerButton::DPadDown)
            || input.axis(&ControllerAxis::LeftY) > 0.65;
        if soft_drop {
            self.game.act("down");
        } else {
            self.gravity_elapsed += interval;
            if self.gravity_elapsed >= self.game.gravity_interval() {
                self.gravity_elapsed = self
                    .gravity_elapsed
                    .saturating_sub(self.game.gravity_interval());
                self.game.gravity_tick();
            }
        }

        let axis_x = input.axis(&ControllerAxis::LeftX);
        let horizontal = if input.button_down(&ControllerButton::DPadLeft) || axis_x < -0.65 {
            -1
        } else if input.button_down(&ControllerButton::DPadRight) || axis_x > 0.65 {
            1
        } else {
            0
        };
        if horizontal == 0 || horizontal != self.horizontal_direction {
            self.horizontal_direction = horizontal;
            self.horizontal_elapsed = Duration::ZERO;
        } else {
            self.horizontal_elapsed += interval;
            if self.horizontal_elapsed >= Duration::from_millis(150) {
                self.game.act(if horizontal < 0 { "left" } else { "right" });
                self.horizontal_elapsed = Duration::from_millis(100);
            }
        }
        match self.render() {
            Ok(()) => true,
            Err(error) => {
                eprintln!("Game tick render failed: {error}");
                false
            }
        }
    }
    fn view(&self, _context: ViewContext) -> impl View<PluginMessage> {
        self.accepted.node().view(&self.images, &self.stylesheet)
    }
    fn title(&self) -> &str {
        "Twinkle Blocks"
    }
    fn initial_size(&self) -> (u32, u32) {
        (800, 720)
    }

    fn take_changed_view_ids(&mut self) -> Option<Vec<UiId>> {
        self.changed_view_ids.take()
    }
}

pub(super) fn original_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    match std::env::args().nth(1).as_deref() {
        None => twinkle_ui::run(Blocks::new()?),
        Some("--validate") => {
            let mut timing = Game::default();
            let level_one_gravity = timing.gravity_interval();
            timing.lines = 20;
            if timing.level() != 3 || timing.gravity_interval() >= level_one_gravity {
                return Err("gravity cadence does not accelerate with level".into());
            }
            let mut bag = Game::default();
            for _ in 0..2 {
                let mut seen = [false; 7];
                for _ in 0..7 {
                    seen[bag.draw_piece() as usize] = true;
                }
                if !seen.into_iter().all(|piece| piece) {
                    return Err("the seven-piece bag omitted or repeated a shape".into());
                }
            }
            let mut square = Game {
                shape: 0,
                ..Game::default()
            };
            square.act("rotate");
            if square.rotation != 0 || square.coords() != SHAPES[0] {
                return Err("the square piece changed geometry when rotated".into());
            }
            let mut wall_kick = Game {
                shape: 1,
                x: 7,
                rotation: 1,
                ..Game::default()
            };
            wall_kick.act("rotate");
            if wall_kick.rotation != 2 || wall_kick.x != 6 {
                return Err("the line piece did not apply its bounded wall kick".into());
            }

            let mut app = Blocks::new()?;
            let down = |edge| ApplicationControllerEvent {
                device: ControllerId(1),
                family: ControllerFamily::Xbox,
                backend_order: 1,
                produced_unix_ms: 1,
                input: ApplicationControllerInput::Button {
                    button: ControllerButton::DPadDown,
                    edge,
                    repeat: false,
                },
            };
            let mut input_host = twinkle_ui::UiHost::new(Blocks::new()?, 800, 720);
            let tree = input_host.inspect().resources;
            println!(
                "Gameplay component tree: {} bytes ({:.1} KiB), {} resolved nodes, {} retained nodes, {} paint primitives, {} accessibility nodes.",
                tree.estimated_retained_bytes,
                tree.estimated_retained_bytes as f64 / 1024.0,
                tree.node_count,
                tree.retained_node_count,
                tree.paint_primitive_count,
                tree.accessibility_node_count,
            );
            input_host.handle_application_controller_input(down(KeyEdge::Pressed));
            if input_host
                .application()
                .poll_interval_controller(input_host.controller_snapshot())
                != Some(input_host.application().game.soft_drop_interval())
            {
                return Err("held Down did not select the soft-drop cadence".into());
            }
            input_host.handle_application_controller_input(down(KeyEdge::Released));
            if input_host
                .application()
                .poll_interval_controller(input_host.controller_snapshot())
                != Some(input_host.application().game.gravity_interval())
            {
                return Err("released Down did not restore gravity cadence".into());
            }

            let mut horizontal_host = twinkle_ui::UiHost::new(Blocks::new()?, 800, 720);
            horizontal_host.handle_application_controller_input(ApplicationControllerEvent {
                device: ControllerId(1),
                family: ControllerFamily::Xbox,
                backend_order: 1,
                produced_unix_ms: 1,
                input: ApplicationControllerInput::Button {
                    button: ControllerButton::DPadLeft,
                    edge: KeyEdge::Pressed,
                    repeat: false,
                },
            });
            let immediate_x = horizontal_host.application().game.x;
            for _ in 0..4 {
                horizontal_host.poll();
            }
            if horizontal_host.application().game.x >= immediate_x {
                return Err("held Left did not auto-shift after its initial move".into());
            }

            let mut ticks = 0;
            let neutral_input = ControllerSnapshot::default();
            for _ in 0..400 {
                if !app.poll_controller(&neutral_input) {
                    break;
                }
                ticks += 1;
            }
            if !app.game.over || ticks < 40 {
                return Err(format!(
                    "polling stopped after {ticks} ticks without a normal top-out"
                )
                .into());
            }
            if app.accepted.node().button_action("continue").is_some()
                || app.accepted.node().button_action("restart").is_none()
            {
                return Err("game over did not present New game without Continue".into());
            }
            let start = || ApplicationControllerEvent {
                device: ControllerId(1),
                family: ControllerFamily::Xbox,
                backend_order: 1,
                produced_unix_ms: 1,
                input: ApplicationControllerInput::Button {
                    button: ControllerButton::Start,
                    edge: KeyEdge::Pressed,
                    repeat: false,
                },
            };
            let outcome = app.controller_input(start());
            if outcome.changed || app.menu_open {
                return Err("controller Start dismissed the terminal game-over screen".into());
            }

            app = Blocks::new()?;
            let outcome = app.controller_input(start());
            if !outcome.changed || !app.menu_open {
                return Err(
                    "controller Start did not open the conditional game-menu screen".into(),
                );
            }
            let continue_action = app
                .accepted
                .node()
                .button_action("continue")
                .ok_or("conditional menu screen is missing its Continue action")?;
            let mut host = twinkle_ui::UiHost::new(app, 800, 720);
            host.step(twinkle_ui::HostBatch {
                application_changed: true,
                ..twinkle_ui::HostBatch::default()
            });
            if !host
                .semantic_nodes()
                .iter()
                .any(|node| node.name.as_deref() == Some("New game"))
            {
                return Err(format!(
                    "the conditional JSX game menu did not expose its actions: semantics={:?}",
                    host.semantic_nodes()
                        .iter()
                        .filter_map(|node| node.name.as_deref())
                        .collect::<Vec<_>>()
                )
                .into());
            }
            host.application_mut()
                .update(PluginMessage::Click(continue_action));
            host.step(twinkle_ui::HostBatch {
                application_changed: true,
                ..twinkle_ui::HostBatch::default()
            });
            if host.application().menu_open
                || host
                    .semantic_nodes()
                    .iter()
                    .any(|node| node.name.as_deref() == Some("Continue"))
            {
                return Err("Continue did not replace the menu with gameplay".into());
            }
            let mut controller_host = twinkle_ui::UiHost::new(Blocks::new()?, 800, 720);
            let outcome = controller_host.handle_application_controller_input(start());
            if !outcome.changed
                || !controller_host.application().menu_open
                || !controller_host
                    .semantic_nodes()
                    .iter()
                    .any(|node| node.name.as_deref() == Some("New game"))
            {
                return Err("physical controller Start did not present the menu screen".into());
            }
            controller_host.handle_application_controller_input(start());
            if controller_host.application().menu_open
                || controller_host.inspect().controller_target.is_some()
                || controller_host
                    .semantic_nodes()
                    .iter()
                    .any(|node| node.name.as_deref() == Some("Continue"))
            {
                return Err(
                    "closing the menu retained menu focus instead of returning to gameplay".into(),
                );
            }
            println!(
                "JSX render survived {ticks} gravity ticks and switched cleanly between gameplay and its menu screen."
            );
            Ok(())
        }
        _ => Err("usage: twinkle-blocks [--validate]".into()),
    }
}

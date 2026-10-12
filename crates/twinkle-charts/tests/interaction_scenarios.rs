use std::sync::Arc;

use twinkle_charts::{
    Channel, ChartData, ChartEvent, ChartInputBindings, ChartLimits, ChartModel, ChartState,
    ChartTable, ChartTableEvent, ChartTheme, ChartView, Column, DataValue, Dataset, DatasetId,
    Encoding, FieldRef, Legend, PanZoomPolicy, Recipes, RowKey, Scale, ScaleKind, ViewportCommand,
};
use twinkle_testkit::{Scenario, Selector};
use twinkle_ui::{
    ActionKind, Application, Button, Column as UiColumn, Component, ComponentBuilderExt,
    DragGesture, DragPhase, Point, Rect, SemanticRole, Spacer, VerticalScroll, ViewContext,
    WheelGesture,
    backend::PaintCommand,
    retained_surface::{RetainedSurfaceInput, RetainedSurfaceRevisions},
};

const BOUNDS: Rect = Rect::new(0.0, 0.0, 400.0, 240.0);

#[derive(Clone, Debug)]
enum Message {
    Chart(ChartEvent),
    Pointer(Point),
    Drag(DragGesture),
    Wheel(WheelGesture),
    Scroll(f32),
    Table(ChartTableEvent),
    Reorder,
    RemoveActive,
    DarkTheme,
}

fn pointer(point: Point) -> Message {
    Message::Pointer(point)
}
fn drag(_: Message, gesture: DragGesture) -> Message {
    Message::Drag(gesture)
}
fn wheel(gesture: WheelGesture) -> Message {
    Message::Wheel(gesture)
}

struct ChartScenarioApp {
    chart: ChartView,
    spec: Arc<twinkle_charts::ChartSpec>,
    limits: ChartLimits,
    state: ChartState,
    revision: u64,
    events: Vec<ChartEvent>,
    drag_phases: Vec<DragPhase>,
    wheel_events: usize,
}

impl ChartScenarioApp {
    fn new() -> Self {
        let limits = ChartLimits::default();
        let data = scenario_data(1, &["a", "b", "c"], &limits);
        let mut spec = Recipes::line(
            "Scenario chart",
            FieldRef::new("scenario", "x"),
            FieldRef::new("scenario", "y"),
        );
        spec.layers[0].encodings.push(Encoding {
            channel: Channel::Series,
            field: FieldRef::new("scenario", "series"),
            scale: Some(Arc::from("series")),
        });
        spec.scales.push(Scale {
            name: Arc::from("series"),
            kind: ScaleKind::Color,
            domain: None,
            clamp: false,
            reversed: false,
            unit: None,
        });
        spec.legends.push(Legend {
            scale: Arc::from("series"),
            title: Some(Arc::from("Series")),
            toggleable: true,
        });
        let spec = Arc::new(spec);
        let mut chart =
            ChartView::new(ChartModel::new(Arc::clone(&spec), data, limits.clone()).unwrap())
                .unwrap();
        chart.update(input(1)).unwrap();
        Self {
            chart,
            spec,
            limits,
            state: ChartState::default(),
            revision: 1,
            events: Vec::new(),
            drag_phases: Vec::new(),
            wheel_events: 0,
        }
    }

    fn policy() -> PanZoomPolicy {
        PanZoomPolicy {
            x_extent: Some([0.0, 2.0]),
            y_extent: Some([0.0, 4.0]),
            minimum_x_span: 0.25,
            minimum_y_span: 0.5,
        }
    }

    fn publish_viewport(&mut self, command: ViewportCommand) {
        let Some(ChartEvent::ViewportChanged(viewport)) =
            self.chart.viewport_event(command, Self::policy())
        else {
            return;
        };
        self.state.viewport = Some(viewport);
        self.revision += 1;
        self.chart
            .update_state(self.state.clone(), input(self.revision))
            .unwrap();
    }

    fn replace_rows(&mut self, rows: &[&str]) {
        self.revision += 1;
        self.chart
            .replace_and_update(
                Arc::clone(&self.spec),
                scenario_data(self.revision, rows, &self.limits),
                input(self.revision),
            )
            .unwrap();
    }
}

fn scenario_data(revision: u64, rows: &[&str], limits: &ChartLimits) -> Arc<ChartData> {
    let values = |row: &str| match row {
        "a" => (0.0, 1.0),
        "b" => (1.0, 3.0),
        "c" => (2.0, 2.0),
        _ => unreachable!(),
    };
    Arc::new(
        ChartData::new(
            revision,
            vec![
                Dataset::new(
                    DatasetId::from("scenario"),
                    rows.iter()
                        .map(|row| RowKey::from(*row))
                        .collect::<Vec<_>>(),
                    vec![
                        Column::new(
                            "x",
                            rows.iter()
                                .map(|row| DataValue::Number(values(row).0))
                                .collect::<Vec<_>>(),
                        ),
                        Column::new(
                            "y",
                            rows.iter()
                                .map(|row| DataValue::Number(values(row).1))
                                .collect::<Vec<_>>(),
                        ),
                        Column::new(
                            "series",
                            rows.iter()
                                .map(|row| {
                                    DataValue::Category(Arc::from(if *row == "b" {
                                        "B"
                                    } else {
                                        "A"
                                    }))
                                })
                                .collect::<Vec<_>>(),
                        ),
                    ],
                )
                .unwrap(),
            ],
            limits,
        )
        .unwrap(),
    )
}

impl Application for ChartScenarioApp {
    type Message = Message;

    fn update(&mut self, message: Message) {
        match message {
            Message::Pointer(point) => {
                let _ = self.chart.inspect_nearest_detail(point);
            }
            Message::Chart(ChartEvent::Navigate(command)) => {
                if let Some(event) = self.chart.navigate(command) {
                    self.events.push(event);
                }
            }
            Message::Chart(ChartEvent::Viewport(command)) => self.publish_viewport(command),
            Message::Chart(ChartEvent::ToggleSeries { series, visible }) => {
                let visible_series = self
                    .state
                    .visible_series
                    .get_or_insert_with(|| [Arc::from("A"), Arc::from("B")].into_iter().collect());
                if visible {
                    visible_series.insert(series);
                } else {
                    visible_series.remove(&series);
                }
                self.revision += 1;
                self.chart
                    .update_state(self.state.clone(), input(self.revision))
                    .unwrap();
            }
            Message::Chart(event) => self.events.push(event),
            Message::Drag(gesture) => self.drag_phases.push(gesture.phase),
            Message::Wheel(gesture) => {
                self.wheel_events += 1;
                self.publish_viewport(if gesture.delta_y < 0.0 {
                    ViewportCommand::ZoomIn
                } else {
                    ViewportCommand::ZoomOut
                });
            }
            Message::Scroll(_) => {}
            Message::Table(_) => {}
            Message::Reorder => self.replace_rows(&["c", "a", "b"]),
            Message::RemoveActive => self.replace_rows(&["c", "a"]),
            Message::DarkTheme => {
                self.revision += 1;
                self.chart
                    .update_theme(ChartTheme::dark(), input(self.revision))
                    .unwrap();
            }
        }
    }

    fn view(&self, _: ViewContext) -> impl twinkle_ui::View<Message> {
        UiColumn::new()
            .child(
                self.chart
                    .element_with_input(
                        "scenario-chart",
                        Message::Chart,
                        ChartInputBindings {
                            pointer_move: Some(pointer),
                            drag: Some((Message::Pointer(Point { x: 0.0, y: 0.0 }), drag)),
                            wheel: Some(wheel),
                        },
                    )
                    .unwrap(),
            )
            .child(Button::new(Message::Reorder, "Reorder data"))
            .child(Button::new(Message::RemoveActive, "Remove active row"))
            .child(Button::new(Message::DarkTheme, "Use dark chart theme"))
            .into_element()
    }
}

fn scroll(offset: f32) -> Message {
    Message::Scroll(offset)
}

struct NestedScrollApp {
    chart: ChartView,
    offset: f32,
}

impl NestedScrollApp {
    fn new() -> Self {
        Self {
            chart: ChartScenarioApp::new().chart,
            offset: 0.0,
        }
    }
}

impl Application for NestedScrollApp {
    type Message = Message;

    fn update(&mut self, message: Message) {
        if let Message::Scroll(offset) = message {
            self.offset = offset;
        }
    }

    fn view(&self, _: ViewContext) -> impl twinkle_ui::View<Message> {
        VerticalScroll::new(Message::Scroll(self.offset), self.offset)
            .height(200.0)
            .controlled(true)
            .on_scroll(scroll)
            .child(
                UiColumn::new()
                    .child(
                        self.chart
                            .element_with_pointer("nested-chart", Message::Chart, pointer)
                            .unwrap(),
                    )
                    .child(Spacer::new().height(400.0)),
            )
    }
}

struct TableScenarioApp {
    table: ChartTable,
    offset: usize,
    inspected: Option<RowKey>,
}

impl TableScenarioApp {
    fn new() -> Self {
        let limits = ChartLimits {
            max_visible_table_rows: 2,
            ..ChartLimits::default()
        };
        let data = Arc::new(
            ChartData::new(
                9,
                vec![
                    Dataset::new(
                        DatasetId::from("table"),
                        (0..5)
                            .map(|index| RowKey::from(format!("r{index}")))
                            .collect::<Vec<_>>(),
                        vec![
                            Column::new(
                                "value",
                                (0..5)
                                    .map(|index| DataValue::Number(index as f64))
                                    .collect::<Vec<_>>(),
                            )
                            .with_unit("ms"),
                        ],
                    )
                    .unwrap(),
                ],
                &limits,
            )
            .unwrap(),
        );
        Self {
            table: ChartTable::new(data, limits),
            offset: 0,
            inspected: None,
        }
    }
}

impl Application for TableScenarioApp {
    type Message = Message;

    fn update(&mut self, message: Message) {
        if let Message::Table(event) = message {
            match event {
                ChartTableEvent::Page { offset, .. } => self.offset = offset,
                ChartTableEvent::Inspect { row, .. } => self.inspected = Some(row),
                ChartTableEvent::SetHiddenSeriesIncluded(_) => {}
            }
        }
    }

    fn view(&self, _: ViewContext) -> impl twinkle_ui::View<Message> {
        let page = self.table.page("table", self.offset).unwrap();
        self.table
            .element(&page, "scenario-table", Message::Table)
            .into_element()
    }
}

fn input(revision: u64) -> RetainedSurfaceInput {
    RetainedSurfaceInput::new(
        BOUNDS,
        1.0,
        RetainedSurfaceRevisions {
            content: revision,
            ..RetainedSurfaceRevisions::default()
        },
    )
}

#[test]
fn testkit_routes_pointer_accessibility_controller_wheel_and_drag_capture() {
    let selector = Selector::role_name(SemanticRole::GraphicalCustomControl, "Scenario chart");
    let mut scenario = Scenario::new(ChartScenarioApp::new(), 400, 240);

    scenario.pointer_move(&selector).unwrap();
    assert!(
        scenario
            .host()
            .application()
            .chart
            .active_inspection()
            .is_some()
    );
    assert!(scenario.semantic_nodes().iter().any(|node| {
        node.role == Some(SemanticRole::Tooltip)
            && node.name.as_deref().is_some_and(|name| name.contains("x:"))
    }));
    let inspected = scenario
        .host()
        .application()
        .chart
        .active_inspection()
        .unwrap()
        .row;
    scenario
        .host_mut()
        .handle_event(twinkle_ui::UiEvent::PointerMoved(Point {
            x: BOUNDS.size.width + 50.0,
            y: BOUNDS.size.height + 50.0,
        }));
    assert_eq!(
        scenario
            .host()
            .application()
            .chart
            .active_inspection()
            .unwrap()
            .row,
        inspected
    );

    scenario.keyboard_activate(&selector).unwrap();
    scenario
        .accessibility_action(&selector, ActionKind::Increment)
        .unwrap();
    scenario
        .controller_semantic_action(&selector, ActionKind::Decrement)
        .unwrap();
    assert_eq!(scenario.host().application().events.len(), 3);

    scenario.pointer_scroll(&selector, 0.0, -12.0).unwrap();
    assert_eq!(scenario.host().application().wheel_events, 1);
    assert!(scenario.host().application().state.viewport.is_some());
    scenario
        .pointer_activate(&Selector::role_name(SemanticRole::Button, "Reset zoom"))
        .unwrap();
    assert_eq!(
        scenario.host().application().state.viewport,
        Some(ChartScenarioApp::policy().reset())
    );

    let point = scenario.pointer_move(&selector).unwrap();
    scenario
        .host_mut()
        .handle_event(twinkle_ui::UiEvent::PointerPressed(point));
    scenario.window_focus(false).unwrap();
    assert!(
        scenario
            .host()
            .application()
            .drag_phases
            .contains(&DragPhase::Cancelled)
    );
}

#[test]
fn testkit_toggles_legends_and_reprojects_theme_changes() {
    let mut scenario = Scenario::new(ChartScenarioApp::new(), 400, 360);
    let series = Selector::role_name(SemanticRole::Button, "A series");
    scenario.pointer_activate(&series).unwrap();
    assert!(
        !scenario
            .host()
            .application()
            .state
            .visible_series
            .as_ref()
            .unwrap()
            .contains("A")
    );
    assert!(
        scenario
            .host()
            .application()
            .chart
            .hits()
            .iter()
            .all(|hit| hit.series.as_deref() != Some("A"))
    );

    scenario
        .pointer_activate(&Selector::role_name(
            SemanticRole::Button,
            "Use dark chart theme",
        ))
        .unwrap();
    assert!(scenario
        .host()
        .application()
        .chart
        .snapshot()
        .output
        .paint
        .iter()
        .any(|command| matches!(command, PaintCommand::Fill { color, .. } if *color == ChartTheme::dark().background)));
}

#[test]
fn testkit_preserves_active_identity_through_reorder_and_removal() {
    let chart = Selector::role_name(SemanticRole::GraphicalCustomControl, "Scenario chart");
    let mut scenario = Scenario::new(ChartScenarioApp::new(), 400, 320);

    scenario
        .accessibility_action(&chart, ActionKind::Increment)
        .unwrap();
    assert_eq!(
        scenario
            .host()
            .application()
            .chart
            .active_inspection()
            .unwrap()
            .row,
        RowKey::from("b")
    );

    scenario
        .pointer_activate(&Selector::role_name(SemanticRole::Button, "Reorder data"))
        .unwrap();
    assert_eq!(
        scenario
            .host()
            .application()
            .chart
            .active_inspection()
            .unwrap()
            .row,
        RowKey::from("b")
    );
    scenario.keyboard_activate(&chart).unwrap();
    assert!(matches!(
        scenario.host().application().events.last(),
        Some(ChartEvent::Select { row, revision: 2, .. }) if row == &RowKey::from("b")
    ));

    scenario
        .pointer_activate(&Selector::role_name(
            SemanticRole::Button,
            "Remove active row",
        ))
        .unwrap();
    assert_eq!(
        scenario
            .host()
            .application()
            .chart
            .active_inspection()
            .unwrap()
            .row,
        RowKey::from("a")
    );
}

#[test]
fn testkit_preserves_nested_scroll_when_chart_has_no_wheel_binding() {
    let selector = Selector::role_name(SemanticRole::GraphicalCustomControl, "Scenario chart");
    let mut scenario = Scenario::new(NestedScrollApp::new(), 400, 200);
    scenario.pointer_scroll(&selector, 0.0, 40.0).unwrap();
    assert!(scenario.host().application().offset > 0.0);
}

#[test]
fn testkit_pages_and_inspects_exact_table_rows_through_semantic_controls() {
    let mut scenario = Scenario::new(TableScenarioApp::new(), 640, 300);
    assert!(scenario.semantic_nodes().iter().any(|node| {
        node.role == Some(SemanticRole::Grid)
            && node.name.as_deref() == Some("Data table for table")
    }));

    scenario
        .pointer_activate(&Selector::role_name(SemanticRole::Button, "Next page"))
        .unwrap();
    assert_eq!(scenario.host().application().offset, 2);
    let third_row = Selector::role_name(SemanticRole::GridCell, "row 3 of 5 — value: 2 ms");
    scenario.keyboard_activate(&third_row).unwrap();
    assert_eq!(
        scenario.host().application().inspected,
        Some(RowKey::from("r2"))
    );

    scenario
        .controller_activate(&Selector::role_name(SemanticRole::Button, "Previous page"))
        .unwrap();
    assert_eq!(scenario.host().application().offset, 0);
}

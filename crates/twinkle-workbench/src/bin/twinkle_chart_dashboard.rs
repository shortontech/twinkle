//! Stateful native chart consumer using only public `twinkle-charts` APIs.

use std::{collections::BTreeSet, error::Error, sync::Arc, time::Duration};

use twinkle::retained_surface::{RetainedSurfaceInput, RetainedSurfaceRevisions};
use twinkle::{
    Application, Button, Column as UiColumn, Component, DragGesture, DragPhase, Grid, Point, Rect,
    Row, Text, ViewContext, WheelGesture,
};
use twinkle_charts::{
    ChartData, ChartEvent, ChartInputBindings, ChartLimits, ChartModel, ChartState, ChartTable,
    ChartTableEvent, ChartTableFilter, ChartView, Column, CsvOptions, DataValue, Dataset,
    DatasetId, FieldRef, Legend, PanZoomPolicy, RasterOptions, Recipes, RowKey, Scale, ScaleKind,
    Viewport, ViewportCommand, export_csv, export_raster,
};

const CHART_BOUNDS: Rect = Rect::new(0.0, 0.0, 760.0, 410.0);
const GPU_ADMISSION_CHART_BOUNDS: Rect = Rect::new(0.0, 0.0, 240.0, 145.0);

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Chart(ChartEvent),
    Pointer(Point),
    Drag(DragGesture),
    Wheel(WheelGesture),
    Table(ChartTableEvent),
    ExportCsv,
    ExportRaster,
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

struct Dashboard {
    data: Arc<ChartData>,
    limits: ChartLimits,
    chart: ChartView,
    state: ChartState,
    table_offset: usize,
    table_mode: bool,
    revision: u64,
    drag_origin: Option<(Point, Viewport)>,
    status: String,
    gpu_admission_step: Option<u8>,
    gpu_admission_charts: Option<Vec<ChartView>>,
}

impl Dashboard {
    fn new() -> Result<Self, Box<dyn Error>> {
        let limits = ChartLimits::default();
        let data = Arc::new(data(&limits)?);
        let state = ChartState {
            visible_series: Some(
                [Arc::from("North"), Arc::from("South")]
                    .into_iter()
                    .collect(),
            ),
            ..ChartState::default()
        };
        let chart = ChartView::new(
            ChartModel::new(Arc::new(spec()), data.clone(), limits.clone())?
                .with_state(state.clone()),
        )?;
        let mut dashboard = Self {
            data,
            limits,
            chart,
            state,
            table_offset: 0,
            table_mode: false,
            revision: 1,
            drag_origin: None,
            status: "Hover a datum or use keyboard/controller navigation".into(),
            gpu_admission_step: None,
            gpu_admission_charts: None,
        };
        dashboard.prepare()?;
        Ok(dashboard)
    }

    fn input(&self) -> RetainedSurfaceInput {
        RetainedSurfaceInput::new(
            CHART_BOUNDS,
            1.0,
            RetainedSurfaceRevisions {
                content: self.revision,
                ..RetainedSurfaceRevisions::default()
            },
        )
    }

    fn prepare(&mut self) -> Result<(), twinkle_charts::ChartError> {
        self.chart.update_state(self.state.clone(), self.input())?;
        Ok(())
    }

    fn policy() -> PanZoomPolicy {
        PanZoomPolicy {
            x_extent: Some([1.0, 6.0]),
            y_extent: Some([0.0, 42.0]),
            minimum_x_span: 0.5,
            minimum_y_span: 4.0,
        }
    }

    fn apply_viewport(&mut self, viewport: Viewport) {
        self.state.viewport = Some(viewport);
        self.revision = self.revision.saturating_add(1);
        if let Err(error) = self.prepare() {
            self.status = error.to_string();
        }
    }

    fn enable_gpu_admission(mut self) -> Result<Self, twinkle_charts::ChartError> {
        let chart_spec = Arc::new(spec());
        let mut charts = Vec::with_capacity(16);
        for index in 0..16 {
            let mut chart = ChartView::new(
                ChartModel::new(
                    Arc::clone(&chart_spec),
                    Arc::clone(&self.data),
                    self.limits.clone(),
                )?
                .with_state(self.state.clone()),
            )?;
            chart.update(RetainedSurfaceInput::new(
                GPU_ADMISSION_CHART_BOUNDS,
                1.0,
                RetainedSurfaceRevisions {
                    content: self.data.revision().saturating_add(index as u64),
                    ..RetainedSurfaceRevisions::default()
                },
            ))?;
            charts.push(chart);
        }
        self.gpu_admission_step = Some(0);
        self.gpu_admission_charts = Some(charts);
        Ok(self)
    }
}

impl Application for Dashboard {
    type Message = Message;

    fn title(&self) -> &str {
        "Twinkle native chart dashboard"
    }

    fn initial_size(&self) -> (u32, u32) {
        (1080, 760)
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::Pointer(point) => {
                if let Some(detail) = self.chart.inspect_nearest_detail(point) {
                    self.status = format!("Inspecting {} / row {}", detail.dataset, detail.row.0);
                }
            }
            Message::Chart(ChartEvent::Navigate(command)) => {
                if let Some(event) = self.chart.navigate(command) {
                    self.status = format!("{event:?}");
                }
            }
            Message::Chart(ChartEvent::Viewport(command)) => {
                if let Some(ChartEvent::ViewportChanged(viewport)) =
                    self.chart.viewport_event(command, Self::policy())
                {
                    self.apply_viewport(viewport);
                }
            }
            Message::Chart(ChartEvent::ViewportChanged(viewport)) => self.apply_viewport(viewport),
            Message::Chart(ChartEvent::ToggleSeries { series, visible }) => {
                let visible_series = self.state.visible_series.get_or_insert_with(BTreeSet::new);
                if visible {
                    visible_series.insert(series);
                } else {
                    visible_series.remove(&series);
                }
                self.revision = self.revision.saturating_add(1);
                if let Err(error) = self.prepare() {
                    self.status = error.to_string();
                }
            }
            Message::Chart(ChartEvent::TableMode(visible)) => self.table_mode = visible,
            Message::Chart(event) => self.status = format!("{event:?}"),
            Message::Wheel(gesture) => {
                let command = if gesture.delta_y < 0.0 {
                    ViewportCommand::ZoomIn
                } else {
                    ViewportCommand::ZoomOut
                };
                if let Some(ChartEvent::ViewportChanged(viewport)) =
                    self.chart.viewport_event(command, Self::policy())
                {
                    self.apply_viewport(viewport);
                }
            }
            Message::Drag(gesture) => match gesture.phase {
                DragPhase::Started => {
                    self.drag_origin = Some((
                        gesture.position,
                        self.state
                            .viewport
                            .unwrap_or_else(|| Self::policy().reset()),
                    ));
                }
                DragPhase::Moved => {
                    if let Some((origin, viewport)) = self.drag_origin {
                        let dx = f64::from(origin.x - gesture.position.x) / 80.0;
                        let dy = f64::from(gesture.position.y - origin.y) / 12.0;
                        if let Some(viewport) = Self::policy().pan(viewport, [dx, dy]) {
                            self.apply_viewport(viewport);
                        }
                    }
                }
                DragPhase::Ended | DragPhase::Cancelled => self.drag_origin = None,
            },
            Message::Table(ChartTableEvent::Page { offset, .. }) => self.table_offset = offset,
            Message::Table(ChartTableEvent::SetHiddenSeriesIncluded(_)) => {}
            Message::Table(ChartTableEvent::Inspect { row, .. }) => {
                self.status = format!("Table row {}", row.0)
            }
            Message::ExportCsv => match export_csv(&self.data, CsvOptions::default()) {
                Ok(bytes) => self.status = format!("CSV export prepared: {} bytes", bytes.len()),
                Err(error) => self.status = error.to_string(),
            },
            Message::ExportRaster => match export_raster(
                self.chart.snapshot(),
                RasterOptions {
                    logical_width: CHART_BOUNDS.size.width as u32,
                    logical_height: CHART_BOUNDS.size.height as u32,
                    ..RasterOptions::default()
                },
            ) {
                Ok(bytes) => self.status = format!("RGBA export prepared: {} bytes", bytes.len()),
                Err(error) => self.status = error.to_string(),
            },
        }
    }

    fn poll(&mut self) -> bool {
        let Some(step) = self.gpu_admission_step else {
            return false;
        };
        match step {
            // Rebuild an unchanged declaration first so the presenter trace
            // records retained scene reuse independently of chart mutation.
            0 => self.gpu_admission_step = Some(1),
            1 => {
                let charts = self
                    .gpu_admission_charts
                    .as_mut()
                    .expect("admission charts exist while admission is active");
                let _ = charts[0].inspect_nearest_detail(Point { x: 120.0, y: 72.0 });
                self.status = "GPU admission hover".into();
                self.gpu_admission_step = Some(2);
            }
            2 => {
                let charts = self
                    .gpu_admission_charts
                    .as_mut()
                    .expect("admission charts exist while admission is active");
                if let Some(ChartEvent::ViewportChanged(viewport)) =
                    charts[0].viewport_event(ViewportCommand::ZoomIn, Self::policy())
                {
                    self.state.viewport = Some(viewport);
                    self.revision = self.revision.saturating_add(1);
                    let _ = charts[0].update_state(
                        self.state.clone(),
                        RetainedSurfaceInput::new(
                            GPU_ADMISSION_CHART_BOUNDS,
                            1.0,
                            RetainedSurfaceRevisions {
                                content: self.revision,
                                ..RetainedSurfaceRevisions::default()
                            },
                        ),
                    );
                }
                self.status = "GPU admission zoom".into();
                self.gpu_admission_step = Some(3);
            }
            _ => {
                self.gpu_admission_step = None;
                return false;
            }
        }
        true
    }

    fn poll_interval(&self) -> Option<Duration> {
        self.gpu_admission_step.map(|_| Duration::from_millis(250))
    }

    fn view(&self, context: ViewContext) -> impl twinkle::View<Message> {
        let content = if let Some(charts) = &self.gpu_admission_charts {
            Grid::fixed(4)
                .gap(4.0)
                .children(charts.iter().enumerate().map(|(index, chart)| {
                    chart
                        .element(format!("gpu-admission-chart-{index}"), Message::Chart)
                        .expect("accepted admission chart projects through public APIs")
                        .into_element()
                }))
                .into_element()
        } else if self.table_mode {
            let table = ChartTable::new(self.data.clone(), self.limits.clone());
            let page = table
                .page_filtered(
                    "sales",
                    self.table_offset,
                    &ChartTableFilter {
                        series_column: Some(Arc::from("region")),
                        visible_series: self.state.visible_series.clone(),
                        include_hidden_series: false,
                    },
                )
                .expect("accepted dashboard table remains valid");
            table
                .element(&page, "dashboard-table", Message::Table)
                .into_element()
        } else {
            self.chart
                .element_with_input(
                    "dashboard-chart",
                    Message::Chart,
                    ChartInputBindings {
                        pointer_move: Some(pointer),
                        drag: Some((Message::Pointer(Point { x: 0.0, y: 0.0 }), drag)),
                        wheel: Some(wheel),
                    },
                )
                .expect("accepted chart projects through public APIs")
                .into_element()
        };
        UiColumn::new()
            .width(context.viewport.size.width.min(1000.0))
            .gap(10.0)
            .child(Text::new("Operations dashboard").bold(true))
            .child(
                Row::new()
                    .gap(8.0)
                    .child(Button::new(
                        Message::Chart(ChartEvent::TableMode(!self.table_mode)),
                        if self.table_mode {
                            "Show chart"
                        } else {
                            "Show data table"
                        },
                    ))
                    .child(Button::new(Message::ExportCsv, "Prepare CSV"))
                    .child(Button::new(Message::ExportRaster, "Prepare raster")),
            )
            .child(Text::new(&self.status))
            .child(content)
    }
}

fn spec() -> twinkle_charts::ChartSpec {
    let mut spec = Recipes::stacked_area(
        "Regional revenue",
        FieldRef::new("sales", "month"),
        FieldRef::new("sales", "region"),
        FieldRef::new("sales", "revenue"),
        false,
    );
    for encoding in spec
        .layers
        .iter_mut()
        .flat_map(|layer| &mut layer.encodings)
        .filter(|encoding| encoding.channel == twinkle_charts::Channel::Series)
    {
        encoding.scale = Some(Arc::from("series"));
    }
    spec.scales.push(Scale {
        name: Arc::from("series"),
        kind: ScaleKind::Color,
        domain: None,
        clamp: true,
        reversed: false,
        unit: None,
    });
    spec.legends.push(Legend {
        scale: Arc::from("series"),
        title: Some(Arc::from("Region")),
        toggleable: true,
    });
    spec
}

fn data(limits: &ChartLimits) -> Result<ChartData, twinkle_charts::ChartError> {
    let rows = 12;
    let dataset = Dataset::new(
        DatasetId::from("sales"),
        (0..rows)
            .map(|index| RowKey::from(format!("sale-{index}")))
            .collect::<Vec<_>>(),
        vec![
            Column::new(
                "month",
                (0..rows)
                    .map(|index| DataValue::Number((index / 2 + 1) as f64))
                    .collect::<Vec<_>>(),
            ),
            Column::new(
                "region",
                (0..rows)
                    .map(|index| {
                        DataValue::Category(Arc::from(if index % 2 == 0 {
                            "North"
                        } else {
                            "South"
                        }))
                    })
                    .collect::<Vec<_>>(),
            ),
            Column::new(
                "revenue",
                [
                    18.0, 12.0, 23.0, 16.0, 25.0, 19.0, 30.0, 22.0, 34.0, 27.0, 39.0, 31.0,
                ]
                .into_iter()
                .map(DataValue::Number)
                .collect::<Vec<_>>(),
            )
            .with_unit("USD thousands"),
        ],
    )?;
    ChartData::new(1, vec![dataset], limits)
}

fn main() -> Result<(), Box<dyn Error>> {
    let dashboard = Dashboard::new()?;
    let dashboard = if std::env::args().any(|argument| argument == "--gpu-admission") {
        dashboard.enable_gpu_admission()?
    } else {
        dashboard
    };
    twinkle::run(dashboard)
}

#[cfg(test)]
mod tests {
    use super::*;
    use twinkle::{ActionKind, SemanticRole, UiFrame};

    fn context(width: f32) -> ViewContext {
        ViewContext {
            viewport: Rect::new(0.0, 0.0, width, 760.0),
            modality: twinkle::InputModality::default(),
            focused: None,
            controller_target: None,
            available_semantic_actions: Vec::new(),
            navigation_depth: 0,
            open_overlay: None,
        }
    }

    #[test]
    fn public_consumer_routes_chart_table_viewport_and_exports() {
        let mut dashboard = Dashboard::new().unwrap();
        let frame = UiFrame::layout(
            dashboard.view(context(1080.0)),
            Rect::new(0.0, 0.0, 1080.0, 760.0),
        );
        assert!(frame.semantic_nodes().iter().any(|node| {
            node.role == Some(SemanticRole::GraphicalCustomControl)
                && node.actions.contains(&ActionKind::Activate)
        }));
        assert!(
            frame
                .semantic_nodes()
                .iter()
                .any(|node| node.name.as_deref() == Some("Zoom in"))
        );

        dashboard.update(Message::Chart(ChartEvent::Viewport(
            ViewportCommand::ZoomIn,
        )));
        assert!(dashboard.state.viewport.is_some());
        dashboard.update(Message::Pointer(Point { x: 200.0, y: 180.0 }));
        assert!(dashboard.chart.active_inspection().is_some());
        dashboard.update(Message::ExportCsv);
        assert!(dashboard.status.starts_with("CSV export prepared:"));
        dashboard.update(Message::ExportRaster);
        assert!(dashboard.status.starts_with("RGBA export prepared:"));

        dashboard.update(Message::Chart(ChartEvent::TableMode(true)));
        let table = UiFrame::layout(
            dashboard.view(context(820.0)),
            Rect::new(0.0, 0.0, 820.0, 760.0),
        );
        assert!(
            table
                .semantic_nodes()
                .iter()
                .any(|node| node.role == Some(SemanticRole::Grid))
        );
    }

    #[test]
    fn gpu_admission_uses_sixteen_independent_charts_and_mutates_only_the_first() {
        let mut dashboard = Dashboard::new().unwrap().enable_gpu_admission().unwrap();
        let charts = dashboard.gpu_admission_charts.as_ref().unwrap();
        assert_eq!(charts.len(), 16);
        assert!(
            charts
                .iter()
                .all(|chart| chart.active_inspection().is_none())
        );

        assert!(dashboard.poll()); // unchanged retained frame
        assert!(dashboard.poll()); // one-chart hover
        let charts = dashboard.gpu_admission_charts.as_ref().unwrap();
        assert!(charts[0].active_inspection().is_some());
        assert!(
            charts[1..]
                .iter()
                .all(|chart| chart.active_inspection().is_none())
        );

        let frame = UiFrame::layout(
            dashboard.view(context(1080.0)),
            Rect::new(0.0, 0.0, 1080.0, 760.0),
        );
        assert_eq!(
            frame
                .semantic_nodes()
                .iter()
                .filter(|node| node.role == Some(SemanticRole::GraphicalCustomControl))
                .count(),
            16
        );
    }
}

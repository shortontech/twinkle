//! Stateful native chart consumer using only public `twinkle-charts` APIs.

use std::{cell::RefCell, collections::BTreeSet, error::Error, rc::Rc, sync::Arc, time::Duration};

use twinkle::retained_surface::{RetainedSurfaceInput, RetainedSurfaceRevisions};
use twinkle::{
    Application, Button, Column as UiColumn, Component, DragGesture, DragPhase, Grid, Point, Rect,
    Row, Text, ViewContext, WheelGesture,
};
use twinkle_charts::{
    ChartData, ChartEvent, ChartInputBindings, ChartLimits, ChartModel, ChartState, ChartTable,
    ChartTableEvent, ChartTableFilter, ChartTooltipField, ChartTooltipFormat, ChartTooltipSpec,
    ChartView, Column, CsvOptions, DataValue, Dataset, DatasetId, FieldRef, Legend, PanZoomPolicy,
    RasterOptions, Recipes, RowKey, Scale, ScaleKind, Viewport, ViewportCommand, export_csv,
    export_raster,
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

#[derive(Clone, Copy)]
struct PanDrag {
    origin: Point,
    viewport: Viewport,
    plot: Rect,
    activated: bool,
}

impl PanDrag {
    fn displacement(&mut self, position: Point) -> Option<[f64; 2]> {
        let dx = self.origin.x - position.x;
        let dy = position.y - self.origin.y;
        self.activated |= dx.hypot(dy) >= 4.0;
        if !self.activated || self.plot.size.width <= 0.0 || self.plot.size.height <= 0.0 {
            return None;
        }
        Some([
            self.viewport
                .x
                .map_or(0.0, |[a, b]| f64::from(dx / self.plot.size.width) * (b - a)),
            self.viewport.y.map_or(0.0, |[a, b]| {
                f64::from(dy / self.plot.size.height) * (b - a)
            }),
        ])
    }
}

struct Dashboard {
    data: Arc<ChartData>,
    limits: ChartLimits,
    chart: Rc<RefCell<ChartView>>,
    state: ChartState,
    table_offset: usize,
    table_mode: bool,
    revision: u64,
    drag_origin: Option<PanDrag>,
    pan_zoom_policy: PanZoomPolicy,
    status: String,
    gpu_admission_step: Option<u8>,
    gpu_admission_charts: Option<Vec<ChartView>>,
}

impl Dashboard {
    fn new() -> Result<Self, Box<dyn Error>> {
        let limits = ChartLimits::default();
        let data = Arc::new(data(&limits)?);
        let chart_spec = spec();
        let transformed = twinkle_charts::execute_transforms(
            data.dataset("sales").expect("dashboard sales dataset"),
            &chart_spec.layers[0].transforms,
            &limits,
        )?;
        let extent = |columns: &[&str]| {
            columns
                .iter()
                .flat_map(|name| {
                    transformed
                        .column(name)
                        .into_iter()
                        .flat_map(|column| column.values.iter().filter_map(DataValue::number))
                })
                .fold([f64::INFINITY, f64::NEG_INFINITY], |range, value| {
                    [range[0].min(value), range[1].max(value)]
                })
        };
        let x_extent = extent(&["month"]);
        let y_extent = extent(&["revenue_start", "revenue_end"]);
        let pan_zoom_policy = PanZoomPolicy {
            x_extent: Some(x_extent),
            y_extent: Some(y_extent),
            minimum_x_span: (x_extent[1] - x_extent[0]) / 100.0,
            minimum_y_span: (y_extent[1] - y_extent[0]) / 100.0,
        };
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
                .with_state(state.clone())
                .with_tooltip(ChartTooltipSpec {
                    fields: vec![
                        ChartTooltipField {
                            column: Arc::from("region"),
                            label: Arc::from("Region"),
                            unit: None,
                            format: ChartTooltipFormat::Automatic,
                        },
                        ChartTooltipField {
                            column: Arc::from("month"),
                            label: Arc::from("Month"),
                            unit: None,
                            format: ChartTooltipFormat::Number { decimal_places: 0 },
                        },
                        ChartTooltipField {
                            column: Arc::from("revenue_raw"),
                            label: Arc::from("Revenue"),
                            unit: Some(Arc::from("USD thousands")),
                            format: ChartTooltipFormat::Number { decimal_places: 0 },
                        },
                    ],
                    include_data_table_hint: true,
                })?,
        )?;
        let mut dashboard = Self {
            data,
            limits,
            chart: Rc::new(RefCell::new(chart)),
            state,
            table_offset: 0,
            table_mode: false,
            revision: 1,
            drag_origin: None,
            pan_zoom_policy,
            status: "Hover a datum or use keyboard/controller navigation".into(),
            gpu_admission_step: None,
            gpu_admission_charts: None,
        };
        dashboard.prepare()?;
        Ok(dashboard)
    }

    fn input(&self) -> RetainedSurfaceInput {
        let accepted = self.chart.borrow().snapshot().input;
        RetainedSurfaceInput::new(
            accepted.map_or(CHART_BOUNDS, |input| input.bounds),
            accepted.map_or(1.0, |input| input.scale_factor),
            RetainedSurfaceRevisions {
                content: self.revision,
                ..RetainedSurfaceRevisions::default()
            },
        )
    }

    fn prepare(&mut self) -> Result<(), twinkle_charts::ChartError> {
        let input = self.input();
        self.chart
            .borrow_mut()
            .update_state(self.state.clone(), input)?;
        Ok(())
    }

    fn apply_viewport(&mut self, viewport: Viewport) {
        self.state.viewport = Some(viewport);
        self.revision = self.revision.saturating_add(1);
        if let Err(error) = self.prepare() {
            self.status = error.to_string();
        }
    }

    fn enable_gpu_admission(mut self) -> Result<Self, twinkle_charts::ChartError> {
        // Preserve the compact, guide-free admission workload independently
        // of the main dashboard's explanatory axes.
        let mut compact_spec = spec();
        compact_spec.axes.clear();
        let chart_spec = Arc::new(compact_spec);
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
                if let Some(detail) = self.chart.borrow_mut().inspect_nearest_detail(point) {
                    self.status = format!("Inspecting {} / row {}", detail.dataset, detail.row.0);
                }
            }
            Message::Chart(ChartEvent::Navigate(command)) => {
                if let Some(event) = self.chart.borrow_mut().navigate(command) {
                    self.status = format!("{event:?}");
                }
            }
            Message::Chart(ChartEvent::Viewport(command)) => {
                let event = self
                    .chart
                    .borrow()
                    .viewport_event(command, self.pan_zoom_policy);
                if let Some(ChartEvent::ViewportChanged(viewport)) = event {
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
                let event = self
                    .chart
                    .borrow()
                    .viewport_event(command, self.pan_zoom_policy);
                if let Some(ChartEvent::ViewportChanged(viewport)) = event {
                    self.apply_viewport(viewport);
                }
            }
            Message::Drag(gesture) => match gesture.phase {
                DragPhase::Started => {
                    let retained_origin = self
                        .chart
                        .borrow()
                        .snapshot()
                        .input
                        .map_or(Point::default(), |input| input.bounds.origin);
                    let local = Point {
                        x: gesture.position.x - gesture.bounds.origin.x + retained_origin.x,
                        y: gesture.position.y - gesture.bounds.origin.y + retained_origin.y,
                    };
                    self.drag_origin = self
                        .chart
                        .borrow()
                        .plot_bounds()
                        .filter(|plot| {
                            local.x >= plot.origin.x
                                && local.y >= plot.origin.y
                                && local.x < plot.origin.x + plot.size.width
                                && local.y < plot.origin.y + plot.size.height
                        })
                        .map(|plot| PanDrag {
                            origin: gesture.position,
                            viewport: self
                                .state
                                .viewport
                                .unwrap_or_else(|| self.pan_zoom_policy.reset()),
                            plot,
                            activated: false,
                        });
                }
                DragPhase::Moved => {
                    if let Some(drag) = &mut self.drag_origin {
                        let Some(delta) = drag.displacement(gesture.position) else {
                            return;
                        };
                        if let Some(viewport) = self.pan_zoom_policy.pan(drag.viewport, delta) {
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
                self.chart.borrow().snapshot(),
                RasterOptions {
                    logical_width: self.input().bounds.size.width.ceil() as u32,
                    logical_height: self.input().bounds.size.height.ceil() as u32,
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
                    charts[0].viewport_event(ViewportCommand::ZoomIn, self.pan_zoom_policy)
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

    fn view(&self, _context: ViewContext) -> impl twinkle::View<Message> {
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
            let chart = Rc::clone(&self.chart);
            twinkle::Container::new()
                .id("dashboard-chart-layout")
                .fill_width()
                .fill_height()
                .into_element()
                .with_layout_child_preferred(CHART_BOUNDS.size, move |allocation| {
                    let mut chart = chart.borrow_mut();
                    let mut input = chart.snapshot().input.expect("initial chart preparation");
                    input.bounds =
                        Rect::new(0.0, 0.0, allocation.size.width, allocation.size.height);
                    chart.update(input).expect("valid dashboard allocation");
                    chart
                        .element_with_input(
                            "dashboard-chart",
                            Message::Chart,
                            ChartInputBindings {
                                pointer_move: Some(pointer),
                                drag: Some((Message::Pointer(Point::default()), drag)),
                                wheel: Some(wheel),
                            },
                        )
                        .expect("accepted chart projects through public APIs")
                        .into_element()
                })
        };
        UiColumn::new()
            .fill_width()
            .fill_height()
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
    spec.axes.extend([
        twinkle_charts::Axis {
            scale: Arc::from("x"),
            title: Some(Arc::from("Month")),
            unit: None,
            tick_count: Some(6),
            grid: false,
        },
        twinkle_charts::Axis {
            scale: Arc::from("y"),
            title: Some(Arc::from("USD thousands")),
            unit: None,
            tick_count: Some(5),
            grid: true,
        },
    ]);
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
    let arguments: Vec<String> = std::env::args().collect();
    if arguments
        .get(1)
        .is_some_and(|arg| matches!(arg.as_str(), "--render-chart" | "--render-dashboard"))
    {
        if arguments.len() != 6 {
            return Err(
                "usage: --render-chart output.png logical-width logical-height device-scale".into(),
            );
        }
        let width: f32 = arguments[3].parse()?;
        let height: f32 = arguments[4].parse()?;
        let scale: f32 = arguments[5].parse()?;
        if [width, height, scale]
            .into_iter()
            .any(|v| !v.is_finite() || v <= 0.0)
        {
            return Err("render dimensions and device scale must be finite and positive".into());
        }
        let bounds = Rect::new(0.0, 0.0, width, height);
        dashboard
            .chart
            .borrow_mut()
            .update(RetainedSurfaceInput::new(
                bounds,
                scale,
                RetainedSurfaceRevisions {
                    content: dashboard.revision,
                    ..Default::default()
                },
            ))?;
        let element = dashboard
            .chart
            .borrow()
            .element_with_input(
                "dashboard-chart",
                Message::Chart,
                ChartInputBindings {
                    pointer_move: Some(pointer),
                    drag: Some((Message::Pointer(Point::default()), drag)),
                    wheel: Some(wheel),
                },
            )?
            .into_element();
        let frame = if arguments[1] == "--render-dashboard" {
            twinkle::UiFrame::layout(
                dashboard.view(ViewContext {
                    viewport: bounds,
                    modality: twinkle::InputModality::default(),
                    focused: None,
                    controller_target: None,
                    available_semantic_actions: Vec::new(),
                    navigation_depth: 0,
                    open_overlay: None,
                }),
                bounds,
            )
        } else {
            twinkle::UiFrame::layout(element, bounds)
        };
        let renderer = twinkle::GpuOffscreenRenderer::new()?;
        let physical_width = (width * scale).ceil() as u32;
        let physical_height = (height * scale).ceil() as u32;
        let pixels = renderer.render(frame.commands(), physical_width, physical_height, scale)?;
        let rgba: Vec<u8> = pixels.iter().flat_map(|p| [p.r, p.g, p.b, p.a]).collect();
        image::save_buffer(
            &arguments[2],
            &rgba,
            physical_width,
            physical_height,
            image::ColorType::Rgba8,
        )?;
        println!(
            "Rendered {} at {width}×{height} logical pixels, {scale}× DPI",
            arguments[2]
        );
        return Ok(());
    }
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

    #[test]
    fn dashboard_chart_tracks_available_space_and_reuses_unchanged_layout() {
        let mut dashboard = Dashboard::new().unwrap();
        let mut previous = None;
        for (width, height) in [(1080.0, 760.0), (640.0, 480.0), (900.0, 600.0)] {
            let bounds = Rect::new(0.0, 0.0, width, height);
            let mut ctx = context(width);
            ctx.viewport = bounds;
            let frame = UiFrame::layout(dashboard.view(ctx.clone()), bounds);
            let chart = frame
                .resolved_layout()
                .nodes()
                .iter()
                .find(|node| node.id.as_str().ends_with("/dashboard-chart-layout"))
                .expect("chart surface in layout")
                .allocated;
            assert!(chart.origin.y > 0.0);
            assert_eq!(chart.size.width, width);
            assert!((chart.origin.y + chart.size.height - height).abs() < 0.01);
            let accepted = dashboard.chart.borrow().snapshot().input.unwrap().bounds;
            assert_eq!(accepted.size, chart.size);
            if let Some(previous) = previous {
                assert_ne!(accepted.size, previous);
            }
            previous = Some(accepted.size);
            let plot = dashboard.chart.borrow().plot_bounds().unwrap();
            let local = Point {
                x: plot.origin.x + plot.size.width * 0.8,
                y: plot.origin.y + plot.size.height * 0.9,
            };
            let expected_row = dashboard.chart.borrow().nearest(local).unwrap().row.clone();
            let outcome = frame.handle_event(
                &mut twinkle::UiStateStore::default(),
                twinkle::UiEvent::PointerMoved(Point {
                    x: chart.origin.x + local.x,
                    y: chart.origin.y + local.y,
                }),
            );
            assert!(!outcome.messages.is_empty());
            for message in outcome.messages {
                dashboard.update(message);
            }
            assert_eq!(
                dashboard.chart.borrow().active_inspection().unwrap().row,
                expected_row
            );
            let misses = dashboard.chart.borrow().diagnostics().cache_misses;
            let _ = UiFrame::layout(dashboard.view(ctx), bounds);
            assert_eq!(dashboard.chart.borrow().diagnostics().cache_misses, misses);
        }
    }

    #[test]
    fn measured_legend_retains_both_series_outside_the_plot_across_sizes_and_dpi() {
        for width in [140.0, 260.0, 760.0] {
            for scale in [1.0, 1.25, 1.5, 2.0] {
                let dashboard = Dashboard::new().unwrap();
                let bounds = Rect::new(13.0, 29.0, width, 240.0);
                dashboard
                    .chart
                    .borrow_mut()
                    .update(RetainedSurfaceInput::new(bounds, scale, Default::default()))
                    .unwrap();
                let plot = dashboard.chart.borrow().plot_bounds().unwrap();
                let legends: Vec<_> = dashboard
                    .chart
                    .borrow()
                    .snapshot()
                    .output
                    .interactions
                    .iter()
                    .filter(|hit| hit.id.as_str().starts_with("chart/legend/"))
                    .cloned()
                    .collect();
                assert_eq!(legends.len(), 2, "width {width}, scale {scale}");
                for legend in &legends {
                    assert!(legend.bounds.origin.y + legend.bounds.size.height <= plot.origin.y);
                    assert!(legend.bounds.origin.x >= bounds.origin.x);
                    assert!(
                        legend.bounds.origin.x + legend.bounds.size.width
                            <= bounds.origin.x + width
                    );
                }
                let a = legends[0].bounds;
                let b = legends[1].bounds;
                assert!(
                    a.origin.x + a.size.width <= b.origin.x
                        || a.origin.y + a.size.height <= b.origin.y
                );
            }
        }
    }

    #[test]
    fn pan_uses_plot_fraction_and_visible_domain_and_latches_activation() {
        for (width, height) in [(200.0, 100.0), (800.0, 500.0)] {
            let mut drag = PanDrag {
                origin: Point { x: 123.0, y: 456.0 },
                viewport: Viewport {
                    x: Some([2.0, 4.0]),
                    y: Some([20.0, 60.0]),
                },
                plot: Rect::new(30.0, 70.0, width, height),
                activated: false,
            };
            assert_eq!(drag.displacement(Point { x: 124.0, y: 457.0 }), None);
            let delta = drag
                .displacement(Point {
                    x: drag.origin.x + width * 0.25,
                    y: drag.origin.y + height * 0.5,
                })
                .unwrap();
            assert_eq!(delta, [-0.5, 20.0]);
            // Returning inside the initial dead zone must not leave a stale pan.
            assert_eq!(drag.displacement(drag.origin), Some([0.0, 0.0]));
        }
    }
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
        assert!(dashboard.chart.borrow().active_inspection().is_some());
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
    fn click_jitter_does_not_replace_the_automatic_chart_domain() {
        let mut dashboard = Dashboard::new().unwrap();
        let origin = Point { x: 610.0, y: 110.0 };
        dashboard.update(Message::Drag(DragGesture {
            phase: DragPhase::Started,
            position: origin,
            bounds: CHART_BOUNDS,
        }));
        dashboard.update(Message::Drag(DragGesture {
            phase: DragPhase::Moved,
            position: Point {
                x: origin.x + 2.0,
                y: origin.y + 1.0,
            },
            bounds: CHART_BOUNDS,
        }));
        dashboard.update(Message::Drag(DragGesture {
            phase: DragPhase::Ended,
            position: origin,
            bounds: CHART_BOUNDS,
        }));

        assert_eq!(dashboard.state.viewport, None);
        assert_eq!(
            dashboard.chart.borrow().snapshot().input,
            Some(dashboard.input())
        );
    }

    #[test]
    fn reset_and_full_extent_pan_preserve_the_automatic_stack_geometry() {
        let mut dashboard = Dashboard::new().unwrap();
        assert_eq!(dashboard.pan_zoom_policy.x_extent, Some([1.0, 6.0]));
        assert_eq!(dashboard.pan_zoom_policy.y_extent, Some([0.0, 70.0]));
        let initial = dashboard.chart.borrow().snapshot().output.paint.clone();
        dashboard.update(Message::Chart(ChartEvent::Viewport(ViewportCommand::Reset)));
        assert_eq!(dashboard.chart.borrow().snapshot().output.paint, initial);

        let origin = Point { x: 200.0, y: 180.0 };
        dashboard.update(Message::Drag(DragGesture {
            phase: DragPhase::Started,
            position: origin,
            bounds: CHART_BOUNDS,
        }));
        dashboard.update(Message::Drag(DragGesture {
            phase: DragPhase::Moved,
            position: Point { x: 260.0, y: 210.0 },
            bounds: CHART_BOUNDS,
        }));
        assert_eq!(dashboard.chart.borrow().snapshot().output.paint, initial);
    }

    #[test]
    fn zoomed_drag_moves_by_the_accepted_plot_fraction() {
        let mut dashboard = Dashboard::new().unwrap();
        let initial = Viewport {
            x: Some([2.0, 4.0]),
            y: Some([20.0, 40.0]),
        };
        dashboard.apply_viewport(initial);
        let plot = dashboard.chart.borrow().plot_bounds().unwrap();
        let origin = Point { x: 300.0, y: 200.0 };
        dashboard.update(Message::Drag(DragGesture {
            phase: DragPhase::Started,
            position: origin,
            bounds: CHART_BOUNDS,
        }));
        dashboard.update(Message::Drag(DragGesture {
            phase: DragPhase::Moved,
            position: Point {
                x: origin.x + plot.size.width * 0.25,
                y: origin.y,
            },
            bounds: CHART_BOUNDS,
        }));
        let actual = dashboard.state.viewport.unwrap();
        for (actual, expected) in actual.x.unwrap().into_iter().zip([1.5, 3.5]) {
            assert!((actual - expected).abs() < 1e-6);
        }
        assert_eq!(actual.y, initial.y);
        dashboard.update(Message::Drag(DragGesture {
            phase: DragPhase::Moved,
            position: origin,
            bounds: CHART_BOUNDS,
        }));
        assert_eq!(dashboard.state.viewport, Some(initial));
    }

    #[test]
    fn header_drag_does_not_pan_and_offset_plot_drag_does() {
        let mut dashboard = Dashboard::new().unwrap();
        let bounds = Rect::new(
            83.0,
            129.0,
            CHART_BOUNDS.size.width,
            CHART_BOUNDS.size.height,
        );
        let header = Point {
            x: bounds.origin.x + 10.0,
            y: bounds.origin.y + 2.0,
        };
        dashboard.update(Message::Drag(DragGesture {
            phase: DragPhase::Started,
            position: header,
            bounds,
        }));
        assert!(dashboard.drag_origin.is_none());
        let plot = dashboard.chart.borrow().plot_bounds().unwrap();
        let in_plot = Point {
            x: bounds.origin.x + plot.origin.x + plot.size.width * 0.5,
            y: bounds.origin.y + plot.origin.y + plot.size.height * 0.5,
        };
        dashboard.update(Message::Drag(DragGesture {
            phase: DragPhase::Started,
            position: in_plot,
            bounds,
        }));
        assert!(dashboard.drag_origin.is_some());
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

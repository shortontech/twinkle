use std::{collections::BTreeMap, sync::Arc, time::Instant};

use twinkle_ui::{
    ActionKind, Point, Rect, SemanticRole, UiId,
    backend::{PaintCommand, PathGeometry, PathGeometryKind, PathLineCap, PathLineJoin},
    retained_surface::{
        RetainedSurfaceChanges, RetainedSurfaceHost, RetainedSurfaceInput,
        RetainedSurfaceInteraction, RetainedSurfaceLifecycle, RetainedSurfaceLimits,
        RetainedSurfaceModel, RetainedSurfaceOutput, RetainedSurfaceSnapshot,
    },
};

use crate::{
    Channel, ChartData, ChartDiagnostics, ChartError, ChartLimits, ChartSpec, ChartTheme,
    DataValue, Encoding, Mark,
};

/// Accepted spatial-index entry retaining stable source identity.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartHit {
    pub dataset: Arc<str>,
    pub row: crate::RowKey,
    pub series: Option<Arc<str>>,
    pub bounds: Rect,
    pub revision: u64,
}

/// Persistent chart preparation state. Clone it to stage a transactional host candidate.
#[derive(Clone, Debug)]
pub struct ChartModel {
    spec: Arc<ChartSpec>,
    data: Arc<ChartData>,
    limits: ChartLimits,
    theme: ChartTheme,
    diagnostics: ChartDiagnostics,
    hits: Vec<ChartHit>,
    hit_x_order: Vec<usize>,
    renderer_generation: u64,
}

impl ChartModel {
    pub fn new(
        spec: Arc<ChartSpec>,
        data: Arc<ChartData>,
        limits: ChartLimits,
    ) -> Result<Self, ChartError> {
        spec.validate(&data, &limits)?;
        let theme = ChartTheme::default();
        theme.validate()?;
        Ok(Self {
            spec,
            data,
            limits,
            theme,
            diagnostics: ChartDiagnostics::default(),
            hits: Vec::new(),
            hit_x_order: Vec::new(),
            renderer_generation: 0,
        })
    }
    pub fn with_theme(mut self, theme: ChartTheme) -> Result<Self, ChartError> {
        theme.validate()?;
        self.theme = theme;
        Ok(self)
    }
    pub fn replace(
        &mut self,
        spec: Arc<ChartSpec>,
        data: Arc<ChartData>,
    ) -> Result<(), ChartError> {
        spec.validate(&data, &self.limits)?;
        self.spec = spec;
        self.data = data;
        Ok(())
    }
    pub fn diagnostics(&self) -> &ChartDiagnostics {
        &self.diagnostics
    }
    pub fn hits(&self) -> &[ChartHit] {
        &self.hits
    }
    pub fn nearest(&self, point: Point) -> Option<&ChartHit> {
        let insertion = self
            .hit_x_order
            .partition_point(|index| hit_center_x(&self.hits[*index]) < point.x);
        let start = insertion.saturating_sub(512);
        let end = insertion.saturating_add(512).min(self.hit_x_order.len());
        self.hit_x_order[start..end]
            .iter()
            .map(|index| &self.hits[*index])
            .min_by(|a, b| {
                distance(a.bounds, point)
                    .total_cmp(&distance(b.bounds, point))
                    .then_with(|| a.dataset.cmp(&b.dataset))
                    .then_with(|| a.row.cmp(&b.row))
            })
    }

    fn prepare(
        &self,
        input: RetainedSurfaceInput,
    ) -> Result<
        (
            RetainedSurfaceOutput,
            Vec<ChartHit>,
            Vec<usize>,
            ChartDiagnostics,
        ),
        ChartError,
    > {
        self.spec.validate(&self.data, &self.limits)?;
        let started = Instant::now();
        let mut output = RetainedSurfaceOutput {
            measured: input.bounds.size,
            damage: vec![input.bounds],
            ..RetainedSurfaceOutput::default()
        };
        let plot = inset(input.bounds, 36.0, 18.0, 18.0, 30.0);
        output.paint.push(PaintCommand::Fill {
            rect: input.bounds,
            color: self.theme.background,
        });
        output.paint.push(PaintCommand::Stroke {
            rect: plot,
            color: self.theme.grid,
            width: 1.0,
        });
        output.paint.push(PaintCommand::Text {
            bounds: Rect::new(
                input.bounds.origin.x + 8.0,
                input.bounds.origin.y + 4.0,
                (input.bounds.size.width - 16.0).max(0.0),
                22.0,
            ),
            text: self.spec.title.to_string(),
            scale: 1.0,
            color: self.theme.foreground,
            align: twinkle_ui::TextAlign::Start,
            bold: true,
            wrap: false,
        });
        output
            .semantics
            .push(twinkle_ui::retained_surface::RetainedSurfaceSemantic {
                id: UiId::from("chart"),
                bounds: input.bounds,
                role: SemanticRole::GraphicalCustomControl,
                label: Some(self.spec.title.to_string()),
                state: Some(self.spec.summary.to_string()),
                protected: false,
            });
        output.interactions.push(RetainedSurfaceInteraction {
            id: UiId::from("chart"),
            bounds: input.bounds,
            action: ActionKind::Activate,
            focusable: true,
        });
        let mut hits = Vec::new();
        if plot.size.width > 1.0 && plot.size.height > 1.0 {
            for (layer_index, layer) in self
                .spec
                .layers
                .iter()
                .filter(|layer| layer.visible)
                .enumerate()
            {
                self.prepare_layer(layer_index, layer, plot, &mut output, &mut hits)?;
            }
        }
        if self.data.row_count() == 0 {
            output.paint.push(PaintCommand::Text {
                bounds: plot,
                text: "No data".into(),
                scale: 1.0,
                color: self.theme.muted,
                align: twinkle_ui::TextAlign::Center,
                bold: false,
                wrap: false,
            });
        }
        let geometry_vertices = output
            .paint
            .iter()
            .map(|command| match command {
                PaintCommand::Path { geometry, .. } => geometry.triangles().len(),
                _ => 0,
            })
            .sum();
        if geometry_vertices > self.limits.max_geometry_vertices {
            return Err(ChartError::Limit {
                resource: "prepared geometry vertices",
                actual: geometry_vertices,
                limit: self.limits.max_geometry_vertices,
            });
        }
        let mut hit_x_order: Vec<usize> = (0..hits.len()).collect();
        hit_x_order.sort_by(|a, b| {
            hit_center_x(&hits[*a])
                .total_cmp(&hit_center_x(&hits[*b]))
                .then_with(|| hits[*a].row.cmp(&hits[*b].row))
        });
        let index_bytes = hit_x_order.capacity() * std::mem::size_of::<usize>();
        let steady_bytes = hits.capacity() * std::mem::size_of::<ChartHit>()
            + index_bytes
            + geometry_vertices * std::mem::size_of::<Point>();
        if steady_bytes > self.limits.max_steady_bytes {
            return Err(ChartError::Limit {
                resource: "chart steady bytes",
                actual: steady_bytes,
                limit: self.limits.max_steady_bytes,
            });
        }
        let raster_marks = output
            .paint
            .iter()
            .filter(|command| {
                matches!(
                    command,
                    PaintCommand::Fill { .. } | PaintCommand::RoundedFill { .. }
                )
            })
            .count()
            .saturating_sub(1);
        let displayed_rows = hits.len().min(
            raster_marks
                .saturating_add(geometry_vertices / 6 + usize::from(geometry_vertices != 0)),
        );
        let mut notices = self.diagnostics.notices.clone();
        if displayed_rows < hits.len() {
            notices.push(crate::ChartDiagnostic {
                stage: crate::ChartStage::Geometry,
                severity: crate::DiagnosticSeverity::Info,
                message: format!(
                    "pixel-aware reduction displays {displayed_rows} of {} exact indexed rows",
                    hits.len()
                ),
                affected_rows: hits.len() - displayed_rows,
            });
        }
        let diagnostics = ChartDiagnostics {
            preparation_time: started.elapsed(),
            input_rows: self.data.row_count(),
            transformed_rows: self.data.row_count(),
            displayed_rows,
            geometry_vertices,
            index_bytes,
            guide_count: self.spec.axes.len()
                + self.spec.legends.len()
                + self.spec.annotations.len(),
            paint_commands: output.paint.len(),
            interaction_records: output.interactions.len(),
            semantic_records: output.semantics.len(),
            steady_bytes,
            peak_bytes: steady_bytes,
            cache_misses: 1,
            notices,
            ..self.diagnostics.clone()
        };
        Ok((output, hits, hit_x_order, diagnostics))
    }

    fn prepare_layer(
        &self,
        layer_index: usize,
        layer: &crate::Layer,
        plot: Rect,
        output: &mut RetainedSurfaceOutput,
        hits: &mut Vec<ChartHit>,
    ) -> Result<(), ChartError> {
        if let Mark::Arc { inner_radius } = layer.mark {
            return self.prepare_arc(layer_index, layer, plot, inner_radius, output, hits);
        }
        let x = channel(&layer.encodings, Channel::X).ok_or_else(|| {
            ChartError::invalid(crate::ChartStage::Geometry, "mark requires x encoding")
        })?;
        let y = channel(&layer.encodings, Channel::Y).ok_or_else(|| {
            ChartError::invalid(crate::ChartStage::Geometry, "mark requires y encoding")
        })?;
        if x.field.dataset != y.field.dataset {
            return Err(ChartError::invalid(
                crate::ChartStage::Geometry,
                "x and y must currently share a dataset",
            ));
        }
        let source = self
            .data
            .dataset(&x.field.dataset)
            .ok_or_else(|| ChartError::MissingDataset(x.field.dataset.to_string()))?;
        let transformed;
        let dataset = if layer.transforms.is_empty() {
            source
        } else {
            transformed = crate::execute_transforms(source, &layer.transforms, &self.limits)?;
            &transformed
        };
        let x_column =
            dataset
                .column(&x.field.column)
                .ok_or_else(|| ChartError::MissingColumn {
                    dataset: x.field.dataset.to_string(),
                    column: x.field.column.to_string(),
                })?;
        let y_column =
            dataset
                .column(&y.field.column)
                .ok_or_else(|| ChartError::MissingColumn {
                    dataset: y.field.dataset.to_string(),
                    column: y.field.column.to_string(),
                })?;
        let y2_column = channel(&layer.encodings, Channel::Y2)
            .map(|encoding| {
                dataset
                    .column(&encoding.field.column)
                    .ok_or_else(|| ChartError::MissingColumn {
                        dataset: encoding.field.dataset.to_string(),
                        column: encoding.field.column.to_string(),
                    })
            })
            .transpose()?;
        let x_coordinates = column_coordinates(x_column);
        let values: Vec<(usize, f64, f64)> = x_column
            .values
            .iter()
            .zip(y_column.values.iter())
            .enumerate()
            .filter_map(|(index, (_x, y))| {
                x_coordinates[index]
                    .zip(coordinate(y, index))
                    .map(|(x, y)| (index, x, y))
            })
            .collect();
        if values.is_empty() {
            return Ok(());
        }
        let x_domain = extent(values.iter().map(|(_, x, _)| *x));
        let mut y_domain = extent(values.iter().map(|(_, _, y)| *y).chain(
            y2_column.into_iter().flat_map(|column| {
                column
                    .values
                    .iter()
                    .enumerate()
                    .filter_map(|(index, value)| coordinate(value, index))
            }),
        ));
        let baseline = match layer.mark {
            Mark::Area { baseline, .. } => Some(baseline as f64),
            Mark::Bar { .. } | Mark::Rect => Some(0.0),
            _ => None,
        };
        if let Some(baseline) = baseline {
            y_domain[0] = y_domain[0].min(baseline);
            y_domain[1] = y_domain[1].max(baseline);
        }
        let categorical_x_count = x_column
            .values
            .iter()
            .filter_map(DataValue::category)
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let x_inset = if categorical_x_count == 0 {
            0.0
        } else {
            plot.size.width / categorical_x_count as f32 / 2.0
        };
        let map = |x: f64, y: f64| Point {
            x: map_value(
                x,
                x_domain,
                [
                    plot.origin.x + x_inset,
                    plot.origin.x + plot.size.width - x_inset,
                ],
            ),
            y: map_value(
                y,
                y_domain,
                [plot.origin.y + plot.size.height, plot.origin.y],
            ),
        };
        let color = self.theme.series_cue(layer_index).color;
        match &layer.mark {
            Mark::Line { .. } => {
                let mut triangles = Vec::new();
                let mut segment = Vec::new();
                let max_points = (plot.size.width.ceil() as usize).saturating_mul(4).max(2);
                for (index, y) in y_column.values.iter().enumerate() {
                    match (x_coordinates[index], coordinate(y, index)) {
                        (Some(x), Some(y)) => segment.push(map(x, y)),
                        _ => {
                            triangles.extend(stroke_polyline(
                                &reduce_extrema(&segment, max_points),
                                2.0,
                            ));
                            segment.clear();
                        }
                    }
                }
                triangles.extend(stroke_polyline(&reduce_extrema(&segment, max_points), 2.0));
                if !triangles.is_empty() {
                    output
                        .paint
                        .push(path_command(triangles, plot, color, values.len())?);
                }
            }
            Mark::Area { baseline, .. } => {
                let mut triangles = Vec::new();
                if let Some(base_column) = y2_column {
                    for pair in values.windows(2) {
                        if pair[1].0 != pair[0].0 + 1 {
                            continue;
                        }
                        let Some(base0) = coordinate(&base_column.values[pair[0].0], pair[0].0)
                        else {
                            continue;
                        };
                        let Some(base1) = coordinate(&base_column.values[pair[1].0], pair[1].0)
                        else {
                            continue;
                        };
                        let top0 = map(pair[0].1, pair[0].2);
                        let top1 = map(pair[1].1, pair[1].2);
                        let bottom0 = map(pair[0].1, base0);
                        let bottom1 = map(pair[1].1, base1);
                        triangles.extend([top0, bottom0, top1, top1, bottom0, bottom1]);
                    }
                } else {
                    let baseline = map_value(
                        *baseline,
                        y_domain,
                        [plot.origin.y + plot.size.height, plot.origin.y],
                    );
                    let mut segment = Vec::new();
                    let max_points = (plot.size.width.ceil() as usize).saturating_mul(4).max(2);
                    for (index, y) in y_column.values.iter().enumerate() {
                        match (x_coordinates[index], coordinate(y, index)) {
                            (Some(x), Some(y)) => segment.push(map(x, y)),
                            _ => {
                                triangles.extend(area_triangles(
                                    &reduce_extrema(&segment, max_points),
                                    baseline,
                                ));
                                segment.clear();
                            }
                        }
                    }
                    triangles.extend(area_triangles(
                        &reduce_extrema(&segment, max_points),
                        baseline,
                    ));
                }
                if !triangles.is_empty() {
                    output
                        .paint
                        .push(fill_path_command(triangles, color, values.len())?);
                }
            }
            Mark::Point { .. } => {
                let max_marks =
                    ((plot.size.width * plot.size.height / 16.0) as usize).clamp(1, 50_000);
                let step = values.len().div_ceil(max_marks).max(1);
                for (_, x, y) in values.iter().step_by(step) {
                    let center = map(*x, *y);
                    let bounds = Rect::new(center.x - 3.0, center.y - 3.0, 6.0, 6.0);
                    output.paint.push(PaintCommand::RoundedFill {
                        rect: bounds,
                        color,
                        radius: 3.0,
                    });
                }
            }
            Mark::Bar { .. } | Mark::Rect => {
                if values.len() > 60_000 {
                    return Err(ChartError::Limit {
                        resource: "bar/heatmap paint marks",
                        actual: values.len(),
                        limit: 60_000,
                    });
                }
                let category_count = x_coordinates
                    .iter()
                    .flatten()
                    .map(|value| value.to_bits())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    .max(1);
                let series_column = channel(&layer.encodings, Channel::Series)
                    .and_then(|encoding| dataset.column(&encoding.field.column));
                let series_coordinates = series_column.map(column_coordinates);
                let series_count = series_coordinates.as_ref().map_or(1, |coordinates| {
                    coordinates
                        .iter()
                        .flatten()
                        .map(|value| value.to_bits())
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        .max(1)
                });
                let grouped = y2_column.is_none() && series_count > 1;
                let category_width = (plot.size.width / category_count as f32 * 0.72).max(1.0);
                let width = if grouped {
                    category_width / series_count as f32
                } else {
                    category_width
                };
                for (index, x, y) in &values {
                    let mut point = map(*x, *y);
                    if grouped {
                        let series = series_coordinates
                            .as_ref()
                            .and_then(|coordinates| coordinates[*index])
                            .unwrap_or(0.0) as f32;
                        point.x += (series - (series_count as f32 - 1.0) / 2.0) * width;
                    }
                    let baseline_value = y2_column
                        .and_then(|column| coordinate(&column.values[*index], *index))
                        .unwrap_or(0.0);
                    let baseline = map_value(
                        baseline_value,
                        y_domain,
                        [plot.origin.y + plot.size.height, plot.origin.y],
                    );
                    let left = (point.x - width / 2.0).max(plot.origin.x);
                    let right = (point.x + width / 2.0).min(plot.origin.x + plot.size.width);
                    let bounds = Rect::new(
                        left,
                        point.y.min(baseline),
                        (right - left).max(0.0),
                        (point.y - baseline).abs().max(1.0),
                    );
                    output.paint.push(PaintCommand::Fill {
                        rect: bounds,
                        color,
                    });
                    self.push_hit(dataset, *index, bounds, hits);
                }
            }
            _ => {
                return Err(ChartError::Unsupported(format!(
                    "{:?} native preparation",
                    layer.mark
                )));
            }
        }
        if matches!(
            layer.mark,
            Mark::Line { .. } | Mark::Area { .. } | Mark::Point { .. }
        ) {
            for (index, x, y) in values {
                let p = map(x, y);
                self.push_hit(
                    dataset,
                    index,
                    Rect::new(p.x - 5.0, p.y - 5.0, 10.0, 10.0),
                    hits,
                );
            }
        }
        Ok(())
    }

    fn prepare_arc(
        &self,
        layer_index: usize,
        layer: &crate::Layer,
        plot: Rect,
        inner_ratio: f32,
        output: &mut RetainedSurfaceOutput,
        hits: &mut Vec<ChartHit>,
    ) -> Result<(), ChartError> {
        if !(0.0..1.0).contains(&inner_ratio) {
            return Err(ChartError::invalid(
                crate::ChartStage::Geometry,
                "arc inner radius must be in [0, 1)",
            ));
        }
        let value = channel(&layer.encodings, Channel::Y).ok_or_else(|| {
            ChartError::invalid(crate::ChartStage::Geometry, "arc requires y value encoding")
        })?;
        let dataset = self
            .data
            .dataset(&value.field.dataset)
            .ok_or_else(|| ChartError::MissingDataset(value.field.dataset.to_string()))?;
        let column =
            dataset
                .column(&value.field.column)
                .ok_or_else(|| ChartError::MissingColumn {
                    dataset: value.field.dataset.to_string(),
                    column: value.field.column.to_string(),
                })?;
        let values: Vec<(usize, f64)> = column
            .values
            .iter()
            .enumerate()
            .filter_map(|(index, value)| match value {
                DataValue::Number(value) if value.is_finite() && *value >= 0.0 => {
                    Some((index, *value))
                }
                _ => None,
            })
            .collect();
        let total: f64 = values.iter().map(|(_, value)| value).sum();
        if total <= 0.0 {
            return Ok(());
        }
        let center = Point {
            x: plot.origin.x + plot.size.width / 2.0,
            y: plot.origin.y + plot.size.height / 2.0,
        };
        let radius = plot.size.width.min(plot.size.height) * 0.45;
        let inner = radius * inner_ratio;
        let mut start = -std::f32::consts::FRAC_PI_2;
        for (slice, (row, value)) in values.into_iter().enumerate() {
            let end = start + std::f32::consts::TAU * (value / total) as f32;
            let steps = (((end - start).abs() * radius / 5.0).ceil() as usize).clamp(2, 128);
            let mut triangles = Vec::with_capacity(steps * 6);
            for step in 0..steps {
                let a0 = start + (end - start) * step as f32 / steps as f32;
                let a1 = start + (end - start) * (step + 1) as f32 / steps as f32;
                let outer0 = Point {
                    x: center.x + radius * a0.cos(),
                    y: center.y + radius * a0.sin(),
                };
                let outer1 = Point {
                    x: center.x + radius * a1.cos(),
                    y: center.y + radius * a1.sin(),
                };
                if inner == 0.0 {
                    triangles.extend([center, outer0, outer1]);
                } else {
                    let inner0 = Point {
                        x: center.x + inner * a0.cos(),
                        y: center.y + inner * a0.sin(),
                    };
                    let inner1 = Point {
                        x: center.x + inner * a1.cos(),
                        y: center.y + inner * a1.sin(),
                    };
                    triangles.extend([inner0, outer0, inner1, inner1, outer0, outer1]);
                }
            }
            output.paint.push(fill_path_command(
                triangles,
                self.theme.series_cue(layer_index + slice).color,
                steps + 1,
            )?);
            self.push_hit(
                dataset,
                row,
                Rect::new(
                    center.x - radius,
                    center.y - radius,
                    radius * 2.0,
                    radius * 2.0,
                ),
                hits,
            );
            start = end;
        }
        Ok(())
    }

    fn push_hit(
        &self,
        dataset: &crate::Dataset,
        row: usize,
        bounds: Rect,
        hits: &mut Vec<ChartHit>,
    ) {
        let key = dataset.keys()[row].clone();
        hits.push(ChartHit {
            dataset: dataset.id.0.clone(),
            row: key.clone(),
            series: None,
            bounds,
            revision: self.data.revision(),
        });
    }
}

impl RetainedSurfaceModel for ChartModel {
    fn update(
        &mut self,
        input: RetainedSurfaceInput,
        _changes: RetainedSurfaceChanges,
    ) -> Result<RetainedSurfaceOutput, String> {
        match self.prepare(input) {
            Ok((output, hits, hit_x_order, diagnostics)) => {
                self.hits = hits;
                self.hit_x_order = hit_x_order;
                self.diagnostics = diagnostics;
                Ok(output)
            }
            Err(error) => {
                self.diagnostics.rejected_updates =
                    self.diagnostics.rejected_updates.saturating_add(1);
                Err(error.to_string())
            }
        }
    }
    fn lifecycle(&mut self, event: RetainedSurfaceLifecycle) {
        match event {
            RetainedSurfaceLifecycle::RendererLost => {
                self.renderer_generation = self.renderer_generation.saturating_add(1)
            }
            RetainedSurfaceLifecycle::Detached | RetainedSurfaceLifecycle::Destroyed => {
                self.hits.clear();
                self.hit_x_order.clear();
            }
            _ => {}
        }
    }
    fn retained_bytes(&self) -> usize {
        self.hits.capacity() * std::mem::size_of::<ChartHit>()
            + self.hit_x_order.capacity() * std::mem::size_of::<usize>()
    }
}

/// Transactional retained host and normal Twinkle projection authority.
pub struct ChartView {
    host: RetainedSurfaceHost<ChartModel>,
}
impl ChartView {
    pub fn new(model: ChartModel) -> Result<Self, ChartError> {
        let limits = RetainedSurfaceLimits {
            max_retained_bytes: model.limits.max_steady_bytes,
            ..RetainedSurfaceLimits::default()
        };
        Ok(Self {
            host: RetainedSurfaceHost::new(model, limits)
                .map_err(|message| ChartError::invalid(crate::ChartStage::Validate, message))?,
        })
    }
    pub fn update(
        &mut self,
        input: RetainedSurfaceInput,
    ) -> Result<&RetainedSurfaceSnapshot, ChartError> {
        let mut candidate = self.host.begin_candidate();
        candidate
            .update(input)
            .map_err(|message| ChartError::invalid(crate::ChartStage::Geometry, message))?;
        let old = self.host.finalize_candidate(candidate);
        drop(old);
        Ok(self.host.snapshot())
    }
    pub fn snapshot(&self) -> &RetainedSurfaceSnapshot {
        self.host.snapshot()
    }
    pub fn detach(&mut self) {
        self.host.detach();
    }
    pub fn renderer_lost(&mut self) {
        self.host.renderer_lost();
    }
}

fn channel(encodings: &[Encoding], target: Channel) -> Option<&Encoding> {
    encodings.iter().find(|encoding| encoding.channel == target)
}

fn coordinate(value: &DataValue, category_index: usize) -> Option<f64> {
    match value {
        DataValue::Number(value) if value.is_finite() => Some(*value),
        DataValue::Timestamp(value) => Some(*value as f64),
        DataValue::Category(_) => Some(category_index as f64),
        _ => None,
    }
}
fn column_coordinates(column: &crate::Column) -> Vec<Option<f64>> {
    let mut categories: BTreeMap<Arc<str>, usize> = BTreeMap::new();
    column
        .values
        .iter()
        .enumerate()
        .map(|(index, value)| match value {
            DataValue::Category(category) => {
                let next = categories.len();
                Some(*categories.entry(category.clone()).or_insert(next) as f64)
            }
            _ => coordinate(value, index),
        })
        .collect()
}
fn extent(values: impl Iterator<Item = f64>) -> [f64; 2] {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for value in values {
        min = min.min(value);
        max = max.max(value);
    }
    if min == max {
        let d = min.abs().max(1.0) * 0.5;
        [min - d, max + d]
    } else {
        [min, max]
    }
}
fn map_value(value: f64, domain: [f64; 2], range: [f32; 2]) -> f32 {
    (range[0] as f64 + (value - domain[0]) / (domain[1] - domain[0]) * (range[1] - range[0]) as f64)
        as f32
}
fn inset(rect: Rect, left: f32, top: f32, right: f32, bottom: f32) -> Rect {
    Rect::new(
        rect.origin.x + left,
        rect.origin.y + top,
        (rect.size.width - left - right).max(0.0),
        (rect.size.height - top - bottom).max(0.0),
    )
}
fn distance(rect: Rect, point: Point) -> f32 {
    let x = point
        .x
        .clamp(rect.origin.x, rect.origin.x + rect.size.width);
    let y = point
        .y
        .clamp(rect.origin.y, rect.origin.y + rect.size.height);
    (point.x - x).hypot(point.y - y)
}
fn hit_center_x(hit: &ChartHit) -> f32 {
    hit.bounds.origin.x + hit.bounds.size.width / 2.0
}
fn stroke_polyline(points: &[Point], width: f32) -> Vec<Point> {
    let mut out = Vec::with_capacity(points.len().saturating_sub(1) * 6);
    for pair in points.windows(2) {
        let dx = pair[1].x - pair[0].x;
        let dy = pair[1].y - pair[0].y;
        let length = dx.hypot(dy);
        if length <= f32::EPSILON {
            continue;
        }
        let nx = -dy / length * width / 2.0;
        let ny = dx / length * width / 2.0;
        let a = Point {
            x: pair[0].x + nx,
            y: pair[0].y + ny,
        };
        let b = Point {
            x: pair[0].x - nx,
            y: pair[0].y - ny,
        };
        let c = Point {
            x: pair[1].x + nx,
            y: pair[1].y + ny,
        };
        let d = Point {
            x: pair[1].x - nx,
            y: pair[1].y - ny,
        };
        out.extend([a, b, c, c, b, d]);
    }
    out
}

fn area_triangles(points: &[Point], baseline: f32) -> Vec<Point> {
    let mut triangles = Vec::with_capacity(points.len().saturating_sub(1) * 6);
    for pair in points.windows(2) {
        triangles.extend([
            pair[0],
            Point {
                x: pair[0].x,
                y: baseline,
            },
            pair[1],
            pair[1],
            Point {
                x: pair[0].x,
                y: baseline,
            },
            Point {
                x: pair[1].x,
                y: baseline,
            },
        ]);
    }
    triangles
}

/// Bucket reduction preserves both endpoints and the ordered min/max extrema
/// in each bucket. It operates on mapped coordinates so work follows pixels.
fn reduce_extrema(points: &[Point], max_points: usize) -> Vec<Point> {
    if points.len() <= max_points || max_points < 4 {
        return points.to_vec();
    }
    let interior = &points[1..points.len() - 1];
    let buckets = (max_points - 2) / 2;
    let width = interior.len().div_ceil(buckets.max(1));
    let mut result = Vec::with_capacity(max_points + 2);
    result.push(points[0]);
    for bucket in interior.chunks(width.max(1)) {
        let min = bucket
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.y.total_cmp(&b.y))
            .map(|(index, point)| (index, *point))
            .unwrap();
        let max = bucket
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.y.total_cmp(&b.y))
            .map(|(index, point)| (index, *point))
            .unwrap();
        if min.0 <= max.0 {
            result.push(min.1);
            if min.0 != max.0 {
                result.push(max.1);
            }
        } else {
            result.push(max.1);
            result.push(min.1);
        }
    }
    result.push(*points.last().unwrap());
    result
}
fn path_command(
    triangles: Vec<Point>,
    _bounds: Rect,
    color: u32,
    source_vertices: usize,
) -> Result<PaintCommand, ChartError> {
    let min_x = triangles
        .iter()
        .map(|point| point.x)
        .fold(f32::INFINITY, f32::min);
    let min_y = triangles
        .iter()
        .map(|point| point.y)
        .fold(f32::INFINITY, f32::min);
    let max_x = triangles
        .iter()
        .map(|point| point.x)
        .fold(f32::NEG_INFINITY, f32::max);
    let max_y = triangles
        .iter()
        .map(|point| point.y)
        .fold(f32::NEG_INFINITY, f32::max);
    let bounds = Rect::new(min_x, min_y, max_x - min_x, max_y - min_y);
    let geometry = PathGeometry::new(
        triangles,
        bounds,
        PathGeometryKind::Stroke {
            width: 2.0,
            join: PathLineJoin::Bevel,
            cap: PathLineCap::Round,
            miter_limit: 4.0,
        },
        0.25,
        source_vertices,
    )
    .map_err(|message| ChartError::invalid(crate::ChartStage::Geometry, message))?;
    Ok(PaintCommand::Path {
        geometry: Arc::new(geometry),
        color,
    })
}

fn fill_path_command(
    triangles: Vec<Point>,
    color: u32,
    source_vertices: usize,
) -> Result<PaintCommand, ChartError> {
    let min_x = triangles
        .iter()
        .map(|point| point.x)
        .fold(f32::INFINITY, f32::min);
    let min_y = triangles
        .iter()
        .map(|point| point.y)
        .fold(f32::INFINITY, f32::min);
    let max_x = triangles
        .iter()
        .map(|point| point.x)
        .fold(f32::NEG_INFINITY, f32::max);
    let max_y = triangles
        .iter()
        .map(|point| point.y)
        .fold(f32::NEG_INFINITY, f32::max);
    let geometry = PathGeometry::new(
        triangles,
        Rect::new(min_x, min_y, max_x - min_x, max_y - min_y),
        PathGeometryKind::Fill {
            rule: twinkle_ui::backend::PathFillRule::NonZero,
        },
        0.25,
        source_vertices,
    )
    .map_err(|message| ChartError::invalid(crate::ChartStage::Geometry, message))?;
    Ok(PaintCommand::Path {
        geometry: Arc::new(geometry),
        color,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, Dataset, DatasetId, FieldRef, Recipes, RowKey};
    fn model() -> ChartModel {
        let data = Dataset::new(
            DatasetId::from("d"),
            vec![RowKey::from("a"), RowKey::from("b")],
            vec![
                Column::new("x", vec![DataValue::Number(0.0), DataValue::Number(1.0)]),
                Column::new("y", vec![DataValue::Number(2.0), DataValue::Number(4.0)]),
            ],
        )
        .unwrap();
        let limits = ChartLimits::default();
        let data = Arc::new(ChartData::new(7, vec![data], &limits).unwrap());
        ChartModel::new(
            Arc::new(Recipes::line(
                "Trend",
                FieldRef::new("d", "x"),
                FieldRef::new("d", "y"),
            )),
            data,
            limits,
        )
        .unwrap()
    }
    #[test]
    fn prepares_bounded_path_and_semantics() {
        let mut view = ChartView::new(model()).unwrap();
        let snapshot = view
            .update(RetainedSurfaceInput::new(
                Rect::new(0.0, 0.0, 400.0, 240.0),
                1.0,
                twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                    content: 7,
                    ..Default::default()
                },
            ))
            .unwrap();
        assert!(
            snapshot
                .output
                .paint
                .iter()
                .any(|command| matches!(command, PaintCommand::Path { .. }))
        );
        assert_eq!(snapshot.output.interactions.len(), 1);
        assert!(
            snapshot
                .output
                .semantics
                .iter()
                .any(|item| item.role == SemanticRole::GraphicalCustomControl)
        );
    }

    #[test]
    fn histogram_recipe_executes_bins_and_aggregate_before_rendering() {
        let limits = ChartLimits::default();
        let dataset = Dataset::new(
            DatasetId::from("d"),
            vec![RowKey::from("a"), RowKey::from("b"), RowKey::from("c")],
            vec![Column::new(
                "value",
                vec![
                    DataValue::Number(0.0),
                    DataValue::Number(4.0),
                    DataValue::Number(10.0),
                ],
            )],
        )
        .unwrap();
        let data = Arc::new(ChartData::new(9, vec![dataset], &limits).unwrap());
        let spec = crate::Recipes::histogram(
            "Distribution",
            FieldRef::new("d", "value"),
            crate::BinOptions {
                start: 0.0,
                end: 10.0,
                count: 2,
                density: false,
            },
        );
        let mut view =
            ChartView::new(ChartModel::new(Arc::new(spec), data, limits).unwrap()).unwrap();
        let snapshot = view
            .update(RetainedSurfaceInput::new(
                Rect::new(0.0, 0.0, 400.0, 240.0),
                1.0,
                twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                    content: 9,
                    ..Default::default()
                },
            ))
            .unwrap();
        assert_eq!(snapshot.output.interactions.len(), 1);
        assert!(
            snapshot
                .output
                .paint
                .iter()
                .any(|command| matches!(command, PaintCommand::Fill { .. }))
        );
    }

    #[test]
    fn pixel_reduction_preserves_endpoints_and_bucket_extrema() {
        let mut points: Vec<Point> = (0..10_000)
            .map(|index| Point {
                x: index as f32,
                y: (index % 11) as f32,
            })
            .collect();
        points[5_000].y = 10_000.0;
        let reduced = reduce_extrema(&points, 400);
        assert_eq!(reduced.first(), points.first());
        assert_eq!(reduced.last(), points.last());
        assert!(reduced.iter().any(|point| point.y == 10_000.0));
        assert!(reduced.len() <= 402);
    }

    #[test]
    fn grouped_and_normalized_stacked_bars_lower_to_distinct_native_rectangles() {
        let limits = ChartLimits::default();
        let dataset = Dataset::new(
            DatasetId::from("d"),
            vec![RowKey::from("a1"), RowKey::from("a2"), RowKey::from("b1")],
            vec![
                Column::new(
                    "category",
                    vec![
                        DataValue::Category(Arc::from("a")),
                        DataValue::Category(Arc::from("a")),
                        DataValue::Category(Arc::from("b")),
                    ],
                ),
                Column::new(
                    "series",
                    vec![
                        DataValue::Category(Arc::from("one")),
                        DataValue::Category(Arc::from("two")),
                        DataValue::Category(Arc::from("one")),
                    ],
                ),
                Column::new(
                    "value",
                    vec![
                        DataValue::Number(2.0),
                        DataValue::Number(3.0),
                        DataValue::Number(4.0),
                    ],
                ),
            ],
        )
        .unwrap();
        let data = Arc::new(ChartData::new(10, vec![dataset], &limits).unwrap());
        let input = RetainedSurfaceInput::new(
            Rect::new(0.0, 0.0, 400.0, 240.0),
            1.0,
            twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                content: 10,
                ..Default::default()
            },
        );
        let render = |spec| {
            let mut view = ChartView::new(
                ChartModel::new(Arc::new(spec), data.clone(), limits.clone()).unwrap(),
            )
            .unwrap();
            view.update(input).unwrap().output.paint.clone()
        };
        let category = FieldRef::new("d", "category");
        let series = FieldRef::new("d", "series");
        let value = FieldRef::new("d", "value");
        let grouped = render(crate::Recipes::grouped_bar(
            "Grouped",
            category.clone(),
            series.clone(),
            value.clone(),
        ));
        let grouped_rects: Vec<Rect> = grouped
            .iter()
            .filter_map(|command| match command {
                PaintCommand::Fill { rect, .. } if rect.size.width < 300.0 => Some(*rect),
                _ => None,
            })
            .collect();
        assert_eq!(grouped_rects.len(), 3);
        assert_ne!(grouped_rects[0].origin.x, grouped_rects[1].origin.x);
        let stacked = render(crate::Recipes::stacked_bar(
            "Stacked", category, series, value, true,
        ));
        let stacked_rects: Vec<Rect> = stacked
            .iter()
            .filter_map(|command| match command {
                PaintCommand::Fill { rect, .. } if rect.size.width < 300.0 => Some(*rect),
                _ => None,
            })
            .collect();
        assert_eq!(stacked_rects.len(), 3);
        assert!(
            (stacked_rects[0].origin.y
                - (stacked_rects[1].origin.y + stacked_rects[1].size.height))
                .abs()
                < 0.01
                || (stacked_rects[1].origin.y
                    - (stacked_rects[0].origin.y + stacked_rects[0].size.height))
                    .abs()
                    < 0.01
        );
    }
}

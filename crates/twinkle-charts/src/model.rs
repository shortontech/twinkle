use std::{collections::BTreeMap, sync::Arc, time::Instant};

use twinkle_ui::{
    ActionKind, DragGesture, Point, Rect, SemanticRole, Size, UiId, WheelGesture,
    backend::{PaintCommand, PathGeometry, PathGeometryKind, PathLineCap, PathLineJoin},
    retained_surface::{
        RetainedSurfaceChanges, RetainedSurfaceHost, RetainedSurfaceInput,
        RetainedSurfaceInteraction, RetainedSurfaceLifecycle, RetainedSurfaceLimits,
        RetainedSurfaceModel, RetainedSurfaceOutput, RetainedSurfaceSnapshot,
    },
};

use crate::{
    Channel, ChartData, ChartDiagnostics, ChartError, ChartEvent, ChartLimits, ChartSpec,
    ChartState, ChartTheme, ContinuousScale, DataValue, Encoding, LinearScale, LogScale, Mark,
    Scale, ScaleKind,
};

#[derive(Clone, Copy, Debug)]
enum AxisMapper {
    Linear(LinearScale),
    Log(LogScale),
}

impl AxisMapper {
    fn new(
        domain: [f64; 2],
        mut range: [f32; 2],
        scale: Option<&Scale>,
    ) -> Result<Self, ChartError> {
        if scale.is_some_and(|scale| scale.reversed) {
            range.swap(0, 1);
        }
        let range = [f64::from(range[0]), f64::from(range[1])];
        match scale.map(|scale| &scale.kind) {
            Some(ScaleKind::Log { base }) => Ok(Self::Log(LogScale::new(
                domain,
                range,
                *base,
                scale.is_some_and(|scale| scale.clamp),
            )?)),
            _ => Ok(Self::Linear(LinearScale::new(
                domain,
                range,
                scale.is_some_and(|scale| scale.clamp),
            )?)),
        }
    }

    fn validate(&self, value: f64) -> Result<(), ChartError> {
        if !value.is_finite() || matches!(self, Self::Log(_)) && value <= 0.0 {
            Err(ChartError::invalid(
                crate::ChartStage::Domain,
                "scale input must be finite and logarithmic inputs must be positive",
            ))
        } else {
            Ok(())
        }
    }

    fn map(&self, value: f64) -> f32 {
        let (mapped, range) = match self {
            Self::Linear(scale) => (scale.map(value), scale.range()),
            Self::Log(scale) => (scale.map(value), scale.range()),
        };
        mapped.clamp(range[0].min(range[1]), range[0].max(range[1])) as f32
    }

    fn map_unclamped(&self, value: f64) -> f32 {
        let (domain, range, transformed) = match self {
            Self::Linear(scale) => (scale.domain(), scale.range(), value),
            Self::Log(scale) => {
                let domain = scale.domain();
                ([domain[0].ln(), domain[1].ln()], scale.range(), value.ln())
            }
        };
        let t = (transformed - domain[0]) / (domain[1] - domain[0]);
        (range[0] + t * (range[1] - range[0])) as f32
    }

    fn invert(&self, position: f32) -> f64 {
        match self {
            Self::Linear(scale) => scale.invert(f64::from(position)),
            Self::Log(scale) => scale.invert(f64::from(position)),
        }
    }
}

/// Accepted spatial-index entry retaining stable source identity.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartHit {
    pub dataset: Arc<str>,
    pub row: crate::RowKey,
    pub series: Option<Arc<str>>,
    pub bounds: Rect,
    /// Two adjacent half-segments of an area band, in perimeter order.
    /// `bounds` additionally clips this geometry to the visible plot.
    pub area_band: Option<Arc<[Point; 6]>>,
    pub revision: u64,
    /// Index into the accepted view's shared inspection-source pool.
    pub source_index: usize,
    /// Exact row position inside that inspection source.
    pub row_index: usize,
}

// Allocation payload plus Arc's strong/weak counters. Kept separate so line,
// point, and bar hits do not pay the area-band payload cost.
const AREA_BAND_BYTES: usize = std::mem::size_of::<[Point; 6]>() + 2 * std::mem::size_of::<usize>();

fn area_band_bytes(hits: &[ChartHit]) -> usize {
    hits.iter()
        .filter(|hit| hit.area_band.is_some())
        .count()
        .saturating_mul(AREA_BAND_BYTES)
}

/// Exact accepted datum exposed by pointer or keyboard inspection.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartInspection {
    pub dataset: Arc<str>,
    pub row: crate::RowKey,
    pub series: Option<Arc<str>>,
    pub revision: u64,
    pub cells: Vec<crate::ChartTableCell>,
    /// Ordered, presentation-safe subset selected by the chart tooltip contract.
    pub display: Vec<ChartTooltipValue>,
    pub include_data_table_hint: bool,
    /// Accepted plot allocation for this datum (including its facet, if any).
    pub plot_bounds: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChartTooltipValue {
    pub label: Arc<str>,
    pub formatted: String,
    pub unit: Option<Arc<str>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChartTooltipFormat {
    Automatic,
    Number { decimal_places: u8 },
    Percent { decimal_places: u8 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChartTooltipField {
    pub column: Arc<str>,
    pub label: Arc<str>,
    pub unit: Option<Arc<str>>,
    pub format: ChartTooltipFormat,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChartTooltipSpec {
    pub fields: Vec<ChartTooltipField>,
    pub include_data_table_hint: bool,
}

/// Immutable, bounded inspection handle for an accepted chart generation.
///
/// Hosts may retain this alongside a projected snapshot to resolve pointer
/// coordinates without retaining mutable chart state or scanning source rows.
#[derive(Clone, Debug)]
pub struct ChartInspector {
    model: Arc<ChartModel>,
}

impl PartialEq for ChartInspector {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.model, &other.model)
            || (self.model.data.revision() == other.model.data.revision()
                && self.model.hits == other.model.hits)
    }
}

impl ChartInspector {
    pub fn inspect_nearest(&self, point: Point) -> Option<ChartInspection> {
        let hit = self.model.nearest(point)?;
        self.model.inspection(hit)
    }

    /// Resolve a caller-controlled exact datum for passive inspection UI.
    ///
    /// This lookup occurs while admitting a new controlled chart declaration;
    /// pointer motion on an accepted chart continues to use the ordered index.
    pub fn inspect_identity(
        &self,
        dataset: &str,
        row: &crate::RowKey,
    ) -> Option<(ChartHit, ChartInspection)> {
        let hit = self
            .model
            .hits
            .iter()
            .find(|hit| hit.dataset.as_ref() == dataset && &hit.row == row)?;
        Some((hit.clone(), self.model.inspection(hit)?))
    }

    pub fn is_empty(&self) -> bool {
        self.model.hits.is_empty()
    }

    pub fn retained_bytes(&self) -> usize {
        self.model.retained_bytes()
    }
}

#[derive(Clone, Debug)]
struct InspectionSource {
    dataset: Arc<crate::Dataset>,
    owned_bytes: usize,
    layer_index: usize,
    plot_bounds: Rect,
}

/// Domain/data stage shared by guide measurement and geometry preparation.
struct ResolvedLayer {
    dataset: crate::Dataset,
    x_coordinates: Vec<Option<f64>>,
    y_coordinates: Vec<Option<f64>>,
    all_values: Vec<(usize, f64, f64)>,
    values: Vec<(usize, f64, f64)>,
    x_domain: [f64; 2],
    y_domain: [f64; 2],
}

#[derive(Default)]
struct GuideSpace {
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct PreparationScratch {
    coordinate: usize,
    geometry: usize,
    transform: usize,
}

impl PreparationScratch {
    fn total(self) -> usize {
        self.coordinate
            .saturating_add(self.geometry)
            .saturating_add(self.transform)
    }
}

/// Persistent chart preparation state. Clone it to stage a transactional host candidate.
#[derive(Clone, Debug)]
pub struct ChartModel {
    spec: Arc<ChartSpec>,
    data: Arc<ChartData>,
    limits: ChartLimits,
    theme: ChartTheme,
    state: ChartState,
    diagnostics: ChartDiagnostics,
    plot_bounds: Option<Rect>,
    hits: Vec<ChartHit>,
    area_band_bytes: usize,
    hit_x_order: Vec<usize>,
    inspection_sources: Vec<InspectionSource>,
    tooltip: Option<ChartTooltipSpec>,
    data_owned: bool,
    renderer_generation: u64,
}

impl ChartModel {
    pub fn new(
        spec: Arc<ChartSpec>,
        data: Arc<ChartData>,
        limits: ChartLimits,
    ) -> Result<Self, ChartError> {
        data.validate(&limits)?;
        spec.validate(&data, &limits)?;
        let theme = ChartTheme::default();
        theme.validate()?;
        Ok(Self {
            spec,
            data,
            limits,
            theme,
            state: ChartState::default(),
            diagnostics: ChartDiagnostics::default(),
            plot_bounds: None,
            hits: Vec::new(),
            area_band_bytes: 0,
            hit_x_order: Vec::new(),
            inspection_sources: Vec::new(),
            tooltip: None,
            data_owned: false,
            renderer_generation: 0,
        })
    }
    pub fn with_theme(mut self, theme: ChartTheme) -> Result<Self, ChartError> {
        theme.validate()?;
        self.theme = theme;
        Ok(self)
    }
    pub fn with_state(mut self, state: ChartState) -> Self {
        self.state = state;
        self
    }
    pub fn with_tooltip(mut self, tooltip: ChartTooltipSpec) -> Result<Self, ChartError> {
        if tooltip.fields.len() > self.limits.max_columns {
            return Err(ChartError::Limit {
                resource: "tooltip fields",
                actual: tooltip.fields.len(),
                limit: self.limits.max_columns,
            });
        }
        let mut total = 0usize;
        for field in &tooltip.fields {
            for text in [
                field.column.as_ref(),
                field.label.as_ref(),
                field.unit.as_deref().unwrap_or(""),
            ] {
                if text.len() > self.limits.max_label_bytes {
                    return Err(ChartError::Limit {
                        resource: "tooltip label bytes",
                        actual: text.len(),
                        limit: self.limits.max_label_bytes,
                    });
                }
                total = total.saturating_add(text.len());
            }
            if matches!(field.format, ChartTooltipFormat::Number { decimal_places }
                | ChartTooltipFormat::Percent { decimal_places } if decimal_places > 17)
            {
                return Err(ChartError::invalid(
                    crate::ChartStage::Validate,
                    "tooltip decimal places must be at most 17",
                ));
            }
        }
        if total > self.limits.max_total_label_bytes {
            return Err(ChartError::Limit {
                resource: "tooltip total label bytes",
                actual: total,
                limit: self.limits.max_total_label_bytes,
            });
        }
        self.tooltip = Some(tooltip);
        Ok(self)
    }
    pub fn replace(
        &mut self,
        spec: Arc<ChartSpec>,
        data: Arc<ChartData>,
    ) -> Result<(), ChartError> {
        data.validate(&self.limits)?;
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
        self.nearest_index(point).map(|index| &self.hits[index])
    }
    /// Resolve a hit against the exact accepted source used to prepare it.
    ///
    /// The lookup deliberately rejects stale or mismatched hits. In
    /// particular, transformed layers return their derived values while an
    /// unsuccessfully prepared candidate cannot leak into the accepted view.
    pub fn inspection(&self, hit: &ChartHit) -> Option<ChartInspection> {
        if hit.revision != self.data.revision() {
            return None;
        }
        let source = self.inspection_sources.get(hit.source_index)?;
        let dataset = &source.dataset;
        let row = dataset.row(hit.row_index)?;
        if row.key() != &hit.row {
            return None;
        }
        let cells = dataset
            .columns()
            .iter()
            .map(|column| {
                let value = column.values[hit.row_index].clone();
                crate::ChartTableCell {
                    column: column.name.clone(),
                    unit: column.unit.clone(),
                    formatted: crate::table::format_value(&value),
                    value,
                }
            })
            .collect::<Vec<_>>();
        let default_tooltip;
        let tooltip = if let Some(tooltip) = &self.tooltip {
            tooltip
        } else {
            default_tooltip = default_tooltip_spec(
                self.spec
                    .layers
                    .iter()
                    .filter(|layer| layer.visible)
                    .nth(source.layer_index),
            );
            &default_tooltip
        };
        let display = tooltip
            .fields
            .iter()
            .filter_map(|field| {
                let cell = cells.iter().find(|cell| cell.column == field.column)?;
                Some(ChartTooltipValue {
                    label: field.label.clone(),
                    formatted: format_tooltip_value(&cell.value, field.format),
                    unit: field.unit.clone().or_else(|| cell.unit.clone()),
                })
            })
            .collect();
        Some(ChartInspection {
            dataset: dataset.id.0.clone(),
            row: row.key().clone(),
            series: hit.series.clone(),
            revision: hit.revision,
            cells,
            display,
            include_data_table_hint: tooltip.include_data_table_hint,
            plot_bounds: source.plot_bounds,
        })
    }
    fn nearest_index(&self, point: Point) -> Option<usize> {
        let insertion = self
            .hit_x_order
            .partition_point(|index| hit_center_x(&self.hits[*index]) < point.x);
        let start = insertion.saturating_sub(512);
        let end = insertion.saturating_add(512).min(self.hit_x_order.len());
        self.hit_x_order[start..end].iter().copied().min_by(|a, b| {
            hit_distance(&self.hits[*a], point)
                .total_cmp(&hit_distance(&self.hits[*b], point))
                .then_with(|| self.hits[*a].dataset.cmp(&self.hits[*b].dataset))
                .then_with(|| self.hits[*a].row.cmp(&self.hits[*b].row))
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
            Vec<InspectionSource>,
            ChartDiagnostics,
            Rect,
        ),
        ChartError,
    > {
        self.spec.validate(&self.data, &self.limits)?;
        let scratch = self.preparation_scratch_ceiling();
        let transient_bytes = scratch.total();
        let visible_layers = self
            .spec
            .layers
            .iter()
            .filter(|layer| layer.visible)
            .count();
        let indexed_candidate_bytes = self
            .data
            .row_count()
            .saturating_mul(visible_layers.max(1))
            .saturating_mul(
                std::mem::size_of::<ChartHit>().saturating_add(std::mem::size_of::<usize>()),
            );
        let area_candidate_bytes = self
            .spec
            .layers
            .iter()
            .filter(|layer| {
                layer.visible
                    && matches!(layer.mark, Mark::Area { .. })
                    && channel(&layer.encodings, Channel::Y2).is_some()
            })
            .count()
            .saturating_mul(self.data.row_count())
            .saturating_mul(AREA_BAND_BYTES);
        let preflight_peak = transient_bytes
            .saturating_add(indexed_candidate_bytes)
            .saturating_add(area_candidate_bytes);
        if preflight_peak > self.limits.max_peak_bytes {
            return Err(ChartError::Limit {
                resource: "chart preflight peak bytes",
                actual: preflight_peak,
                limit: self.limits.max_peak_bytes,
            });
        }
        if let Some(viewport) = self.state.viewport {
            for domain in [viewport.x, viewport.y].into_iter().flatten() {
                if domain.into_iter().any(|value| !value.is_finite()) || domain[0] >= domain[1] {
                    return Err(ChartError::invalid(
                        crate::ChartStage::Domain,
                        "viewport domains must be finite and strictly increasing",
                    ));
                }
            }
        }
        let started = Instant::now();
        let model = self;
        let mut resolved_layers = if self.spec.facet.is_none() {
            model
                .spec
                .layers
                .iter()
                .filter(|layer| layer.visible)
                .map(|layer| {
                    if matches!(layer.mark, Mark::Arc { .. }) {
                        Ok(None)
                    } else {
                        model.resolve_layer(layer).map(Some)
                    }
                })
                .collect::<Result<Vec<_>, ChartError>>()?
        } else {
            Vec::new()
        };
        unify_resolved_domains(&self.spec, &mut resolved_layers);
        let guides = model.measure_guides(&resolved_layers)?;
        let mut output = RetainedSurfaceOutput {
            measured: input.bounds.size,
            damage: vec![input.bounds],
            ..RetainedSurfaceOutput::default()
        };
        let fonts = self.theme.typography;
        let title_size = measured_text(self.spec.title.as_ref(), -fonts.title, true, None);
        let table_size = measured_text("Data table", -fonts.control, false, None);
        let chrome_em = measured_text("M", -fonts.body, false, None).height.max(1.0);
        let chrome_padding = chrome_em * 0.4;
        let control_padding = measured_text("M", -fonts.control, false, None).height * 0.55;
        let controls_width = ["+", "−", "Reset"]
            .iter()
            .map(|label| {
                measured_text(label, -fonts.control, false, None).width + control_padding * 2.0
            })
            .sum::<f32>();
        let table_width = table_size.width + chrome_padding * 2.0;
        let row_height = title_size.height.max(table_size.height) + chrome_padding * 2.0;
        let controls_wrap = title_size.width + controls_width + table_width + chrome_padding * 3.0
            > input.bounds.size.width;
        let header_height = row_height * if controls_wrap { 2.0 } else { 1.0 };
        let header_height = header_height.min(input.bounds.size.height);
        let left_guide = guides.left.max(chrome_em * 2.5);
        let right_guide = guides.right.max(chrome_em * 1.25);
        let bottom_guide = guides.bottom.max(chrome_em * 2.0);
        let legend_width = (input.bounds.size.width - chrome_padding * 2.0).max(0.0);
        let legend_height = if self.spec.legends.is_empty() {
            0.0
        } else {
            legend_item_bounds(
                &self.legend_labels(&resolved_layers)?,
                -fonts.label,
                legend_width,
            )
            .last()
            .map_or(0.0, |rect| rect.origin.y + rect.size.height)
        }
        .min((input.bounds.size.height - header_height).max(0.0));
        let legend_bounds = Rect::new(
            input.bounds.origin.x + chrome_padding.min(input.bounds.size.width),
            input.bounds.origin.y + header_height,
            legend_width,
            legend_height,
        );
        let plot = inset(
            input.bounds,
            left_guide,
            header_height + legend_height + guides.top,
            right_guide,
            bottom_guide,
        );
        output.paint.push(PaintCommand::Fill {
            rect: input.bounds,
            color: self.theme.background,
        });
        output.paint.push(PaintCommand::Stroke {
            rect: plot,
            color: self.theme.grid,
            width: 1.0,
        });
        let table_width = input.bounds.size.width.min(table_width).max(0.0);
        let controls_y = input.bounds.origin.y
            + if controls_wrap {
                row_height.min(input.bounds.size.height)
            } else {
                0.0
            };
        let table_bounds = Rect::new(
            input.bounds.origin.x + input.bounds.size.width - table_width,
            controls_y,
            table_width,
            row_height
                .min((input.bounds.size.height - (controls_y - input.bounds.origin.y)).max(0.0)),
        );
        output.paint.push(PaintCommand::Text {
            bounds: Rect::new(
                input.bounds.origin.x + chrome_padding.min(input.bounds.size.width),
                input.bounds.origin.y + chrome_padding.min(input.bounds.size.height),
                (input.bounds.size.width
                    - if controls_wrap {
                        chrome_padding * 2.0
                    } else {
                        table_width + controls_width + chrome_padding * 3.0
                    })
                .max(0.0),
                title_size
                    .height
                    .min((header_height - chrome_padding).max(0.0)),
            ),
            text: self.spec.title.to_string(),
            scale: -fonts.title,
            color: self.theme.foreground,
            align: twinkle_ui::TextAlign::Start,
            bold: true,
            wrap: false,
        });
        output.paint.push(PaintCommand::RoundedFill {
            rect: table_bounds,
            color: if self.theme.high_contrast {
                self.theme.focus
            } else {
                self.theme.grid
            },
            radius: 4.0,
        });
        output.paint.push(PaintCommand::Text {
            bounds: table_bounds,
            text: "Data table".into(),
            scale: -fonts.control,
            color: self.theme.foreground,
            align: twinkle_ui::TextAlign::Center,
            bold: false,
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
        output.interactions.push(RetainedSurfaceInteraction {
            id: UiId::from("chart/table"),
            bounds: table_bounds,
            action: ActionKind::Activate,
            focusable: true,
        });
        output
            .semantics
            .push(twinkle_ui::retained_surface::RetainedSurfaceSemantic {
                id: UiId::from("chart/table"),
                bounds: table_bounds,
                role: SemanticRole::Button,
                label: Some("View chart data table".into()),
                state: Some("Exact values, paged".into()),
                protected: false,
            });
        let mut hits = Vec::new();
        let mut inspection_sources = Vec::new();
        if plot.size.width > 1.0 && plot.size.height > 1.0 && self.spec.facet.is_some() {
            self.prepare_facets(plot, input, &mut output, &mut hits, &mut inspection_sources)?;
        } else if plot.size.width > 1.0 && plot.size.height > 1.0 {
            for ((layer_index, layer), resolved) in model
                .spec
                .layers
                .iter()
                .filter(|layer| layer.visible)
                .enumerate()
                .zip(resolved_layers)
            {
                model.prepare_layer(
                    layer_index,
                    layer,
                    resolved,
                    plot,
                    legend_bounds,
                    &mut output,
                    &mut hits,
                    &mut inspection_sources,
                )?;
            }
        }
        if self.data.row_count() == 0 {
            output.paint.push(PaintCommand::Text {
                bounds: plot,
                text: "No data".into(),
                scale: -fonts.body,
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
            + area_band_bytes(&hits)
            + index_bytes
            + inspection_sources
                .iter()
                .map(|source| source.owned_bytes)
                .sum::<usize>()
            + geometry_vertices * std::mem::size_of::<Point>();
        if steady_bytes > self.limits.max_steady_bytes {
            return Err(ChartError::Limit {
                resource: "chart steady bytes",
                actual: steady_bytes,
                limit: self.limits.max_steady_bytes,
            });
        }
        // Candidate steady ownership and the admitted scratch ceiling overlap
        // during synchronous publication. The preflight above rejects the
        // row/index portion before large preparation vectors are allocated;
        // this final exact accepted-owner addition catches richer derived data.
        let peak_bytes = steady_bytes.saturating_add(transient_bytes);
        if peak_bytes > self.limits.max_peak_bytes {
            return Err(ChartError::Limit {
                resource: "chart peak bytes",
                actual: peak_bytes,
                limit: self.limits.max_peak_bytes,
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
        let missing_values = self
            .data
            .datasets()
            .iter()
            .flat_map(|dataset| dataset.columns())
            .flat_map(|column| column.values.iter())
            .filter(|value| {
                value.is_null() || matches!(value, DataValue::Number(number) if !number.is_finite())
            })
            .count();
        let displayed_series = hits
            .iter()
            .filter_map(|hit| hit.series.as_deref())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let selected = hits
            .iter()
            .filter(|hit| self.state.selected_rows.contains(&hit.row))
            .count();
        let reduction = if displayed_rows < hits.len() {
            format!(
                " Reduced geometry displays {displayed_rows} marks while {} exact values remain inspectable.",
                hits.len()
            )
        } else {
            String::new()
        };
        let series_summary = if displayed_series == 0 {
            String::new()
        } else {
            format!(" {displayed_series} displayed series.")
        };
        let state_summary = format!(
            "{}. {} source rows; {} inspectable plotted values; {missing_values} missing or invalid values.{series_summary} {selected} selected.{reduction} Activate for the exact paged data table.",
            self.spec.summary,
            self.data.row_count(),
            hits.len(),
        );
        if let Some(root) = output
            .semantics
            .iter_mut()
            .find(|semantic| semantic.id.as_str() == "chart")
        {
            root.state = Some(state_summary);
        }
        let diagnostics = ChartDiagnostics {
            preparation_time: started.elapsed(),
            input_rows: self.data.row_count(),
            transformed_rows: inspection_sources
                .iter()
                .map(|source| source.dataset.len())
                .sum(),
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
            coordinate_scratch_bytes: scratch.coordinate,
            geometry_scratch_bytes: scratch.geometry,
            transform_scratch_bytes: scratch.transform,
            transient_bytes,
            peak_bytes,
            cache_misses: 1,
            notices,
            ..self.diagnostics.clone()
        };
        Ok((
            output,
            hits,
            hit_x_order,
            inspection_sources,
            diagnostics,
            plot,
        ))
    }

    fn preparation_scratch_ceiling(&self) -> PreparationScratch {
        let rows = self.data.row_count();
        let coordinate_work = rows.saturating_mul(
            std::mem::size_of::<Option<f64>>()
                .saturating_mul(2)
                .saturating_add(std::mem::size_of::<(usize, f64, f64)>().saturating_mul(2))
                .saturating_add(std::mem::size_of::<usize>().saturating_mul(5)),
        );
        let geometry_work = self
            .limits
            .max_geometry_vertices
            .saturating_mul(std::mem::size_of::<Point>());
        let has_transforms = self
            .spec
            .layers
            .iter()
            .any(|layer| !layer.transforms.is_empty());
        let transform_work = if has_transforms {
            let cells = self
                .data
                .datasets()
                .iter()
                .map(|dataset| dataset.len().saturating_mul(dataset.columns().len()))
                .sum::<usize>();
            cells
                .saturating_mul(
                    std::mem::size_of::<DataValue>()
                        .saturating_add(std::mem::size_of::<String>())
                        .saturating_add(32),
                )
                .saturating_add(
                    rows.saturating_mul(
                        std::mem::size_of::<crate::RowKey>()
                            .saturating_add(std::mem::size_of::<BTreeMap<String, DataValue>>()),
                    ),
                )
        } else {
            0
        };
        PreparationScratch {
            coordinate: coordinate_work.saturating_mul(
                self.spec
                    .layers
                    .iter()
                    .filter(|layer| layer.visible)
                    .count()
                    .max(1),
            ),
            geometry: geometry_work,
            transform: transform_work,
        }
    }

    fn prepare_facets(
        &self,
        plot: Rect,
        input: RetainedSurfaceInput,
        output: &mut RetainedSurfaceOutput,
        hits: &mut Vec<ChartHit>,
        inspection_sources: &mut Vec<InspectionSource>,
    ) -> Result<(), ChartError> {
        let facet = self.spec.facet.as_ref().expect("facet branch checked");
        let field = facet.row.as_ref().or(facet.column.as_ref()).unwrap();
        let source = self
            .data
            .dataset(&field.dataset)
            .ok_or_else(|| ChartError::MissingDataset(field.dataset.to_string()))?;
        let row_column = facet
            .row
            .as_ref()
            .map(|field| {
                source
                    .column(&field.column)
                    .ok_or_else(|| ChartError::MissingColumn {
                        dataset: field.dataset.to_string(),
                        column: field.column.to_string(),
                    })
            })
            .transpose()?;
        let column_column = facet
            .column
            .as_ref()
            .map(|field| {
                source
                    .column(&field.column)
                    .ok_or_else(|| ChartError::MissingColumn {
                        dataset: field.dataset.to_string(),
                        column: field.column.to_string(),
                    })
            })
            .transpose()?;
        let row_values = facet_values(row_column);
        let column_values = facet_values(column_column);
        let facet_count = row_values
            .len()
            .checked_mul(column_values.len())
            .ok_or_else(|| {
                ChartError::invalid(crate::ChartStage::Layout, "facet count overflow")
            })?;
        if facet_count > self.limits.max_facets {
            return Err(ChartError::Limit {
                resource: "facets",
                actual: facet_count,
                limit: self.limits.max_facets,
            });
        }
        let cell_width = plot.size.width / column_values.len().max(1) as f32;
        let cell_height = plot.size.height / row_values.len().max(1) as f32;
        let shared_spec = if facet.scales == crate::FacetScalePolicy::Shared {
            let resolved = self
                .spec
                .layers
                .iter()
                .filter(|layer| layer.visible)
                .map(|layer| {
                    if matches!(layer.mark, Mark::Arc { .. }) {
                        Ok(None)
                    } else {
                        self.resolve_layer(layer).map(Some)
                    }
                })
                .collect::<Result<Vec<_>, ChartError>>()?;
            let mut spec = (*self.spec).clone();
            for (scale, domain) in resolved_shared_domains(&self.spec, &resolved) {
                spec.scales[scale].domain = Some(domain);
            }
            spec
        } else {
            (*self.spec).clone()
        };

        for (row_index, row_value) in row_values.iter().enumerate() {
            for (column_index, column_value) in column_values.iter().enumerate() {
                let facet_label = match (facet.row.is_some(), facet.column.is_some()) {
                    (true, true) => format!("{} / {}", row_value.label, column_value.label),
                    (true, false) => row_value.label.clone(),
                    (false, true) => column_value.label.clone(),
                    (false, false) => unreachable!(),
                };
                let indices = (0..source.len())
                    .filter(|index| {
                        row_column.is_none_or(|column| {
                            facet_value_key(&column.values[*index]) == row_value.key
                        }) && column_column.is_none_or(|column| {
                            facet_value_key(&column.values[*index]) == column_value.key
                        })
                    })
                    .collect::<Vec<_>>();
                let mut datasets = Vec::with_capacity(self.data.datasets().len());
                for dataset in self.data.datasets() {
                    if dataset.id == source.id {
                        let keys = indices
                            .iter()
                            .map(|index| dataset.keys()[*index].clone())
                            .collect::<Vec<_>>();
                        let columns = dataset
                            .columns()
                            .iter()
                            .map(|column| crate::Column {
                                name: column.name.clone(),
                                values: indices
                                    .iter()
                                    .map(|index| column.values[*index].clone())
                                    .collect::<Vec<_>>()
                                    .into(),
                                unit: column.unit.clone(),
                            })
                            .collect::<Vec<_>>();
                        datasets.push(crate::Dataset::new(dataset.id.clone(), keys, columns)?);
                    } else {
                        datasets.push(dataset.clone());
                    }
                }
                let data = Arc::new(ChartData::new(
                    self.data.revision(),
                    datasets,
                    &self.limits,
                )?);
                let mut spec = shared_spec.clone();
                spec.facet = None;
                spec.title = Arc::from(facet_label.clone());
                spec.summary = Arc::from(format!("Facet {facet_label}. {}", self.spec.summary));
                let mut submodel = ChartModel::new(Arc::new(spec), data, self.limits.clone())?
                    .with_theme(self.theme.clone())?
                    .with_state(self.state.clone());
                submodel.data_owned = true;
                let bounds = Rect::new(
                    plot.origin.x + column_index as f32 * cell_width,
                    plot.origin.y + row_index as f32 * cell_height,
                    cell_width,
                    cell_height,
                );
                let (mut prepared, mut facet_hits, _, mut facet_sources, _, _) = submodel.prepare(
                    RetainedSurfaceInput::new(bounds, input.scale_factor, input.revisions),
                )?;
                let source_offset = inspection_sources.len();
                for hit in &mut facet_hits {
                    hit.source_index = hit.source_index.saturating_add(source_offset);
                }
                inspection_sources.append(&mut facet_sources);
                if matches!(prepared.paint.first(), Some(PaintCommand::Fill { .. })) {
                    prepared.paint.remove(0);
                }
                output.paint.extend(prepared.paint);
                output.damage.extend(prepared.damage);
                let prefix = format!(
                    "facet/{}/{}",
                    stable_id_fragment(&row_value.key),
                    stable_id_fragment(&column_value.key)
                );
                for interaction in prepared.interactions {
                    if interaction.id.as_str() == "chart" {
                        continue;
                    }
                    output.interactions.push(RetainedSurfaceInteraction {
                        id: UiId::from(format!("{prefix}/{}", interaction.id.as_str())),
                        ..interaction
                    });
                }
                for semantic in prepared.semantics {
                    let is_root = semantic.id.as_str() == "chart";
                    output
                        .semantics
                        .push(twinkle_ui::retained_surface::RetainedSurfaceSemantic {
                            id: UiId::from(format!("{prefix}/{}", semantic.id.as_str())),
                            role: if is_root {
                                SemanticRole::Group
                            } else {
                                semantic.role
                            },
                            label: if is_root {
                                Some(format!("{facet_label} facet"))
                            } else {
                                semantic.label
                            },
                            ..semantic
                        });
                }
                hits.append(&mut facet_hits);
            }
        }
        Ok(())
    }

    fn measure_guides(&self, layers: &[Option<ResolvedLayer>]) -> Result<GuideSpace, ChartError> {
        let scale = -self.theme.typography.label;
        let gap = measured_text("M", scale, false, None).height * 0.25;
        let mut space = GuideSpace::default();
        for (layer, resolved) in self
            .spec
            .layers
            .iter()
            .filter(|layer| layer.visible)
            .zip(layers)
        {
            let Some(resolved) = resolved else { continue };
            if resolved.all_values.is_empty() {
                continue;
            }
            for axis in &self.spec.axes {
                let axis_channel = layer
                    .encodings
                    .iter()
                    .find(|encoding| {
                        matches!(encoding.channel, Channel::X | Channel::Y)
                            && encoding.scale.as_deref() == Some(axis.scale.as_ref())
                    })
                    .map(|encoding| encoding.channel);
                let Some(axis_channel) = axis_channel else {
                    continue;
                };
                let domain = if axis_channel == Channel::X {
                    resolved.x_domain
                } else {
                    resolved.y_domain
                };
                let scale_spec = self.spec.scales.iter().find(|s| s.name == axis.scale);
                let mapper = AxisMapper::new(domain, [0.0, 1.0], scale_spec)?;
                let count = axis
                    .tick_count
                    .unwrap_or(5)
                    .clamp(2, self.limits.max_ticks_per_axis);
                let mut ticks = Size::default();
                for i in 0..count {
                    let start = mapper.map(domain[0]);
                    let end = mapper.map(domain[1]);
                    let value =
                        mapper.invert(start + (end - start) * i as f32 / (count - 1) as f32);
                    let size = measured_text(
                        &format_tick(value, axis.unit.as_deref()),
                        scale,
                        false,
                        None,
                    );
                    ticks.width = ticks.width.max(size.width);
                    ticks.height = ticks.height.max(size.height);
                }
                let title = axis
                    .title
                    .as_ref()
                    .map(|title| measured_text(title, scale, true, None));
                if axis_channel == Channel::X {
                    space.bottom = space
                        .bottom
                        .max(gap * 2.0 + ticks.height + title.map_or(0.0, |s| gap + s.height));
                } else {
                    let width = ticks.width.max(title.map_or(0.0, |s| s.width)) + gap * 2.0;
                    if self.y_axis_ordinal(&axis.scale) == 0 {
                        space.left = space.left.max(width);
                    } else {
                        space.right = space.right.max(width);
                    }
                    space.top = space
                        .top
                        .max(ticks.height * 0.5 + title.map_or(0.0, |s| s.height + gap));
                }
            }
        }
        Ok(space)
    }

    fn resolve_layer(&self, layer: &crate::Layer) -> Result<ResolvedLayer, ChartError> {
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
        let x2_column = channel(&layer.encodings, Channel::X2)
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
        let y_coordinates = column_coordinates(y_column);
        let series_column = channel(&layer.encodings, Channel::Series)
            .and_then(|encoding| dataset.column(&encoding.field.column));
        let row_visible = |index: usize| {
            let Some(visible) = &self.state.visible_series else {
                return true;
            };
            series_column
                .map(|column| {
                    visible.contains(&Arc::<str>::from(format_data_value(&column.values[index])))
                })
                .unwrap_or(true)
        };
        let all_values: Vec<(usize, f64, f64)> = x_column
            .values
            .iter()
            .zip(y_column.values.iter())
            .enumerate()
            .filter_map(|(index, (_x, _y))| {
                x_coordinates[index]
                    .zip(y_coordinates[index])
                    .map(|(x, y)| (index, x, y))
            })
            .collect();
        let values: Vec<(usize, f64, f64)> = all_values
            .iter()
            .copied()
            .filter(|(index, _, _)| row_visible(*index))
            .collect();
        let domain_values = match self.spec.visibility_domain {
            crate::VisibilityDomainPolicy::Original => &all_values,
            crate::VisibilityDomainPolicy::VisibleSeries if values.is_empty() => &all_values,
            crate::VisibilityDomainPolicy::VisibleSeries => &values,
        };
        let mut x_domain = extent(domain_values.iter().map(|(_, x, _)| *x).chain(
            x2_column.into_iter().flat_map(|column| {
                column
                    .values
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| {
                        self.spec.visibility_domain == crate::VisibilityDomainPolicy::Original
                            || row_visible(*index)
                    })
                    .filter_map(|(index, value)| coordinate(value, index))
            }),
        ));
        let mut y_domain = extent(domain_values.iter().map(|(_, _, y)| *y).chain(
            y2_column.into_iter().flat_map(|column| {
                column
                    .values
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| {
                        self.spec.visibility_domain == crate::VisibilityDomainPolicy::Original
                            || row_visible(*index)
                    })
                    .filter_map(|(index, value)| coordinate(value, index))
            }),
        ));
        let y_baseline = match layer.mark {
            Mark::Area { baseline, .. } => Some(baseline as f64),
            Mark::Bar {
                orientation: crate::Orientation::Vertical,
            } => Some(0.0),
            _ => None,
        };
        if let Some(baseline) = y_baseline {
            y_domain[0] = y_domain[0].min(baseline);
            y_domain[1] = y_domain[1].max(baseline);
        }
        if matches!(
            layer.mark,
            Mark::Bar {
                orientation: crate::Orientation::Horizontal
            }
        ) {
            x_domain[0] = x_domain[0].min(0.0);
            x_domain[1] = x_domain[1].max(0.0);
        }
        let x_scale = x.scale.as_deref();
        let y_scale = y.scale.as_deref();
        for annotation in &self.spec.annotations {
            match annotation {
                crate::Annotation::Rule {
                    scale,
                    value,
                    include_in_domain: true,
                    ..
                } => {
                    include_domain_value(&mut x_domain, x_scale, scale, *value);
                    include_domain_value(&mut y_domain, y_scale, scale, *value);
                }
                crate::Annotation::Band {
                    scale,
                    start,
                    end,
                    include_in_domain: true,
                    ..
                } => {
                    include_domain_value(&mut x_domain, x_scale, scale, *start);
                    include_domain_value(&mut x_domain, x_scale, scale, *end);
                    include_domain_value(&mut y_domain, y_scale, scale, *start);
                    include_domain_value(&mut y_domain, y_scale, scale, *end);
                }
                _ => {}
            }
        }
        if let Some(domain) = self
            .spec
            .scales
            .iter()
            .find(|scale| Some(scale.name.as_ref()) == x_scale)
            .and_then(|scale| scale.domain)
        {
            x_domain = normalized_domain(domain);
        }
        if let Some(domain) = self
            .spec
            .scales
            .iter()
            .find(|scale| Some(scale.name.as_ref()) == y_scale)
            .and_then(|scale| scale.domain)
        {
            y_domain = normalized_domain(domain);
        }
        if let Some(viewport) = self.state.viewport {
            if let Some(domain) = viewport.x {
                x_domain = normalized_domain(domain);
            }
            if let Some(domain) = viewport.y {
                y_domain = normalized_domain(domain);
            }
        }
        Ok(ResolvedLayer {
            dataset: dataset.clone(),
            x_coordinates,
            y_coordinates,
            all_values,
            values,
            x_domain,
            y_domain,
        })
    }

    fn prepare_layer(
        &self,
        layer_index: usize,
        layer: &crate::Layer,
        resolved: Option<ResolvedLayer>,
        plot: Rect,
        legend_bounds: Rect,
        output: &mut RetainedSurfaceOutput,
        hits: &mut Vec<ChartHit>,
        inspection_sources: &mut Vec<InspectionSource>,
    ) -> Result<(), ChartError> {
        if let Mark::Arc { inner_radius } = layer.mark {
            return self.prepare_arc(
                layer_index,
                layer,
                plot,
                inner_radius,
                output,
                hits,
                inspection_sources,
            );
        }
        let ResolvedLayer {
            dataset,
            x_coordinates,
            y_coordinates,
            all_values,
            values,
            x_domain,
            y_domain,
        } = resolved.expect("Cartesian layer was resolved before layout");
        let dataset = &dataset;
        let x = channel(&layer.encodings, Channel::X).expect("resolved x encoding");
        let y = channel(&layer.encodings, Channel::Y).expect("resolved y encoding");
        let x_scale = x.scale.as_deref();
        let y_scale = y.scale.as_deref();
        let inspection_source = register_inspection_source(
            inspection_sources,
            dataset,
            self.data_owned || !layer.transforms.is_empty(),
            layer_index,
            plot,
        );
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
        let x2_column = channel(&layer.encodings, Channel::X2)
            .map(|encoding| {
                dataset
                    .column(&encoding.field.column)
                    .ok_or_else(|| ChartError::MissingColumn {
                        dataset: encoding.field.dataset.to_string(),
                        column: encoding.field.column.to_string(),
                    })
            })
            .transpose()?;
        let series_column = channel(&layer.encodings, Channel::Series)
            .and_then(|encoding| dataset.column(&encoding.field.column));
        if layer_index == 0 {
            // Keep controls available when controlled state hides every series.
            self.prepare_legends(legend_bounds, series_column, output)?;
        }
        if all_values.is_empty() {
            return Ok(());
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
        let categorical_y_count = y_column
            .values
            .iter()
            .filter_map(DataValue::category)
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let y_inset = if categorical_y_count == 0 {
            0.0
        } else {
            plot.size.height / categorical_y_count as f32 / 2.0
        };
        let x_scale_spec = self
            .spec
            .scales
            .iter()
            .find(|scale| Some(scale.name.as_ref()) == x_scale);
        let y_scale_spec = self
            .spec
            .scales
            .iter()
            .find(|scale| Some(scale.name.as_ref()) == y_scale);
        let x_mapper = AxisMapper::new(
            x_domain,
            [
                plot.origin.x + x_inset,
                plot.origin.x + plot.size.width - x_inset,
            ],
            x_scale_spec,
        )?;
        let default_y_range = [
            plot.origin.y + plot.size.height - y_inset,
            plot.origin.y + y_inset,
        ];
        let declared_y_range = [
            plot.origin.y + y_inset,
            plot.origin.y + plot.size.height - y_inset,
        ];
        let y_mapper = AxisMapper::new(
            y_domain,
            if y_scale_spec.is_some() {
                declared_y_range
            } else {
                default_y_range
            },
            y_scale_spec,
        )?;
        for (_, x, y) in &all_values {
            x_mapper.validate(*x)?;
            y_mapper.validate(*y)?;
        }
        for value in x2_column
            .into_iter()
            .flat_map(|column| column.values.iter().enumerate())
            .filter_map(|(index, value)| coordinate(value, index))
        {
            x_mapper.validate(value)?;
        }
        for value in y2_column
            .into_iter()
            .flat_map(|column| column.values.iter().enumerate())
            .filter_map(|(index, value)| coordinate(value, index))
        {
            y_mapper.validate(value)?;
        }
        let map = |x: f64, y: f64| Point {
            x: x_mapper.map(x),
            y: y_mapper.map(y),
        };
        let map_unclamped = |x: f64, y: f64| Point {
            x: x_mapper.map_unclamped(x),
            y: y_mapper.map_unclamped(y),
        };
        let values = values
            .into_iter()
            .filter(|(_, x, y)| in_domain(*x, x_domain) && in_domain(*y, y_domain))
            .collect::<Vec<_>>();
        self.prepare_guides(
            layer_index,
            plot,
            x_domain,
            y_domain,
            x_scale,
            y_scale,
            &x_mapper,
            &y_mapper,
            output,
        )?;
        let mut series_groups = series_groups(dataset, &layer.encodings, &self.state);
        let continuous_mark = matches!(layer.mark, Mark::Line { .. } | Mark::Area { .. });
        for rows in &mut series_groups {
            rows.retain(|index| {
                // Keep missing samples in continuous series: their position
                // separates path segments rather than joining across a gap.
                continuous_mark
                    || (x_coordinates[*index].is_some_and(|value| in_domain(value, x_domain))
                        && y_coordinates[*index].is_some_and(|value| in_domain(value, y_domain)))
            });
        }
        let color = self.theme.series_cue(layer_index).color;
        match &layer.mark {
            Mark::Line {
                interpolation,
                points,
            } => {
                let max_points = (plot.size.width.ceil() as usize).saturating_mul(4).max(2);
                for (series_index, rows) in series_groups.iter().enumerate() {
                    let mut triangles = Vec::new();
                    let mut segment = Vec::new();
                    let mut markers = Vec::new();
                    for index in rows.iter().copied() {
                        match (x_coordinates[index], y_coordinates[index]) {
                            (Some(x), Some(y)) => segment.push(map_unclamped(x, y)),
                            _ => {
                                let reduced = reduce_extrema(
                                    &interpolate_points(&segment, *interpolation),
                                    max_points,
                                );
                                triangles.extend(stroke_polyline(&reduced, 2.0));
                                if *points {
                                    markers.extend(reduced);
                                }
                                segment.clear();
                            }
                        }
                    }
                    let reduced =
                        reduce_extrema(&interpolate_points(&segment, *interpolation), max_points);
                    triangles.extend(stroke_polyline(&reduced, 2.0));
                    if *points {
                        markers.extend(reduced);
                    }
                    let series_color = self.theme.series_cue(layer_index + series_index).color;
                    let triangles = clip_triangles_to_rect(&triangles, plot);
                    if !triangles.is_empty() {
                        output.paint.push(path_command(
                            triangles,
                            plot,
                            series_color,
                            rows.len().max(2),
                        )?);
                    }
                    for point in markers.into_iter().take(16_384) {
                        output.paint.push(PaintCommand::RoundedFill {
                            rect: Rect::new(point.x - 2.5, point.y - 2.5, 5.0, 5.0),
                            color: series_color,
                            radius: 2.5,
                        });
                    }
                }
            }
            Mark::Area { baseline, .. } => {
                for (series_index, rows) in series_groups.iter().enumerate() {
                    let mut triangles = Vec::new();
                    if let Some(base_column) = y2_column {
                        for pair in rows.windows(2) {
                            let (left, right) = (pair[0], pair[1]);
                            let (Some(x0), Some(y0), Some(x1), Some(y1), Some(base0), Some(base1)) = (
                                x_coordinates[left],
                                y_coordinates[left],
                                x_coordinates[right],
                                y_coordinates[right],
                                coordinate(&base_column.values[left], left),
                                coordinate(&base_column.values[right], right),
                            ) else {
                                continue;
                            };
                            let top0 = map_unclamped(x0, y0);
                            let top1 = map_unclamped(x1, y1);
                            let bottom0 = map_unclamped(x0, base0);
                            let bottom1 = map_unclamped(x1, base1);
                            triangles.extend(band_triangles(top0, bottom0, top1, bottom1));
                        }
                    } else {
                        y_mapper.validate(*baseline)?;
                        let baseline = y_mapper.map(*baseline);
                        let max_points = (plot.size.width.ceil() as usize).saturating_mul(4).max(2);
                        let mut segment = Vec::new();
                        for index in rows.iter().copied() {
                            match (x_coordinates[index], y_coordinates[index]) {
                                (Some(x), Some(y)) => segment.push(map_unclamped(x, y)),
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
                    let triangles = clip_triangles_to_rect(&triangles, plot);
                    if !triangles.is_empty() {
                        output.paint.push(fill_path_command(
                            triangles,
                            self.theme.series_cue(layer_index + series_index).color,
                            rows.len().max(2),
                        )?);
                    }
                    for (position, index) in rows.iter().copied().enumerate() {
                        if let Some(base_column) = y2_column {
                            let sample = |row: usize| -> Option<(Point, Point)> {
                                let x = x_coordinates[row]?;
                                Some((
                                    map_unclamped(x, y_coordinates[row]?),
                                    map_unclamped(x, coordinate(&base_column.values[row], row)?),
                                ))
                            };
                            let Some((top, bottom)) = sample(index) else {
                                continue;
                            };
                            let midpoint = |a: Point, b: Point| Point {
                                x: a.x + (b.x - a.x) * 0.5,
                                y: a.y + (b.y - a.y) * 0.5,
                            };
                            let previous = position
                                .checked_sub(1)
                                .and_then(|i| sample(rows[i]))
                                .unwrap_or((top, bottom));
                            let next = rows
                                .get(position + 1)
                                .and_then(|i| sample(*i))
                                .unwrap_or((top, bottom));
                            let band = [
                                midpoint(previous.0, top),
                                top,
                                midpoint(top, next.0),
                                midpoint(bottom, next.1),
                                bottom,
                                midpoint(previous.1, bottom),
                            ];
                            let min_x = band
                                .iter()
                                .map(|p| p.x)
                                .fold(f32::INFINITY, f32::min)
                                .max(plot.origin.x);
                            let max_x = band
                                .iter()
                                .map(|p| p.x)
                                .fold(f32::NEG_INFINITY, f32::max)
                                .min(plot.origin.x + plot.size.width);
                            let min_y = band
                                .iter()
                                .map(|p| p.y)
                                .fold(f32::INFINITY, f32::min)
                                .max(plot.origin.y);
                            let max_y = band
                                .iter()
                                .map(|p| p.y)
                                .fold(f32::NEG_INFINITY, f32::max)
                                .min(plot.origin.y + plot.size.height);
                            if max_x > min_x && max_y > min_y {
                                self.push_hit(
                                    dataset,
                                    inspection_source,
                                    index,
                                    series_column,
                                    Rect::new(min_x, min_y, max_x - min_x, max_y - min_y),
                                    hits,
                                );
                                hits.last_mut().unwrap().area_band = Some(Arc::new(band));
                            }
                            continue;
                        }
                        let (Some(x), Some(y)) = (x_coordinates[index], y_coordinates[index])
                        else {
                            continue;
                        };
                        let center_x = x_mapper.map_unclamped(x);
                        let previous_x = position
                            .checked_sub(1)
                            .and_then(|position| x_coordinates[rows[position]])
                            .map(|value| x_mapper.map_unclamped(value));
                        let next_x = rows
                            .get(position + 1)
                            .and_then(|index| x_coordinates[*index])
                            .map(|value| x_mapper.map_unclamped(value));
                        let left = previous_x
                            .map_or(plot.origin.x, |previous| (previous + center_x) * 0.5);
                        let right = next_x.map_or(plot.origin.x + plot.size.width, |next| {
                            (center_x + next) * 0.5
                        });
                        let base = y2_column
                            .and_then(|column| coordinate(&column.values[index], index))
                            .unwrap_or(*baseline);
                        let top_y = y_mapper
                            .map_unclamped(y)
                            .clamp(plot.origin.y, plot.origin.y + plot.size.height);
                        let base_y = y_mapper
                            .map_unclamped(base)
                            .clamp(plot.origin.y, plot.origin.y + plot.size.height);
                        let left = left.clamp(plot.origin.x, plot.origin.x + plot.size.width);
                        let right = right.clamp(plot.origin.x, plot.origin.x + plot.size.width);
                        if right > left && (top_y - base_y).abs() > f32::EPSILON {
                            self.push_hit(
                                dataset,
                                inspection_source,
                                index,
                                series_column,
                                Rect::new(
                                    left,
                                    top_y.min(base_y),
                                    right - left,
                                    (top_y - base_y).abs(),
                                ),
                                hits,
                            );
                        }
                    }
                }
            }
            Mark::Point { symbol } => {
                let max_marks =
                    ((plot.size.width * plot.size.height / 16.0) as usize).clamp(1, 50_000);
                let step = values.len().div_ceil(max_marks).max(1);
                let size_column = channel(&layer.encodings, Channel::Size)
                    .and_then(|encoding| dataset.column(&encoding.field.column));
                let size_max = size_column
                    .into_iter()
                    .flat_map(|column| column.values.iter())
                    .filter_map(DataValue::number)
                    .filter(|value| value.is_finite() && *value >= 0.0)
                    .fold(0.0_f64, f64::max);
                for (index, x, y) in values.iter().step_by(step) {
                    let center = map(*x, *y);
                    let radius = size_column
                        .and_then(|column| column.values[*index].number())
                        .filter(|value| value.is_finite() && *value >= 0.0)
                        .map_or(3.0, |value| {
                            if size_max == 0.0 {
                                0.0
                            } else {
                                (value / size_max).sqrt() as f32 * 10.0
                            }
                        });
                    if radius == 0.0 {
                        continue;
                    }
                    paint_symbol(output, plot, center, radius, *symbol, color)?;
                }
            }
            Mark::Rect => {
                if values.len() > 60_000 {
                    return Err(ChartError::Limit {
                        resource: "heatmap cells",
                        actual: values.len(),
                        limit: 60_000,
                    });
                }
                let x_count = x_coordinates
                    .iter()
                    .flatten()
                    .map(|value| value.to_bits())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    .max(1);
                let y_count = y_coordinates
                    .iter()
                    .flatten()
                    .map(|value| value.to_bits())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    .max(1);
                let cell_width = plot.size.width / x_count as f32;
                let cell_height = plot.size.height / y_count as f32;
                let fill_column = channel(&layer.encodings, Channel::Fill)
                    .and_then(|encoding| dataset.column(&encoding.field.column));
                let numeric_fill: Vec<f64> = fill_column
                    .into_iter()
                    .flat_map(|column| column.values.iter())
                    .filter_map(DataValue::number)
                    .filter(|value| value.is_finite())
                    .collect();
                let fill_domain =
                    (!numeric_fill.is_empty()).then(|| extent(numeric_fill.iter().copied()));
                for (index, x, y) in &values {
                    let center = map(*x, *y);
                    let left = (center.x - cell_width / 2.0).max(plot.origin.x);
                    let right = (center.x + cell_width / 2.0).min(plot.origin.x + plot.size.width);
                    let top = (center.y - cell_height / 2.0).max(plot.origin.y);
                    let bottom =
                        (center.y + cell_height / 2.0).min(plot.origin.y + plot.size.height);
                    let value = fill_column.map(|column| &column.values[*index]);
                    let cell_color = match (value, fill_domain) {
                        (Some(DataValue::Number(value)), Some(domain)) if value.is_finite() => self
                            .theme
                            .sequential((value - domain[0]) / (domain[1] - domain[0])),
                        (Some(DataValue::Category(_)), _) => {
                            self.theme
                                .series_cue(
                                    column_coordinates(fill_column.unwrap())[*index].unwrap_or(0.0)
                                        as usize,
                                )
                                .color
                        }
                        _ => self.theme.missing,
                    };
                    let bounds =
                        Rect::new(left, top, (right - left).max(0.0), (bottom - top).max(0.0));
                    output.paint.push(PaintCommand::Fill {
                        rect: bounds,
                        color: cell_color,
                    });
                    if value.is_none_or(DataValue::is_null) {
                        output.paint.push(PaintCommand::Stroke {
                            rect: bounds,
                            color: self.theme.foreground,
                            width: 1.0,
                        });
                    }
                    self.push_hit(
                        dataset,
                        inspection_source,
                        *index,
                        series_column,
                        bounds,
                        hits,
                    );
                }
            }
            Mark::Bar { orientation } => {
                if values.len() > 60_000 {
                    return Err(ChartError::Limit {
                        resource: "bar/heatmap paint marks",
                        actual: values.len(),
                        limit: 60_000,
                    });
                }
                let horizontal = *orientation == crate::Orientation::Horizontal;
                let category_coordinates = if horizontal {
                    &y_coordinates
                } else {
                    &x_coordinates
                };
                let category_count = category_coordinates
                    .iter()
                    .flatten()
                    .map(|value| value.to_bits())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    .max(1);
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
                let category_span = if horizontal {
                    plot.size.height
                } else {
                    plot.size.width
                };
                let category_thickness = (category_span / category_count as f32 * 0.72).max(1.0);
                let thickness = if grouped {
                    category_thickness / series_count as f32
                } else {
                    category_thickness
                };
                for (index, x, y) in &values {
                    let mut point = map(*x, *y);
                    if grouped {
                        let series = series_coordinates
                            .as_ref()
                            .and_then(|coordinates| coordinates[*index])
                            .unwrap_or(0.0) as f32;
                        let offset = (series - (series_count as f32 - 1.0) / 2.0) * thickness;
                        if horizontal {
                            point.y += offset;
                        } else {
                            point.x += offset;
                        }
                    }
                    let bounds = if horizontal {
                        let baseline_value = x2_column
                            .and_then(|column| coordinate(&column.values[*index], *index))
                            .unwrap_or(0.0);
                        x_mapper.validate(baseline_value)?;
                        let baseline = x_mapper.map(baseline_value);
                        let top = (point.y - thickness / 2.0).max(plot.origin.y);
                        let bottom =
                            (point.y + thickness / 2.0).min(plot.origin.y + plot.size.height);
                        Rect::new(
                            point.x.min(baseline),
                            top,
                            (point.x - baseline).abs().max(1.0),
                            (bottom - top).max(0.0),
                        )
                    } else {
                        let baseline_value = y2_column
                            .and_then(|column| coordinate(&column.values[*index], *index))
                            .unwrap_or(0.0);
                        y_mapper.validate(baseline_value)?;
                        let baseline = y_mapper.map(baseline_value);
                        let left = (point.x - thickness / 2.0).max(plot.origin.x);
                        let right =
                            (point.x + thickness / 2.0).min(plot.origin.x + plot.size.width);
                        Rect::new(
                            left,
                            point.y.min(baseline),
                            (right - left).max(0.0),
                            (point.y - baseline).abs().max(1.0),
                        )
                    };
                    output.paint.push(PaintCommand::Fill {
                        rect: bounds,
                        color,
                    });
                    self.push_hit(
                        dataset,
                        inspection_source,
                        *index,
                        series_column,
                        bounds,
                        hits,
                    );
                }
            }
            Mark::Rule => {
                let mut triangles = Vec::new();
                for (index, x, y) in &values {
                    let start = map(*x, *y);
                    let end_x = x2_column
                        .and_then(|column| coordinate(&column.values[*index], *index))
                        .unwrap_or(*x);
                    let end_y = y2_column
                        .and_then(|column| coordinate(&column.values[*index], *index))
                        .unwrap_or(*y);
                    x_mapper.validate(end_x)?;
                    y_mapper.validate(end_y)?;
                    let end = map(end_x, end_y);
                    triangles.extend(stroke_polyline(&[start, end], 1.5));
                    let left = start.x.min(end.x);
                    let top = start.y.min(end.y);
                    self.push_hit(
                        dataset,
                        inspection_source,
                        *index,
                        series_column,
                        Rect::new(
                            left - 4.0,
                            top - 4.0,
                            (start.x - end.x).abs() + 8.0,
                            (start.y - end.y).abs() + 8.0,
                        ),
                        hits,
                    );
                }
                if !triangles.is_empty() {
                    output.paint.push(path_command(
                        triangles,
                        plot,
                        color,
                        values.len().saturating_mul(2),
                    )?);
                }
            }
            Mark::Text => {
                let text_column = channel(&layer.encodings, Channel::Text)
                    .and_then(|encoding| dataset.column(&encoding.field.column))
                    .ok_or_else(|| {
                        ChartError::invalid(
                            crate::ChartStage::Geometry,
                            "text mark requires a text encoding",
                        )
                    })?;
                for (index, x, y) in &values {
                    let point = map(*x, *y);
                    let width = plot.size.width.min(160.0);
                    let height = plot.size.height.min(20.0);
                    let bounds = Rect::new(
                        point
                            .x
                            .clamp(plot.origin.x, plot.origin.x + plot.size.width - width),
                        point
                            .y
                            .clamp(plot.origin.y, plot.origin.y + plot.size.height - height),
                        width,
                        height,
                    );
                    output.paint.push(PaintCommand::Text {
                        bounds,
                        text: format_data_value(&text_column.values[*index]),
                        scale: -self.theme.typography.label,
                        color,
                        align: twinkle_ui::TextAlign::Start,
                        bold: false,
                        wrap: false,
                    });
                    self.push_hit(
                        dataset,
                        inspection_source,
                        *index,
                        series_column,
                        bounds,
                        hits,
                    );
                }
            }
            _ => {
                return Err(ChartError::Unsupported(format!(
                    "{:?} native preparation",
                    layer.mark
                )));
            }
        }
        if matches!(layer.mark, Mark::Line { .. } | Mark::Point { .. }) {
            for (index, x, y) in values {
                let p = map(x, y);
                self.push_hit(
                    dataset,
                    inspection_source,
                    index,
                    series_column,
                    Rect::new(p.x - 5.0, p.y - 5.0, 10.0, 10.0),
                    hits,
                );
            }
        }
        Ok(())
    }

    fn legend_labels(
        &self,
        resolved: &[Option<ResolvedLayer>],
    ) -> Result<Vec<Arc<str>>, ChartError> {
        let Some((layer_index, encoding)) = self
            .spec
            .layers
            .iter()
            .filter(|layer| layer.visible)
            .enumerate()
            .find_map(|(index, layer)| {
                channel(&layer.encodings, Channel::Series).map(|encoding| (index, encoding))
            })
        else {
            return Ok(Vec::new());
        };
        let dataset = resolved
            .get(layer_index)
            .and_then(Option::as_ref)
            .map(|layer| &layer.dataset)
            .or_else(|| self.data.dataset(&encoding.field.dataset))
            .ok_or_else(|| ChartError::MissingDataset(encoding.field.dataset.to_string()))?;
        let column =
            dataset
                .column(&encoding.field.column)
                .ok_or_else(|| ChartError::MissingColumn {
                    dataset: encoding.field.dataset.to_string(),
                    column: encoding.field.column.to_string(),
                })?;
        let mut labels = Vec::<Arc<str>>::new();
        for value in column.values.iter() {
            let label = Arc::<str>::from(format_data_value(value));
            if !labels.contains(&label) {
                labels.push(label);
                if labels.len() > self.limits.max_series {
                    return Err(ChartError::Limit {
                        resource: "series",
                        actual: labels.len(),
                        limit: self.limits.max_series,
                    });
                }
            }
        }
        Ok(labels)
    }

    fn prepare_legends(
        &self,
        plot: Rect,
        series_column: Option<&crate::Column>,
        output: &mut RetainedSurfaceOutput,
    ) -> Result<(), ChartError> {
        if self.spec.legends.is_empty() {
            return Ok(());
        }
        let Some(column) = series_column else {
            return Ok(());
        };
        let mut labels = Vec::<Arc<str>>::new();
        for value in column.values.iter() {
            let label = Arc::<str>::from(format_data_value(value));
            if !labels.contains(&label) {
                labels.push(label);
            }
        }
        if labels.len() > self.limits.max_series {
            return Err(ChartError::Limit {
                resource: "series",
                actual: labels.len(),
                limit: self.limits.max_series,
            });
        }
        let label_scale = -self.theme.typography.label;
        let em = measured_text("M", label_scale, false, None).height.max(1.0);
        let block_padding = em * 0.25;
        let swatch = em * 0.65;
        let gap = em * 0.35;
        let allocations = legend_item_bounds(&labels, label_scale, plot.size.width);
        let toggleable = self.spec.legends.iter().any(|legend| legend.toggleable);
        for (index, (label, allocation)) in labels.into_iter().zip(allocations).enumerate() {
            if allocation.origin.y + allocation.size.height > plot.size.height {
                break;
            }
            let bounds = Rect::new(
                plot.origin.x + allocation.origin.x,
                plot.origin.y + allocation.origin.y,
                allocation.size.width,
                allocation.size.height,
            );
            let y = bounds.origin.y;
            let width = bounds.size.width;
            let row_height = bounds.size.height;
            let visible = self
                .state
                .visible_series
                .as_ref()
                .is_none_or(|series| series.contains(&label));
            output.paint.push(PaintCommand::RoundedFill {
                rect: Rect::new(
                    bounds.origin.x + block_padding,
                    y + (row_height - swatch) * 0.5,
                    swatch,
                    swatch,
                ),
                color: if visible {
                    self.theme.series_cue(index).color
                } else {
                    self.theme.missing
                },
                radius: swatch * 0.2,
            });
            output.paint.push(PaintCommand::Text {
                bounds: Rect::new(
                    bounds.origin.x + block_padding + swatch + gap,
                    y,
                    (width - block_padding * 2.0 - swatch - gap).max(0.0),
                    row_height,
                ),
                text: label.to_string(),
                scale: label_scale,
                color: self.theme.foreground,
                align: twinkle_ui::TextAlign::Start,
                bold: false,
                wrap: false,
            });
            let id = UiId::from(format!("chart/legend/{label}"));
            if toggleable {
                output.interactions.push(RetainedSurfaceInteraction {
                    id: id.clone(),
                    bounds,
                    action: ActionKind::Activate,
                    focusable: true,
                });
            }
            output
                .semantics
                .push(twinkle_ui::retained_surface::RetainedSurfaceSemantic {
                    id,
                    bounds,
                    role: if toggleable {
                        SemanticRole::Button
                    } else {
                        SemanticRole::Text
                    },
                    label: Some(format!("{label} series")),
                    state: Some(if visible {
                        "visible".into()
                    } else {
                        "hidden".into()
                    }),
                    protected: false,
                });
        }
        Ok(())
    }

    fn prepare_guides(
        &self,
        layer_index: usize,
        plot: Rect,
        x_domain: [f64; 2],
        y_domain: [f64; 2],
        x_scale: Option<&str>,
        y_scale: Option<&str>,
        x_mapper: &AxisMapper,
        y_mapper: &AxisMapper,
        output: &mut RetainedSurfaceOutput,
    ) -> Result<(), ChartError> {
        for axis in &self.spec.axes {
            let text_scale = -self.theme.typography.label;
            let line_height = measured_text("M", text_scale, false, None).height;
            let gap = line_height * 0.25;
            let mut last_label: Option<Rect> = None;
            let count = axis
                .tick_count
                .unwrap_or(5)
                .clamp(2, self.limits.max_ticks_per_axis);
            let is_x = x_scale == Some(axis.scale.as_ref());
            let is_y = y_scale == Some(axis.scale.as_ref());
            if (!is_x && !is_y)
                || !self.is_first_visible_layer_for_scale(layer_index, axis.scale.as_ref())
            {
                continue;
            }
            let secondary_y = !is_x && is_y && self.y_axis_ordinal(axis.scale.as_ref()) > 0;
            let domain = if is_x {
                x_domain
            } else if y_scale == Some(axis.scale.as_ref()) {
                y_domain
            } else {
                continue;
            };
            for index in 0..count {
                let t = index as f32 / (count - 1) as f32;
                if is_x {
                    let start = x_mapper.map(domain[0]);
                    let x = start + (x_mapper.map(domain[1]) - start) * t;
                    let value = x_mapper.invert(x);
                    if axis.grid {
                        output.paint.push(PaintCommand::Fill {
                            rect: Rect::new(x, plot.origin.y, 1.0, plot.size.height),
                            color: self.theme.grid,
                        });
                    }
                    let text = format_tick(value, axis.unit.as_deref());
                    let size = measured_text(&text, text_scale, false, None);
                    let width = plot.size.width.min(size.width);
                    let origin = (x - width / 2.0).clamp(
                        plot.origin.x,
                        (plot.origin.x + plot.size.width - width).max(plot.origin.x),
                    );
                    let bounds = Rect::new(
                        origin,
                        plot.origin.y + plot.size.height + gap,
                        width,
                        size.height,
                    );
                    if last_label.is_some_and(|last| rects_overlap(last, bounds, gap)) {
                        continue;
                    }
                    last_label = Some(bounds);
                    output.paint.push(PaintCommand::Text {
                        bounds,
                        text,
                        scale: text_scale,
                        color: self.theme.muted,
                        align: twinkle_ui::TextAlign::Center,
                        bold: false,
                        wrap: false,
                    });
                } else {
                    let start = y_mapper.map(domain[0]);
                    let y = start + (y_mapper.map(domain[1]) - start) * t;
                    let value = y_mapper.invert(y);
                    if axis.grid {
                        output.paint.push(PaintCommand::Fill {
                            rect: Rect::new(plot.origin.x, y, plot.size.width, 1.0),
                            color: self.theme.grid,
                        });
                    }
                    let text = format_tick(value, axis.unit.as_deref());
                    let size = measured_text(&text, text_scale, false, None);
                    let label_y = y - size.height * 0.5;
                    let bounds = Rect::new(
                        if secondary_y {
                            plot.origin.x + plot.size.width + gap
                        } else {
                            plot.origin.x - size.width - gap
                        },
                        label_y,
                        size.width,
                        size.height,
                    );
                    if last_label.is_some_and(|last| rects_overlap(last, bounds, gap)) {
                        continue;
                    }
                    last_label = Some(bounds);
                    output.paint.push(PaintCommand::Text {
                        bounds,
                        text,
                        scale: text_scale,
                        color: self.theme.muted,
                        align: if secondary_y {
                            twinkle_ui::TextAlign::Start
                        } else {
                            twinkle_ui::TextAlign::End
                        },
                        bold: false,
                        wrap: false,
                    });
                }
            }
            if let Some(title) = &axis.title {
                let size = measured_text(title, text_scale, true, None);
                output.paint.push(PaintCommand::Text {
                    bounds: if is_x {
                        Rect::new(
                            plot.origin.x,
                            plot.origin.y + plot.size.height + line_height + gap * 2.0,
                            plot.size.width,
                            size.height,
                        )
                    } else if secondary_y {
                        Rect::new(
                            plot.origin.x + plot.size.width + gap,
                            plot.origin.y - size.height - gap - line_height * 0.5,
                            size.width,
                            size.height,
                        )
                    } else {
                        Rect::new(
                            plot.origin.x - size.width - gap,
                            plot.origin.y - size.height - gap - line_height * 0.5,
                            size.width,
                            size.height,
                        )
                    },
                    text: title.to_string(),
                    scale: text_scale,
                    color: self.theme.foreground,
                    align: if is_x {
                        twinkle_ui::TextAlign::Center
                    } else if secondary_y {
                        twinkle_ui::TextAlign::Start
                    } else {
                        twinkle_ui::TextAlign::End
                    },
                    bold: true,
                    wrap: false,
                });
            }
        }
        for annotation in &self.spec.annotations {
            let annotation_scale = match annotation {
                crate::Annotation::Rule { scale, .. } | crate::Annotation::Band { scale, .. } => {
                    Some(scale.as_ref())
                }
                crate::Annotation::Text { .. } => None,
            };
            if annotation_scale
                .is_some_and(|scale| !self.is_first_visible_layer_for_scale(layer_index, scale))
                || (annotation_scale.is_none() && layer_index != 0)
            {
                continue;
            }
            match annotation {
                crate::Annotation::Rule {
                    scale,
                    value,
                    label,
                    ..
                } => {
                    if x_scale == Some(scale.as_ref()) {
                        x_mapper.validate(*value)?;
                        let x = x_mapper.map(*value);
                        output.paint.push(PaintCommand::Fill {
                            rect: Rect::new(x, plot.origin.y, 1.5, plot.size.height),
                            color: self.theme.selection,
                        });
                        if let Some(label) = label {
                            annotation_label(
                                output,
                                plot,
                                label,
                                x,
                                plot.origin.y,
                                self.theme.foreground,
                                -self.theme.typography.label,
                            );
                        }
                    } else if y_scale == Some(scale.as_ref()) {
                        y_mapper.validate(*value)?;
                        let y = y_mapper.map(*value);
                        output.paint.push(PaintCommand::Fill {
                            rect: Rect::new(plot.origin.x, y, plot.size.width, 1.5),
                            color: self.theme.selection,
                        });
                        if let Some(label) = label {
                            annotation_label(
                                output,
                                plot,
                                label,
                                plot.origin.x,
                                y,
                                self.theme.foreground,
                                -self.theme.typography.label,
                            );
                        }
                    }
                }
                crate::Annotation::Band {
                    scale,
                    start,
                    end,
                    label,
                    ..
                } => {
                    if x_scale == Some(scale.as_ref()) {
                        x_mapper.validate(*start)?;
                        x_mapper.validate(*end)?;
                        let a = x_mapper.map(*start);
                        let b = x_mapper.map(*end);
                        output.paint.push(PaintCommand::Fill {
                            rect: Rect::new(
                                a.min(b),
                                plot.origin.y,
                                (b - a).abs(),
                                plot.size.height,
                            ),
                            color: 0x303975D8,
                        });
                        if let Some(label) = label {
                            annotation_label(
                                output,
                                plot,
                                label,
                                a.min(b),
                                plot.origin.y,
                                self.theme.foreground,
                                -self.theme.typography.label,
                            );
                        }
                    } else if y_scale == Some(scale.as_ref()) {
                        y_mapper.validate(*start)?;
                        y_mapper.validate(*end)?;
                        let a = y_mapper.map(*start);
                        let b = y_mapper.map(*end);
                        output.paint.push(PaintCommand::Fill {
                            rect: Rect::new(
                                plot.origin.x,
                                a.min(b),
                                plot.size.width,
                                (b - a).abs(),
                            ),
                            color: 0x303975D8,
                        });
                        if let Some(label) = label {
                            annotation_label(
                                output,
                                plot,
                                label,
                                plot.origin.x,
                                a.min(b),
                                self.theme.foreground,
                                -self.theme.typography.label,
                            );
                        }
                    }
                }
                crate::Annotation::Text { text, x, y } => {
                    x_mapper.validate(*x)?;
                    y_mapper.validate(*y)?;
                    let point = Point {
                        x: x_mapper.map(*x),
                        y: y_mapper.map(*y),
                    };
                    annotation_label(
                        output,
                        plot,
                        text,
                        point.x,
                        point.y,
                        self.theme.foreground,
                        -self.theme.typography.label,
                    );
                }
            }
        }
        Ok(())
    }

    fn y_axis_ordinal(&self, scale: &str) -> usize {
        let mut seen = Vec::<&str>::new();
        for axis in &self.spec.axes {
            let used_on_y = self.spec.layers.iter().any(|layer| {
                layer.encodings.iter().any(|encoding| {
                    matches!(encoding.channel, Channel::Y | Channel::Y2)
                        && encoding.scale.as_deref() == Some(axis.scale.as_ref())
                })
            });
            if used_on_y && !seen.contains(&axis.scale.as_ref()) {
                seen.push(axis.scale.as_ref());
            }
        }
        seen.iter().position(|name| *name == scale).unwrap_or(0)
    }

    fn is_first_visible_layer_for_scale(&self, layer_index: usize, scale: &str) -> bool {
        self.spec
            .layers
            .iter()
            .filter(|layer| layer.visible)
            .position(|layer| {
                layer.encodings.iter().any(|encoding| {
                    matches!(
                        encoding.channel,
                        Channel::X | Channel::X2 | Channel::Y | Channel::Y2
                    ) && encoding.scale.as_deref() == Some(scale)
                })
            })
            == Some(layer_index)
    }

    fn prepare_arc(
        &self,
        layer_index: usize,
        layer: &crate::Layer,
        plot: Rect,
        inner_ratio: f32,
        output: &mut RetainedSurfaceOutput,
        hits: &mut Vec<ChartHit>,
        inspection_sources: &mut Vec<InspectionSource>,
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
        let inspection_source = register_inspection_source(
            inspection_sources,
            dataset,
            self.data_owned,
            layer_index,
            plot,
        );
        let column =
            dataset
                .column(&value.field.column)
                .ok_or_else(|| ChartError::MissingColumn {
                    dataset: value.field.dataset.to_string(),
                    column: value.field.column.to_string(),
                })?;
        if column.values.iter().any(
            |value| matches!(value, DataValue::Number(number) if !number.is_finite() || *number < 0.0),
        ) {
            return Err(ChartError::invalid(
                crate::ChartStage::Transform,
                "pie and donut values must be finite and nonnegative",
            ));
        }
        let category_column = channel(&layer.encodings, Channel::X)
            .and_then(|encoding| dataset.column(&encoding.field.column));
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
            let category = category_column
                .map(|column| format_data_value(&column.values[row]))
                .unwrap_or_else(|| dataset.keys()[row].0.to_string());
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
            let middle = (start + end) / 2.0;
            let label_radius = inner + (radius - inner) * 0.58;
            annotation_label(
                output,
                plot,
                &format!("{category}: {value} ({:.1}%)", value / total * 100.0),
                center.x + label_radius * middle.cos(),
                center.y + label_radius * middle.sin(),
                self.theme.foreground,
                -self.theme.typography.label,
            );
            self.push_hit(
                dataset,
                inspection_source,
                row,
                None,
                Rect::new(
                    center.x - radius,
                    center.y - radius,
                    radius * 2.0,
                    radius * 2.0,
                ),
                hits,
            );
            if let Some(hit) = hits.last_mut() {
                hit.series = Some(Arc::from(category));
            }
            start = end;
        }
        Ok(())
    }

    fn push_hit(
        &self,
        dataset: &crate::Dataset,
        source_index: usize,
        row: usize,
        series_column: Option<&crate::Column>,
        bounds: Rect,
        hits: &mut Vec<ChartHit>,
    ) {
        let key = dataset.keys()[row].clone();
        hits.push(ChartHit {
            dataset: dataset.id.0.clone(),
            row: key.clone(),
            series: series_column.map(|column| Arc::from(format_data_value(&column.values[row]))),
            bounds,
            area_band: None,
            revision: self.data.revision(),
            source_index,
            row_index: row,
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
            Ok((output, hits, hit_x_order, inspection_sources, diagnostics, plot)) => {
                self.plot_bounds = Some(plot);
                self.area_band_bytes = area_band_bytes(&hits);
                self.hits = hits;
                self.hit_x_order = hit_x_order;
                self.inspection_sources = inspection_sources;
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
                self.plot_bounds = None;
                self.area_band_bytes = 0;
                self.hits.clear();
                self.hits.shrink_to_fit();
                self.hit_x_order.clear();
                self.hit_x_order.shrink_to_fit();
                self.inspection_sources.clear();
                self.inspection_sources.shrink_to_fit();
            }
            _ => {}
        }
    }
    fn retained_bytes(&self) -> usize {
        self.hits.capacity() * std::mem::size_of::<ChartHit>()
            + self.area_band_bytes
            + self.hit_x_order.capacity() * std::mem::size_of::<usize>()
            + self.inspection_sources.capacity() * std::mem::size_of::<InspectionSource>()
            + self
                .inspection_sources
                .iter()
                .map(|source| source.owned_bytes)
                .sum::<usize>()
    }
}

fn register_inspection_source(
    sources: &mut Vec<InspectionSource>,
    dataset: &crate::Dataset,
    owned: bool,
    layer_index: usize,
    plot_bounds: Rect,
) -> usize {
    let index = sources.len();
    sources.push(InspectionSource {
        dataset: Arc::new(dataset.clone()),
        owned_bytes: owned.then(|| dataset_owned_bytes(dataset)).unwrap_or(0),
        layer_index,
        plot_bounds,
    });
    index
}

fn dataset_owned_bytes(dataset: &crate::Dataset) -> usize {
    let mut bytes = dataset
        .len()
        .saturating_mul(std::mem::size_of::<crate::RowKey>())
        .saturating_add(
            dataset
                .columns()
                .len()
                .saturating_mul(std::mem::size_of::<crate::Column>()),
        );
    for key in dataset.keys() {
        bytes = bytes.saturating_add(key.0.len());
    }
    for column in dataset.columns() {
        bytes = bytes
            .saturating_add(column.name.len())
            .saturating_add(column.unit.as_deref().map_or(0, str::len))
            .saturating_add(
                column
                    .values
                    .len()
                    .saturating_mul(std::mem::size_of::<DataValue>()),
            );
        for value in column.values.iter() {
            if let DataValue::Category(value) = value {
                bytes = bytes.saturating_add(value.len());
            }
        }
    }
    bytes
}

/// Transactional retained host and normal Twinkle projection authority.
#[derive(Clone, Debug)]
pub struct ChartView {
    host: RetainedSurfaceHost<ChartModel>,
    navigation: crate::ChartNavigation,
    fallback: RetainedSurfaceSnapshot,
    last_error: Option<ChartError>,
}

/// Optional native input bindings for one bounded chart surface.
#[derive(Clone)]
pub struct ChartInputBindings<Message> {
    pub pointer_move: Option<fn(Point) -> Message>,
    pub drag: Option<(Message, fn(Message, DragGesture) -> Message)>,
    pub wheel: Option<fn(WheelGesture) -> Message>,
}

impl<Message> Default for ChartInputBindings<Message> {
    fn default() -> Self {
        Self {
            pointer_move: None,
            drag: None,
            wheel: None,
        }
    }
}

#[derive(Clone)]
enum ChartProjectionAction {
    Event(ChartEvent),
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
            navigation: crate::ChartNavigation::default(),
            fallback: RetainedSurfaceSnapshot::default(),
            last_error: None,
        })
    }
    pub fn update(
        &mut self,
        input: RetainedSurfaceInput,
    ) -> Result<&RetainedSurfaceSnapshot, ChartError> {
        if self.host.snapshot().input == Some(input) {
            self.host
                .update(input)
                .map_err(|message| ChartError::invalid(crate::ChartStage::Geometry, message))?;
            self.fallback = RetainedSurfaceSnapshot::default();
            self.last_error = None;
            return Ok(self.host.snapshot());
        }
        let mut candidate = self.host.begin_candidate();
        if let Err(message) = candidate.update(input) {
            let error = ChartError::invalid(crate::ChartStage::Geometry, message);
            self.record_error(input, &error);
            return Err(error);
        }
        let old = self.host.finalize_candidate(candidate);
        drop(old);
        self.fallback = RetainedSurfaceSnapshot::default();
        self.last_error = None;
        self.navigation.reconcile(self.host.model().hits());
        Ok(self.snapshot())
    }
    pub fn snapshot(&self) -> &RetainedSurfaceSnapshot {
        if self.host.snapshot().input.is_some() {
            self.host.snapshot()
        } else {
            &self.fallback
        }
    }
    /// Most recent rejected candidate, cleared by the next accepted update.
    pub fn last_error(&self) -> Option<&ChartError> {
        self.last_error.as_ref()
    }
    pub fn diagnostics(&self) -> &ChartDiagnostics {
        self.host.model().diagnostics()
    }
    /// Plot allocation of the accepted preparation, excluding chart chrome.
    /// Coordinates use the same space as the retained surface input bounds.
    pub fn plot_bounds(&self) -> Option<Rect> {
        self.host.model().plot_bounds
    }
    /// Ownership counters from the retained-surface admission boundary.
    ///
    /// These complement [`ChartDiagnostics`] with the bytes held by the
    /// accepted paint snapshot and make lifecycle contraction observable
    /// without exposing the retained host itself.
    pub fn retained_diagnostics(&self) -> twinkle_ui::retained_surface::RetainedSurfaceDiagnostics {
        self.host.diagnostics()
    }
    pub fn hits(&self) -> &[ChartHit] {
        self.host.model().hits()
    }
    pub fn nearest(&self, point: Point) -> Option<&ChartHit> {
        self.host.model().nearest(point)
    }
    pub fn active_hit(&self) -> Option<&ChartHit> {
        self.navigation.active(self.host.model().hits())
    }
    pub fn inspection(&self, hit: &ChartHit) -> Option<ChartInspection> {
        self.host.model().inspection(hit)
    }
    pub fn inspector(&self) -> ChartInspector {
        ChartInspector {
            model: Arc::new(self.host.model().clone()),
        }
    }
    pub fn active_inspection(&self) -> Option<ChartInspection> {
        self.active_hit()
            .and_then(|hit| self.host.model().inspection(hit))
    }
    pub fn inspect_nearest_detail(&mut self, point: Point) -> Option<ChartInspection> {
        let index = self.host.model().nearest_index(point)?;
        let hit = self
            .navigation
            .set_active_index(self.host.model().hits(), index)?;
        self.host.model().inspection(hit)
    }
    pub fn inspect_nearest(&mut self, point: Point) -> Option<ChartEvent> {
        let index = self.host.model().nearest_index(point)?;
        let hit = self
            .navigation
            .set_active_index(self.host.model().hits(), index)?;
        Some(ChartEvent::Inspect {
            dataset: hit.dataset.clone(),
            row: hit.row.clone(),
            revision: hit.revision,
        })
    }
    pub fn navigate(&mut self, command: crate::NavigationCommand) -> Option<ChartEvent> {
        self.navigation.command(self.host.model().hits(), command)
    }
    /// Resolve a standard viewport-control request into caller-controlled state.
    pub fn viewport_event(
        &self,
        command: crate::ViewportCommand,
        policy: crate::PanZoomPolicy,
    ) -> Option<ChartEvent> {
        let viewport = self
            .host
            .model()
            .state
            .viewport
            .unwrap_or_else(|| policy.reset());
        let center = [
            viewport.x.map_or(0.0, |range| (range[0] + range[1]) * 0.5),
            viewport.y.map_or(0.0, |range| (range[0] + range[1]) * 0.5),
        ];
        let viewport = match command {
            crate::ViewportCommand::ZoomIn => policy.zoom(viewport, 1.25, center)?,
            crate::ViewportCommand::ZoomOut => policy.zoom(viewport, 0.8, center)?,
            crate::ViewportCommand::Reset => policy.reset(),
        };
        Some(ChartEvent::ViewportChanged(viewport))
    }
    pub fn replace_and_update(
        &mut self,
        spec: Arc<ChartSpec>,
        data: Arc<ChartData>,
        input: RetainedSurfaceInput,
    ) -> Result<&RetainedSurfaceSnapshot, ChartError> {
        let mut candidate = self.host.begin_candidate();
        if let Err(error) = candidate.model_mut().replace(spec, data) {
            self.record_error(input, &error);
            return Err(error);
        }
        if let Err(message) = candidate.update(input) {
            let error = ChartError::invalid(crate::ChartStage::Geometry, message);
            self.record_error(input, &error);
            return Err(error);
        }
        let old = self.host.finalize_candidate(candidate);
        drop(old);
        self.fallback = RetainedSurfaceSnapshot::default();
        self.last_error = None;
        self.navigation.reconcile(self.host.model().hits());
        Ok(self.snapshot())
    }
    pub fn update_state(
        &mut self,
        state: ChartState,
        input: RetainedSurfaceInput,
    ) -> Result<&RetainedSurfaceSnapshot, ChartError> {
        let mut candidate = self.host.begin_candidate();
        candidate.model_mut().state = state;
        if let Err(message) = candidate.update(input) {
            let error = ChartError::invalid(crate::ChartStage::Geometry, message);
            self.record_error(input, &error);
            return Err(error);
        }
        let old = self.host.finalize_candidate(candidate);
        drop(old);
        self.fallback = RetainedSurfaceSnapshot::default();
        self.last_error = None;
        self.navigation.reconcile(self.host.model().hits());
        Ok(self.snapshot())
    }
    /// Transactionally replace semantic chart tokens and prepare the affected
    /// presentation while preserving accepted data, state, and navigation.
    pub fn update_theme(
        &mut self,
        theme: ChartTheme,
        input: RetainedSurfaceInput,
    ) -> Result<&RetainedSurfaceSnapshot, ChartError> {
        theme.validate()?;
        let mut candidate = self.host.begin_candidate();
        candidate.model_mut().theme = theme;
        if let Err(message) = candidate.update(input) {
            let error = ChartError::invalid(crate::ChartStage::Geometry, message);
            self.record_error(input, &error);
            return Err(error);
        }
        let old = self.host.finalize_candidate(candidate);
        drop(old);
        self.fallback = RetainedSurfaceSnapshot::default();
        self.last_error = None;
        self.navigation.reconcile(self.host.model().hits());
        Ok(self.snapshot())
    }
    pub fn element<Message: Clone>(
        &self,
        id: impl Into<UiId>,
        message: impl FnMut(ChartEvent) -> Message,
    ) -> Result<impl twinkle_ui::Component<Message>, ChartError> {
        self.element_inner(id, message, ChartInputBindings::default())
    }

    /// Native projection with one bounded pointer-motion target. The supplied
    /// function should produce an application message that calls
    /// [`ChartView::inspect_nearest`] or [`ChartView::inspect_nearest_detail`]
    /// during update; rebuilding then projects the passive inspector.
    pub fn element_with_pointer<Message: Clone>(
        &self,
        id: impl Into<UiId>,
        message: impl FnMut(ChartEvent) -> Message,
        pointer_message: fn(Point) -> Message,
    ) -> Result<impl twinkle_ui::Component<Message>, ChartError> {
        self.element_with_input(
            id,
            message,
            ChartInputBindings {
                pointer_move: Some(pointer_message),
                ..ChartInputBindings::default()
            },
        )
    }

    /// Native projection with explicitly opted-in hover, captured drag, and
    /// wheel bindings. Without a wheel binding, surrounding native scrolling
    /// retains authority.
    pub fn element_with_input<Message: Clone>(
        &self,
        id: impl Into<UiId>,
        message: impl FnMut(ChartEvent) -> Message,
        bindings: ChartInputBindings<Message>,
    ) -> Result<impl twinkle_ui::Component<Message>, ChartError> {
        self.element_inner(id, message, bindings)
    }

    fn element_inner<Message: Clone>(
        &self,
        id: impl Into<UiId>,
        mut message: impl FnMut(ChartEvent) -> Message,
        bindings: ChartInputBindings<Message>,
    ) -> Result<impl twinkle_ui::Component<Message>, ChartError> {
        let visible = self.host.model().state.visible_series.clone();
        let pan_zoom_controls = bindings.wheel.is_some() || bindings.drag.is_some();
        let mut snapshot = self.snapshot().clone();
        if let (Some(hit), Some(detail)) = (self.active_hit(), self.active_inspection()) {
            let _ =
                project_inspection_overlay(&mut snapshot, hit, &detail, &self.host.model().theme);
        }
        if pan_zoom_controls {
            add_viewport_controls(&mut snapshot, &self.host.model().theme);
        }
        let root_semantic = snapshot
            .output
            .semantics
            .iter()
            .find(|semantic| semantic.id.as_str() == "chart")
            .cloned();
        snapshot
            .output
            .interactions
            .retain(|interaction| interaction.id.as_str() != "chart");
        snapshot
            .output
            .semantics
            .retain(|semantic| semantic.id.as_str() != "chart");
        let mut element = snapshot
            .try_element(id, |interaction| {
                if interaction.id.as_str() == "chart/zoom-in" {
                    ChartProjectionAction::Event(ChartEvent::Viewport(
                        crate::ViewportCommand::ZoomIn,
                    ))
                } else if interaction.id.as_str() == "chart/zoom-out" {
                    ChartProjectionAction::Event(ChartEvent::Viewport(
                        crate::ViewportCommand::ZoomOut,
                    ))
                } else if interaction.id.as_str() == "chart/zoom-reset" {
                    ChartProjectionAction::Event(ChartEvent::Viewport(
                        crate::ViewportCommand::Reset,
                    ))
                } else if let Some((_, label)) = interaction.id.as_str().split_once("chart/legend/")
                {
                    let label: Arc<str> = Arc::from(label);
                    let currently_visible = visible
                        .as_ref()
                        .is_none_or(|series| series.contains(&label));
                    ChartProjectionAction::Event(ChartEvent::ToggleSeries {
                        series: label,
                        visible: !currently_visible,
                    })
                } else {
                    ChartProjectionAction::Event(ChartEvent::TableMode(true))
                }
            })
            .map_err(|error| ChartError::invalid(crate::ChartStage::Semantics, error))?
            .semantic_role(SemanticRole::GraphicalCustomControl)
            .message(ChartProjectionAction::Event(ChartEvent::Navigate(
                crate::NavigationCommand::Select,
            )))
            .on_adjust_messages(
                ChartProjectionAction::Event(ChartEvent::Navigate(
                    crate::NavigationCommand::Previous,
                )),
                ChartProjectionAction::Event(ChartEvent::Navigate(crate::NavigationCommand::Next)),
            )
            .on_boundary_messages(
                ChartProjectionAction::Event(ChartEvent::Navigate(crate::NavigationCommand::Home)),
                ChartProjectionAction::Event(ChartEvent::Navigate(crate::NavigationCommand::End)),
            )
            .on_vertical_adjust_messages(
                ChartProjectionAction::Event(ChartEvent::Navigate(
                    crate::NavigationCommand::PreviousSeries,
                )),
                ChartProjectionAction::Event(ChartEvent::Navigate(
                    crate::NavigationCommand::NextSeries,
                )),
            )
            .adjustment_step(0.25);
        if let Some(semantic) = root_semantic {
            if let Some(label) = semantic.label {
                element = element.accessibility_label(label);
            }
            if let Some(state) = semantic.state {
                element = element.accessibility_state(state);
            }
        }
        let mut element = element.map_message(move |action| match action {
            ChartProjectionAction::Event(event) => message(event),
        });
        if let Some(pointer_message) = bindings.pointer_move {
            element = element
                .on_pointer_move(pointer_message)
                .pointer_coordinates(
                    snapshot
                        .input
                        .map_or(Point::default(), |input| input.bounds.origin),
                );
        }
        if let Some(wheel_message) = bindings.wheel {
            element = element.on_wheel(wheel_message);
        }
        if let Some((seed, mapper)) = bindings.drag {
            element = element.on_drag(seed, mapper);
        }
        Ok(element)
    }
    pub fn detach(&mut self) {
        self.host.detach();
        self.navigation = crate::ChartNavigation::default();
        self.fallback = RetainedSurfaceSnapshot::default();
    }
    pub fn renderer_lost(&mut self) {
        self.host.renderer_lost();
    }
    /// Permanently release the accepted presentation and chart-owned indexes.
    /// Subsequent updates fail; construct a new view for a remount.
    pub fn destroy(&mut self) {
        self.host.destroy();
        self.navigation = crate::ChartNavigation::default();
        self.fallback = RetainedSurfaceSnapshot::default();
    }

    fn record_error(&mut self, input: RetainedSurfaceInput, error: &ChartError) {
        self.last_error = Some(error.clone());
        if self.host.snapshot().input.is_none() {
            self.fallback = chart_error_snapshot(
                input,
                self.host.model().spec.title.as_ref(),
                &error.to_string(),
                &self.host.model().theme,
            );
        }
    }
}

fn add_viewport_controls(snapshot: &mut RetainedSurfaceSnapshot, theme: &ChartTheme) {
    let Some(input) = snapshot.input else { return };
    let table_left = snapshot
        .output
        .interactions
        .iter()
        .find(|interaction| interaction.id.as_str() == "chart/table")
        .map_or(
            input.bounds.origin.x + input.bounds.size.width,
            |interaction| interaction.bounds.origin.x,
        );
    let table_top = snapshot
        .output
        .interactions
        .iter()
        .find(|interaction| interaction.id.as_str() == "chart/table")
        .map_or(input.bounds.origin.y, |interaction| {
            interaction.bounds.origin.y
        });
    let scale = -theme.typography.control;
    let em = measured_text("M", scale, false, None).height.max(1.0);
    let inline_padding = em * 0.55;
    let labels = [
        ("chart/zoom-in", "+", "Zoom in"),
        ("chart/zoom-out", "−", "Zoom out"),
        ("chart/zoom-reset", "Reset", "Reset zoom"),
    ];
    let widths = labels
        .map(|(_, label, _)| measured_text(label, scale, false, None).width + inline_padding * 2.0);
    let available = (table_left - input.bounds.origin.x).max(0.0);
    let total = widths.iter().sum::<f32>().min(available);
    let mut x = table_left - total;
    let height = snapshot
        .output
        .interactions
        .iter()
        .find(|interaction| interaction.id.as_str() == "chart/table")
        .map_or(em + inline_padding, |interaction| {
            interaction.bounds.size.height
        });
    for ((id, label, accessible), desired_width) in labels.into_iter().zip(widths) {
        let width = desired_width.min((table_left - x).max(0.0));
        let bounds = Rect::new(x, table_top, width, height);
        x += width;
        if width <= 0.0 || height <= 0.0 {
            continue;
        }
        snapshot.output.paint.push(PaintCommand::RoundedFill {
            rect: bounds,
            color: if theme.high_contrast {
                theme.focus
            } else {
                theme.grid
            },
            radius: 4.0,
        });
        snapshot.output.paint.push(PaintCommand::Text {
            bounds,
            text: label.into(),
            scale,
            color: theme.foreground,
            align: twinkle_ui::TextAlign::Center,
            bold: false,
            wrap: false,
        });
        snapshot
            .output
            .interactions
            .push(RetainedSurfaceInteraction {
                id: UiId::from(id),
                bounds,
                action: ActionKind::Activate,
                focusable: true,
            });
        snapshot
            .output
            .semantics
            .push(twinkle_ui::retained_surface::RetainedSurfaceSemantic {
                id: UiId::from(id),
                bounds,
                role: SemanticRole::Button,
                label: Some(accessible.into()),
                state: Some("Controlled viewport request".into()),
                protected: false,
            });
    }
}

fn inspection_label(inspection: &ChartInspection) -> String {
    let mut parts = inspection
        .display
        .iter()
        .map(|value| {
            value.unit.as_deref().map_or_else(
                || format!("{}: {}", value.label, value.formatted),
                |unit| format!("{}: {} {unit}", value.label, value.formatted),
            )
        })
        .collect::<Vec<_>>();
    if inspection.include_data_table_hint && inspection.cells.len() > inspection.display.len() {
        parts.push("More details in data table".into());
    }
    parts.join("\n")
}

fn default_tooltip_spec(layer: Option<&crate::Layer>) -> ChartTooltipSpec {
    let mut fields = Vec::new();
    if let Some(layer) = layer {
        for channel in [
            Channel::Series,
            Channel::X,
            Channel::Y,
            Channel::Size,
            Channel::Fill,
        ] {
            let Some(encoding) = layer
                .encodings
                .iter()
                .find(|encoding| encoding.channel == channel)
            else {
                continue;
            };
            let mut column = encoding.field.column.clone();
            let mut label = column.replace('_', " ");
            for transform in &layer.transforms {
                if let crate::Transform::Stack { value, .. } = transform
                    && column.as_ref() == format!("{value}_end")
                {
                    column = Arc::from(format!("{value}_raw"));
                    label = value.replace('_', " ");
                }
            }
            if fields
                .iter()
                .any(|field: &ChartTooltipField| field.column == column)
            {
                continue;
            }
            fields.push(ChartTooltipField {
                column,
                label: Arc::from(label),
                unit: None,
                format: ChartTooltipFormat::Automatic,
            });
        }
    }
    ChartTooltipSpec {
        fields,
        include_data_table_hint: true,
    }
}

fn format_tooltip_value(value: &DataValue, format: ChartTooltipFormat) -> String {
    match (value, format) {
        (DataValue::Number(value), ChartTooltipFormat::Number { decimal_places }) => {
            format!(
                "{value:.precision$}",
                precision = usize::from(decimal_places)
            )
        }
        (DataValue::Number(value), ChartTooltipFormat::Percent { decimal_places }) => {
            format!(
                "{:.precision$}%",
                value * 100.0,
                precision = usize::from(decimal_places)
            )
        }
        _ => crate::table::format_value(value),
    }
}

fn rects_overlap(a: Rect, b: Rect, gap: f32) -> bool {
    a.origin.x < b.origin.x + b.size.width + gap
        && b.origin.x < a.origin.x + a.size.width + gap
        && a.origin.y < b.origin.y + b.size.height + gap
        && b.origin.y < a.origin.y + a.size.height + gap
}

fn legend_item_bounds(labels: &[Arc<str>], scale: f32, width: f32) -> Vec<Rect> {
    let em = measured_text("M", scale, false, None).height.max(1.0);
    let row_height = em * 1.5;
    let mut x = 0.0;
    let mut row = 0usize;
    labels
        .iter()
        .map(|label| {
            // The same swatch, gap, and padding used by prepare_legends.
            let item_width = (measured_text(label, scale, false, None).width + em * 1.5).min(width);
            if x > 0.0 && x + item_width > width {
                row += 1;
                x = 0.0;
            }
            let bounds = Rect::new(x, row as f32 * row_height, item_width, row_height);
            x += item_width + em * 0.5;
            bounds
        })
        .collect()
}

fn measured_text(text: &str, scale: f32, bold: bool, wrap_width: Option<f32>) -> Size {
    let measured = twinkle_ui::measure_text(
        text,
        scale,
        bold,
        wrap_width.is_some(),
        None,
        None,
        wrap_width.unwrap_or(f32::INFINITY).max(0.0),
    );
    Size::new(
        measured
            .width
            .min(wrap_width.unwrap_or(f32::INFINITY).max(0.0)),
        measured.height,
    )
}

/// Keep complete fields in priority order; if even the first does not fit,
/// show a measured ellipsis. Exact selected values remain in semantics/table.
fn fit_tooltip(text: &str, scale: f32, available: Size) -> Option<(String, Size)> {
    if available.width <= 0.0 || available.height <= 0.0 {
        return None;
    }
    let measure = |text: &str| {
        twinkle_ui::measure_text(text, scale, false, true, None, None, available.width)
    };
    let fits = |size: Size| size.width <= available.width && size.height <= available.height;
    let full = measure(text);
    if fits(full) {
        return Some((text.to_owned(), full));
    }
    let mut prefix = text;
    while let Some((head, _)) = prefix.rsplit_once('\n') {
        prefix = head;
        let candidate = format!("{prefix}\n…");
        let size = measure(&candidate);
        if fits(size) {
            return Some((candidate, size));
        }
    }
    let ellipsis = measure("…");
    if !fits(ellipsis) {
        return None;
    }
    let boundaries: Vec<usize> = prefix
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(prefix.len()))
        .collect();
    let mut low = 0;
    let mut high = boundaries.len();
    while low + 1 < high {
        let middle = (low + high) / 2;
        let candidate = format!("{}…", &prefix[..boundaries[middle]]);
        if fits(measure(&candidate)) {
            low = middle;
        } else {
            high = middle;
        }
    }
    let text = format!("{}…", &prefix[..boundaries[low]]);
    let size = measure(&text);
    Some((text, size))
}

/// Project one passive, plot-bounded exact-value inspector into an accepted snapshot.
#[doc(hidden)]
pub fn project_inspection_overlay(
    snapshot: &mut RetainedSurfaceSnapshot,
    hit: &ChartHit,
    detail: &ChartInspection,
    theme: &ChartTheme,
) -> bool {
    let limits = RetainedSurfaceLimits::default();
    if snapshot.output.paint.len().saturating_add(3) > limits.max_paint_commands
        || snapshot.output.semantics.len().saturating_add(1) > limits.max_semantics
    {
        return false;
    }
    let Some(_input) = snapshot.input else {
        return false;
    };
    let plot = detail.plot_bounds;
    let accessible_text = inspection_label(detail);
    let scale = -theme.typography.body;
    let em = measured_text("M", scale, false, None).height.max(1.0);
    let padding = em * 0.4;
    let gap = em * 0.5;
    let readable_width = measured_text(&"0".repeat(40), scale, false, None).width;
    let natural = measured_text(&accessible_text, scale, false, None);
    let content_width = natural
        .width
        .min(readable_width)
        .min((plot.size.width - padding * 2.0).max(0.0));
    let Some((text, content)) = fit_tooltip(
        &accessible_text,
        scale,
        Size::new(content_width, (plot.size.height - padding * 2.0).max(0.0)),
    ) else {
        return false;
    };
    let width = (content.width + padding * 2.0).min(plot.size.width);
    let height = (content.height + padding * 2.0).min(plot.size.height);
    let plot_right = plot.origin.x + plot.size.width;
    let plot_bottom = plot.origin.y + plot.size.height;
    let right = hit.bounds.origin.x + hit.bounds.size.width + gap;
    let left = hit.bounds.origin.x - width - gap;
    let x = if right + width <= plot_right {
        right
    } else if left >= plot.origin.x {
        left
    } else {
        right.clamp(plot.origin.x, plot_right - width)
    };
    let below = hit.bounds.origin.y + hit.bounds.size.height + gap;
    let above = hit.bounds.origin.y - height - gap;
    let y = if below + height <= plot_bottom {
        below
    } else if above >= plot.origin.y {
        above
    } else {
        below.clamp(plot.origin.y, plot_bottom - height)
    };
    let bounds = Rect::new(x, y, width, height);
    snapshot.output.paint.push(PaintCommand::Fill {
        rect: bounds,
        color: theme.background,
    });
    snapshot.output.paint.push(PaintCommand::Stroke {
        rect: bounds,
        color: theme.selection,
        width: 1.0,
    });
    snapshot.output.paint.push(PaintCommand::Text {
        bounds: inset(bounds, padding, padding, padding, padding),
        text: text.clone(),
        scale,
        color: theme.foreground,
        align: twinkle_ui::TextAlign::Start,
        bold: false,
        wrap: true,
    });
    snapshot
        .output
        .semantics
        .push(twinkle_ui::retained_surface::RetainedSurfaceSemantic {
            id: UiId::from("chart/inspection"),
            bounds,
            role: SemanticRole::Tooltip,
            label: Some(accessible_text),
            state: Some(format!("row {}; exact accepted values", detail.row.0)),
            protected: false,
        });
    true
}

fn chart_error_snapshot(
    input: RetainedSurfaceInput,
    title: &str,
    error: &str,
    theme: &ChartTheme,
) -> RetainedSurfaceSnapshot {
    let truncate = |value: &str| value.chars().take(512).collect::<String>();
    let title_height = input.bounds.size.height.min(22.0).max(0.0);
    let error_y = input.bounds.origin.y + title_height;
    let error_height = (input.bounds.size.height - title_height).max(0.0);
    let state = format!(
        "Chart unavailable: {}. Activate for the exact paged data table.",
        truncate(error)
    );
    RetainedSurfaceSnapshot {
        input: Some(input),
        output: RetainedSurfaceOutput {
            measured: input.bounds.size,
            paint: vec![
                PaintCommand::Fill {
                    rect: input.bounds,
                    color: theme.background,
                },
                PaintCommand::Text {
                    bounds: Rect::new(
                        input.bounds.origin.x,
                        input.bounds.origin.y,
                        input.bounds.size.width,
                        title_height,
                    ),
                    text: truncate(title),
                    scale: -theme.typography.title,
                    color: theme.foreground,
                    align: twinkle_ui::TextAlign::Start,
                    bold: true,
                    wrap: false,
                },
                PaintCommand::Text {
                    bounds: Rect::new(
                        input.bounds.origin.x,
                        error_y,
                        input.bounds.size.width,
                        error_height,
                    ),
                    text: truncate(error),
                    scale: -theme.typography.body,
                    color: theme.selection,
                    align: twinkle_ui::TextAlign::Start,
                    bold: false,
                    wrap: true,
                },
            ],
            interactions: vec![
                RetainedSurfaceInteraction {
                    id: UiId::from("chart"),
                    bounds: input.bounds,
                    action: ActionKind::Activate,
                    focusable: true,
                },
                RetainedSurfaceInteraction {
                    id: UiId::from("chart/table"),
                    bounds: input.bounds,
                    action: ActionKind::Activate,
                    focusable: true,
                },
            ],
            semantics: vec![
                twinkle_ui::retained_surface::RetainedSurfaceSemantic {
                    id: UiId::from("chart"),
                    bounds: input.bounds,
                    role: SemanticRole::GraphicalCustomControl,
                    label: Some(truncate(title)),
                    state: Some(state),
                    protected: false,
                },
                twinkle_ui::retained_surface::RetainedSurfaceSemantic {
                    id: UiId::from("chart/table"),
                    bounds: input.bounds,
                    role: SemanticRole::Button,
                    label: Some("View chart data table".into()),
                    state: Some("Exact values, paged".into()),
                    protected: false,
                },
            ],
            damage: vec![input.bounds],
        },
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
fn include_domain_value(
    domain: &mut [f64; 2],
    channel_scale: Option<&str>,
    annotation_scale: &str,
    value: f64,
) {
    if channel_scale == Some(annotation_scale) && value.is_finite() {
        domain[0] = domain[0].min(value);
        domain[1] = domain[1].max(value);
    }
}

fn format_tick(value: f64, unit: Option<&str>) -> String {
    let value = if value.abs() >= 1_000_000.0 || (value != 0.0 && value.abs() < 0.001) {
        format!("{value:.2e}")
    } else {
        let text = format!("{value:.3}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    };
    unit.map_or(value.clone(), |unit| format!("{value} {unit}"))
}

pub(crate) fn format_data_value(value: &DataValue) -> String {
    match value {
        DataValue::Null => "Missing".into(),
        DataValue::Number(value) => value.to_string(),
        DataValue::Timestamp(value) => value.to_string(),
        DataValue::Category(value) => value.to_string(),
        DataValue::Boolean(value) => value.to_string(),
    }
}

#[derive(Clone, Debug, PartialEq)]
struct FacetValue {
    key: String,
    label: String,
}

pub(crate) fn facet_value_key(value: &DataValue) -> String {
    match value {
        DataValue::Null => "null".into(),
        DataValue::Number(value) => format!("number:{:016x}", value.to_bits()),
        DataValue::Timestamp(value) => format!("timestamp:{value}"),
        DataValue::Category(value) => format!("category:{}:{value}", value.len()),
        DataValue::Boolean(value) => format!("boolean:{value}"),
    }
}

fn facet_values(column: Option<&crate::Column>) -> Vec<FacetValue> {
    let Some(column) = column else {
        return vec![FacetValue {
            key: "all".into(),
            label: "All".into(),
        }];
    };
    let mut values = Vec::new();
    for value in column.values.iter() {
        let candidate = FacetValue {
            key: facet_value_key(value),
            label: format_data_value(value),
        };
        if !values
            .iter()
            .any(|value: &FacetValue| value.key == candidate.key)
        {
            values.push(candidate);
        }
    }
    if values.is_empty() {
        values.push(FacetValue {
            key: "no-data".into(),
            label: "No data".into(),
        });
    }
    values
}

fn stable_id_fragment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len().saturating_mul(2).max(2));
    for byte in value.as_bytes() {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    if encoded.is_empty() {
        encoded.push_str("00");
    }
    encoded
}

// Resolve shared position scales from the same transformed values, visibility,
// baselines, and annotations used by geometry. Never rescan the raw source here.
fn resolved_shared_domains(
    spec: &ChartSpec,
    resolved: &[Option<ResolvedLayer>],
) -> Vec<(usize, [f64; 2])> {
    let mut domains = Vec::new();
    for (index, scale) in spec.scales.iter().enumerate() {
        if scale.domain.is_some() {
            continue;
        }
        let mut combined: Option<[f64; 2]> = None;
        for (layer, resolved) in spec
            .layers
            .iter()
            .filter(|layer| layer.visible)
            .zip(resolved)
        {
            let Some(resolved) = resolved else { continue };
            for (position, domain) in [
                (Channel::X, resolved.x_domain),
                (Channel::Y, resolved.y_domain),
            ] {
                if channel(&layer.encodings, position)
                    .is_some_and(|encoding| encoding.scale.as_deref() == Some(scale.name.as_ref()))
                {
                    combined = Some(match combined {
                        Some(previous) => [previous[0].min(domain[0]), previous[1].max(domain[1])],
                        None => domain,
                    });
                }
            }
        }
        if let Some(domain) = combined {
            domains.push((index, domain));
        }
    }
    domains
}

fn unify_resolved_domains(spec: &ChartSpec, resolved: &mut [Option<ResolvedLayer>]) {
    if resolved.len() <= 1 {
        return;
    }
    for (index, domain) in resolved_shared_domains(spec, resolved) {
        let name = spec.scales[index].name.as_ref();
        for (layer, resolved) in spec
            .layers
            .iter()
            .filter(|layer| layer.visible)
            .zip(resolved.iter_mut())
        {
            let Some(resolved) = resolved else { continue };
            for (position, target) in [
                (Channel::X, &mut resolved.x_domain),
                (Channel::Y, &mut resolved.y_domain),
            ] {
                if channel(&layer.encodings, position)
                    .is_some_and(|encoding| encoding.scale.as_deref() == Some(name))
                {
                    *target = domain;
                }
            }
        }
    }
}

fn annotation_label(
    output: &mut RetainedSurfaceOutput,
    plot: Rect,
    label: &str,
    x: f32,
    y: f32,
    color: u32,
    scale: f32,
) {
    let measured = measured_text(label, scale, false, None);
    let width = plot.size.width.min(measured.width);
    let height = plot.size.height.min(measured.height);
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let origin_x = x.clamp(
        plot.origin.x,
        (plot.origin.x + plot.size.width - width).max(plot.origin.x),
    );
    let origin_y = y.clamp(
        plot.origin.y,
        (plot.origin.y + plot.size.height - height).max(plot.origin.y),
    );
    output.paint.push(PaintCommand::Text {
        bounds: Rect::new(origin_x, origin_y, width, height),
        text: label.to_owned(),
        scale,
        color,
        align: twinkle_ui::TextAlign::Start,
        bold: false,
        wrap: false,
    });
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
fn series_groups(
    dataset: &crate::Dataset,
    encodings: &[Encoding],
    state: &ChartState,
) -> Vec<Vec<usize>> {
    let Some(column) = channel(encodings, Channel::Series)
        .and_then(|encoding| dataset.column(&encoding.field.column))
    else {
        return vec![(0..dataset.len()).collect()];
    };
    let mut positions = BTreeMap::<String, usize>::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (index, value) in column.values.iter().enumerate() {
        let label = Arc::<str>::from(format_data_value(value));
        if state
            .visible_series
            .as_ref()
            .is_some_and(|visible| !visible.contains(&label))
        {
            continue;
        }
        let key = match value {
            DataValue::Null => "null".to_owned(),
            DataValue::Number(value) => format!("n:{:016x}", value.to_bits()),
            DataValue::Timestamp(value) => format!("t:{value}"),
            DataValue::Category(value) => format!("c:{}:{value}", value.len()),
            DataValue::Boolean(value) => format!("b:{value}"),
        };
        let group = *positions.entry(key).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[group].push(index);
    }
    groups
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
fn normalized_domain(domain: [f64; 2]) -> [f64; 2] {
    extent(domain.into_iter())
}
fn in_domain(value: f64, domain: [f64; 2]) -> bool {
    value >= domain[0].min(domain[1]) && value <= domain[0].max(domain[1])
}
fn inset(rect: Rect, left: f32, top: f32, right: f32, bottom: f32) -> Rect {
    let left = left.max(0.0).min(rect.size.width.max(0.0));
    let top = top.max(0.0).min(rect.size.height.max(0.0));
    Rect::new(
        rect.origin.x + left,
        rect.origin.y + top,
        (rect.size.width - left - right).max(0.0),
        (rect.size.height - top - bottom).max(0.0),
    )
}
fn hit_distance(hit: &ChartHit, point: Point) -> f32 {
    let bounds_distance = distance(hit.bounds, point);
    let Some(vertices) = &hit.area_band else {
        return bounds_distance;
    };
    let mut inside = false;
    let mut edge_distance = f32::INFINITY;
    for i in 0..vertices.len() {
        let a = vertices[i];
        let b = vertices[(i + 1) % vertices.len()];
        if (a.y > point.y) != (b.y > point.y)
            && point.x < (b.x - a.x) * (point.y - a.y) / (b.y - a.y) + a.x
        {
            inside = !inside;
        }
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let length_squared = dx * dx + dy * dy;
        let t = if length_squared > 0.0 {
            (((point.x - a.x) * dx + (point.y - a.y) * dy) / length_squared).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let ex = point.x - (a.x + t * dx);
        let ey = point.y - (a.y + t * dy);
        edge_distance = edge_distance.min(ex * ex + ey * ey);
    }
    bounds_distance.max(if inside { 0.0 } else { edge_distance.sqrt() })
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
#[cfg(test)]
fn hit_center_y(hit: &ChartHit) -> f32 {
    hit.bounds.origin.y + hit.bounds.size.height / 2.0
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
        triangles.extend(band_triangles(
            pair[0],
            Point {
                x: pair[0].x,
                y: baseline,
            },
            pair[1],
            Point {
                x: pair[1].x,
                y: baseline,
            },
        ));
    }
    triangles
}

fn band_triangles(top0: Point, bottom0: Point, top1: Point, bottom1: Point) -> [Point; 6] {
    let height0 = f64::from(top0.y) - f64::from(bottom0.y);
    let height1 = f64::from(top1.y) - f64::from(bottom1.y);
    if (height0 < 0.0 && height1 > 0.0) || (height0 > 0.0 && height1 < 0.0) {
        // A sign-changing band is a bow tie, not a convex quad. Its two
        // lobes meet where the interpolated edges cross; the usual diagonal
        // would fill outside the band and blend overlapping triangles twice.
        let t = height0 / (height0 - height1);
        let crossing = Point {
            x: (f64::from(top0.x) + (f64::from(top1.x) - f64::from(top0.x)) * t) as f32,
            y: (f64::from(top0.y) + (f64::from(top1.y) - f64::from(top0.y)) * t) as f32,
        };
        [top0, bottom0, crossing, crossing, top1, bottom1]
    } else {
        [top0, bottom0, top1, top1, bottom0, bottom1]
    }
}

#[derive(Clone, Copy)]
enum ClipEdge {
    Left(f32),
    Right(f32),
    Top(f32),
    Bottom(f32),
}

fn clip_triangles_to_rect(triangles: &[Point], bounds: Rect) -> Vec<Point> {
    let edges = [
        ClipEdge::Left(bounds.origin.x),
        ClipEdge::Right(bounds.origin.x + bounds.size.width),
        ClipEdge::Top(bounds.origin.y),
        ClipEdge::Bottom(bounds.origin.y + bounds.size.height),
    ];
    let mut clipped = Vec::with_capacity(triangles.len());
    // A convex triangle gains at most one vertex per clipping plane:
    // four rectangle planes therefore need at most seven vertices.
    let mut polygon = [Point::default(); 7];
    let mut scratch = [Point::default(); 7];
    for triangle in triangles.chunks_exact(3) {
        polygon[..3].copy_from_slice(triangle);
        let mut length = 3;
        for edge in edges {
            length = clip_polygon_edge(&polygon[..length], edge, &mut scratch);
            std::mem::swap(&mut polygon, &mut scratch);
            if length == 0 {
                break;
            }
        }
        if length >= 3 {
            for index in 1..length - 1 {
                clipped.extend([polygon[0], polygon[index], polygon[index + 1]]);
            }
        }
    }
    clipped
}

fn clip_polygon_edge(polygon: &[Point], edge: ClipEdge, result: &mut [Point; 7]) -> usize {
    let inside = |point: Point| match edge {
        ClipEdge::Left(x) => point.x >= x,
        ClipEdge::Right(x) => point.x <= x,
        ClipEdge::Top(y) => point.y >= y,
        ClipEdge::Bottom(y) => point.y <= y,
    };
    let intersection = |from: Point, to: Point| match edge {
        ClipEdge::Left(x) | ClipEdge::Right(x) => {
            let t = (x - from.x) / (to.x - from.x);
            Point {
                x,
                y: from.y + (to.y - from.y) * t,
            }
        }
        ClipEdge::Top(y) | ClipEdge::Bottom(y) => {
            let t = (y - from.y) / (to.y - from.y);
            Point {
                x: from.x + (to.x - from.x) * t,
                y,
            }
        }
    };
    let Some(mut previous) = polygon.last().copied() else {
        return 0;
    };
    let mut previous_inside = inside(previous);
    let mut length = 0;
    for current in polygon.iter().copied() {
        let current_inside = inside(current);
        if current_inside != previous_inside {
            result[length] = intersection(previous, current);
            length += 1;
        }
        if current_inside {
            result[length] = current;
            length += 1;
        }
        previous = current;
        previous_inside = current_inside;
    }
    length
}

fn interpolate_points(points: &[Point], interpolation: crate::Interpolation) -> Vec<Point> {
    if interpolation == crate::Interpolation::Straight || points.len() < 2 {
        return points.to_vec();
    }
    let mut result = Vec::with_capacity(points.len().saturating_mul(2).saturating_sub(1));
    result.push(points[0]);
    for pair in points.windows(2) {
        result.push(match interpolation {
            crate::Interpolation::StepBefore => Point {
                x: pair[0].x,
                y: pair[1].y,
            },
            crate::Interpolation::StepAfter => Point {
                x: pair[1].x,
                y: pair[0].y,
            },
            crate::Interpolation::Straight => unreachable!(),
        });
        result.push(pair[1]);
    }
    result
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

fn paint_symbol(
    output: &mut RetainedSurfaceOutput,
    _plot: Rect,
    center: Point,
    radius: f32,
    symbol: crate::Symbol,
    color: u32,
) -> Result<(), ChartError> {
    let bounds = Rect::new(
        center.x - radius,
        center.y - radius,
        radius * 2.0,
        radius * 2.0,
    );
    match symbol {
        crate::Symbol::Circle => output.paint.push(PaintCommand::RoundedFill {
            rect: bounds,
            color,
            radius,
        }),
        crate::Symbol::Square => output.paint.push(PaintCommand::Fill {
            rect: bounds,
            color,
        }),
        crate::Symbol::Triangle => output.paint.push(fill_path_command(
            vec![
                Point {
                    x: center.x,
                    y: center.y - radius,
                },
                Point {
                    x: center.x + radius,
                    y: center.y + radius,
                },
                Point {
                    x: center.x - radius,
                    y: center.y + radius,
                },
            ],
            color,
            3,
        )?),
        crate::Symbol::Diamond => output.paint.push(fill_path_command(
            vec![
                Point {
                    x: center.x,
                    y: center.y - radius,
                },
                Point {
                    x: center.x + radius,
                    y: center.y,
                },
                Point {
                    x: center.x,
                    y: center.y + radius,
                },
                Point {
                    x: center.x,
                    y: center.y - radius,
                },
                Point {
                    x: center.x,
                    y: center.y + radius,
                },
                Point {
                    x: center.x - radius,
                    y: center.y,
                },
            ],
            color,
            4,
        )?),
        crate::Symbol::Cross => {
            let arm = (radius * 0.7).max(1.0);
            output.paint.push(PaintCommand::Fill {
                rect: Rect::new(center.x - arm / 2.0, center.y - radius, arm, radius * 2.0),
                color,
            });
            output.paint.push(PaintCommand::Fill {
                rect: Rect::new(center.x - radius, center.y - arm / 2.0, radius * 2.0, arm),
                color,
            });
        }
    }
    Ok(())
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
    fn undersized_allocations_keep_chrome_bounds_nonnegative_and_inside() {
        let mut view = ChartView::new(model()).unwrap();
        for width in [1.0, 5.0, 20.0, 40.0, 112.0, 260.0] {
            for height in [1.0, 5.0, 20.0, 40.0, 84.0] {
                for scale in [1.0, 1.25, 1.5, 2.0] {
                    let allocation = Rect::new(17.0, 23.0, width, height);
                    view.update(RetainedSurfaceInput::new(
                        allocation,
                        scale,
                        Default::default(),
                    ))
                    .unwrap();
                    let mut snapshot = view.snapshot().clone();
                    add_viewport_controls(&mut snapshot, &ChartTheme::default());
                    let check = |rect: Rect| {
                        assert!(
                            rect.size.width >= 0.0 && rect.size.height >= 0.0,
                            "negative {rect:?}"
                        );
                        assert!(
                            rect.origin.x >= allocation.origin.x
                                && rect.origin.y >= allocation.origin.y,
                            "origin {rect:?}"
                        );
                        assert!(
                            rect.origin.x + rect.size.width <= allocation.origin.x + width + 0.0001,
                            "horizontal {rect:?} in {allocation:?}"
                        );
                        assert!(
                            rect.origin.y + rect.size.height
                                <= allocation.origin.y + height + 0.0001,
                            "vertical {rect:?} in {allocation:?}"
                        );
                    };
                    for command in &snapshot.output.paint {
                        match command {
                            PaintCommand::Fill { rect, .. }
                            | PaintCommand::Stroke { rect, .. }
                            | PaintCommand::RoundedFill { rect, .. } => check(*rect),
                            PaintCommand::Text { bounds, .. } => check(*bounds),
                            _ => {}
                        }
                    }
                    for interaction in &snapshot.output.interactions {
                        check(interaction.bounds);
                    }
                    for semantic in &snapshot.output.semantics {
                        check(semantic.bounds);
                    }
                }
            }
        }
    }

    #[test]
    fn legend_measurement_uses_the_same_transformed_series_as_paint() {
        let mut model = model();
        let mut spec = (*model.spec).clone();
        spec.layers[0].transforms.push(crate::Transform::Bin {
            field: "x".into(),
            output: "bucket".into(),
            options: crate::BinOptions {
                start: 0.0,
                end: 2.0,
                count: 2,
                density: false,
            },
        });
        spec.layers[0].encodings.push(Encoding {
            channel: Channel::Series,
            field: FieldRef::new("d", "bucket"),
            scale: None,
        });
        spec.legends.push(crate::Legend {
            scale: Arc::from("x"),
            title: None,
            toggleable: true,
        });
        model.replace(Arc::new(spec), model.data.clone()).unwrap();
        let mut view = ChartView::new(model).unwrap();
        for scale in [1.0, 1.25, 1.5, 2.0] {
            view.update(RetainedSurfaceInput::new(
                Rect::new(0.0, 0.0, 260.0, 180.0),
                scale,
                Default::default(),
            ))
            .unwrap();
            let legends: Vec<_> = view
                .snapshot()
                .output
                .semantics
                .iter()
                .filter(|semantic| semantic.id.as_str().starts_with("chart/legend/"))
                .collect();
            assert_eq!(legends.len(), 2);
            for legend in legends {
                assert!(
                    legend.bounds.origin.y + legend.bounds.size.height
                        <= view.plot_bounds().unwrap().origin.y
                );
            }
        }
    }

    #[test]
    fn inspection_uses_its_own_layer_and_accepted_plot_not_paint_order() {
        let mut model = model();
        let mut spec = (*model.spec).clone();
        let mut second = spec.layers[0].clone();
        for encoding in &mut second.encodings {
            if encoding.channel == Channel::X {
                encoding.field.column = Arc::from("y");
            }
            if encoding.channel == Channel::Y {
                encoding.field.column = Arc::from("x");
            }
        }
        spec.layers.push(second);
        model.replace(Arc::new(spec), model.data.clone()).unwrap();
        let mut view = ChartView::new(model).unwrap();
        view.update(RetainedSurfaceInput::new(
            Rect::new(20.0, 30.0, 640.0, 400.0),
            1.25,
            Default::default(),
        ))
        .unwrap();
        let hit = view
            .hits()
            .iter()
            .find(|hit| hit.source_index == 1)
            .unwrap();
        let detail = view.inspection(hit).unwrap();
        assert_eq!(
            detail
                .display
                .iter()
                .map(|field| field.label.as_ref())
                .collect::<Vec<_>>(),
            vec!["y", "x"]
        );
        assert_eq!(detail.plot_bounds, view.plot_bounds().unwrap());
        let mut snapshot = view.snapshot().clone();
        snapshot.output.paint.insert(
            0,
            PaintCommand::Stroke {
                rect: Rect::new(21.0, 31.0, 2.0, 2.0),
                color: 0,
                width: 1.0,
            },
        );
        assert!(project_inspection_overlay(
            &mut snapshot,
            hit,
            &detail,
            &ChartTheme::default()
        ));
        let tooltip = snapshot.output.semantics.last().unwrap();
        let plot = detail.plot_bounds;
        assert!(
            tooltip.bounds.origin.x >= plot.origin.x && tooltip.bounds.origin.y >= plot.origin.y
        );
        assert!(
            tooltip.bounds.origin.x + tooltip.bounds.size.width <= plot.origin.x + plot.size.width
        );
        assert!(
            tooltip.bounds.origin.y + tooltip.bounds.size.height
                <= plot.origin.y + plot.size.height
        );
    }

    #[test]
    fn signed_band_pixels_and_inspection_match_the_analytic_strip() {
        let top0 = Point { x: 4.0, y: 12.0 };
        let top1 = Point { x: 12.0, y: 4.0 };
        let bottom0 = Point { x: 4.0, y: 8.0 };
        let bottom1 = Point { x: 12.0, y: 8.0 };
        let crossing = Point { x: 8.0, y: 8.0 };
        let bounds = Rect::new(4.0, 4.0, 8.0, 8.0);
        let command = path_command(
            band_triangles(top0, bottom0, top1, bottom1).to_vec(),
            bounds,
            0xff0077bb,
            6,
        )
        .unwrap();
        let hit = ChartHit {
            dataset: Arc::from("signed"),
            row: crate::RowKey::from("row"),
            series: None,
            bounds,
            area_band: Some(Arc::new([top0, crossing, top1, bottom1, crossing, bottom0])),
            revision: 1,
            source_index: 0,
            row_index: 0,
        };
        let contains = |x: f64, y: f64, scale: f64| {
            (4.0 * scale..=12.0 * scale).contains(&x)
                && y >= (16.0 * scale - x).min(8.0 * scale)
                && y <= (16.0 * scale - x).max(8.0 * scale)
        };
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            let mut renderer = twinkle_ui::SoftwareRenderer::new_pixel_buffer(32, 32, scale);
            renderer.render(std::slice::from_ref(&command));
            for y in 0..32 {
                for x in 0..32 {
                    let inside = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)]
                        .into_iter()
                        .filter(|(dx, dy)| contains(x as f64 + dx, y as f64 + dy, f64::from(scale)))
                        .count();
                    let expected = (255.0 * inside as f32 / 4.0).round() as u8;
                    assert_eq!(
                        renderer.pixels()[y * 32 + x].a,
                        expected,
                        "signed band pixel ({x},{y}) at {scale}"
                    );
                    let point = Point {
                        x: (x as f32 + 0.5) / scale,
                        y: (y as f32 + 0.5) / scale,
                    };
                    assert_eq!(
                        hit_distance(&hit, point) < 0.0001,
                        contains(x as f64 + 0.5, y as f64 + 0.5, f64::from(scale))
                    );
                }
            }
        }
    }

    #[test]
    fn signed_area_bands_split_at_the_interpolated_edge_crossing() {
        for baseline in [0.0, 3.0] {
            for direction in [-1.0, 1.0] {
                let top0 = Point {
                    x: 0.0,
                    y: baseline + direction * 4.0,
                };
                let bottom0 = Point {
                    x: 0.0,
                    y: baseline,
                };
                let top1 = Point {
                    x: 8.0,
                    y: baseline - direction * 4.0,
                };
                let bottom1 = Point {
                    x: 8.0,
                    y: baseline,
                };
                let triangles = band_triangles(top0, bottom0, top1, bottom1);
                assert_eq!(
                    triangles[2],
                    Point {
                        x: 4.0,
                        y: baseline
                    }
                );
                assert_eq!(triangles[3], triangles[2]);
                let area: f32 = triangles
                    .chunks_exact(3)
                    .map(|p| {
                        ((p[1].x - p[0].x) * (p[2].y - p[0].y)
                            - (p[1].y - p[0].y) * (p[2].x - p[0].x))
                            .abs()
                            * 0.5
                    })
                    .sum();
                assert_eq!(area, 16.0);
                assert_eq!(area_triangles(&[top0, top1], baseline), triangles);
            }
        }
        // A varying stack baseline must be interpolated too, not replaced
        // by zero at the sign change.
        let triangles = band_triangles(
            Point { x: 0.0, y: 8.0 },
            Point { x: 0.0, y: 2.0 },
            Point { x: 8.0, y: 2.0 },
            Point { x: 8.0, y: 4.0 },
        );
        assert_eq!(triangles[2], Point { x: 6.0, y: 3.5 });
    }

    #[test]
    fn triangle_clipping_preserves_coverage_and_stays_inside_all_four_planes() {
        let bounds = Rect::new(0.0, 0.0, 10.0, 10.0);
        let area = |triangles: &[Point]| {
            triangles
                .chunks_exact(3)
                .map(|p| {
                    ((p[1].x - p[0].x) * (p[2].y - p[0].y) - (p[1].y - p[0].y) * (p[2].x - p[0].x))
                        .abs()
                        * 0.5
                })
                .sum::<f32>()
        };
        let enclosing = [
            Point { x: -30.0, y: -30.0 },
            Point { x: 60.0, y: -30.0 },
            Point { x: -30.0, y: 60.0 },
        ];
        let clipped = clip_triangles_to_rect(&enclosing, bounds);
        assert!((area(&clipped) - 100.0).abs() < 1e-4);
        // Deterministic stress includes reversed winding and degenerate edges.
        let mut seed = 7u32;
        for _ in 0..10_000 {
            let mut point = || {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let x = (seed % 61) as f32 - 25.0;
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                Point {
                    x,
                    y: (seed % 61) as f32 - 25.0,
                }
            };
            let triangle = [point(), point(), point()];
            let clipped = clip_triangles_to_rect(&triangle, bounds);
            assert!(clipped.len() <= 15);
            for p in &clipped {
                assert!(p.x >= -1e-4 && p.x <= 10.0001 && p.y >= -1e-4 && p.y <= 10.0001);
            }
            assert!(area(&clipped) <= area(&triangle) + 0.001);
        }
    }

    #[test]
    fn tooltip_contract_honors_hint_formatting_and_space() {
        let tooltip = ChartTooltipSpec {
            fields: vec![ChartTooltipField {
                column: Arc::from("y"),
                label: Arc::from("Revenue"),
                unit: Some(Arc::from("USD")),
                format: ChartTooltipFormat::Number { decimal_places: 2 },
            }],
            include_data_table_hint: false,
        };
        let mut view = ChartView::new(model().with_tooltip(tooltip.clone()).unwrap()).unwrap();
        view.update(RetainedSurfaceInput::new(
            Rect::new(0.0, 0.0, 400.0, 240.0),
            1.0,
            Default::default(),
        ))
        .unwrap();
        let mut detail = view.inspection(&view.hits()[0]).unwrap();
        assert_eq!(detail.cells.len(), 2); // exact record is not pruned
        assert_eq!(inspection_label(&detail), "Revenue: 2.00 USD");
        detail.include_data_table_hint = true;
        assert!(inspection_label(&detail).contains("data table"));
        let text = "Revenue: 2.00 USD\nRegion: North\nMore details in data table";
        let available = Size::new(180.0, measured_text("M", -12.0, false, None).height * 2.0);
        let (fitted, size) = fit_tooltip(text, -12.0, available).unwrap();
        assert_eq!(fitted, "Revenue: 2.00 USD\n…");
        assert!(size.width <= available.width && size.height <= available.height);
        assert!(fit_tooltip(text, -12.0, Size::new(1.0, 1.0)).is_none());
        let mut invalid = tooltip;
        invalid.fields[0].format = ChartTooltipFormat::Number {
            decimal_places: 255,
        };
        assert!(model().with_tooltip(invalid).is_err());
    }

    #[test]
    fn missing_samples_do_not_join_continuous_segments() {
        let limits = ChartLimits::default();
        let dataset = Dataset::new(
            DatasetId::from("d"),
            (0..5)
                .map(|i| RowKey::from(format!("r{i}")))
                .collect::<Vec<_>>(),
            vec![
                Column::new(
                    "x",
                    (0..5)
                        .map(|i| DataValue::Number(i as f64))
                        .collect::<Vec<_>>(),
                ),
                Column::new(
                    "y",
                    vec![
                        DataValue::Number(1.0),
                        DataValue::Number(2.0),
                        DataValue::Null,
                        DataValue::Number(2.0),
                        DataValue::Number(1.0),
                    ],
                ),
            ],
        )
        .unwrap();
        let data = Arc::new(ChartData::new(1, vec![dataset], &limits).unwrap());
        let mut view =
            ChartView::new(ChartModel::new(model().spec.clone(), data, limits).unwrap()).unwrap();
        view.update(RetainedSurfaceInput::new(
            Rect::new(0.0, 0.0, 800.0, 400.0),
            1.0,
            Default::default(),
        ))
        .unwrap();
        let plot = view.plot_bounds().unwrap();
        let middle = plot.origin.x + plot.size.width * 0.5;
        let mut triangles = 0;
        for command in &view.snapshot().output.paint {
            if let PaintCommand::Path { geometry, .. } = command {
                for triangle in geometry.triangles().chunks_exact(3) {
                    triangles += 1;
                    assert!(
                        triangle.iter().all(|p| p.x < middle)
                            || triangle.iter().all(|p| p.x > middle)
                    );
                }
            }
        }
        assert!(triangles > 0);
    }

    #[test]
    fn typography_uses_logical_pixels_for_layout_at_every_device_scale() {
        let bounds = Rect::new(41.0, 73.0, 800.0, 500.0);
        let mut reference = None;
        let mut theme = ChartTheme::default();
        theme.typography.title = 27.0;
        theme.typography.control = 18.0;
        theme.typography.body = 16.0;
        theme.typography.label = 15.0;
        let mut view = ChartView::new(model().with_theme(theme).unwrap()).unwrap();
        // Reuse the retained view to exercise moving a window between displays,
        // including returning to an earlier scale without a content revision.
        for device_scale in [1.0, 1.25, 1.5, 1.75, 2.0, 3.0, 1.25, 1.0] {
            view.update(RetainedSurfaceInput::new(
                bounds,
                device_scale,
                Default::default(),
            ))
            .unwrap();
            let title = view
                .snapshot()
                .output
                .paint
                .iter()
                .find_map(|command| match command {
                    PaintCommand::Text {
                        text,
                        bounds,
                        scale,
                        ..
                    } if text == "Trend" => {
                        assert_eq!(*scale, -27.0);
                        Some(*bounds)
                    }
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                title.size.height,
                measured_text("Trend", -27.0, true, None).height
            );
            let plot = view.plot_bounds().unwrap();
            assert!(title.origin.y + title.size.height <= plot.origin.y);
            let layout = (title, plot);
            if let Some(reference) = reference {
                assert_eq!(
                    layout, reference,
                    "device scale {device_scale} changed logical layout"
                );
            } else {
                reference = Some(layout);
            }
        }
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
        assert_eq!(snapshot.output.interactions.len(), 2);
        let summary = snapshot
            .output
            .semantics
            .iter()
            .find(|item| item.role == SemanticRole::GraphicalCustomControl)
            .and_then(|item| item.state.clone())
            .unwrap();
        assert!(summary.contains("2 source rows"));
        assert!(summary.contains("exact paged data table"));
        assert!(matches!(
            view.navigate(crate::NavigationCommand::Next),
            Some(ChartEvent::Inspect { revision: 7, .. })
        ));
        assert!(matches!(
            view.navigate(crate::NavigationCommand::Select),
            Some(ChartEvent::Select { revision: 7, .. })
        ));
        assert!(view.inspect_nearest(Point { x: 380.0, y: 20.0 }).is_some());
        assert!(view.active_hit().is_some());
        let inspection = view.active_inspection().unwrap();
        assert_eq!(inspection.revision, 7);
        assert_eq!(inspection.row, RowKey::from("b"));
        assert_eq!(
            inspection
                .cells
                .iter()
                .find(|cell| cell.column.as_ref() == "y")
                .map(|cell| &cell.value),
            Some(&DataValue::Number(4.0))
        );
    }

    #[test]
    fn identical_input_reuses_accepted_output_without_model_preparation() {
        let mut view = ChartView::new(model()).unwrap();
        let input = RetainedSurfaceInput::new(
            Rect::new(0.0, 0.0, 400.0, 240.0),
            1.0,
            twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                content: 7,
                ..Default::default()
            },
        );
        view.update(input).unwrap();
        let paint = view.snapshot().output.paint.as_ptr();
        let chart_diagnostics = view.diagnostics().clone();
        let retained = view.retained_diagnostics();

        view.update(input).unwrap();
        assert_eq!(view.snapshot().output.paint.as_ptr(), paint);
        assert_eq!(view.diagnostics(), &chart_diagnostics);
        assert_eq!(
            view.retained_diagnostics().update_calls,
            retained.update_calls
        );
        assert_eq!(
            view.retained_diagnostics().unchanged_reuses,
            retained.unchanged_reuses + 1
        );
    }

    #[test]
    fn native_projection_routes_semantic_keyboard_and_controller_navigation() {
        let mut view = ChartView::new(model()).unwrap();
        let input = RetainedSurfaceInput::new(
            Rect::new(0.0, 0.0, 400.0, 240.0),
            1.0,
            twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                content: 7,
                ..Default::default()
            },
        );
        view.update(input).unwrap();
        let element = view.element("native-chart", |event| event).unwrap();
        let frame = twinkle_ui::UiFrame::layout(element, input.bounds);
        let chart = frame
            .semantic_nodes()
            .into_iter()
            .find(|node| node.role == Some(SemanticRole::GraphicalCustomControl))
            .unwrap();
        assert_eq!(
            chart.actions,
            vec![
                ActionKind::Increment,
                ActionKind::Decrement,
                ActionKind::Activate
            ]
        );
        let previous = frame
            .perform_semantic_action(
                &chart.id,
                twinkle_ui::SemanticAction::Invoke(ActionKind::Decrement),
            )
            .unwrap();
        assert_eq!(
            previous.messages,
            vec![ChartEvent::Navigate(crate::NavigationCommand::Previous)]
        );
        let next = frame
            .perform_semantic_action(
                &chart.id,
                twinkle_ui::SemanticAction::Invoke(ActionKind::Increment),
            )
            .unwrap();
        assert_eq!(
            next.messages,
            vec![ChartEvent::Navigate(crate::NavigationCommand::Next)]
        );
        assert_eq!(
            frame
                .perform_semantic_action(
                    &chart.id,
                    twinkle_ui::SemanticAction::Invoke(ActionKind::Activate),
                )
                .unwrap()
                .messages,
            vec![ChartEvent::Navigate(crate::NavigationCommand::Select)]
        );
        let table = frame
            .semantic_nodes()
            .into_iter()
            .find(|node| {
                node.role == Some(SemanticRole::Button)
                    && node.name.as_deref() == Some("View chart data table")
            })
            .unwrap();
        assert_eq!(
            frame
                .perform_semantic_action(
                    &table.id,
                    twinkle_ui::SemanticAction::Invoke(ActionKind::Activate),
                )
                .unwrap()
                .messages,
            vec![ChartEvent::TableMode(true)]
        );

        let mut state = twinkle_ui::UiStateStore::default();
        frame.handle_event(&mut state, twinkle_ui::UiEvent::FocusNext);
        frame.handle_event(&mut state, twinkle_ui::UiEvent::KeyboardNavigateActivate);
        assert_eq!(
            frame
                .handle_event(&mut state, twinkle_ui::UiEvent::KeyboardNavigateRight)
                .messages,
            vec![ChartEvent::Navigate(crate::NavigationCommand::Next)]
        );
        assert_eq!(
            frame
                .handle_event(&mut state, twinkle_ui::UiEvent::KeyboardNavigateStart)
                .messages,
            vec![ChartEvent::Navigate(crate::NavigationCommand::Home)]
        );
        assert_eq!(
            frame
                .handle_event(&mut state, twinkle_ui::UiEvent::KeyboardNavigateEnd)
                .messages,
            vec![ChartEvent::Navigate(crate::NavigationCommand::End)]
        );
        assert_eq!(
            frame
                .handle_event(&mut state, twinkle_ui::UiEvent::KeyboardNavigateUp)
                .messages,
            vec![ChartEvent::Navigate(
                crate::NavigationCommand::PreviousSeries
            )]
        );
        assert_eq!(
            frame
                .handle_event(&mut state, twinkle_ui::UiEvent::KeyboardNavigateDown)
                .messages,
            vec![ChartEvent::Navigate(crate::NavigationCommand::NextSeries)]
        );
        assert_eq!(
            frame
                .handle_event(&mut state, twinkle_ui::UiEvent::ControllerActivate)
                .messages,
            vec![ChartEvent::Navigate(crate::NavigationCommand::Select)]
        );

        view.navigate(crate::NavigationCommand::Next);
        let inspected = twinkle_ui::UiFrame::layout(
            view.element("native-chart", |event| event).unwrap(),
            input.bounds,
        );
        let tooltip = inspected
            .semantic_nodes()
            .into_iter()
            .find(|node| node.role == Some(SemanticRole::Tooltip))
            .expect("active datum has a bounded visible inspector");
        assert!(
            tooltip.name.as_deref().unwrap().contains("y: 4"),
            "{tooltip:?}"
        );
    }

    #[test]
    fn native_projection_emits_one_coordinate_bearing_pointer_message() {
        #[derive(Clone, Debug, PartialEq)]
        enum Message {
            Chart(ChartEvent),
            Pointer(Point),
            Drag(DragGesture),
            Wheel(WheelGesture),
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

        let mut view = ChartView::new(model()).unwrap();
        let bounds = Rect::new(0.0, 0.0, 400.0, 240.0);
        view.update(RetainedSurfaceInput::new(
            bounds,
            1.0,
            twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                content: 7,
                ..Default::default()
            },
        ))
        .unwrap();
        let frame = twinkle_ui::UiFrame::layout(
            view.element_with_input(
                "chart",
                Message::Chart,
                ChartInputBindings {
                    pointer_move: Some(pointer),
                    drag: Some((Message::Pointer(Point { x: 0.0, y: 0.0 }), drag)),
                    wheel: Some(wheel),
                },
            )
            .unwrap(),
            bounds,
        );
        let point = Point { x: 200.0, y: 100.0 };
        assert_eq!(
            frame
                .handle_event(
                    &mut twinkle_ui::UiStateStore::default(),
                    twinkle_ui::UiEvent::PointerMoved(point),
                )
                .messages,
            vec![Message::Pointer(point)]
        );
        let wheel_gesture = WheelGesture {
            position: point,
            delta_x: 0.0,
            delta_y: -8.0,
        };
        // The host delivers window coordinates. The reusable chart must
        // translate through its final allocation before querying retained hits.
        let shifted = twinkle_ui::UiFrame::layout(
            view.element_with_pointer("shifted-chart", Message::Chart, pointer)
                .unwrap(),
            Rect::new(73.0, 129.0, bounds.size.width, bounds.size.height),
        );
        assert_eq!(
            shifted
                .handle_event(
                    &mut twinkle_ui::UiStateStore::default(),
                    twinkle_ui::UiEvent::PointerMoved(Point {
                        x: point.x + 73.0,
                        y: point.y + 129.0,
                    }),
                )
                .messages,
            vec![Message::Pointer(point)],
        );
        let wheel_outcome = frame.handle_event(
            &mut twinkle_ui::UiStateStore::default(),
            twinkle_ui::UiEvent::Scroll {
                point,
                delta_y: -8.0,
            },
        );
        assert_eq!(wheel_outcome.messages, vec![Message::Wheel(wheel_gesture)]);
        assert_eq!(
            wheel_outcome.disposition,
            twinkle_ui::EventDisposition::Handled
        );

        let mut drag_state = twinkle_ui::UiStateStore::default();
        let started =
            frame.handle_event(&mut drag_state, twinkle_ui::UiEvent::PointerPressed(point));
        assert!(matches!(
            started.messages.as_slice(),
            [Message::Drag(DragGesture {
                phase: twinkle_ui::DragPhase::Started,
                ..
            })]
        ));
        let outside = Point { x: 500.0, y: 500.0 };
        let moved = frame.handle_event(&mut drag_state, twinkle_ui::UiEvent::PointerMoved(outside));
        assert!(moved.messages.iter().any(|message| matches!(
            message,
            Message::Drag(DragGesture { phase: twinkle_ui::DragPhase::Moved, position, .. }) if *position == outside
        )));
        let zoom_in = frame
            .semantic_nodes()
            .into_iter()
            .find(|node| node.name.as_deref() == Some("Zoom in"))
            .expect("opt-in chart exposes standard zoom controls");
        assert_eq!(
            frame
                .perform_semantic_action(
                    &zoom_in.id,
                    twinkle_ui::SemanticAction::Invoke(ActionKind::Activate),
                )
                .unwrap()
                .messages,
            vec![Message::Chart(ChartEvent::Viewport(
                crate::ViewportCommand::ZoomIn
            ))]
        );
        let policy = crate::PanZoomPolicy {
            x_extent: Some([0.0, 10.0]),
            y_extent: Some([0.0, 20.0]),
            minimum_x_span: 1.0,
            minimum_y_span: 2.0,
        };
        assert_eq!(
            view.viewport_event(crate::ViewportCommand::ZoomIn, policy),
            Some(ChartEvent::ViewportChanged(crate::Viewport {
                x: Some([1.0, 9.0]),
                y: Some([2.0, 18.0]),
            }))
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
        assert_eq!(snapshot.output.interactions.len(), 2);
        assert!(
            snapshot
                .output
                .paint
                .iter()
                .any(|command| matches!(command, PaintCommand::Fill { .. }))
        );
        let detail = view
            .inspection(view.hits().first().expect("histogram hit"))
            .expect("accepted transformed row");
        let columns = detail
            .cells
            .iter()
            .map(|cell| cell.column.as_ref())
            .collect::<Vec<_>>();
        assert!(columns.contains(&"bin"));
        assert!(columns.contains(&"count"));
        assert!(columns.contains(&"__source_count"));
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

        let horizontal = render(crate::Recipes::grouped_horizontal_bar(
            "Horizontal grouped",
            FieldRef::new("d", "category"),
            FieldRef::new("d", "series"),
            FieldRef::new("d", "value"),
        ));
        let horizontal_rects: Vec<Rect> = horizontal
            .iter()
            .filter_map(|command| match command {
                PaintCommand::Fill { rect, .. } if *rect != input.bounds => Some(*rect),
                _ => None,
            })
            .collect();
        assert_eq!(horizontal_rects.len(), 3);
        assert_ne!(horizontal_rects[0].origin.y, horizontal_rects[1].origin.y);
        assert!(horizontal_rects[0].size.width > horizontal_rects[0].size.height);

        let horizontal_stacked = render(crate::Recipes::stacked_horizontal_bar(
            "Horizontal stacked",
            FieldRef::new("d", "category"),
            FieldRef::new("d", "series"),
            FieldRef::new("d", "value"),
            true,
        ));
        let horizontal_stacked_rects: Vec<Rect> = horizontal_stacked
            .iter()
            .filter_map(|command| match command {
                PaintCommand::Fill { rect, .. } if *rect != input.bounds => Some(*rect),
                _ => None,
            })
            .collect();
        assert_eq!(horizontal_stacked_rects.len(), 3);
        assert!(
            (horizontal_stacked_rects[0].origin.x
                - (horizontal_stacked_rects[1].origin.x + horizontal_stacked_rects[1].size.width))
                .abs()
                < 0.01
                || (horizontal_stacked_rects[1].origin.x
                    - (horizontal_stacked_rects[0].origin.x
                        + horizontal_stacked_rects[0].size.width))
                    .abs()
                    < 0.01
        );
    }

    #[test]
    fn multi_series_lines_and_stacked_areas_prepare_separate_paths() {
        let limits = ChartLimits::default();
        let dataset = Dataset::new(
            DatasetId::from("d"),
            (0..6)
                .map(|index| RowKey::from(format!("r{index}")))
                .collect::<Vec<_>>(),
            vec![
                Column::new(
                    "x",
                    vec![0.0, 0.0, 1.0, 1.0, 2.0, 2.0]
                        .into_iter()
                        .map(DataValue::Number)
                        .collect::<Vec<_>>(),
                ),
                Column::new(
                    "series",
                    ["one", "two", "one", "two", "one", "two"]
                        .into_iter()
                        .map(|value| DataValue::Category(Arc::from(value)))
                        .collect::<Vec<_>>(),
                ),
                Column::new(
                    "value",
                    vec![1.0, 2.0, 2.0, 3.0, 3.0, 4.0]
                        .into_iter()
                        .map(DataValue::Number)
                        .collect::<Vec<_>>(),
                ),
            ],
        )
        .unwrap();
        let data = Arc::new(ChartData::new(11, vec![dataset], &limits).unwrap());
        let input = RetainedSurfaceInput::new(
            Rect::new(0.0, 0.0, 400.0, 240.0),
            1.0,
            twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                content: 11,
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
        let x = FieldRef::new("d", "x");
        let series = FieldRef::new("d", "series");
        let value = FieldRef::new("d", "value");
        let mut line = crate::Recipes::line("Lines", x.clone(), value.clone());
        line.layers[0].encodings.push(crate::Encoding {
            channel: Channel::Series,
            field: series.clone(),
            scale: None,
        });
        assert_eq!(
            render(line)
                .iter()
                .filter(|command| matches!(command, PaintCommand::Path { .. }))
                .count(),
            2
        );
        let mut line = crate::Recipes::line("Lines", x.clone(), value.clone());
        line.layers[0].encodings.push(crate::Encoding {
            channel: Channel::Series,
            field: series.clone(),
            scale: None,
        });
        let mut series_view =
            ChartView::new(ChartModel::new(Arc::new(line), data.clone(), limits.clone()).unwrap())
                .unwrap();
        series_view.update(input).unwrap();
        assert_eq!(series_view.hits()[0].series.as_deref(), Some("one"));
        assert_eq!(series_view.hits()[1].series.as_deref(), Some("two"));
        let area = crate::Recipes::stacked_area("Areas", x, series, value, true);
        assert_eq!(
            render(area)
                .iter()
                .filter(|command| matches!(command, PaintCommand::Path { .. }))
                .count(),
            2
        );

        let mut hit_view = ChartView::new(
            ChartModel::new(
                Arc::new(crate::Recipes::stacked_area(
                    "Hit areas",
                    FieldRef::new("d", "x"),
                    FieldRef::new("d", "series"),
                    FieldRef::new("d", "value"),
                    false,
                )),
                data.clone(),
                limits.clone(),
            )
            .unwrap(),
        )
        .unwrap();
        hit_view.update(input).unwrap();
        let plot = hit_view.plot_bounds().unwrap();
        // A raw-value line sharing the stack's y scale must not shrink the
        // stack from its total of seven to the raw column's maximum of four.
        let mut layered = crate::Recipes::stacked_area(
            "Shared transformed domain",
            FieldRef::new("d", "x"),
            FieldRef::new("d", "series"),
            FieldRef::new("d", "value"),
            false,
        );
        let mut raw_line = layered.layers[0].clone();
        raw_line.mark =
            crate::Recipes::line("Raw", FieldRef::new("d", "x"), FieldRef::new("d", "value"))
                .layers[0]
                .mark
                .clone();
        raw_line.transforms.clear();
        raw_line
            .encodings
            .retain(|encoding| encoding.channel != Channel::Y2);
        raw_line
            .encodings
            .iter_mut()
            .find(|encoding| encoding.channel == Channel::Y)
            .unwrap()
            .field
            .column = Arc::from("value");
        layered.layers.push(raw_line);
        let layered_model =
            ChartModel::new(Arc::new(layered), data.clone(), limits.clone()).unwrap();
        let mut resolved = layered_model
            .spec
            .layers
            .iter()
            .map(|layer| layered_model.resolve_layer(layer).map(Some))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(resolved[0].as_ref().unwrap().y_domain, [0.0, 7.0]);
        assert_eq!(resolved[1].as_ref().unwrap().y_domain, [1.0, 4.0]);
        unify_resolved_domains(&layered_model.spec, &mut resolved);
        assert!(
            resolved
                .iter()
                .all(|layer| layer.as_ref().unwrap().y_domain == [0.0, 7.0])
        );
        let mut layered_view = ChartView::new(layered_model).unwrap();
        layered_view.update(input).unwrap();
        for (single, layered) in hit_view.hits().iter().zip(layered_view.hits()) {
            assert_eq!(single.bounds, layered.bounds);
            assert_eq!(single.area_band, layered.area_band);
        }
        // Domain x=[0,2], y=[0,7]. The boundary is y=1+x;
        // neither sample's rectangular hit region describes its slope.
        for (x, y, expected_series, expected_row) in [
            (0.4, 1.3, "one", "r0"),
            (0.4, 1.5, "two", "r1"),
            (0.6, 1.5, "one", "r2"),
            (0.6, 1.7, "two", "r3"),
            (1.6, 2.5, "one", "r4"),
            (1.6, 2.7, "two", "r5"),
        ] {
            let point = Point {
                x: plot.origin.x + x / 2.0 * plot.size.width,
                y: plot.origin.y + (1.0 - y / 7.0) * plot.size.height,
            };
            let hit = hit_view.nearest(point).unwrap();
            assert_eq!(hit.series.as_deref(), Some(expected_series), "at {x},{y}");
            assert_eq!(hit.row, RowKey::from(expected_row), "at {x},{y}");
            assert_eq!(hit_distance(hit, point), 0.0);
        }
        let south = hit_view
            .hits()
            .iter()
            .find(|hit| hit.row == RowKey::from("r3"))
            .unwrap();
        let point_inside_south_band = Point {
            x: south.bounds.origin.x + south.bounds.size.width * 0.5,
            y: south.bounds.origin.y + south.bounds.size.height * 0.5,
        };
        assert_eq!(
            hit_view
                .nearest(point_inside_south_band)
                .map(|hit| &hit.row),
            Some(&RowKey::from("r3"))
        );

        let viewport_area = crate::Recipes::stacked_area(
            "Viewport areas",
            FieldRef::new("d", "x"),
            FieldRef::new("d", "series"),
            FieldRef::new("d", "value"),
            false,
        );
        let mut viewport_view = ChartView::new(
            ChartModel::new(Arc::new(viewport_area), data, limits)
                .unwrap()
                .with_state(crate::ChartState {
                    viewport: Some(crate::Viewport {
                        x: Some([-0.1, 2.1]),
                        y: Some([0.0, 4.0]),
                    }),
                    ..crate::ChartState::default()
                }),
        )
        .unwrap();
        let snapshot = viewport_view.update(input).unwrap();
        let paths = snapshot
            .output
            .paint
            .iter()
            .filter_map(|command| match command {
                PaintCommand::Path { geometry, .. } => Some(geometry),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(paths.len(), 2);
        let right_edges = paths
            .iter()
            .map(|path| path.bounds().origin.x + path.bounds().size.width)
            .collect::<Vec<_>>();
        assert!((right_edges[0] - right_edges[1]).abs() < 0.01);
        assert!(
            paths
                .iter()
                .all(|path| path.triangles().iter().all(|point| {
                    point.x >= input.bounds.origin.x
                        && point.x <= input.bounds.origin.x + input.bounds.size.width
                        && point.y >= input.bounds.origin.y
                        && point.y <= input.bounds.origin.y + input.bounds.size.height
                }))
        );
    }

    #[test]
    fn step_interpolation_inserts_orthogonal_corner_without_crossing_gaps() {
        let points = [Point { x: 0.0, y: 1.0 }, Point { x: 2.0, y: 3.0 }];
        assert_eq!(
            interpolate_points(&points, crate::Interpolation::StepBefore),
            vec![points[0], Point { x: 0.0, y: 3.0 }, points[1]]
        );
        assert_eq!(
            interpolate_points(&points, crate::Interpolation::StepAfter),
            vec![points[0], Point { x: 2.0, y: 1.0 }, points[1]]
        );
        assert!(interpolate_points(&[], crate::Interpolation::StepAfter).is_empty());
    }

    #[test]
    fn axes_grids_and_domain_including_annotations_remain_bounded() {
        let mut model = model();
        let mut spec = (*model.spec).clone();
        spec.axes.push(crate::Axis {
            scale: Arc::from("x"),
            title: Some(Arc::from("Time")),
            unit: Some(Arc::from("s")),
            tick_count: Some(4),
            grid: true,
        });
        spec.axes.push(crate::Axis {
            scale: Arc::from("y"),
            title: Some(Arc::from("Value")),
            unit: None,
            tick_count: Some(3),
            grid: true,
        });
        spec.annotations.push(crate::Annotation::Rule {
            scale: Arc::from("y"),
            value: 10.0,
            label: Some(Arc::from("Target")),
            include_in_domain: true,
        });
        model.replace(Arc::new(spec), model.data.clone()).unwrap();
        let mut view = ChartView::new(model).unwrap();
        let snapshot = view
            .update(RetainedSurfaceInput::new(
                Rect::new(0.0, 0.0, 180.0, 180.0),
                1.25,
                twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                    content: 12,
                    ..Default::default()
                },
            ))
            .unwrap();
        assert!(
            snapshot.output.paint.iter().any(
                |command| matches!(command, PaintCommand::Text { text, .. } if text == "Target")
            )
        );
        assert!(snapshot.output.paint.len() >= 12);
    }

    #[test]
    fn heatmap_cells_distinguish_missing_fill_and_bubbles_map_area() {
        let limits = ChartLimits::default();
        let dataset = Dataset::new(
            DatasetId::from("d"),
            vec![RowKey::from("a"), RowKey::from("b")],
            vec![
                Column::new(
                    "x",
                    vec![
                        DataValue::Category(Arc::from("left")),
                        DataValue::Category(Arc::from("right")),
                    ],
                ),
                Column::new(
                    "y",
                    vec![
                        DataValue::Category(Arc::from("top")),
                        DataValue::Category(Arc::from("top")),
                    ],
                ),
                Column::new("value", vec![DataValue::Number(0.0), DataValue::Null]),
                Column::new("size", vec![DataValue::Number(1.0), DataValue::Number(4.0)]),
            ],
        )
        .unwrap();
        let data = Arc::new(ChartData::new(13, vec![dataset], &limits).unwrap());
        let input = RetainedSurfaceInput::new(
            Rect::new(0.0, 0.0, 300.0, 180.0),
            1.0,
            twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                content: 13,
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
        let heatmap = render(crate::Recipes::heatmap(
            "Heat",
            FieldRef::new("d", "x"),
            FieldRef::new("d", "y"),
            FieldRef::new("d", "value"),
        ));
        assert!(heatmap.iter().any(|command| matches!(command, PaintCommand::Fill { color, .. } if *color == ChartTheme::default().missing)));
        assert!(
            heatmap
                .iter()
                .any(|command| matches!(command, PaintCommand::Stroke { .. }))
        );
        let bubbles = render(crate::Recipes::bubble(
            "Bubbles",
            FieldRef::new("d", "size"),
            FieldRef::new("d", "size"),
            FieldRef::new("d", "size"),
        ));
        let radii: Vec<f32> = bubbles
            .iter()
            .filter_map(|command| match command {
                PaintCommand::RoundedFill { radius, .. } if *radius != 4.0 => Some(*radius),
                _ => None,
            })
            .collect();
        assert_eq!(radii, vec![5.0, 10.0]);
    }

    #[test]
    fn rejected_replacement_preserves_accepted_paint_hits_and_revision() {
        let model = model();
        let accepted_data = model.data.clone();
        let mut view = ChartView::new(model).unwrap();
        let input = RetainedSurfaceInput::new(
            Rect::new(0.0, 0.0, 300.0, 180.0),
            1.0,
            twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                content: 7,
                ..Default::default()
            },
        );
        view.update(input).unwrap();
        let paint = view.snapshot().output.paint.clone();
        let hits = view.hits().to_vec();
        let accepted_inspection = view.inspection(&hits[0]).unwrap();
        let invalid = Arc::new(crate::Recipes::line(
            "Invalid",
            FieldRef::new("d", "missing"),
            FieldRef::new("d", "y"),
        ));
        assert!(
            view.replace_and_update(
                invalid,
                accepted_data,
                RetainedSurfaceInput::new(
                    input.bounds,
                    1.0,
                    twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                        content: 8,
                        ..Default::default()
                    }
                )
            )
            .is_err()
        );
        assert_eq!(view.snapshot().output.paint, paint);
        assert_eq!(view.hits(), hits);
        assert_eq!(view.inspection(&hits[0]), Some(accepted_inspection));
        assert_eq!(view.snapshot().input.unwrap().revisions.content, 7);
    }

    #[test]
    fn theme_updates_are_transactional_and_preserve_active_identity() {
        let mut view = ChartView::new(model()).unwrap();
        let input = RetainedSurfaceInput::new(
            Rect::new(0.0, 0.0, 300.0, 180.0),
            1.0,
            twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                content: 1,
                ..Default::default()
            },
        );
        view.update(input).unwrap();
        view.navigate(crate::NavigationCommand::Next);
        let active = view.active_hit().unwrap().row.clone();

        let dark_input = RetainedSurfaceInput::new(
            input.bounds,
            1.0,
            twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                content: 1,
                theme: 1,
                ..Default::default()
            },
        );
        view.update_theme(ChartTheme::dark(), dark_input).unwrap();
        assert_eq!(view.active_hit().unwrap().row, active);
        assert!(view.snapshot().output.paint.iter().any(
            |command| matches!(command, PaintCommand::Fill { color, .. } if *color == ChartTheme::dark().background)
        ));

        let accepted = view.snapshot().clone();
        let mut invalid = ChartTheme::default();
        invalid.categorical = Arc::from([]);
        assert!(view.update_theme(invalid, dark_input).is_err());
        assert_eq!(view.snapshot(), &accepted);
        assert_eq!(view.active_hit().unwrap().row, active);
    }

    #[test]
    fn retained_transitions_match_forced_fresh_preparation() {
        fn data(revision: u64, rows: &[(&str, f64, f64, &str)]) -> Arc<ChartData> {
            let limits = ChartLimits::default();
            Arc::new(
                ChartData::new(
                    revision,
                    vec![
                        Dataset::new(
                            DatasetId::from("oracle"),
                            rows.iter()
                                .map(|row| RowKey::from(row.0))
                                .collect::<Vec<_>>(),
                            vec![
                                Column::new(
                                    "x",
                                    rows.iter()
                                        .map(|row| DataValue::Number(row.1))
                                        .collect::<Vec<_>>(),
                                ),
                                Column::new(
                                    "y",
                                    rows.iter()
                                        .map(|row| DataValue::Number(row.2))
                                        .collect::<Vec<_>>(),
                                ),
                                Column::new(
                                    "series",
                                    rows.iter()
                                        .map(|row| DataValue::Category(Arc::from(row.3)))
                                        .collect::<Vec<_>>(),
                                ),
                            ],
                        )
                        .unwrap(),
                    ],
                    &limits,
                )
                .unwrap(),
            )
        }
        fn spec() -> Arc<ChartSpec> {
            let mut spec = Recipes::line(
                "Oracle",
                FieldRef::new("oracle", "x"),
                FieldRef::new("oracle", "y"),
            );
            spec.layers[0].encodings.push(Encoding {
                channel: Channel::Series,
                field: FieldRef::new("oracle", "series"),
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
            spec.legends.push(crate::Legend {
                scale: Arc::from("series"),
                title: Some(Arc::from("Series")),
                toggleable: true,
            });
            Arc::new(spec)
        }
        fn assert_fresh_parity(view: &ChartView, input: RetainedSurfaceInput) {
            let accepted = view.host.model();
            let model = ChartModel::new(
                Arc::clone(&accepted.spec),
                Arc::clone(&accepted.data),
                accepted.limits.clone(),
            )
            .unwrap()
            .with_theme(accepted.theme.clone())
            .unwrap()
            .with_state(accepted.state.clone());
            let mut fresh = ChartView::new(model).unwrap();
            fresh.update(input).unwrap();
            assert_eq!(view.snapshot().output.paint, fresh.snapshot().output.paint);
            assert_eq!(
                view.snapshot().output.interactions,
                fresh.snapshot().output.interactions
            );
            assert_eq!(
                view.snapshot().output.semantics,
                fresh.snapshot().output.semantics
            );
            assert_eq!(
                view.snapshot().output.measured,
                fresh.snapshot().output.measured
            );
            assert_eq!(view.hits(), fresh.hits());
        }
        let initial_data = data(
            1,
            &[
                ("a", 0.0, 1.0, "one"),
                ("b", 1.0, 3.0, "two"),
                ("c", 2.0, 2.0, "one"),
            ],
        );
        let spec = spec();
        let mut view = ChartView::new(
            ChartModel::new(Arc::clone(&spec), initial_data, ChartLimits::default()).unwrap(),
        )
        .unwrap();
        let input = |bounds, scale, content, theme, font| {
            RetainedSurfaceInput::new(
                bounds,
                scale,
                twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                    content,
                    theme,
                    font,
                    ..Default::default()
                },
            )
        };

        let mut accepted_input = input(Rect::new(0.0, 0.0, 320.0, 180.0), 1.0, 1, 0, 0);
        view.update(accepted_input).unwrap();
        assert_fresh_parity(&view, accepted_input);

        accepted_input = input(Rect::new(0.0, 0.0, 640.0, 300.0), 1.25, 1, 0, 1);
        view.update(accepted_input).unwrap();
        assert_fresh_parity(&view, accepted_input);

        let state = ChartState {
            visible_series: Some([Arc::from("two")].into_iter().collect()),
            viewport: Some(crate::Viewport {
                x: Some([0.25, 1.75]),
                y: Some([0.0, 4.0]),
            }),
            ..Default::default()
        };
        accepted_input = input(accepted_input.bounds, 1.25, 2, 0, 1);
        view.update_state(state, accepted_input).unwrap();
        assert_fresh_parity(&view, accepted_input);

        accepted_input = input(accepted_input.bounds, 1.25, 2, 1, 1);
        view.update_theme(ChartTheme::dark(), accepted_input)
            .unwrap();
        assert_fresh_parity(&view, accepted_input);

        let replacement = data(
            3,
            &[
                ("c", 2.0, 4.0, "one"),
                ("b", 1.0, 2.0, "two"),
                ("d", 3.0, 5.0, "two"),
            ],
        );
        accepted_input = input(accepted_input.bounds, 1.25, 3, 1, 1);
        view.replace_and_update(spec, replacement, accepted_input)
            .unwrap();
        assert_fresh_parity(&view, accepted_input);
    }

    #[test]
    fn structural_cap_replacement_matrix_preserves_then_contracts_accepted_state() {
        fn data_with(rows: usize, extra_column: bool, revision: u64) -> Arc<ChartData> {
            let keys = (0..rows)
                .map(|index| RowKey::from(format!("r{index}")))
                .collect::<Vec<_>>();
            let mut columns = vec![
                Column::new(
                    "x",
                    (0..rows)
                        .map(|index| DataValue::Number(index as f64))
                        .collect::<Vec<_>>(),
                ),
                Column::new(
                    "y",
                    (0..rows)
                        .map(|index| DataValue::Number((index + 1) as f64))
                        .collect::<Vec<_>>(),
                ),
            ];
            if extra_column {
                columns.push(Column::new(
                    "extra",
                    (0..rows)
                        .map(|index| DataValue::Number(index as f64))
                        .collect::<Vec<_>>(),
                ));
            }
            let dataset = Dataset::new(DatasetId::from("d"), keys, columns).unwrap();
            Arc::new(ChartData::new(revision, vec![dataset], &ChartLimits::default()).unwrap())
        }

        fn base_spec() -> Arc<ChartSpec> {
            Arc::new(Recipes::line(
                "Trend",
                FieldRef::new("d", "x"),
                FieldRef::new("d", "y"),
            ))
        }

        fn assert_atomic_rejection(
            limits: ChartLimits,
            candidate_spec: Arc<ChartSpec>,
            candidate_data: Arc<ChartData>,
            revision: u64,
        ) {
            let accepted_data = data_with(2, false, 7);
            let mut view = ChartView::new(
                ChartModel::new(base_spec(), accepted_data.clone(), limits.clone()).unwrap(),
            )
            .unwrap();
            let accepted_input = RetainedSurfaceInput::new(
                Rect::new(0.0, 0.0, 400.0, 240.0),
                1.0,
                twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                    content: 7,
                    ..Default::default()
                },
            );
            view.update(accepted_input).unwrap();
            let snapshot = view.snapshot().clone();
            let hits = view.hits().to_vec();
            let inspection = view.inspection(&hits[0]).unwrap();

            let rejected = RetainedSurfaceInput::new(
                accepted_input.bounds,
                1.0,
                twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                    content: revision,
                    ..Default::default()
                },
            );
            assert!(
                view.replace_and_update(candidate_spec, candidate_data, rejected)
                    .is_err()
            );
            assert_eq!(view.snapshot(), &snapshot);
            assert_eq!(view.hits(), hits);
            assert_eq!(view.inspection(&hits[0]), Some(inspection));

            let replacement = data_with(1, false, revision + 1);
            view.replace_and_update(
                base_spec(),
                replacement,
                RetainedSurfaceInput::new(
                    accepted_input.bounds,
                    1.0,
                    twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                        content: revision + 1,
                        ..Default::default()
                    },
                ),
            )
            .unwrap();
            assert_eq!(view.hits().len(), 1);
            view.detach();
            assert_eq!(view.retained_diagnostics().output_retained_bytes, 0);
            view.destroy();
            assert!(view.hits().is_empty());
        }

        let mut row_limits = ChartLimits::default();
        row_limits.max_rows = 2;
        assert_atomic_rejection(row_limits, base_spec(), data_with(3, false, 20), 20);

        let mut column_limits = ChartLimits::default();
        column_limits.max_columns = 2;
        assert_atomic_rejection(column_limits, base_spec(), data_with(2, true, 21), 21);

        let mut layer_limits = ChartLimits::default();
        layer_limits.max_layers = 1;
        let mut layered = (*base_spec()).clone();
        layered.layers.push(layered.layers[0].clone());
        assert_atomic_rejection(layer_limits, Arc::new(layered), data_with(2, false, 22), 22);

        let mut transform_limits = ChartLimits::default();
        transform_limits.max_transforms = 0;
        let mut transformed = (*base_spec()).clone();
        transformed.layers[0]
            .transforms
            .push(crate::Transform::Sort {
                field: "x".into(),
                descending: false,
            });
        assert_atomic_rejection(
            transform_limits,
            Arc::new(transformed),
            data_with(2, false, 23),
            23,
        );

        let mut tick_limits = ChartLimits::default();
        tick_limits.max_ticks_per_axis = 1;
        let mut guided = (*base_spec()).clone();
        guided.axes.push(crate::Axis {
            scale: Arc::from("x"),
            title: Some(Arc::from("X")),
            unit: None,
            tick_count: Some(2),
            grid: true,
        });
        assert_atomic_rejection(tick_limits, Arc::new(guided), data_with(2, false, 24), 24);

        let mut annotation_limits = ChartLimits::default();
        annotation_limits.max_annotations = 0;
        let mut annotated = (*base_spec()).clone();
        annotated.annotations.push(crate::Annotation::Text {
            text: Arc::from("note"),
            x: 0.0,
            y: 0.0,
        });
        assert_atomic_rejection(
            annotation_limits,
            Arc::new(annotated),
            data_with(2, false, 25),
            25,
        );

        let mut label_limits = ChartLimits::default();
        label_limits.max_label_bytes = 24;
        let mut long_label = (*base_spec()).clone();
        long_label.title = Arc::from("x".repeat(25));
        assert_atomic_rejection(
            label_limits,
            Arc::new(long_label),
            data_with(2, false, 26),
            26,
        );

        let mut total_label_limits = ChartLimits::default();
        total_label_limits.max_total_label_bytes =
            base_spec().title.len() + base_spec().summary.len();
        let mut total_labels = (*base_spec()).clone();
        total_labels.annotations.push(crate::Annotation::Text {
            text: Arc::from("extra"),
            x: 0.0,
            y: 0.0,
        });
        assert_atomic_rejection(
            total_label_limits,
            Arc::new(total_labels),
            data_with(2, false, 27),
            27,
        );
    }

    #[test]
    fn peak_budget_rejects_candidate_before_any_state_is_published() {
        let mut constrained = ChartLimits::default();
        constrained.max_peak_bytes = 1;
        let base = model();
        let preflight = ChartModel::new(base.spec.clone(), base.data.clone(), constrained.clone())
            .unwrap()
            .prepare(RetainedSurfaceInput::new(
                Rect::new(0.0, 0.0, 300.0, 180.0),
                1.0,
                twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                    content: 7,
                    ..Default::default()
                },
            ))
            .unwrap_err();
        assert!(matches!(
            preflight,
            ChartError::Limit {
                resource: "chart preflight peak bytes",
                ..
            }
        ));
        let mut view = ChartView::new(
            ChartModel::new(base.spec.clone(), base.data.clone(), constrained).unwrap(),
        )
        .unwrap();
        let input = RetainedSurfaceInput::new(
            Rect::new(0.0, 0.0, 300.0, 180.0),
            1.0,
            twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                content: 7,
                ..Default::default()
            },
        );
        let error = view.update(input).unwrap_err();
        assert!(matches!(
            error,
            ChartError::Invalid {
                stage: crate::ChartStage::Geometry,
                ..
            }
        ));
        assert!(view.hits().is_empty());
        assert!(view.active_inspection().is_none());
        assert!(
            view.snapshot().input.is_some(),
            "bounded fallback is visible"
        );

        let mut accepted = ChartView::new(model()).unwrap();
        accepted.update(input).unwrap();
        assert!(accepted.diagnostics().transient_bytes > 0);
        assert_eq!(
            accepted.diagnostics().transient_bytes,
            accepted
                .diagnostics()
                .coordinate_scratch_bytes
                .saturating_add(accepted.diagnostics().geometry_scratch_bytes)
                .saturating_add(accepted.diagnostics().transform_scratch_bytes)
        );
        assert_eq!(
            accepted.diagnostics().peak_bytes,
            accepted
                .diagnostics()
                .steady_bytes
                .saturating_add(accepted.diagnostics().transient_bytes)
        );

        let base = model();
        let mut geometry_limits = ChartLimits::default();
        geometry_limits.max_geometry_vertices = 1;
        let geometry = ChartModel::new(base.spec.clone(), base.data.clone(), geometry_limits)
            .unwrap()
            .prepare(input)
            .unwrap_err();
        assert!(matches!(
            geometry,
            ChartError::Limit {
                resource: "prepared geometry vertices",
                ..
            }
        ));

        let mut steady_limits = ChartLimits::default();
        steady_limits.max_steady_bytes = 1;
        let steady = ChartModel::new(base.spec.clone(), base.data.clone(), steady_limits)
            .unwrap()
            .prepare(input)
            .unwrap_err();
        assert!(matches!(
            steady,
            ChartError::Limit {
                resource: "chart steady bytes",
                ..
            }
        ));
    }

    #[test]
    fn pie_labels_exact_percentages_and_rejects_negative_values() {
        let limits = ChartLimits::default();
        let make_data = |values: Vec<DataValue>| {
            let dataset = Dataset::new(
                DatasetId::from("d"),
                vec![RowKey::from("a"), RowKey::from("b")],
                vec![
                    Column::new(
                        "category",
                        vec![
                            DataValue::Category(Arc::from("Alpha")),
                            DataValue::Category(Arc::from("Beta")),
                        ],
                    ),
                    Column::new("value", values),
                ],
            )
            .unwrap();
            Arc::new(ChartData::new(14, vec![dataset], &limits).unwrap())
        };
        let spec = Arc::new(crate::Recipes::pie(
            "Share",
            FieldRef::new("d", "category"),
            FieldRef::new("d", "value"),
        ));
        let input = RetainedSurfaceInput::new(
            Rect::new(0.0, 0.0, 320.0, 220.0),
            1.0,
            twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                content: 14,
                ..Default::default()
            },
        );
        let mut view = ChartView::new(
            ChartModel::new(
                spec.clone(),
                make_data(vec![DataValue::Number(1.0), DataValue::Number(3.0)]),
                limits.clone(),
            )
            .unwrap(),
        )
        .unwrap();
        let snapshot = view.update(input).unwrap();
        assert!(snapshot.output.paint.iter().any(|command| matches!(command, PaintCommand::Text { text, .. } if text == "Alpha: 1 (25.0%)")));
        assert_eq!(view.hits()[0].series.as_deref(), Some("Alpha"));
        let mut invalid = ChartView::new(
            ChartModel::new(
                spec,
                make_data(vec![DataValue::Number(-1.0), DataValue::Number(3.0)]),
                limits,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(invalid.update(input).is_err());
    }

    #[test]
    fn controlled_legends_filter_series_without_losing_controls_or_original_domain() {
        let limits = ChartLimits::default();
        let dataset = Dataset::new(
            DatasetId::from("d"),
            (0..4)
                .map(|index| RowKey::from(format!("r{index}")))
                .collect::<Vec<_>>(),
            vec![
                Column::new(
                    "x",
                    vec![0.0, 1.0, 0.0, 1.0]
                        .into_iter()
                        .map(DataValue::Number)
                        .collect::<Vec<_>>(),
                ),
                Column::new(
                    "series",
                    ["one", "one", "two", "two"]
                        .into_iter()
                        .map(|value| DataValue::Category(Arc::from(value)))
                        .collect::<Vec<_>>(),
                ),
                Column::new(
                    "value",
                    vec![1.0, 2.0, 50.0, 100.0]
                        .into_iter()
                        .map(DataValue::Number)
                        .collect::<Vec<_>>(),
                ),
            ],
        )
        .unwrap();
        let data = Arc::new(ChartData::new(15, vec![dataset], &limits).unwrap());
        let mut spec = Recipes::line(
            "Controlled",
            FieldRef::new("d", "x"),
            FieldRef::new("d", "value"),
        );
        spec.layers[0].encodings.push(crate::Encoding {
            channel: Channel::Series,
            field: FieldRef::new("d", "series"),
            scale: Some(Arc::from("series")),
        });
        spec.scales.push(crate::Scale {
            name: Arc::from("series"),
            kind: crate::ScaleKind::Color,
            domain: None,
            clamp: false,
            reversed: false,
            unit: None,
        });
        spec.legends.push(crate::Legend {
            scale: Arc::from("series"),
            title: Some(Arc::from("Series")),
            toggleable: true,
        });
        let mut view =
            ChartView::new(ChartModel::new(Arc::new(spec), data, limits).unwrap()).unwrap();
        let input = |content| {
            RetainedSurfaceInput::new(
                Rect::new(0.0, 0.0, 400.0, 240.0),
                1.0,
                twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                    content,
                    ..Default::default()
                },
            )
        };
        let initial = view.update(input(15)).unwrap().clone();
        let first_geometry = initial
            .output
            .paint
            .iter()
            .find_map(|command| match command {
                PaintCommand::Path { geometry, .. } => Some(geometry.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            initial
                .output
                .paint
                .iter()
                .filter(|command| matches!(command, PaintCommand::Path { .. }))
                .count(),
            2
        );
        assert_eq!(initial.output.interactions.len(), 4);

        let one = ChartState {
            visible_series: Some([Arc::from("one")].into_iter().collect()),
            ..Default::default()
        };
        let filtered = view.update_state(one, input(16)).unwrap();
        let geometries = filtered
            .output
            .paint
            .iter()
            .filter_map(|command| match command {
                PaintCommand::Path { geometry, .. } => Some(geometry),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(geometries, vec![&first_geometry]);
        assert_eq!(view.hits().len(), 2);

        let none = ChartState {
            visible_series: Some(Default::default()),
            ..Default::default()
        };
        let hidden = view.update_state(none, input(17)).unwrap();
        assert!(
            !hidden
                .output
                .paint
                .iter()
                .any(|command| matches!(command, PaintCommand::Path { .. }))
        );
        assert_eq!(hidden.output.interactions.len(), 4);
        assert_eq!(hidden.input.unwrap().revisions.content, 17);
        assert_eq!(view.hits().len(), 0);
    }

    #[test]
    fn facets_are_bounded_stable_and_support_shared_or_independent_domains() {
        let limits = ChartLimits::default();
        let dataset = Dataset::new(
            DatasetId::from("d"),
            (0..4)
                .map(|index| RowKey::from(format!("r{index}")))
                .collect::<Vec<_>>(),
            vec![
                Column::new(
                    "x",
                    vec![0.0, 1.0, 0.0, 1.0]
                        .into_iter()
                        .map(DataValue::Number)
                        .collect::<Vec<_>>(),
                ),
                Column::new(
                    "value",
                    vec![1.0, 2.0, 100.0, 200.0]
                        .into_iter()
                        .map(DataValue::Number)
                        .collect::<Vec<_>>(),
                ),
                Column::new(
                    "region",
                    ["small", "small", "large", "large"]
                        .into_iter()
                        .map(|value| DataValue::Category(Arc::from(value)))
                        .collect::<Vec<_>>(),
                ),
            ],
        )
        .unwrap();
        let data = Arc::new(ChartData::new(16, vec![dataset], &limits).unwrap());
        let input = RetainedSurfaceInput::new(
            Rect::new(0.0, 0.0, 600.0, 260.0),
            1.0,
            twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                content: 16,
                ..Default::default()
            },
        );
        let render = |data: Arc<ChartData>, policy| {
            let mut spec = Recipes::line(
                "Facets",
                FieldRef::new("d", "x"),
                FieldRef::new("d", "value"),
            );
            spec.facet = Some(crate::Facet {
                row: None,
                column: Some(FieldRef::new("d", "region")),
                scales: policy,
            });
            let mut view =
                ChartView::new(ChartModel::new(Arc::new(spec), data, limits.clone()).unwrap())
                    .unwrap();
            view.update(input).unwrap();
            let first_plot = view.inspection(&view.hits()[0]).unwrap().plot_bounds;
            let second_plot = view.inspection(&view.hits()[2]).unwrap().plot_bounds;
            assert!(first_plot.origin.x + first_plot.size.width <= second_plot.origin.x);
            for hit in view.hits() {
                let detail = view.inspection(hit).unwrap();
                let mut snapshot = view.snapshot().clone();
                assert!(project_inspection_overlay(
                    &mut snapshot,
                    hit,
                    &detail,
                    &ChartTheme::default()
                ));
                let tooltip = snapshot.output.semantics.last().unwrap().bounds;
                assert!(tooltip.origin.x >= detail.plot_bounds.origin.x);
                assert!(
                    tooltip.origin.x + tooltip.size.width
                        <= detail.plot_bounds.origin.x + detail.plot_bounds.size.width
                );
            }
            (
                view.hits().to_vec(),
                view.snapshot().output.semantics.clone(),
            )
        };
        let (shared, semantics) = render(data.clone(), crate::FacetScalePolicy::Shared);
        assert_eq!(shared.len(), 4);
        assert_eq!(
            semantics
                .iter()
                .filter(|semantic| semantic.role == SemanticRole::Group)
                .count(),
            2
        );
        assert!(semantics.iter().any(|semantic| {
            semantic.id.as_str().starts_with("facet/")
                && semantic.id.as_str().ends_with("/chart")
                && semantic.label.as_deref() == Some("small facet")
        }));
        let shared_small_delta = (hit_center_y(&shared[0]) - hit_center_y(&shared[1])).abs();
        let shared_large_delta = (hit_center_y(&shared[2]) - hit_center_y(&shared[3])).abs();
        assert!(shared_small_delta * 20.0 < shared_large_delta);

        let (independent, _) = render(data.clone(), crate::FacetScalePolicy::Independent);
        let independent_small_delta =
            (hit_center_y(&independent[0]) - hit_center_y(&independent[1])).abs();
        let independent_large_delta =
            (hit_center_y(&independent[2]) - hit_center_y(&independent[3])).abs();
        assert!((independent_small_delta - independent_large_delta).abs() < 0.01);

        let reordered = Dataset::new(
            DatasetId::from("d"),
            ["r2", "r3", "r0", "r1"]
                .into_iter()
                .map(RowKey::from)
                .collect::<Vec<_>>(),
            vec![
                Column::new(
                    "x",
                    [0.0, 1.0, 0.0, 1.0]
                        .into_iter()
                        .map(DataValue::Number)
                        .collect::<Vec<_>>(),
                ),
                Column::new(
                    "value",
                    [100.0, 200.0, 1.0, 2.0]
                        .into_iter()
                        .map(DataValue::Number)
                        .collect::<Vec<_>>(),
                ),
                Column::new(
                    "region",
                    ["large", "large", "small", "small"]
                        .into_iter()
                        .map(|value| DataValue::Category(Arc::from(value)))
                        .collect::<Vec<_>>(),
                ),
            ],
        )
        .unwrap();
        let reordered_data = Arc::new(ChartData::new(17, vec![reordered], &limits).unwrap());
        let (_, reordered_semantics) = render(reordered_data, crate::FacetScalePolicy::Shared);
        let group_ids = |items: &[twinkle_ui::retained_surface::RetainedSurfaceSemantic]| {
            items
                .iter()
                .filter(|semantic| semantic.role == SemanticRole::Group)
                .map(|semantic| semantic.id.as_str().to_owned())
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert_eq!(group_ids(&semantics), group_ids(&reordered_semantics));

        let mut constrained = limits;
        constrained.max_facets = 1;
        let mut spec = Recipes::line(
            "Too many",
            FieldRef::new("d", "x"),
            FieldRef::new("d", "value"),
        );
        spec.facet = Some(crate::Facet {
            row: None,
            column: Some(FieldRef::new("d", "region")),
            scales: crate::FacetScalePolicy::Shared,
        });
        assert!(ChartModel::new(Arc::new(spec), data, constrained).is_err());
    }

    #[test]
    fn declared_log_scale_and_controlled_viewport_drive_native_coordinates() {
        let limits = ChartLimits::default();
        let dataset = Dataset::new(
            DatasetId::from("d"),
            ["a", "b", "c"]
                .into_iter()
                .map(RowKey::from)
                .collect::<Vec<_>>(),
            vec![
                Column::new(
                    "x",
                    [1.0, 10.0, 100.0]
                        .into_iter()
                        .map(DataValue::Number)
                        .collect::<Vec<_>>(),
                ),
                Column::new(
                    "y",
                    [1.0, 2.0, 3.0]
                        .into_iter()
                        .map(DataValue::Number)
                        .collect::<Vec<_>>(),
                ),
            ],
        )
        .unwrap();
        let data = Arc::new(ChartData::new(18, vec![dataset], &limits).unwrap());
        let mut spec = Recipes::line("Log", FieldRef::new("d", "x"), FieldRef::new("d", "y"));
        spec.scales
            .iter_mut()
            .find(|scale| scale.name.as_ref() == "x")
            .unwrap()
            .kind = ScaleKind::Log { base: 10.0 };
        let mut view =
            ChartView::new(ChartModel::new(Arc::new(spec), data.clone(), limits.clone()).unwrap())
                .unwrap();
        let input = |content| {
            RetainedSurfaceInput::new(
                Rect::new(0.0, 0.0, 400.0, 240.0),
                1.0,
                twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                    content,
                    ..Default::default()
                },
            )
        };
        view.update(input(18)).unwrap();
        let x = view.hits().iter().map(hit_center_x).collect::<Vec<_>>();
        assert!(((x[1] - x[0]) - (x[2] - x[1])).abs() < 0.01);

        let state = ChartState {
            viewport: Some(crate::Viewport {
                x: Some([1.0, 10.0]),
                y: None,
            }),
            ..Default::default()
        };
        view.update_state(state, input(19)).unwrap();
        assert_eq!(view.hits().len(), 2);
        assert!(view.hits().iter().all(|hit| hit_center_x(hit) <= 382.0));

        let invalid = ChartState {
            viewport: Some(crate::Viewport {
                x: Some([10.0, 1.0]),
                y: None,
            }),
            ..Default::default()
        };
        let accepted = view.snapshot().clone();
        assert!(view.update_state(invalid, input(20)).is_err());
        assert_eq!(view.snapshot(), &accepted);

        let invalid_dataset = Dataset::new(
            DatasetId::from("d"),
            [RowKey::from("zero"), RowKey::from("one")],
            vec![
                Column::new("x", vec![DataValue::Number(0.0), DataValue::Number(1.0)]),
                Column::new("y", vec![DataValue::Number(1.0), DataValue::Number(2.0)]),
            ],
        )
        .unwrap();
        let invalid_data = Arc::new(ChartData::new(20, vec![invalid_dataset], &limits).unwrap());
        let mut invalid_spec = Recipes::line(
            "Invalid log",
            FieldRef::new("d", "x"),
            FieldRef::new("d", "y"),
        );
        invalid_spec
            .scales
            .iter_mut()
            .find(|scale| scale.name.as_ref() == "x")
            .unwrap()
            .kind = ScaleKind::Log { base: 10.0 };
        let mut invalid_view =
            ChartView::new(ChartModel::new(Arc::new(invalid_spec), invalid_data, limits).unwrap())
                .unwrap();
        assert!(invalid_view.update(input(20)).is_err());
    }

    #[test]
    fn layered_combo_resolves_named_scales_once_for_every_layer() {
        let limits = ChartLimits::default();
        let dataset = Dataset::new(
            DatasetId::from("d"),
            [RowKey::from("a"), RowKey::from("b")],
            vec![
                Column::new("x", vec![DataValue::Number(0.0), DataValue::Number(1.0)]),
                Column::new("bars", vec![DataValue::Number(1.0), DataValue::Number(2.0)]),
                Column::new(
                    "line",
                    vec![DataValue::Number(100.0), DataValue::Number(200.0)],
                ),
            ],
        )
        .unwrap();
        let data = Arc::new(ChartData::new(21, vec![dataset], &limits).unwrap());
        let mut spec = Recipes::bar("Combo", FieldRef::new("d", "x"), FieldRef::new("d", "bars"));
        spec.layers.push(
            crate::Layer::new(Mark::Line {
                interpolation: crate::Interpolation::Straight,
                points: true,
            })
            .encode(crate::Encoding {
                channel: Channel::X,
                field: FieldRef::new("d", "x"),
                scale: Some(Arc::from("x")),
            })
            .encode(crate::Encoding {
                channel: Channel::Y,
                field: FieldRef::new("d", "line"),
                scale: Some(Arc::from("y")),
            }),
        );
        spec.annotations.push(crate::Annotation::Band {
            scale: Arc::from("y"),
            start: 90.0,
            end: 110.0,
            label: Some(Arc::from("Target")),
            include_in_domain: false,
        });
        let mut view =
            ChartView::new(ChartModel::new(Arc::new(spec), data, limits).unwrap()).unwrap();
        view.update(RetainedSurfaceInput::new(
            Rect::new(0.0, 0.0, 400.0, 240.0),
            1.0,
            twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                content: 21,
                ..Default::default()
            },
        ))
        .unwrap();
        assert_eq!(view.hits().len(), 4);
        assert!((hit_center_y(&view.hits()[0]) - hit_center_y(&view.hits()[2])).abs() > 50.0);
        assert!(
            view.snapshot().output.paint.iter().any(
                |command| matches!(command, PaintCommand::Text { text, .. } if text == "Target")
            )
        );
    }

    #[test]
    fn independent_y_scales_use_deterministic_left_and_right_guides() {
        let limits = ChartLimits::default();
        let dataset = Dataset::new(
            DatasetId::from("d"),
            [RowKey::from("a"), RowKey::from("b")],
            vec![
                Column::new("x", vec![DataValue::Number(0.0), DataValue::Number(1.0)]),
                Column::new("left", vec![DataValue::Number(1.0), DataValue::Number(2.0)]),
                Column::new(
                    "right",
                    vec![DataValue::Number(100.0), DataValue::Number(200.0)],
                ),
            ],
        )
        .unwrap();
        let data = Arc::new(ChartData::new(23, vec![dataset], &limits).unwrap());
        let mut spec = Recipes::line(
            "Two axes",
            FieldRef::new("d", "x"),
            FieldRef::new("d", "left"),
        );
        spec.layers[0]
            .encodings
            .iter_mut()
            .find(|encoding| encoding.channel == Channel::Y)
            .unwrap()
            .scale = Some(Arc::from("left"));
        spec.scales
            .iter_mut()
            .find(|scale| scale.name.as_ref() == "y")
            .unwrap()
            .name = Arc::from("left");
        spec.scales.push(Scale {
            name: Arc::from("right"),
            kind: ScaleKind::Linear,
            domain: None,
            clamp: false,
            reversed: true,
            unit: Some(Arc::from("percent")),
        });
        spec.layers.push(
            crate::Layer::new(Mark::Line {
                interpolation: crate::Interpolation::Straight,
                points: true,
            })
            .encode(crate::Encoding {
                channel: Channel::X,
                field: FieldRef::new("d", "x"),
                scale: Some(Arc::from("x")),
            })
            .encode(crate::Encoding {
                channel: Channel::Y,
                field: FieldRef::new("d", "right"),
                scale: Some(Arc::from("right")),
            }),
        );
        spec.axes.extend([
            crate::Axis {
                scale: Arc::from("left"),
                title: Some(Arc::from("Revenue")),
                unit: Some(Arc::from("USD")),
                tick_count: Some(3),
                grid: true,
            },
            crate::Axis {
                scale: Arc::from("right"),
                title: Some(Arc::from("Rate")),
                unit: Some(Arc::from("percent")),
                tick_count: Some(3),
                grid: false,
            },
        ]);
        let valid_spec = spec.clone();
        let mut view =
            ChartView::new(ChartModel::new(Arc::new(spec), data.clone(), limits.clone()).unwrap())
                .unwrap();
        view.update(RetainedSurfaceInput::new(
            Rect::new(0.0, 0.0, 400.0, 240.0),
            1.0,
            twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                content: 23,
                ..Default::default()
            },
        ))
        .unwrap();
        let titles = view
            .snapshot()
            .output
            .paint
            .iter()
            .filter_map(|command| match command {
                PaintCommand::Text { bounds, text, .. } if text == "Revenue" || text == "Rate" => {
                    Some((text.as_str(), *bounds))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(titles.len(), 2, "shared guides render exactly once");
        let plot = view.plot_bounds().unwrap();
        assert!(titles.iter().any(|(text, bounds)| *text == "Revenue"
            && bounds.origin.x + bounds.size.width <= plot.origin.x));
        assert!(
            titles.iter().any(|(text, bounds)| *text == "Rate"
                && bounds.origin.x >= plot.origin.x + plot.size.width)
        );

        let mut unlabeled = valid_spec.clone();
        for size in [12.0, 18.0, 27.0] {
            let mut theme = ChartTheme::default();
            theme.typography.label = size;
            let input = RetainedSurfaceInput::new(
                Rect::new(17.0, 31.0, 900.0, 450.0),
                1.25,
                Default::default(),
            );
            view.update_theme(theme, input).unwrap();
            let plot = view.plot_bounds().unwrap();
            let mut guide_bounds = Vec::new();
            for command in &view.snapshot().output.paint {
                if let PaintCommand::Text {
                    text,
                    bounds,
                    scale,
                    bold,
                    ..
                } = command
                    && (text == "Revenue"
                        || text == "Rate"
                        || text.ends_with("USD")
                        || text.ends_with("percent"))
                {
                    let measured = measured_text(text, *scale, *bold, None);
                    assert!(
                        bounds.size.width >= measured.width
                            && bounds.size.height >= measured.height
                    );
                    assert!(!rects_overlap(*bounds, plot, 0.0));
                    assert!(
                        guide_bounds
                            .iter()
                            .all(|other| !rects_overlap(*other, *bounds, 0.0))
                    );
                    guide_bounds.push(*bounds);
                }
            }
            assert!(guide_bounds.len() >= 6);
        }
        let right_axis = unlabeled
            .axes
            .iter_mut()
            .find(|axis| axis.scale.as_ref() == "right")
            .unwrap();
        right_axis.title = None;
        right_axis.unit = None;
        assert!(ChartModel::new(Arc::new(unlabeled), data.clone(), limits.clone()).is_err());

        let mut three_axes = valid_spec;
        three_axes.scales.push(Scale {
            name: Arc::from("third"),
            kind: ScaleKind::Linear,
            domain: None,
            clamp: false,
            reversed: true,
            unit: None,
        });
        three_axes.axes.push(crate::Axis {
            scale: Arc::from("third"),
            title: Some(Arc::from("Third")),
            unit: None,
            tick_count: Some(3),
            grid: false,
        });
        three_axes.layers.push(
            crate::Layer::new(Mark::Line {
                interpolation: crate::Interpolation::Straight,
                points: false,
            })
            .encode(crate::Encoding {
                channel: Channel::X,
                field: FieldRef::new("d", "x"),
                scale: Some(Arc::from("x")),
            })
            .encode(crate::Encoding {
                channel: Channel::Y,
                field: FieldRef::new("d", "right"),
                scale: Some(Arc::from("third")),
            }),
        );
        assert!(ChartModel::new(Arc::new(three_axes), data, limits).is_err());
    }

    #[test]
    fn custom_rule_text_and_symbol_marks_lower_to_native_primitives() {
        let limits = ChartLimits::default();
        let dataset = Dataset::new(
            DatasetId::from("d"),
            [RowKey::from("a"), RowKey::from("b")],
            vec![
                Column::new("x", vec![DataValue::Number(0.0), DataValue::Number(1.0)]),
                Column::new("y", vec![DataValue::Number(0.0), DataValue::Number(1.0)]),
                Column::new("x2", vec![DataValue::Number(1.0), DataValue::Number(0.0)]),
                Column::new("y2", vec![DataValue::Number(1.0), DataValue::Number(0.0)]),
                Column::new(
                    "label",
                    vec![
                        DataValue::Category(Arc::from("first")),
                        DataValue::Category(Arc::from("second")),
                    ],
                ),
            ],
        )
        .unwrap();
        let data = Arc::new(ChartData::new(22, vec![dataset], &limits).unwrap());
        let position = |channel, column| crate::Encoding {
            channel,
            field: FieldRef::new("d", column),
            scale: match channel {
                Channel::X | Channel::X2 => Some(Arc::from("x")),
                Channel::Y | Channel::Y2 => Some(Arc::from("y")),
                _ => None,
            },
        };
        let base = Recipes::line(
            "Primitives",
            FieldRef::new("d", "x"),
            FieldRef::new("d", "y"),
        );
        let mut spec = crate::ChartSpec::new("Primitives", "Custom primitive chart");
        spec.scales = base.scales;
        spec.layers.push(
            crate::Layer::new(Mark::Rule)
                .encode(position(Channel::X, "x"))
                .encode(position(Channel::Y, "y"))
                .encode(position(Channel::X2, "x2"))
                .encode(position(Channel::Y2, "y2")),
        );
        spec.layers.push(
            crate::Layer::new(Mark::Text)
                .encode(position(Channel::X, "x"))
                .encode(position(Channel::Y, "y"))
                .encode(crate::Encoding {
                    channel: Channel::Text,
                    field: FieldRef::new("d", "label"),
                    scale: None,
                }),
        );
        spec.layers.push(
            crate::Layer::new(Mark::Point {
                symbol: crate::Symbol::Triangle,
            })
            .encode(position(Channel::X, "x"))
            .encode(position(Channel::Y, "y")),
        );
        let mut view =
            ChartView::new(ChartModel::new(Arc::new(spec), data, limits).unwrap()).unwrap();
        view.update(RetainedSurfaceInput::new(
            Rect::new(0.0, 0.0, 400.0, 240.0),
            1.0,
            twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                content: 22,
                ..Default::default()
            },
        ))
        .unwrap();
        assert_eq!(view.hits().len(), 6);
        assert!(
            view.snapshot().output.paint.iter().any(
                |command| matches!(command, PaintCommand::Text { text, .. } if text == "first")
            )
        );
        assert!(
            view.snapshot()
                .output
                .paint
                .iter()
                .filter(|command| matches!(command, PaintCommand::Path { .. }))
                .count()
                >= 3
        );
    }

    #[test]
    fn lifecycle_contracts_output_and_indexes_then_reattaches() {
        let mut view = ChartView::new(model()).unwrap();
        let input = RetainedSurfaceInput::new(
            Rect::new(12.0, 8.0, 400.0, 240.0),
            1.25,
            twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                content: 7,
                ..Default::default()
            },
        );
        view.update(input).unwrap();
        let accepted = view.retained_diagnostics();
        assert!(accepted.output_retained_bytes > 0);
        assert!(accepted.model_retained_bytes > 0);
        assert!(!view.hits().is_empty());

        view.renderer_lost();
        let lost = view.retained_diagnostics();
        assert_eq!(lost.output_retained_bytes, 0);
        assert_eq!(lost.paint_commands, 0);
        // Spatial indexes are renderer-independent and survive device loss.
        assert!(!view.hits().is_empty());

        view.update(input).unwrap();
        assert!(view.retained_diagnostics().output_retained_bytes > 0);
        view.detach();
        let detached = view.retained_diagnostics();
        assert_eq!(detached.output_retained_bytes, 0);
        assert_eq!(detached.model_retained_bytes, 0);
        assert!(view.snapshot().input.is_none());
        assert!(view.hits().is_empty());
        assert!(view.active_hit().is_none());

        view.update(input).unwrap();
        assert_eq!(view.hits().len(), 2);
        assert!(view.retained_diagnostics().output_retained_bytes > 0);
    }

    #[test]
    fn replacement_shrinks_owned_index_and_destroy_is_terminal() {
        let limits = ChartLimits::default();
        let keys = (0..2_000)
            .map(|index| RowKey::from(format!("row-{index}")))
            .collect::<Vec<_>>();
        let values = (0..2_000)
            .map(|index| DataValue::Number(index as f64))
            .collect::<Vec<_>>();
        let dataset = Dataset::new(
            DatasetId::from("d"),
            keys,
            vec![Column::new("x", values.clone()), Column::new("y", values)],
        )
        .unwrap();
        let data = Arc::new(ChartData::new(1, vec![dataset], &limits).unwrap());
        let spec = Arc::new(Recipes::scatter(
            "Many points",
            FieldRef::new("d", "x"),
            FieldRef::new("d", "y"),
        ));
        let mut view =
            ChartView::new(ChartModel::new(Arc::clone(&spec), data, limits.clone()).unwrap())
                .unwrap();
        let input = |revision| {
            RetainedSurfaceInput::new(
                Rect::new(0.0, 0.0, 600.0, 400.0),
                1.0,
                twinkle_ui::retained_surface::RetainedSurfaceRevisions {
                    content: revision,
                    ..Default::default()
                },
            )
        };
        view.update(input(1)).unwrap();
        assert_eq!(view.hits().len(), 2_000);
        let large_bytes = view.retained_diagnostics().model_retained_bytes;

        let small = Dataset::new(
            DatasetId::from("d"),
            [RowKey::from("a"), RowKey::from("b")],
            vec![
                Column::new("x", [DataValue::Number(0.0), DataValue::Number(1.0)]),
                Column::new("y", [DataValue::Number(1.0), DataValue::Number(0.0)]),
            ],
        )
        .unwrap();
        let small = Arc::new(ChartData::new(2, vec![small], &limits).unwrap());
        view.replace_and_update(spec, small, input(2)).unwrap();
        assert_eq!(view.hits().len(), 2);
        assert!(view.retained_diagnostics().model_retained_bytes < large_bytes);

        view.destroy();
        assert!(view.hits().is_empty());
        assert!(view.snapshot().input.is_none());
        assert_eq!(view.retained_diagnostics().output_retained_bytes, 0);
        assert!(view.update(input(3)).is_err());
    }
}

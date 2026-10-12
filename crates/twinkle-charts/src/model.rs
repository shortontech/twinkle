use std::{collections::BTreeMap, sync::Arc, time::Instant};

use twinkle_ui::{
    ActionKind, DragGesture, Point, Rect, SemanticRole, UiId, WheelGesture,
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
    pub revision: u64,
    /// Index into the accepted view's shared inspection-source pool.
    pub source_index: usize,
    /// Exact row position inside that inspection source.
    pub row_index: usize,
}

/// Exact accepted datum exposed by pointer or keyboard inspection.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartInspection {
    pub dataset: Arc<str>,
    pub row: crate::RowKey,
    pub series: Option<Arc<str>>,
    pub revision: u64,
    pub cells: Vec<crate::ChartTableCell>,
}

#[derive(Clone, Debug)]
struct InspectionSource {
    dataset: Arc<crate::Dataset>,
    owned_bytes: usize,
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
    hits: Vec<ChartHit>,
    hit_x_order: Vec<usize>,
    inspection_sources: Vec<InspectionSource>,
    data_owned: bool,
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
            state: ChartState::default(),
            diagnostics: ChartDiagnostics::default(),
            hits: Vec::new(),
            hit_x_order: Vec::new(),
            inspection_sources: Vec::new(),
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
        let dataset = &self.inspection_sources.get(hit.source_index)?.dataset;
        let row = dataset.row(hit.row_index)?;
        if row.key() != &hit.row {
            return None;
        }
        Some(ChartInspection {
            dataset: dataset.id.0.clone(),
            row: row.key().clone(),
            series: hit.series.clone(),
            revision: hit.revision,
            cells: dataset
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
                .collect(),
        })
    }
    fn nearest_index(&self, point: Point) -> Option<usize> {
        let insertion = self
            .hit_x_order
            .partition_point(|index| hit_center_x(&self.hits[*index]) < point.x);
        let start = insertion.saturating_sub(512);
        let end = insertion.saturating_add(512).min(self.hit_x_order.len());
        self.hit_x_order[start..end].iter().copied().min_by(|a, b| {
            distance(self.hits[*a].bounds, point)
                .total_cmp(&distance(self.hits[*b].bounds, point))
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
        ),
        ChartError,
    > {
        self.spec.validate(&self.data, &self.limits)?;
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
        let table_width = input.bounds.size.width.min(88.0).max(0.0);
        let table_bounds = Rect::new(
            input.bounds.origin.x + input.bounds.size.width - table_width,
            input.bounds.origin.y,
            table_width,
            input.bounds.size.height.min(24.0).max(0.0),
        );
        output.paint.push(PaintCommand::Text {
            bounds: Rect::new(
                input.bounds.origin.x + 8.0,
                input.bounds.origin.y + 4.0,
                (input.bounds.size.width - table_width - 16.0).max(0.0),
                22.0,
            ),
            text: self.spec.title.to_string(),
            scale: 1.0,
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
            scale: 0.9,
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
            let shared_model = if self.spec.layers.len() > 1 {
                let mut model = self.clone();
                model.spec = Arc::new(spec_with_shared_domains(&self.spec, &self.data));
                Some(model)
            } else {
                None
            };
            let model = shared_model.as_ref().unwrap_or(self);
            for (layer_index, layer) in model
                .spec
                .layers
                .iter()
                .filter(|layer| layer.visible)
                .enumerate()
            {
                model.prepare_layer(
                    layer_index,
                    layer,
                    plot,
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
        // The baseline synchronous pipeline retains all accepted owners until
        // admission, so its observable lower bound for peak ownership is the
        // complete candidate steady set. Reject even this lower bound before
        // publishing; finer stage scratch accounting can only increase it.
        let peak_bytes = steady_bytes;
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
            peak_bytes,
            cache_misses: 1,
            notices,
            ..self.diagnostics.clone()
        };
        Ok((output, hits, hit_x_order, inspection_sources, diagnostics))
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
            spec_with_shared_domains(&self.spec, &self.data)
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
                let (mut prepared, mut facet_hits, _, mut facet_sources, _) = submodel.prepare(
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

    fn prepare_layer(
        &self,
        layer_index: usize,
        layer: &crate::Layer,
        plot: Rect,
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
        let inspection_source = register_inspection_source(
            inspection_sources,
            dataset,
            self.data_owned || !layer.transforms.is_empty(),
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
        if layer_index == 0 {
            // Keep controls available when controlled state hides every series.
            self.prepare_legends(plot, series_column, output)?;
        }
        if all_values.is_empty() {
            return Ok(());
        }
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
        let values = values
            .into_iter()
            .filter(|(_, x, y)| in_domain(*x, x_domain) && in_domain(*y, y_domain))
            .collect::<Vec<_>>();
        if layer_index == 0 {
            self.prepare_guides(
                plot, x_domain, y_domain, x_scale, y_scale, &x_mapper, &y_mapper, output,
            )?;
        }
        let mut series_groups = series_groups(dataset, &layer.encodings, &self.state);
        for rows in &mut series_groups {
            rows.retain(|index| {
                x_coordinates[*index].is_some_and(|value| in_domain(value, x_domain))
                    && y_coordinates[*index].is_some_and(|value| in_domain(value, y_domain))
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
                            (Some(x), Some(y)) => segment.push(map(x, y)),
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
                            let top0 = map(x0, y0);
                            let top1 = map(x1, y1);
                            let bottom0 = map(x0, base0);
                            let bottom1 = map(x1, base1);
                            triangles.extend([top0, bottom0, top1, top1, bottom0, bottom1]);
                        }
                    } else {
                        y_mapper.validate(*baseline)?;
                        let baseline = y_mapper.map(*baseline);
                        let max_points = (plot.size.width.ceil() as usize).saturating_mul(4).max(2);
                        let mut segment = Vec::new();
                        for index in rows.iter().copied() {
                            match (x_coordinates[index], y_coordinates[index]) {
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
                        output.paint.push(fill_path_command(
                            triangles,
                            self.theme.series_cue(layer_index + series_index).color,
                            rows.len().max(2),
                        )?);
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
                        scale: 0.8,
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
        if matches!(
            layer.mark,
            Mark::Line { .. } | Mark::Area { .. } | Mark::Point { .. }
        ) {
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
        let row_height = 20.0;
        let width = plot.size.width.min(130.0);
        let toggleable = self.spec.legends.iter().any(|legend| legend.toggleable);
        for (index, label) in labels.into_iter().enumerate() {
            let y = plot.origin.y + index as f32 * row_height;
            if y + row_height > plot.origin.y + plot.size.height {
                break;
            }
            let bounds = Rect::new(
                plot.origin.x + plot.size.width - width,
                y,
                width,
                row_height,
            );
            let visible = self
                .state
                .visible_series
                .as_ref()
                .is_none_or(|series| series.contains(&label));
            output.paint.push(PaintCommand::RoundedFill {
                rect: Rect::new(bounds.origin.x + 2.0, y + 5.0, 10.0, 10.0),
                color: if visible {
                    self.theme.series_cue(index).color
                } else {
                    self.theme.missing
                },
                radius: 2.0,
            });
            output.paint.push(PaintCommand::Text {
                bounds: Rect::new(
                    bounds.origin.x + 16.0,
                    y,
                    (width - 16.0).max(0.0),
                    row_height,
                ),
                text: label.to_string(),
                scale: 0.75,
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
            let count = axis
                .tick_count
                .unwrap_or(5)
                .clamp(2, self.limits.max_ticks_per_axis);
            let is_x = x_scale == Some(axis.scale.as_ref());
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
                    let width = plot.size.width.min(60.0);
                    let origin = (x - width / 2.0).clamp(
                        plot.origin.x,
                        (plot.origin.x + plot.size.width - width).max(plot.origin.x),
                    );
                    output.paint.push(PaintCommand::Text {
                        bounds: Rect::new(
                            origin,
                            plot.origin.y + plot.size.height + 3.0,
                            width,
                            18.0,
                        ),
                        text: format_tick(value, axis.unit.as_deref()),
                        scale: 0.75,
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
                    output.paint.push(PaintCommand::Text {
                        bounds: Rect::new((plot.origin.x - 34.0).max(0.0), y - 9.0, 30.0, 18.0),
                        text: format_tick(value, axis.unit.as_deref()),
                        scale: 0.7,
                        color: self.theme.muted,
                        align: twinkle_ui::TextAlign::End,
                        bold: false,
                        wrap: false,
                    });
                }
            }
        }
        for annotation in &self.spec.annotations {
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
                    annotation_label(output, plot, text, point.x, point.y, self.theme.foreground);
                }
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
        let inspection_source =
            register_inspection_source(inspection_sources, dataset, self.data_owned);
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
            Ok((output, hits, hit_x_order, inspection_sources, diagnostics)) => {
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
) -> usize {
    let index = sources.len();
    sources.push(InspectionSource {
        dataset: Arc::new(dataset.clone()),
        owned_bytes: owned.then(|| dataset_owned_bytes(dataset)).unwrap_or(0),
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
        let mut snapshot = self.snapshot().clone();
        if let (Some(hit), Some(detail), Some(input)) =
            (self.active_hit(), self.active_inspection(), snapshot.input)
        {
            let text = inspection_label(&detail);
            let width = input.bounds.size.width.min(300.0).max(0.0);
            let height = input.bounds.size.height.min(52.0).max(0.0);
            let x = (hit.bounds.origin.x + hit.bounds.size.width + 8.0)
                .min(input.bounds.origin.x + input.bounds.size.width - width)
                .max(input.bounds.origin.x);
            let y = (hit.bounds.origin.y - height - 8.0)
                .min(input.bounds.origin.y + input.bounds.size.height - height)
                .max(input.bounds.origin.y);
            let bounds = Rect::new(x, y, width, height);
            snapshot.output.paint.push(PaintCommand::Fill {
                rect: bounds,
                color: self.host.model().theme.background,
            });
            snapshot.output.paint.push(PaintCommand::Stroke {
                rect: bounds,
                color: self.host.model().theme.selection,
                width: 1.0,
            });
            snapshot.output.paint.push(PaintCommand::Text {
                bounds: inset(bounds, 6.0, 6.0, 6.0, 6.0),
                text: text.clone(),
                scale: 1.0,
                color: self.host.model().theme.foreground,
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
                    label: Some(text),
                    state: Some(format!("row {}; exact accepted values", detail.row.0)),
                    protected: false,
                });
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
                if let Some((_, label)) = interaction.id.as_str().split_once("chart/legend/") {
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
            element = element.on_pointer_move(pointer_message);
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

fn inspection_label(inspection: &ChartInspection) -> String {
    let mut parts = Vec::with_capacity(inspection.cells.len().min(6) + 1);
    if let Some(series) = &inspection.series {
        parts.push(format!("series: {series}"));
    }
    parts.extend(inspection.cells.iter().take(6).map(|cell| {
        cell.unit.as_deref().map_or_else(
            || format!("{}: {}", cell.column, cell.formatted),
            |unit| format!("{}: {} {unit}", cell.column, cell.formatted),
        )
    }));
    if inspection.cells.len() > 6 {
        parts.push(format!(
            "+{} more fields in data table",
            inspection.cells.len() - 6
        ));
    }
    parts.join(" · ").chars().take(512).collect()
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
                    scale: 1.0,
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
                    scale: 1.0,
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

fn spec_with_shared_domains(spec: &ChartSpec, data: &ChartData) -> ChartSpec {
    let mut shared = spec.clone();
    for scale in &mut shared.scales {
        if scale.domain.is_some() {
            continue;
        }
        let mut values = Vec::new();
        for encoding in shared
            .layers
            .iter()
            .flat_map(|layer| layer.encodings.iter())
            .filter(|encoding| encoding.scale.as_deref() == Some(scale.name.as_ref()))
        {
            let Some(column) = data
                .dataset(&encoding.field.dataset)
                .and_then(|dataset| dataset.column(&encoding.field.column))
            else {
                continue;
            };
            values.extend(column.values.iter().filter_map(|value| match value {
                DataValue::Number(value) if value.is_finite() => Some(*value),
                DataValue::Timestamp(value) => Some(*value as f64),
                _ => None,
            }));
        }
        for layer in &shared.layers {
            let baseline_channel = match layer.mark {
                Mark::Area { baseline, .. } => Some((Channel::Y, baseline as f64)),
                Mark::Bar {
                    orientation: crate::Orientation::Vertical,
                } => Some((Channel::Y, 0.0)),
                Mark::Bar {
                    orientation: crate::Orientation::Horizontal,
                } => Some((Channel::X, 0.0)),
                _ => None,
            };
            if let Some((channel, baseline)) = baseline_channel
                && layer.encodings.iter().any(|encoding| {
                    encoding.channel == channel
                        && encoding.scale.as_deref() == Some(scale.name.as_ref())
                })
            {
                values.push(baseline);
            }
        }
        if !values.is_empty() {
            scale.domain = Some(extent(values.into_iter()));
        }
    }
    shared
}

fn annotation_label(
    output: &mut RetainedSurfaceOutput,
    plot: Rect,
    label: &str,
    x: f32,
    y: f32,
    color: u32,
) {
    let width = plot.size.width.min(140.0);
    let height = plot.size.height.min(20.0);
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
        scale: 0.75,
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
                Rect::new(0.0, 0.0, 180.0, 100.0),
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
    fn peak_budget_rejects_candidate_before_any_state_is_published() {
        let mut constrained = ChartLimits::default();
        constrained.max_peak_bytes = 1;
        let base = model();
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

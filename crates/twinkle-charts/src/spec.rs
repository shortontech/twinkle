use std::{collections::BTreeSet, sync::Arc, time::Duration};

use crate::{ChartData, ChartError, Transform};

/// Hard admission and ownership limits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChartLimits {
    pub max_rows: usize,
    pub max_series: usize,
    pub max_facets: usize,
    pub max_layers: usize,
    pub max_transforms: usize,
    pub max_geometry_vertices: usize,
    pub max_ticks_per_axis: usize,
    pub max_annotations: usize,
    pub max_visible_table_rows: usize,
    pub max_columns: usize,
    pub max_label_bytes: usize,
    pub max_total_label_bytes: usize,
    pub max_steady_bytes: usize,
    pub max_peak_bytes: usize,
}

impl Default for ChartLimits {
    fn default() -> Self {
        Self {
            max_rows: 1_000_000,
            max_series: 256,
            max_facets: 64,
            max_layers: 32,
            max_transforms: 32,
            max_geometry_vertices: 1_000_000,
            max_ticks_per_axis: 256,
            max_annotations: 1_024,
            max_visible_table_rows: 256,
            max_columns: 256,
            max_label_bytes: 4 * 1024,
            max_total_label_bytes: 4 * 1024 * 1024,
            max_steady_bytes: 128 * 1024 * 1024,
            max_peak_bytes: 256 * 1024 * 1024,
        }
    }
}

/// Stable field reference resolved during preparation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FieldRef {
    pub dataset: Arc<str>,
    pub column: Arc<str>,
}
impl FieldRef {
    pub fn new(dataset: impl Into<Arc<str>>, column: impl Into<Arc<str>>) -> Self {
        Self {
            dataset: dataset.into(),
            column: column.into(),
        }
    }
}

/// Visual channel.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Channel {
    X,
    X2,
    Y,
    Y2,
    Series,
    Group,
    Fill,
    Stroke,
    Opacity,
    Symbol,
    Size,
    Text,
}

/// Channel-to-field binding.
#[derive(Clone, Debug, PartialEq)]
pub struct Encoding {
    pub channel: Channel,
    pub field: FieldRef,
    pub scale: Option<Arc<str>>,
}

/// Line interpolation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Interpolation {
    Straight,
    StepBefore,
    StepAfter,
}

/// Bar orientation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Orientation {
    Vertical,
    Horizontal,
}

/// Built-in symbol shape.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Symbol {
    Circle,
    Square,
    Triangle,
    Diamond,
    Cross,
}

/// Composable chart mark descriptor.
#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    Rect,
    Bar {
        orientation: Orientation,
    },
    Line {
        interpolation: Interpolation,
        points: bool,
    },
    Area {
        interpolation: Interpolation,
        baseline: f64,
    },
    Point {
        symbol: Symbol,
    },
    Arc {
        inner_radius: f32,
    },
    Rule,
    Text,
}

/// Scale kind and explicit policy.
#[derive(Clone, Debug, PartialEq)]
pub enum ScaleKind {
    Linear,
    Log {
        base: f64,
    },
    Time,
    Ordinal,
    Band {
        inner_padding: f64,
        outer_padding: f64,
    },
    Color,
    Size,
}

/// Named scale shared by encodings.
#[derive(Clone, Debug, PartialEq)]
pub struct Scale {
    pub name: Arc<str>,
    pub kind: ScaleKind,
    pub domain: Option<[f64; 2]>,
    pub clamp: bool,
    pub reversed: bool,
    pub unit: Option<Arc<str>>,
}

/// Axis guide.
#[derive(Clone, Debug, PartialEq)]
pub struct Axis {
    pub scale: Arc<str>,
    pub title: Option<Arc<str>>,
    pub unit: Option<Arc<str>>,
    pub tick_count: Option<usize>,
    pub grid: bool,
}

/// Legend guide.
#[derive(Clone, Debug, PartialEq)]
pub struct Legend {
    pub scale: Arc<str>,
    pub title: Option<Arc<str>>,
    pub toggleable: bool,
}

/// Reference guide or text annotation.
#[derive(Clone, Debug, PartialEq)]
pub enum Annotation {
    Rule {
        scale: Arc<str>,
        value: f64,
        label: Option<Arc<str>>,
        include_in_domain: bool,
    },
    Band {
        scale: Arc<str>,
        start: f64,
        end: f64,
        label: Option<Arc<str>>,
        include_in_domain: bool,
    },
    Text {
        text: Arc<str>,
        x: f64,
        y: f64,
    },
}

/// One composable paint layer.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub mark: Mark,
    pub encodings: Vec<Encoding>,
    pub transforms: Vec<Transform>,
    pub visible: bool,
}
impl Layer {
    pub fn new(mark: Mark) -> Self {
        Self {
            mark,
            encodings: Vec::new(),
            transforms: Vec::new(),
            visible: true,
        }
    }
    pub fn encode(mut self, encoding: Encoding) -> Self {
        self.encodings.push(encoding);
        self
    }
}

/// Facet scale behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FacetScalePolicy {
    Shared,
    Independent,
}
/// Bounded small-multiple descriptor.
#[derive(Clone, Debug, PartialEq)]
pub struct Facet {
    pub row: Option<FieldRef>,
    pub column: Option<FieldRef>,
    pub scales: FacetScalePolicy,
}

/// Domain behavior after hiding a series.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VisibilityDomainPolicy {
    Original,
    VisibleSeries,
}

/// Validated, composable chart grammar.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartSpec {
    pub title: Arc<str>,
    pub summary: Arc<str>,
    pub layers: Vec<Layer>,
    pub scales: Vec<Scale>,
    pub axes: Vec<Axis>,
    pub legends: Vec<Legend>,
    pub annotations: Vec<Annotation>,
    pub facet: Option<Facet>,
    pub visibility_domain: VisibilityDomainPolicy,
}

impl ChartSpec {
    pub fn new(title: impl Into<Arc<str>>, summary: impl Into<Arc<str>>) -> Self {
        Self {
            title: title.into(),
            summary: summary.into(),
            layers: Vec::new(),
            scales: Vec::new(),
            axes: Vec::new(),
            legends: Vec::new(),
            annotations: Vec::new(),
            facet: None,
            visibility_domain: VisibilityDomainPolicy::Original,
        }
    }
    pub fn layer(mut self, layer: Layer) -> Self {
        self.layers.push(layer);
        self
    }
    pub fn scale(mut self, scale: Scale) -> Self {
        self.scales.push(scale);
        self
    }
    pub fn axis(mut self, axis: Axis) -> Self {
        self.axes.push(axis);
        self
    }
    pub fn legend(mut self, legend: Legend) -> Self {
        self.legends.push(legend);
        self
    }
    pub fn annotation(mut self, annotation: Annotation) -> Self {
        self.annotations.push(annotation);
        self
    }
    pub fn validate(&self, data: &ChartData, limits: &ChartLimits) -> Result<(), ChartError> {
        for (name, actual, limit) in [
            ("layers", self.layers.len(), limits.max_layers),
            (
                "annotations",
                self.annotations.len(),
                limits.max_annotations,
            ),
        ] {
            if actual > limit {
                return Err(ChartError::Limit {
                    resource: name,
                    actual,
                    limit,
                });
            }
        }
        let transform_count = self
            .layers
            .iter()
            .map(|layer| layer.transforms.len())
            .sum::<usize>();
        if transform_count > limits.max_transforms {
            return Err(ChartError::Limit {
                resource: "transform steps",
                actual: transform_count,
                limit: limits.max_transforms,
            });
        }
        let mut scale_names = BTreeSet::new();
        for scale in &self.scales {
            if !scale_names.insert(scale.name.as_ref()) {
                return Err(ChartError::DuplicateKey(scale.name.to_string()));
            }
        }
        for layer in &self.layers {
            for encoding in &layer.encodings {
                let dataset = data.dataset(&encoding.field.dataset).ok_or_else(|| {
                    ChartError::MissingDataset(encoding.field.dataset.to_string())
                })?;
                if dataset.column(&encoding.field.column).is_none() {
                    return Err(ChartError::MissingColumn {
                        dataset: encoding.field.dataset.to_string(),
                        column: encoding.field.column.to_string(),
                    });
                }
                if let Some(scale) = &encoding.scale
                    && !scale_names.contains(scale.as_ref())
                {
                    return Err(ChartError::invalid(
                        crate::ChartStage::Validate,
                        format!("encoding references unknown scale `{scale}`"),
                    ));
                }
            }
        }
        for axis in &self.axes {
            if !scale_names.contains(axis.scale.as_ref()) {
                return Err(ChartError::invalid(
                    crate::ChartStage::Validate,
                    format!("axis references unknown scale `{}`", axis.scale),
                ));
            }
            if axis.tick_count.unwrap_or(0) > limits.max_ticks_per_axis {
                return Err(ChartError::Limit {
                    resource: "axis ticks",
                    actual: axis.tick_count.unwrap(),
                    limit: limits.max_ticks_per_axis,
                });
            }
        }
        let label_bytes = self.title.len()
            + self.summary.len()
            + self
                .axes
                .iter()
                .filter_map(|axis| axis.title.as_ref())
                .map(|s| s.len())
                .sum::<usize>()
            + self
                .annotations
                .iter()
                .map(|a| match a {
                    Annotation::Rule { label, .. } | Annotation::Band { label, .. } => {
                        label.as_ref().map_or(0, |s| s.len())
                    }
                    Annotation::Text { text, .. } => text.len(),
                })
                .sum::<usize>();
        if label_bytes > limits.max_total_label_bytes {
            return Err(ChartError::Limit {
                resource: "label bytes",
                actual: label_bytes,
                limit: limits.max_total_label_bytes,
            });
        }
        Ok(())
    }
}

/// Caller-controlled persistent chart state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChartState {
    pub selected_rows: BTreeSet<crate::RowKey>,
    pub visible_series: Option<BTreeSet<Arc<str>>>,
    pub viewport: Option<Viewport>,
}
/// Controlled continuous viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub x: Option<[f64; 2]>,
    pub y: Option<[f64; 2]>,
}

/// Typed application event carrying stable identity and accepted revision.
#[derive(Clone, Debug, PartialEq)]
pub enum ChartEvent {
    Select {
        dataset: Arc<str>,
        row: crate::RowKey,
        series: Option<Arc<str>>,
        revision: u64,
    },
    Inspect {
        dataset: Arc<str>,
        row: crate::RowKey,
        revision: u64,
    },
    ViewportChanged(Viewport),
    ToggleSeries {
        series: Arc<str>,
        visible: bool,
    },
    TableMode(bool),
}

/// Preparation and retained-update counters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChartDiagnostics {
    pub preparation_time: Duration,
    pub input_rows: usize,
    pub transformed_rows: usize,
    pub displayed_rows: usize,
    pub geometry_vertices: usize,
    pub index_bytes: usize,
    pub guide_count: usize,
    pub paint_commands: usize,
    pub interaction_records: usize,
    pub semantic_records: usize,
    pub steady_bytes: usize,
    pub peak_bytes: usize,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub rejected_updates: u64,
    pub queue_depth: usize,
    pub stale_results: u64,
    pub notices: Vec<crate::ChartDiagnostic>,
}

/// Deterministic recipe family identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChartFamily {
    Line,
    Bar,
    Area,
    Scatter,
    Bubble,
    Pie,
    Donut,
    Histogram,
    Heatmap,
    Sparkline,
    Combo,
    Table,
}

/// Convenience recipes lower to the same grammar as custom charts.
pub struct Recipes;
impl Recipes {
    pub fn line(title: impl Into<Arc<str>>, x: FieldRef, y: FieldRef) -> ChartSpec {
        Self::xy(
            title,
            ChartFamily::Line,
            Mark::Line {
                interpolation: Interpolation::Straight,
                points: false,
            },
            x,
            y,
        )
    }
    pub fn area(title: impl Into<Arc<str>>, x: FieldRef, y: FieldRef) -> ChartSpec {
        Self::xy(
            title,
            ChartFamily::Area,
            Mark::Area {
                interpolation: Interpolation::Straight,
                baseline: 0.0,
            },
            x,
            y,
        )
    }
    pub fn scatter(title: impl Into<Arc<str>>, x: FieldRef, y: FieldRef) -> ChartSpec {
        Self::xy(
            title,
            ChartFamily::Scatter,
            Mark::Point {
                symbol: Symbol::Circle,
            },
            x,
            y,
        )
    }
    pub fn bar(title: impl Into<Arc<str>>, category: FieldRef, value: FieldRef) -> ChartSpec {
        Self::xy(
            title,
            ChartFamily::Bar,
            Mark::Bar {
                orientation: Orientation::Vertical,
            },
            category,
            value,
        )
    }
    fn xy(
        title: impl Into<Arc<str>>,
        family: ChartFamily,
        mark: Mark,
        x: FieldRef,
        y: FieldRef,
    ) -> ChartSpec {
        let title = title.into();
        ChartSpec::new(title.clone(), format!("{family:?} chart: {title}"))
            .layer(
                Layer::new(mark)
                    .encode(Encoding {
                        channel: Channel::X,
                        field: x,
                        scale: Some("x".into()),
                    })
                    .encode(Encoding {
                        channel: Channel::Y,
                        field: y,
                        scale: Some("y".into()),
                    }),
            )
            .scale(Scale {
                name: "x".into(),
                kind: ScaleKind::Linear,
                domain: None,
                clamp: false,
                reversed: false,
                unit: None,
            })
            .scale(Scale {
                name: "y".into(),
                kind: ScaleKind::Linear,
                domain: None,
                clamp: false,
                reversed: true,
                unit: None,
            })
    }
}

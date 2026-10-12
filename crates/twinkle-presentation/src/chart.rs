use std::{cell::RefCell, collections::BTreeSet, rc::Rc, sync::Arc};

use serde_json::{Value, json};
use twinkle::retained_surface::{
    RetainedSurfaceInput, RetainedSurfaceRevisions, RetainedSurfaceSnapshot,
};
use twinkle::{Rect, SemanticRole, UiId};
use twinkle_charts::{
    AggregateOp, Annotation, Axis, BinOptions, Channel, ChartData, ChartEvent, ChartInspector,
    ChartLimits, ChartModel, ChartSpec, ChartState, ChartTheme, ChartView, Column, DataValue,
    Dataset, DatasetId, Encoding, Facet, FacetScalePolicy, FieldRef, Interpolation, Layer, Legend,
    Mark, MissingPolicy, Orientation, Recipes, RowKey, Scale, ScaleKind, StackMode, Symbol,
    Transform, Viewport, VisibilityDomainPolicy, project_inspection_overlay,
};

#[derive(Clone)]
enum ChartProjectionAction {
    Event(ChartEvent),
}

#[derive(Clone, Copy)]
struct ChartActionMap {
    table: usize,
    toggle: Option<usize>,
    navigate: Option<usize>,
    select: Option<usize>,
    inspect: Option<usize>,
    viewport: Option<usize>,
    viewport_command: Option<usize>,
}

impl ChartActionMap {
    fn event(self, event: ChartEvent) -> (usize, Value) {
        match event {
            ChartEvent::Navigate(command) => (
                self.navigate.unwrap_or(self.table),
                json!({"kind":"navigate","command":format!("{command:?}")}),
            ),
            ChartEvent::ToggleSeries { series, visible } => (
                self.toggle.unwrap_or(self.table),
                json!({"series": series.as_ref(), "visible": visible}),
            ),
            ChartEvent::TableMode(visible) => (self.table, json!(visible)),
            ChartEvent::Select {
                dataset,
                row,
                series,
                revision,
            } => (
                self.select.unwrap_or(self.table),
                json!({"kind":"select","dataset":dataset.as_ref(),"row":row.0.as_ref(),"series":series.as_deref(),"revision":revision}),
            ),
            ChartEvent::Inspect {
                dataset,
                row,
                revision,
            } => (
                self.inspect.unwrap_or(self.table),
                json!({"kind":"inspect","dataset":dataset.as_ref(),"row":row.0.as_ref(),"revision":revision}),
            ),
            ChartEvent::ViewportChanged(viewport) => (
                self.viewport.unwrap_or(self.table),
                json!({"kind":"viewport","x":viewport.x,"y":viewport.y}),
            ),
            ChartEvent::Viewport(command) => (
                self.viewport_command.unwrap_or(self.table),
                json!({"kind":"viewportCommand","command":format!("{command:?}")}),
            ),
        }
    }
}

#[derive(Debug)]
struct RetainedChartView(Rc<RefCell<ChartView>>);

impl Clone for RetainedChartView {
    fn clone(&self) -> Self {
        Self(Rc::new(RefCell::new(self.0.borrow().clone())))
    }
}

impl RetainedChartView {
    fn new(view: ChartView) -> Self {
        Self(Rc::new(RefCell::new(view)))
    }

    fn shared(&self) -> Self {
        Self(Rc::clone(&self.0))
    }
}

impl PartialEq for RetainedChartView {
    fn eq(&self, other: &Self) -> bool {
        let this = self.0.borrow();
        let other = other.0.borrow();
        this.snapshot() == other.snapshot() && this.hits() == other.hits()
    }
}

/// Host-admitted immutable projection of a JSX chart declaration.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartDeclaration {
    pub(crate) id: String,
    pub(crate) class_name: Option<String>,
    pub(crate) snapshot: RetainedSurfaceSnapshot,
    pub(crate) inspector: ChartInspector,
    pub(crate) table_action: usize,
    pub(crate) toggle_action: Option<usize>,
    pub(crate) navigate_action: Option<usize>,
    pub(crate) select_action: Option<usize>,
    pub(crate) inspect_action: Option<usize>,
    pub(crate) viewport_action: Option<usize>,
    pub(crate) viewport_command_action: Option<usize>,
    pub(crate) pan_action: Option<usize>,
    pub(crate) zoom_action: Option<usize>,
    pub(crate) retained_bytes: usize,
    pub(crate) width: f32,
    pub(crate) height: f32,
    controlled_inspection: Option<(String, RowKey)>,
    retained: RetainedChartView,
}

impl ChartDeclaration {
    fn action_map(&self) -> ChartActionMap {
        ChartActionMap {
            table: self.table_action,
            toggle: self.toggle_action,
            navigate: self.navigate_action,
            select: self.select_action,
            inspect: self.inspect_action,
            viewport: self.viewport_action,
            viewport_command: self.viewport_command_action,
        }
    }

    pub(crate) fn shared_projection(&self) -> Self {
        Self {
            id: self.id.clone(),
            class_name: self.class_name.clone(),
            snapshot: self.snapshot.clone(),
            inspector: self.inspector.clone(),
            table_action: self.table_action,
            toggle_action: self.toggle_action,
            navigate_action: self.navigate_action,
            select_action: self.select_action,
            inspect_action: self.inspect_action,
            viewport_action: self.viewport_action,
            viewport_command_action: self.viewport_command_action,
            pan_action: self.pan_action,
            zoom_action: self.zoom_action,
            retained_bytes: self.retained_bytes,
            width: self.width,
            height: self.height,
            controlled_inspection: self.controlled_inspection.clone(),
            retained: self.retained.shared(),
        }
    }

    pub(crate) fn projected_for_bounds(&self, bounds: Rect) -> Result<Self, String> {
        let mut projection = self.shared_projection();
        let bounds = Rect::new(
            0.0,
            0.0,
            bounds.size.width.max(0.0),
            bounds.size.height.max(0.0),
        );
        let mut input = projection
            .retained
            .0
            .borrow()
            .snapshot()
            .input
            .ok_or("accepted chart projection has no retained input")?;
        if input.bounds != bounds {
            input.bounds = bounds;
            projection
                .retained
                .0
                .borrow_mut()
                .update(input)
                .map_err(|error| error.to_string())?;
        }
        let (inspector, mut snapshot, steady_bytes) = {
            let retained = projection.retained.0.borrow();
            (
                retained.inspector(),
                retained.snapshot().clone(),
                retained.diagnostics().steady_bytes,
            )
        };
        let has_overlay = projection
            .controlled_inspection
            .as_ref()
            .and_then(|(dataset, row)| inspector.inspect_identity(dataset, row))
            .is_some_and(|(hit, detail)| {
                project_inspection_overlay(&mut snapshot, &hit, &detail, &ChartTheme::default())
            });
        projection.width = bounds.size.width;
        projection.height = bounds.size.height;
        projection.inspector = inspector;
        projection.snapshot = snapshot;
        projection.retained_bytes = steady_bytes
            .saturating_add(projection.inspector.retained_bytes())
            .saturating_add(if has_overlay { 2 * 1024 } else { 0 });
        Ok(projection)
    }

    pub fn parse(value: &Value, class_name: Option<String>) -> Result<Self, String> {
        let id = bounded_string(value, "id", 128)?;
        let width = dimension(value, "width")?;
        let height = dimension(value, "height")?;
        let limits = ChartLimits::default();
        let data = Arc::new(parse_data(
            value.get("data").ok_or("chart needs data")?,
            &limits,
        )?);
        let spec = Arc::new(parse_spec(
            value.get("spec").ok_or("chart needs a spec")?,
            &limits,
        )?);
        let needs_toggle = spec.legends.iter().any(|legend| legend.toggleable);
        let toggle_action = action(value, "toggleAction")?;
        if needs_toggle && toggle_action.is_none() {
            return Err("toggleable chart legend needs an onToggleSeries callback".into());
        }
        let state_value = value.get("state");
        let state = parse_state(state_value)?;
        let controlled_inspection = parse_controlled_inspection(state_value)?;
        let mut view = ChartView::new(
            ChartModel::new(spec, data.clone(), limits)
                .map_err(|error| error.to_string())?
                .with_state(state),
        )
        .map_err(|error| error.to_string())?;
        view.update(RetainedSurfaceInput::new(
            Rect::new(0.0, 0.0, width, height),
            1.0,
            RetainedSurfaceRevisions {
                content: data.revision(),
                ..Default::default()
            },
        ))
        .map_err(|error| error.to_string())?;
        let inspector = view.inspector();
        let mut snapshot = view.snapshot().clone();
        let has_overlay = controlled_inspection
            .as_ref()
            .and_then(|(dataset, row)| inspector.inspect_identity(dataset, row))
            .is_some_and(|(hit, detail)| {
                project_inspection_overlay(&mut snapshot, &hit, &detail, &ChartTheme::default())
            });
        let retained_bytes = view
            .diagnostics()
            .steady_bytes
            .saturating_add(inspector.retained_bytes())
            .saturating_add(if has_overlay { 2 * 1024 } else { 0 });
        Ok(Self {
            id,
            class_name,
            snapshot,
            inspector,
            table_action: action(value, "tableAction")?
                .ok_or("chart needs an onTableMode callback")?,
            toggle_action,
            navigate_action: action(value, "chartNavigateAction")?,
            select_action: action(value, "chartSelectAction")?,
            inspect_action: action(value, "chartInspectAction")?,
            viewport_action: action(value, "chartViewportAction")?,
            viewport_command_action: action(value, "chartViewportCommandAction")?,
            pan_action: action(value, "chartPanAction")?,
            zoom_action: action(value, "chartZoomAction")?,
            retained_bytes,
            width,
            height,
            controlled_inspection,
            retained: RetainedChartView::new(view),
        })
    }

    pub(crate) fn resize(&mut self, value: &Value) -> Result<(), String> {
        let width = dimension(value, "width")?;
        let height = dimension(value, "height")?;
        if self.width == width && self.height == height {
            return Ok(());
        }
        let mut input = self
            .snapshot
            .input
            .ok_or("accepted chart projection has no retained input")?;
        input.bounds = Rect::new(0.0, 0.0, width, height);
        self.retained
            .0
            .borrow_mut()
            .update(input)
            .map_err(|error| error.to_string())?;
        self.width = width;
        self.height = height;
        let retained = self.retained.0.borrow();
        self.inspector = retained.inspector();
        self.snapshot = retained.snapshot().clone();
        let has_overlay = self
            .controlled_inspection
            .as_ref()
            .and_then(|(dataset, row)| self.inspector.inspect_identity(dataset, row))
            .is_some_and(|(hit, detail)| {
                project_inspection_overlay(
                    &mut self.snapshot,
                    &hit,
                    &detail,
                    &ChartTheme::default(),
                )
            });
        self.retained_bytes = retained
            .diagnostics()
            .steady_bytes
            .saturating_add(self.inspector.retained_bytes())
            .saturating_add(if has_overlay { 2 * 1024 } else { 0 });
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn retained_update_calls(&self) -> u64 {
        self.retained.0.borrow().retained_diagnostics().update_calls
    }

    pub fn event(&self, event: ChartEvent) -> (usize, Value) {
        self.action_map().event(event)
    }

    pub fn element<Message: Clone>(
        &self,
        mut message: impl FnMut(usize, Value) -> Message,
    ) -> Result<impl twinkle::Component<Message>, String> {
        let visible = self
            .snapshot
            .output
            .semantics
            .iter()
            .filter(|semantic| semantic.id.as_str().contains("chart/legend/"))
            .filter_map(|semantic| {
                (semantic.state.as_deref() == Some("visible")).then(|| {
                    semantic
                        .id
                        .as_str()
                        .split_once("chart/legend/")
                        .unwrap()
                        .1
                        .to_owned()
                })
            })
            .collect::<BTreeSet<_>>();
        let mut snapshot = self.snapshot.clone();
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
            .try_element(UiId::from(self.id.clone()), |interaction| {
                let event = interaction.id.as_str().split_once("chart/legend/").map_or(
                    ChartEvent::TableMode(true),
                    |(_, series)| ChartEvent::ToggleSeries {
                        series: Arc::from(series),
                        visible: !visible.contains(series),
                    },
                );
                ChartProjectionAction::Event(event)
            })
            .map_err(|error| error.to_string())?
            .semantic_role(SemanticRole::GraphicalCustomControl)
            .message(ChartProjectionAction::Event(ChartEvent::Navigate(
                twinkle_charts::NavigationCommand::Select,
            )))
            .on_adjust_messages(
                ChartProjectionAction::Event(ChartEvent::Navigate(
                    twinkle_charts::NavigationCommand::Previous,
                )),
                ChartProjectionAction::Event(ChartEvent::Navigate(
                    twinkle_charts::NavigationCommand::Next,
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
        let actions = self.action_map();
        Ok(element.map_message(move |action| match action {
            ChartProjectionAction::Event(event) => {
                let (action, payload) = actions.event(event);
                message(action, payload)
            }
        }))
    }
}

pub(crate) fn inspection_payload(inspection: &twinkle_charts::ChartInspection) -> Value {
    json!({
        "kind": "inspect",
        "dataset": inspection.dataset.as_ref(),
        "row": inspection.row.0.as_ref(),
        "series": inspection.series.as_deref(),
        "revision": inspection.revision,
        "cells": inspection.cells.iter().map(|cell| json!({
            "column": cell.column.as_ref(),
            "unit": cell.unit.as_deref(),
            "formatted": cell.formatted,
            "value": match &cell.value {
                DataValue::Number(value) => json!(value),
                DataValue::Timestamp(value) => json!(value),
                DataValue::Category(value) => json!(value.as_ref()),
                DataValue::Boolean(value) => json!(value),
                DataValue::Null => Value::Null,
            },
        })).collect::<Vec<_>>(),
    })
}

fn parse_data(value: &Value, limits: &ChartLimits) -> Result<ChartData, String> {
    let revision = value
        .get("revision")
        .and_then(Value::as_u64)
        .filter(|revision| *revision <= 9_007_199_254_740_991)
        .ok_or("chart data revision must be a safe nonnegative integer")?;
    let datasets = value
        .get("datasets")
        .and_then(Value::as_array)
        .ok_or("chart data needs datasets")?;
    let mut parsed = Vec::with_capacity(datasets.len());
    for dataset in datasets {
        let id = bounded_string(dataset, "id", 256)?;
        let keys = dataset
            .get("keys")
            .and_then(Value::as_array)
            .ok_or("chart dataset needs keys")?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .filter(|key| !key.is_empty() && key.len() <= limits.max_label_bytes)
                    .map(RowKey::from)
                    .ok_or_else(|| "chart row key is invalid".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let columns = dataset
            .get("columns")
            .and_then(Value::as_array)
            .ok_or("chart dataset needs columns")?;
        let mut parsed_columns = Vec::with_capacity(columns.len());
        for column in columns {
            let name = bounded_string(column, "name", 256)?;
            let kind = column
                .get("type")
                .and_then(Value::as_str)
                .ok_or("chart column needs a type")?;
            let values = column
                .get("values")
                .and_then(Value::as_array)
                .ok_or("chart column needs values")?
                .iter()
                .map(|value| parse_value(value, kind, limits.max_label_bytes))
                .collect::<Result<Vec<_>, _>>()?;
            let mut parsed = Column::new(name, values);
            if let Some(unit) = column.get("unit") {
                parsed = parsed.with_unit(
                    unit.as_str()
                        .filter(|unit| !unit.is_empty() && unit.len() <= limits.max_label_bytes)
                        .ok_or("chart column unit is invalid")?,
                );
            }
            parsed_columns.push(parsed);
        }
        parsed.push(
            Dataset::new(DatasetId::from(id), keys, parsed_columns)
                .map_err(|error| error.to_string())?,
        );
    }
    ChartData::new(revision, parsed, limits).map_err(|error| error.to_string())
}

fn parse_value(value: &Value, kind: &str, max_label_bytes: usize) -> Result<DataValue, String> {
    if value.is_null() {
        return Ok(DataValue::Null);
    }
    match kind {
        "number" => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(DataValue::Number)
            .ok_or_else(|| "chart numeric value is invalid".into()),
        "timestamp" => value
            .as_i64()
            .filter(|value| value.unsigned_abs() <= 9_007_199_254_740_991)
            .map(DataValue::Timestamp)
            .ok_or_else(|| "chart timestamp is outside the safe integer range".into()),
        "category" => value
            .as_str()
            .filter(|value| value.len() <= max_label_bytes)
            .map(|value| DataValue::Category(Arc::from(value)))
            .ok_or_else(|| "chart category is invalid".into()),
        "boolean" => value
            .as_bool()
            .map(DataValue::Boolean)
            .ok_or_else(|| "chart boolean is invalid".into()),
        _ => Err("chart column type is unsupported".into()),
    }
}

fn parse_spec(value: &Value, limits: &ChartLimits) -> Result<twinkle_charts::ChartSpec, String> {
    let family = value
        .get("family")
        .and_then(Value::as_str)
        .ok_or("chart spec needs a family")?;
    let title = bounded_string(value, "title", 4096)?;
    let dataset = bounded_string(value, "dataset", 256)?;
    if family == "custom" {
        return parse_custom_spec(value, title, dataset, limits);
    }
    let field = |name: &str| -> Result<FieldRef, String> {
        Ok(FieldRef::new(
            dataset.clone(),
            bounded_string(value, name, 256)?,
        ))
    };
    let normalized = value.get("normalized").map_or(Ok(false), |value| {
        value.as_bool().ok_or("normalized must be boolean")
    })?;
    let mut spec = match family {
        "line" => Recipes::line(title, field("x")?, field("y")?),
        "area" => Recipes::area(title, field("x")?, field("y")?),
        "stacked-area" => Recipes::stacked_area(
            title,
            field("x")?,
            field("series")?,
            field("value")?,
            normalized,
        ),
        "scatter" => Recipes::scatter(title, field("x")?, field("y")?),
        "bubble" => Recipes::bubble(title, field("x")?, field("y")?, field("size")?),
        "bar" => Recipes::bar(title, field("category")?, field("value")?),
        "horizontal-bar" => Recipes::horizontal_bar(title, field("category")?, field("value")?),
        "grouped-bar" => {
            Recipes::grouped_bar(title, field("category")?, field("series")?, field("value")?)
        }
        "grouped-horizontal-bar" => Recipes::grouped_horizontal_bar(
            title,
            field("category")?,
            field("series")?,
            field("value")?,
        ),
        "stacked-bar" => Recipes::stacked_bar(
            title,
            field("category")?,
            field("series")?,
            field("value")?,
            normalized,
        ),
        "stacked-horizontal-bar" => Recipes::stacked_horizontal_bar(
            title,
            field("category")?,
            field("series")?,
            field("value")?,
            normalized,
        ),
        "pie" => Recipes::pie(title, field("category")?, field("value")?),
        "donut" => Recipes::donut(
            title,
            field("category")?,
            field("value")?,
            value
                .get("innerRadius")
                .and_then(Value::as_f64)
                .unwrap_or(0.55) as f32,
        ),
        "histogram" => Recipes::histogram(
            title,
            field("value")?,
            BinOptions {
                start: finite_number(value, "start")?,
                end: finite_number(value, "end")?,
                count: value
                    .get("bins")
                    .and_then(Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .filter(|value| *value > 0)
                    .ok_or("histogram bins must be positive")?,
                density: value
                    .get("density")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
        ),
        "heatmap" => Recipes::heatmap(title, field("x")?, field("y")?, field("color")?),
        "sparkline" => Recipes::sparkline(title, field("x")?, field("y")?),
        _ => return Err("chart recipe family is unsupported".into()),
    };
    if let Some(summary) = value.get("summary") {
        spec.summary = Arc::from(
            summary
                .as_str()
                .filter(|summary| !summary.is_empty() && summary.len() <= 4096)
                .ok_or("chart summary is invalid")?,
        );
    }
    if let Some(series) = value.get("series").and_then(Value::as_str)
        && !spec
            .layers
            .iter()
            .flat_map(|layer| layer.encodings.iter())
            .any(|encoding| encoding.channel == Channel::Series)
    {
        spec.layers[0].encodings.push(Encoding {
            channel: Channel::Series,
            field: FieldRef::new(dataset, series),
            scale: Some(Arc::from("series")),
        });
    }
    if spec
        .layers
        .iter_mut()
        .flat_map(|layer| layer.encodings.iter_mut())
        .filter(|encoding| encoding.channel == Channel::Series)
        .map(|encoding| encoding.scale = Some(Arc::from("series")))
        .count()
        > 0
    {
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
            title: None,
            toggleable: true,
        });
    }
    Ok(spec)
}

fn parse_custom_spec(
    value: &Value,
    title: String,
    default_dataset: String,
    limits: &ChartLimits,
) -> Result<ChartSpec, String> {
    let summary = value
        .get("summary")
        .map(|value| {
            value
                .as_str()
                .filter(|text| !text.is_empty() && text.len() <= limits.max_label_bytes)
                .map(str::to_owned)
                .ok_or("chart summary is invalid")
        })
        .transpose()?
        .unwrap_or_else(|| title.clone());
    let mut spec = ChartSpec::new(title, summary);
    spec.visibility_domain = match value
        .get("visibilityDomain")
        .and_then(Value::as_str)
        .unwrap_or("original")
    {
        "original" => VisibilityDomainPolicy::Original,
        "visible-series" => VisibilityDomainPolicy::VisibleSeries,
        _ => return Err("custom chart visibilityDomain is unsupported".into()),
    };

    for scale in bounded_array(value, "scales", limits.max_layers.saturating_mul(4))? {
        let name = bounded_string(scale, "name", limits.max_label_bytes)?;
        let kind_name = scale
            .get("kind")
            .and_then(Value::as_str)
            .ok_or("custom chart scale needs a kind")?;
        let kind = match kind_name {
            "linear" => ScaleKind::Linear,
            "log" => ScaleKind::Log {
                base: optional_finite(scale, "base")?.unwrap_or(10.0),
            },
            "time" => ScaleKind::Time,
            "ordinal" => ScaleKind::Ordinal,
            "band" => ScaleKind::Band {
                inner_padding: bounded_fraction(scale, "innerPadding", 0.1)?,
                outer_padding: bounded_fraction(scale, "outerPadding", 0.05)?,
            },
            "color" => ScaleKind::Color,
            "size" => ScaleKind::Size,
            _ => return Err("custom chart scale kind is unsupported".into()),
        };
        spec.scales.push(Scale {
            name: Arc::from(name),
            kind,
            domain: optional_domain(scale.get("domain"))?,
            clamp: optional_bool(scale, "clamp", true)?,
            reversed: optional_bool(scale, "reversed", false)?,
            unit: optional_bounded_string(scale, "unit", limits.max_label_bytes)?.map(Arc::from),
        });
    }

    for layer in bounded_array(value, "layers", limits.max_layers)? {
        let mark_value = layer.get("mark").ok_or("custom chart layer needs a mark")?;
        let mut parsed = Layer::new(parse_mark(mark_value)?);
        parsed.visible = optional_bool(layer, "visible", true)?;
        for encoding in bounded_array(layer, "encodings", 32)? {
            let channel = parse_channel(
                encoding
                    .get("channel")
                    .and_then(Value::as_str)
                    .ok_or("custom chart encoding needs a channel")?,
            )?;
            let dataset = optional_bounded_string(encoding, "dataset", 256)?
                .unwrap_or_else(|| default_dataset.clone());
            let field = bounded_string(encoding, "field", 256)?;
            parsed.encodings.push(Encoding {
                channel,
                field: FieldRef::new(dataset, field),
                scale: optional_bounded_string(encoding, "scale", limits.max_label_bytes)?
                    .map(Arc::from),
            });
        }
        for transform in bounded_array(layer, "transforms", limits.max_transforms)? {
            parsed.transforms.push(parse_transform(transform, limits)?);
        }
        spec.layers.push(parsed);
    }
    if spec.layers.is_empty() {
        return Err("custom chart needs at least one layer".into());
    }

    for axis in bounded_array(value, "axes", limits.max_layers.saturating_mul(2))? {
        spec.axes.push(Axis {
            scale: Arc::from(bounded_string(axis, "scale", limits.max_label_bytes)?),
            title: optional_bounded_string(axis, "title", limits.max_label_bytes)?.map(Arc::from),
            unit: optional_bounded_string(axis, "unit", limits.max_label_bytes)?.map(Arc::from),
            tick_count: optional_usize(axis, "tickCount")?,
            grid: optional_bool(axis, "grid", false)?,
        });
    }
    for legend in bounded_array(value, "legends", limits.max_series)? {
        spec.legends.push(Legend {
            scale: Arc::from(bounded_string(legend, "scale", limits.max_label_bytes)?),
            title: optional_bounded_string(legend, "title", limits.max_label_bytes)?.map(Arc::from),
            toggleable: optional_bool(legend, "toggleable", false)?,
        });
    }
    for annotation in bounded_array(value, "annotations", limits.max_annotations)? {
        spec.annotations.push(parse_annotation(annotation, limits)?);
    }
    spec.facet = value
        .get("facet")
        .map(|facet| {
            Ok::<_, String>(Facet {
                row: optional_field_ref(facet, "row", &default_dataset)?,
                column: optional_field_ref(facet, "column", &default_dataset)?,
                scales: match facet
                    .get("scales")
                    .and_then(Value::as_str)
                    .unwrap_or("shared")
                {
                    "shared" => FacetScalePolicy::Shared,
                    "independent" => FacetScalePolicy::Independent,
                    _ => return Err("custom chart facet scale policy is unsupported".into()),
                },
            })
        })
        .transpose()?;
    Ok(spec)
}

fn parse_mark(value: &Value) -> Result<Mark, String> {
    let (kind, options) = value
        .as_str()
        .map(|kind| (kind, None))
        .or_else(|| {
            value
                .get("kind")
                .and_then(Value::as_str)
                .map(|kind| (kind, Some(value)))
        })
        .ok_or("custom chart mark is invalid")?;
    let options = options.unwrap_or(&Value::Null);
    Ok(match kind {
        "rect" => Mark::Rect,
        "bar" => Mark::Bar {
            orientation: match options
                .get("orientation")
                .and_then(Value::as_str)
                .unwrap_or("vertical")
            {
                "vertical" => Orientation::Vertical,
                "horizontal" => Orientation::Horizontal,
                _ => return Err("custom chart bar orientation is unsupported".into()),
            },
        },
        "line" => Mark::Line {
            interpolation: parse_interpolation(options)?,
            points: optional_bool(options, "points", false)?,
        },
        "area" => Mark::Area {
            interpolation: parse_interpolation(options)?,
            baseline: optional_finite(options, "baseline")?.unwrap_or(0.0),
        },
        "point" => Mark::Point {
            symbol: match options
                .get("symbol")
                .and_then(Value::as_str)
                .unwrap_or("circle")
            {
                "circle" => Symbol::Circle,
                "square" => Symbol::Square,
                "triangle" => Symbol::Triangle,
                "diamond" => Symbol::Diamond,
                "cross" => Symbol::Cross,
                _ => return Err("custom chart point symbol is unsupported".into()),
            },
        },
        "arc" => {
            let inner_radius = optional_finite(options, "innerRadius")?.unwrap_or(0.0);
            if !(0.0..1.0).contains(&inner_radius) {
                return Err("custom chart innerRadius must be in [0, 1)".into());
            }
            Mark::Arc {
                inner_radius: inner_radius as f32,
            }
        }
        "rule" => Mark::Rule,
        "text" => Mark::Text,
        _ => return Err("custom chart mark kind is unsupported".into()),
    })
}

fn parse_interpolation(value: &Value) -> Result<Interpolation, String> {
    match value
        .get("interpolation")
        .and_then(Value::as_str)
        .unwrap_or("straight")
    {
        "straight" => Ok(Interpolation::Straight),
        "step-before" => Ok(Interpolation::StepBefore),
        "step-after" => Ok(Interpolation::StepAfter),
        _ => Err("custom chart interpolation is unsupported".into()),
    }
}

fn parse_channel(value: &str) -> Result<Channel, String> {
    Ok(match value {
        "x" => Channel::X,
        "x2" => Channel::X2,
        "y" => Channel::Y,
        "y2" => Channel::Y2,
        "series" => Channel::Series,
        "group" => Channel::Group,
        "fill" => Channel::Fill,
        "stroke" => Channel::Stroke,
        "opacity" => Channel::Opacity,
        "symbol" => Channel::Symbol,
        "size" => Channel::Size,
        "text" => Channel::Text,
        _ => return Err("custom chart encoding channel is unsupported".into()),
    })
}

fn parse_transform(value: &Value, limits: &ChartLimits) -> Result<Transform, String> {
    let kind = value
        .get("kind")
        .and_then(Value::as_str)
        .ok_or("custom chart transform needs a kind")?;
    Ok(match kind {
        "sort" => Transform::Sort {
            field: bounded_string(value, "field", 256)?,
            descending: optional_bool(value, "descending", false)?,
        },
        "bin" => Transform::Bin {
            field: bounded_string(value, "field", 256)?,
            output: bounded_string(value, "output", 256)?,
            options: BinOptions {
                start: finite_number(value, "start")?,
                end: finite_number(value, "end")?,
                count: required_positive_usize(value, "count")?,
                density: optional_bool(value, "density", false)?,
            },
        },
        "aggregate" => Transform::Aggregate {
            group_by: bounded_array(value, "groupBy", limits.max_columns)?
                .iter()
                .map(|field| {
                    field
                        .as_str()
                        .filter(|field| !field.is_empty() && field.len() <= 256)
                        .map(str::to_owned)
                        .ok_or_else(|| "custom chart aggregate groupBy field is invalid".into())
                })
                .collect::<Result<Vec<_>, String>>()?,
            field: optional_bounded_string(value, "field", 256)?,
            output: bounded_string(value, "output", 256)?,
            operation: match value
                .get("operation")
                .and_then(Value::as_str)
                .ok_or("custom chart aggregate needs an operation")?
            {
                "count" => AggregateOp::Count,
                "sum" => AggregateOp::Sum,
                "mean" => AggregateOp::Mean,
                "min" => AggregateOp::Min,
                "max" => AggregateOp::Max,
                _ => return Err("custom chart aggregate operation is unsupported".into()),
            },
            missing: match value
                .get("missing")
                .and_then(Value::as_str)
                .unwrap_or("omit")
            {
                "omit" => MissingPolicy::Omit,
                "propagate" => MissingPolicy::Propagate,
                _ => return Err("custom chart aggregate missing policy is unsupported".into()),
            },
        },
        "stack" => Transform::Stack {
            category: bounded_string(value, "category", 256)?,
            series: bounded_string(value, "series", 256)?,
            value: bounded_string(value, "value", 256)?,
            mode: match value.get("mode").and_then(Value::as_str).unwrap_or("raw") {
                "raw" => StackMode::Raw,
                "normalized" => StackMode::Normalized,
                _ => return Err("custom chart stack mode is unsupported".into()),
            },
        },
        _ => return Err("custom chart transform kind is unsupported".into()),
    })
}

fn parse_annotation(value: &Value, limits: &ChartLimits) -> Result<Annotation, String> {
    let kind = value
        .get("kind")
        .and_then(Value::as_str)
        .ok_or("custom chart annotation needs a kind")?;
    Ok(match kind {
        "rule" => Annotation::Rule {
            scale: Arc::from(bounded_string(value, "scale", limits.max_label_bytes)?),
            value: finite_number(value, "value")?,
            label: optional_bounded_string(value, "label", limits.max_label_bytes)?.map(Arc::from),
            include_in_domain: optional_bool(value, "includeInDomain", false)?,
        },
        "band" => Annotation::Band {
            scale: Arc::from(bounded_string(value, "scale", limits.max_label_bytes)?),
            start: finite_number(value, "start")?,
            end: finite_number(value, "end")?,
            label: optional_bounded_string(value, "label", limits.max_label_bytes)?.map(Arc::from),
            include_in_domain: optional_bool(value, "includeInDomain", false)?,
        },
        "text" => Annotation::Text {
            text: Arc::from(bounded_string(value, "text", limits.max_label_bytes)?),
            x: finite_number(value, "x")?,
            y: finite_number(value, "y")?,
        },
        _ => return Err("custom chart annotation kind is unsupported".into()),
    })
}

fn bounded_array<'a>(value: &'a Value, name: &str, limit: usize) -> Result<&'a [Value], String> {
    let values = match value.get(name) {
        None => &[],
        Some(value) => value
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| format!("custom chart {name} must be an array"))?,
    };
    if values.len() > limit {
        return Err(format!("custom chart {name} exceeds its limit"));
    }
    Ok(values)
}

fn optional_bounded_string(
    value: &Value,
    name: &str,
    limit: usize,
) -> Result<Option<String>, String> {
    value
        .get(name)
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.is_empty() && value.len() <= limit)
                .map(str::to_owned)
                .ok_or_else(|| format!("custom chart {name} is invalid"))
        })
        .transpose()
}

fn optional_bool(value: &Value, name: &str, default: bool) -> Result<bool, String> {
    value
        .get(name)
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| format!("custom chart {name} must be boolean"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn optional_finite(value: &Value, name: &str) -> Result<Option<f64>, String> {
    value
        .get(name)
        .map(|value| {
            value
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or_else(|| format!("custom chart {name} must be finite"))
        })
        .transpose()
}

fn bounded_fraction(value: &Value, name: &str, default: f64) -> Result<f64, String> {
    let value = optional_finite(value, name)?.unwrap_or(default);
    (0.0..1.0)
        .contains(&value)
        .then_some(value)
        .ok_or_else(|| format!("custom chart {name} must be in [0, 1)"))
}

fn optional_usize(value: &Value, name: &str) -> Result<Option<usize>, String> {
    value
        .get(name)
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| format!("custom chart {name} must be a nonnegative integer"))
        })
        .transpose()
}

fn required_positive_usize(value: &Value, name: &str) -> Result<usize, String> {
    optional_usize(value, name)?
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("custom chart {name} must be positive"))
}

fn optional_field_ref(
    value: &Value,
    name: &str,
    default_dataset: &str,
) -> Result<Option<FieldRef>, String> {
    let Some(field) = value.get(name) else {
        return Ok(None);
    };
    if let Some(field) = field.as_str() {
        if field.is_empty() || field.len() > 256 {
            return Err(format!("custom chart facet {name} is invalid"));
        }
        return Ok(Some(FieldRef::new(default_dataset, field)));
    }
    Ok(Some(FieldRef::new(
        optional_bounded_string(field, "dataset", 256)?
            .unwrap_or_else(|| default_dataset.to_owned()),
        bounded_string(field, "field", 256)?,
    )))
}

fn parse_state(value: Option<&Value>) -> Result<ChartState, String> {
    let Some(value) = value else {
        return Ok(ChartState::default());
    };
    let visible_series = value
        .get("visibleSeries")
        .map(|value| {
            value
                .as_array()
                .ok_or("visibleSeries must be an array")?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .filter(|value| value.len() <= 4096)
                        .map(Arc::from)
                        .ok_or("visible series label is invalid")
                })
                .collect::<Result<BTreeSet<_>, _>>()
        })
        .transpose()?;
    let selected_rows = value
        .get("selectedRows")
        .map(|value| {
            value
                .as_array()
                .ok_or("selectedRows must be an array")?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .filter(|value| !value.is_empty() && value.len() <= 4096)
                        .map(RowKey::from)
                        .ok_or("selected row key is invalid")
                })
                .collect::<Result<BTreeSet<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let viewport = value
        .get("viewport")
        .map(|viewport| -> Result<Viewport, String> {
            Ok(Viewport {
                x: optional_domain(viewport.get("x"))?,
                y: optional_domain(viewport.get("y"))?,
            })
        })
        .transpose()?;
    Ok(ChartState {
        selected_rows,
        visible_series,
        viewport,
    })
}

fn parse_controlled_inspection(value: Option<&Value>) -> Result<Option<(String, RowKey)>, String> {
    let Some(value) = value.and_then(|value| value.get("inspection")) else {
        return Ok(None);
    };
    let dataset = bounded_string(value, "dataset", 256)?;
    let row = bounded_string(value, "row", 4096)?;
    Ok(Some((dataset, RowKey::from(row))))
}

fn optional_domain(value: Option<&Value>) -> Result<Option<[f64; 2]>, String> {
    let Some(value) = value else { return Ok(None) };
    let values = value
        .as_array()
        .filter(|values| values.len() == 2)
        .ok_or("viewport domain needs two numbers")?;
    let domain = [
        values[0].as_f64().ok_or("invalid viewport domain")?,
        values[1].as_f64().ok_or("invalid viewport domain")?,
    ];
    if domain.into_iter().any(|value| !value.is_finite()) || domain[0] >= domain[1] {
        return Err("viewport domain must be finite and increasing".into());
    }
    Ok(Some(domain))
}

fn bounded_string(value: &Value, name: &str, max: usize) -> Result<String, String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= max)
        .map(str::to_owned)
        .ok_or_else(|| format!("chart {name} must contain 1 to {max} bytes"))
}

fn dimension(value: &Value, name: &str) -> Result<f32, String> {
    value
        .get(name)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && (1.0..=8192.0).contains(value))
        .map(|value| value as f32)
        .ok_or_else(|| format!("chart {name} must be 1 to 8192"))
}

fn finite_number(value: &Value, name: &str) -> Result<f64, String> {
    value
        .get(name)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("chart {name} must be finite"))
}

fn action(value: &Value, name: &str) -> Result<Option<usize>, String> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| format!("chart {name} is invalid")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declaration() -> Value {
        json!({
            "kind":"chart","id":"sales","width":420,"height":240,
            "tableAction":7,"toggleAction":8,"chartNavigateAction":9,
            "chartSelectAction":10,"chartInspectAction":11,
            "chartViewportAction":12,"chartViewportCommandAction":13,
            "chartPanAction":14,"chartZoomAction":15,"children":[],
            "data":{"revision":3,"datasets":[{
                "id":"sales","keys":["north-jan","south-jan","north-feb","south-feb"],
                "columns":[
                    {"name":"month","type":"number","values":[1,1,2,2]},
                    {"name":"region","type":"category","values":["North","South","North","South"]},
                    {"name":"revenue","type":"number","unit":"USD","values":[10,20,15,25]}
                ]
            }]},
            "spec":{"family":"line","title":"Revenue","summary":"Monthly revenue by region",
                "dataset":"sales","x":"month","y":"revenue","series":"region"},
            "state":{"visibleSeries":["North","South"],
                "inspection":{"dataset":"sales","row":"north-jan"}}
        })
    }

    #[test]
    fn typed_json_chart_uses_native_engine_and_bounded_callbacks() {
        use crate::components::{PluginMessage, PluginUiMessage};

        let chart = ChartDeclaration::parse(&declaration(), Some("dashboard".into())).unwrap();
        assert_eq!(chart.snapshot.input.unwrap().revisions.content, 3);
        assert!(
            chart
                .snapshot
                .output
                .paint
                .iter()
                .any(|command| matches!(command, twinkle::backend::PaintCommand::Path { .. }))
        );
        assert_eq!(chart.snapshot.output.interactions.len(), 4);
        assert!(chart.snapshot.output.semantics.iter().any(|semantic| {
            semantic.id.as_str() == "chart/inspection"
                && semantic.role == SemanticRole::Tooltip
                && semantic
                    .label
                    .as_deref()
                    .is_some_and(|label| label.contains("10 USD"))
        }));
        let (action, payload) = chart.event(ChartEvent::ToggleSeries {
            series: Arc::from("North"),
            visible: false,
        });
        assert_eq!(action, 8);
        assert_eq!(payload, json!({"series":"North","visible":false}));
        assert_eq!(
            chart.event(ChartEvent::Navigate(
                twinkle_charts::NavigationCommand::Next
            )),
            (9, json!({"kind":"navigate","command":"Next"}))
        );
        assert_eq!(
            chart
                .event(ChartEvent::Select {
                    dataset: Arc::from("sales"),
                    row: RowKey::from("north-jan"),
                    series: Some(Arc::from("North")),
                    revision: 3,
                })
                .0,
            10
        );
        assert_eq!(
            chart
                .event(ChartEvent::Inspect {
                    dataset: Arc::from("sales"),
                    row: RowKey::from("north-jan"),
                    revision: 3,
                })
                .0,
            11
        );
        assert_eq!(
            chart
                .event(ChartEvent::ViewportChanged(Viewport {
                    x: Some([0.0, 1.0]),
                    y: None,
                }))
                .0,
            12
        );
        assert_eq!(
            chart
                .event(ChartEvent::Viewport(twinkle_charts::ViewportCommand::Reset))
                .0,
            13
        );

        let element = twinkle::Component::into_element(
            chart
                .element(PluginMessage::Chart)
                .expect("accepted chart projects"),
        )
        .on_pointer_move_seeded(
            PluginMessage::ChartInspect(11, chart.inspector.clone()),
            PluginMessage::pointer,
        )
        .on_drag(PluginMessage::ChartDrag(14), PluginMessage::drag)
        .on_wheel_seeded(PluginMessage::ChartWheel(15), PluginMessage::wheel);
        let frame = twinkle::UiFrame::layout(element, Rect::new(0.0, 0.0, 420.0, 240.0));
        let pointer = frame.handle_event(
            &mut twinkle::UiStateStore::default(),
            twinkle::UiEvent::PointerMoved(twinkle::Point { x: 210.0, y: 120.0 }),
        );
        assert_eq!(pointer.messages.len(), 1);
        let PluginMessage::Chart(action, payload) = &pointer.messages[0] else {
            panic!("expected exact chart inspection message")
        };
        assert_eq!(*action, 11);
        assert_eq!(payload["kind"], "inspect");
        assert_eq!(payload["revision"], 3);
        assert_eq!(payload["cells"].as_array().unwrap().len(), 3);

        let point = twinkle::Point { x: 210.0, y: 120.0 };
        let wheel = frame.handle_event(
            &mut twinkle::UiStateStore::default(),
            twinkle::UiEvent::Scroll {
                point,
                delta_y: -12.0,
            },
        );
        assert_eq!(
            wheel.messages,
            vec![PluginMessage::Chart(
                15,
                json!({"kind":"zoom","x":210.0,"y":120.0,"deltaX":0.0,"deltaY":-12.0})
            )]
        );

        let mut state = twinkle::UiStateStore::default();
        frame.handle_event(&mut state, twinkle::UiEvent::PointerPressed(point));
        let dragged = frame.handle_event(
            &mut state,
            twinkle::UiEvent::PointerMoved(twinkle::Point { x: 220.0, y: 125.0 }),
        );
        let PluginMessage::Chart(action, payload) = dragged
            .messages
            .iter()
            .find(|message| matches!(message, PluginMessage::Chart(14, _)))
            .expect("expected chart pan message")
        else {
            unreachable!()
        };
        assert_eq!(*action, 14);
        assert_eq!(payload["kind"], "pan");
        assert_eq!(payload["phase"], "move");
        assert_eq!(payload["x"], 220.0);
    }

    #[test]
    fn retained_chart_resize_reuses_authority_and_preserves_controlled_inspection() {
        let mut value = declaration();
        let mut chart = ChartDeclaration::parse(&value, None).unwrap();
        let original = chart.clone();
        let old_hit = chart.retained.0.borrow().hits()[0].bounds;
        assert_eq!(
            chart
                .retained
                .0
                .borrow()
                .retained_diagnostics()
                .update_calls,
            1
        );

        value["width"] = json!(840);
        value["height"] = json!(360);
        chart.resize(&value).unwrap();
        assert_eq!(
            chart
                .retained
                .0
                .borrow()
                .retained_diagnostics()
                .update_calls,
            2
        );
        assert_eq!(
            chart.snapshot.input.unwrap().bounds,
            Rect::new(0.0, 0.0, 840.0, 360.0)
        );
        assert_ne!(chart.retained.0.borrow().hits()[0].bounds, old_hit);
        assert!(chart.snapshot.output.semantics.iter().any(|semantic| {
            semantic.id.as_str() == "chart/inspection"
                && semantic
                    .label
                    .as_deref()
                    .is_some_and(|label| label.contains("10 USD"))
        }));

        let accepted = chart.clone();
        value["width"] = json!(0);
        assert!(chart.resize(&value).is_err());
        assert_eq!(chart, accepted);
        assert_ne!(chart, original);
    }

    #[test]
    fn custom_grammar_descriptor_lowers_to_the_native_composition_engine() {
        let mut value = declaration();
        value["spec"] = json!({
            "family":"custom","title":"Revenue composition","summary":"Layered revenue",
            "dataset":"sales","visibilityDomain":"visible-series",
            "scales":[
                {"name":"x","kind":"linear"},
                {"name":"y","kind":"linear","domain":[0,30],"unit":"USD"},
                {"name":"series","kind":"color"}
            ],
            "layers":[
                {"mark":{"kind":"bar","orientation":"vertical"},"encodings":[
                    {"channel":"x","field":"month","scale":"x"},
                    {"channel":"y","field":"revenue","scale":"y"},
                    {"channel":"series","field":"region","scale":"series"}
                ]},
                {"mark":{"kind":"line","interpolation":"step-after","points":true},
                 "transforms":[{"kind":"sort","field":"month"}],"encodings":[
                    {"channel":"x","field":"month","scale":"x"},
                    {"channel":"y","field":"revenue","scale":"y"},
                    {"channel":"series","field":"region","scale":"series"}
                ]}
            ],
            "axes":[
                {"scale":"x","title":"Month","tickCount":4,"grid":true},
                {"scale":"y","title":"Revenue","unit":"USD","grid":true}
            ],
            "legends":[{"scale":"series","title":"Region","toggleable":true}],
            "annotations":[
                {"kind":"rule","scale":"y","value":20,"label":"Target"},
                {"kind":"band","scale":"y","start":10,"end":15,"label":"Expected"},
                {"kind":"text","text":"Quarter","x":2,"y":25}
            ],
            "facet":{"column":"region","scales":"shared"}
        });
        let spec = parse_spec(&value["spec"], &ChartLimits::default()).unwrap();
        assert_eq!(spec.layers.len(), 2);
        assert_eq!(spec.scales.len(), 3);
        assert_eq!(spec.axes.len(), 2);
        assert_eq!(spec.annotations.len(), 3);
        assert_eq!(
            spec.visibility_domain,
            VisibilityDomainPolicy::VisibleSeries
        );
        assert!(matches!(
            spec.layers[1].mark,
            Mark::Line {
                interpolation: Interpolation::StepAfter,
                points: true
            }
        ));
        let chart = ChartDeclaration::parse(&value, None).unwrap();
        assert!(
            chart
                .snapshot
                .output
                .paint
                .iter()
                .any(|command| matches!(command, twinkle::backend::PaintCommand::Path { .. }))
        );
    }

    #[test]
    fn custom_grammar_descriptor_covers_every_public_primitive() {
        let encodings = [
            "x", "x2", "y", "y2", "series", "group", "fill", "stroke", "opacity", "symbol", "size",
            "text",
        ]
        .into_iter()
        .map(|channel| json!({"channel":channel,"field":"revenue"}))
        .collect::<Vec<_>>();
        let value = json!({
            "family":"custom","title":"Everything","dataset":"sales",
            "visibilityDomain":"original",
            "scales":[
                {"name":"linear","kind":"linear"},{"name":"log","kind":"log","base":2},
                {"name":"time","kind":"time"},{"name":"ordinal","kind":"ordinal"},
                {"name":"band","kind":"band","innerPadding":0.2,"outerPadding":0.1},
                {"name":"color","kind":"color"},{"name":"size","kind":"size"}
            ],
            "layers":[
                {"mark":"rect","encodings":encodings,"transforms":[
                    {"kind":"sort","field":"month","descending":true},
                    {"kind":"bin","field":"revenue","output":"bucket","start":0,"end":30,"count":3},
                    {"kind":"aggregate","groupBy":["region"],"field":"revenue","output":"total","operation":"sum","missing":"omit"},
                    {"kind":"stack","category":"month","series":"region","value":"revenue","mode":"normalized"}
                ]},
                {"mark":{"kind":"bar","orientation":"horizontal"},"encodings":[]},
                {"mark":{"kind":"line","interpolation":"step-before","points":true},"encodings":[]},
                {"mark":{"kind":"area","interpolation":"step-after","baseline":1},"encodings":[]},
                {"mark":{"kind":"point","symbol":"diamond"},"encodings":[]},
                {"mark":{"kind":"arc","innerRadius":0.5},"encodings":[]},
                {"mark":"rule","encodings":[]},{"mark":"text","encodings":[]}
            ],
            "axes":[{"scale":"linear"}],"legends":[{"scale":"color"}],
            "annotations":[
                {"kind":"rule","scale":"linear","value":2},
                {"kind":"band","scale":"linear","start":1,"end":3},
                {"kind":"text","text":"note","x":1,"y":2}
            ],
            "facet":{"row":{"dataset":"sales","field":"region"},"column":"month","scales":"independent"}
        });
        let spec = parse_spec(&value, &ChartLimits::default()).unwrap();
        assert_eq!(spec.scales.len(), 7);
        assert_eq!(spec.layers.len(), 8);
        assert_eq!(spec.layers[0].encodings.len(), 12);
        assert_eq!(spec.layers[0].transforms.len(), 4);
        assert_eq!(spec.annotations.len(), 3);
        assert_eq!(spec.facet.unwrap().scales, FacetScalePolicy::Independent);
    }

    #[test]
    fn recipe_and_custom_jsx_descriptors_have_native_snapshot_parity() {
        let recipe = declaration();
        let mut custom = recipe.clone();
        custom["spec"] = json!({
            "family":"custom","title":"Revenue","summary":"Monthly revenue by region",
            "dataset":"sales",
            "scales":[
                {"name":"x","kind":"linear","clamp":false},
                {"name":"y","kind":"linear","clamp":false,"reversed":true},
                {"name":"series","kind":"color","clamp":true}
            ],
            "layers":[{"mark":{"kind":"line"},"encodings":[
                {"channel":"x","field":"month","scale":"x"},
                {"channel":"y","field":"revenue","scale":"y"},
                {"channel":"series","field":"region","scale":"series"}
            ]}],
            "legends":[{"scale":"series","toggleable":true}]
        });
        let recipe = ChartDeclaration::parse(&recipe, None).unwrap();
        let custom = ChartDeclaration::parse(&custom, None).unwrap();
        assert_eq!(recipe.snapshot, custom.snapshot);
        let point = twinkle::Point { x: 210.0, y: 120.0 };
        assert_eq!(
            recipe.inspector.inspect_nearest(point),
            custom.inspector.inspect_nearest(point)
        );
    }

    #[test]
    fn hostile_or_incoherent_chart_descriptors_are_rejected() {
        let mut invalid = declaration();
        invalid["data"]["datasets"][0]["columns"][0]["values"] = json!([1]);
        assert!(ChartDeclaration::parse(&invalid, None).is_err());
        let mut forged_revision = declaration();
        forged_revision["data"]["revision"] = json!(9_007_199_254_740_992_u64);
        assert!(ChartDeclaration::parse(&forged_revision, None).is_err());
        let mut invalid_viewport = declaration();
        invalid_viewport["state"]["viewport"] = json!({"x":[10,1]});
        assert!(ChartDeclaration::parse(&invalid_viewport, None).is_err());
        let mut invalid_inspection = declaration();
        invalid_inspection["state"]["inspection"] = json!({"dataset":"sales","row":4});
        assert!(ChartDeclaration::parse(&invalid_inspection, None).is_err());
        let mut malformed_grammar = declaration();
        malformed_grammar["spec"] = json!({
            "family":"custom","title":"Bad","dataset":"sales","layers":"not-an-array"
        });
        assert!(ChartDeclaration::parse(&malformed_grammar, None).is_err());

        let mut removed_inspection = declaration();
        removed_inspection["state"]["inspection"] = json!({"dataset":"sales","row":"removed"});
        let chart = ChartDeclaration::parse(&removed_inspection, None).unwrap();
        assert!(
            chart
                .snapshot
                .output
                .semantics
                .iter()
                .all(|semantic| semantic.id.as_str() != "chart/inspection")
        );
    }

    #[test]
    fn jsx_recipe_surface_admits_every_p0_family_and_variant() {
        let recipes = [
            json!({"family":"line","title":"Line","dataset":"sales","x":"month","y":"revenue","series":"region"}),
            json!({"family":"area","title":"Area","dataset":"sales","x":"month","y":"revenue"}),
            json!({"family":"stacked-area","title":"Stacked area","dataset":"sales","x":"month","series":"region","value":"revenue"}),
            json!({"family":"stacked-area","title":"Normalized area","dataset":"sales","x":"month","series":"region","value":"revenue","normalized":true}),
            json!({"family":"scatter","title":"Scatter","dataset":"sales","x":"orders","y":"revenue"}),
            json!({"family":"bubble","title":"Bubble","dataset":"sales","x":"orders","y":"revenue","size":"orders"}),
            json!({"family":"bar","title":"Bar","dataset":"sales","category":"month","value":"revenue"}),
            json!({"family":"horizontal-bar","title":"Horizontal bar","dataset":"sales","category":"month","value":"revenue"}),
            json!({"family":"grouped-bar","title":"Grouped bar","dataset":"sales","category":"month","series":"region","value":"revenue"}),
            json!({"family":"grouped-horizontal-bar","title":"Grouped horizontal bar","dataset":"sales","category":"month","series":"region","value":"revenue"}),
            json!({"family":"stacked-bar","title":"Stacked bar","dataset":"sales","category":"month","series":"region","value":"revenue"}),
            json!({"family":"stacked-bar","title":"Normalized stacked bar","dataset":"sales","category":"month","series":"region","value":"revenue","normalized":true}),
            json!({"family":"stacked-horizontal-bar","title":"Stacked horizontal bar","dataset":"sales","category":"month","series":"region","value":"revenue"}),
            json!({"family":"stacked-horizontal-bar","title":"Normalized stacked horizontal bar","dataset":"sales","category":"month","series":"region","value":"revenue","normalized":true}),
            json!({"family":"pie","title":"Pie","dataset":"sales","category":"region","value":"revenue"}),
            json!({"family":"donut","title":"Donut","dataset":"sales","category":"region","value":"revenue","innerRadius":0.55}),
            json!({"family":"histogram","title":"Histogram","dataset":"sales","value":"orders","start":0,"end":16,"bins":4}),
            json!({"family":"heatmap","title":"Heatmap","dataset":"sales","x":"month","y":"orders","color":"revenue"}),
            json!({"family":"sparkline","title":"Sparkline","dataset":"sales","x":"month","y":"revenue"}),
        ];
        for recipe in recipes {
            let family = recipe["family"].as_str().unwrap().to_owned();
            let mut value = declaration();
            value["data"]["datasets"][0]["columns"]
                .as_array_mut()
                .unwrap()
                .push(json!({"name":"orders","type":"number","values":[3,8,5,12]}));
            value["spec"] = recipe;
            let chart = ChartDeclaration::parse(&value, None)
                .unwrap_or_else(|error| panic!("{family} JSX recipe failed admission: {error}"));
            assert!(!chart.snapshot.output.paint.is_empty(), "{family}");
            assert!(
                chart
                    .snapshot
                    .output
                    .semantics
                    .iter()
                    .any(|semantic| { semantic.role == SemanticRole::GraphicalCustomControl }),
                "{family}"
            );
        }
    }
}

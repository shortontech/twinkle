use std::{collections::BTreeSet, sync::Arc};

use serde_json::{Value, json};
use twinkle::retained_surface::{
    RetainedSurfaceInput, RetainedSurfaceRevisions, RetainedSurfaceSnapshot,
};
use twinkle::{Rect, SemanticRole, UiId};
use twinkle_charts::{
    BinOptions, Channel, ChartData, ChartEvent, ChartInspector, ChartLimits, ChartModel,
    ChartState, ChartTheme, ChartView, Column, DataValue, Dataset, DatasetId, Encoding, FieldRef,
    Legend, Recipes, RowKey, Scale, ScaleKind, Viewport, project_inspection_overlay,
};

#[derive(Clone)]
enum ChartProjectionAction {
    Event(ChartEvent),
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
}

impl ChartDeclaration {
    pub fn parse(value: &Value, class_name: Option<String>) -> Result<Self, String> {
        let id = bounded_string(value, "id", 128)?;
        let width = dimension(value, "width")?;
        let height = dimension(value, "height")?;
        let limits = ChartLimits::default();
        let data = Arc::new(parse_data(
            value.get("data").ok_or("chart needs data")?,
            &limits,
        )?);
        let spec = Arc::new(parse_spec(value.get("spec").ok_or("chart needs a spec")?)?);
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
            .and_then(|(dataset, row)| inspector.inspect_identity(&dataset, &row))
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
        })
    }

    pub fn event(&self, event: ChartEvent) -> (usize, Value) {
        match event {
            ChartEvent::Navigate(command) => (
                self.navigate_action.unwrap_or(self.table_action),
                json!({"kind":"navigate","command":format!("{command:?}")}),
            ),
            ChartEvent::ToggleSeries { series, visible } => (
                self.toggle_action.unwrap_or(self.table_action),
                json!({"series": series.as_ref(), "visible": visible}),
            ),
            ChartEvent::TableMode(visible) => (self.table_action, json!(visible)),
            ChartEvent::Select {
                dataset,
                row,
                series,
                revision,
            } => (
                self.select_action.unwrap_or(self.table_action),
                json!({"kind":"select","dataset":dataset.as_ref(),"row":row.0.as_ref(),"series":series.as_deref(),"revision":revision}),
            ),
            ChartEvent::Inspect {
                dataset,
                row,
                revision,
            } => (
                self.inspect_action.unwrap_or(self.table_action),
                json!({"kind":"inspect","dataset":dataset.as_ref(),"row":row.0.as_ref(),"revision":revision}),
            ),
            ChartEvent::ViewportChanged(viewport) => (
                self.viewport_action.unwrap_or(self.table_action),
                json!({"kind":"viewport","x":viewport.x,"y":viewport.y}),
            ),
            ChartEvent::Viewport(command) => (
                self.viewport_command_action.unwrap_or(self.table_action),
                json!({"kind":"viewportCommand","command":format!("{command:?}")}),
            ),
        }
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
        Ok(element.map_message(move |action| match action {
            ChartProjectionAction::Event(event) => {
                let (action, payload) = self.event(event);
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

fn parse_spec(value: &Value) -> Result<twinkle_charts::ChartSpec, String> {
    let family = value
        .get("family")
        .and_then(Value::as_str)
        .ok_or("chart spec needs a family")?;
    let title = bounded_string(value, "title", 4096)?;
    let dataset = bounded_string(value, "dataset", 256)?;
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
}

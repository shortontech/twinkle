use std::{collections::BTreeSet, sync::Arc};

use twinkle_ui::{Button, Column as UiColumn, Component, Row as UiRow, SemanticRole, Text, UiId};

use crate::{ChartData, ChartError, ChartLimits, ChartStage, DataValue, RowKey};

/// Exact accessible table cell, independent of plotted reduction.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartTableCell {
    pub column: Arc<str>,
    pub unit: Option<Arc<str>>,
    pub value: DataValue,
    pub formatted: String,
}

/// Stable table row with its absolute position.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartTableRow {
    pub key: RowKey,
    pub position: usize,
    pub cells: Vec<ChartTableCell>,
}

/// One bounded, announced page of an exact chart data table.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartTablePage {
    pub dataset: Arc<str>,
    pub revision: u64,
    pub offset: usize,
    pub total_rows: usize,
    pub source_total_rows: usize,
    pub hidden_row_count: usize,
    pub rows: Vec<ChartTableRow>,
    pub hidden_series_included: bool,
}

/// Controlled filtering policy for the exact table projection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChartTableFilter {
    pub series_column: Option<Arc<str>>,
    pub visible_series: Option<BTreeSet<Arc<str>>>,
    pub include_hidden_series: bool,
}

/// Typed native table request returned through the application's message path.
#[derive(Clone, Debug, PartialEq)]
pub enum ChartTableEvent {
    Inspect {
        dataset: Arc<str>,
        row: RowKey,
        revision: u64,
    },
    Page {
        dataset: Arc<str>,
        offset: usize,
    },
    SetHiddenSeriesIncluded(bool),
}

/// Virtualized/paged table projection for every chart family.
#[derive(Clone, Debug)]
pub struct ChartTable {
    data: Arc<ChartData>,
    limits: ChartLimits,
}

impl ChartTable {
    pub fn new(data: Arc<ChartData>, limits: ChartLimits) -> Self {
        Self { data, limits }
    }
    pub fn page(&self, dataset: &str, offset: usize) -> Result<ChartTablePage, ChartError> {
        self.page_filtered(
            dataset,
            offset,
            &ChartTableFilter {
                include_hidden_series: true,
                ..ChartTableFilter::default()
            },
        )
    }
    pub fn page_filtered(
        &self,
        dataset: &str,
        offset: usize,
        filter: &ChartTableFilter,
    ) -> Result<ChartTablePage, ChartError> {
        let dataset = self
            .data
            .dataset(dataset)
            .ok_or_else(|| ChartError::MissingDataset(dataset.into()))?;
        let series_column = filter
            .series_column
            .as_deref()
            .map(|name| {
                dataset
                    .column(name)
                    .ok_or_else(|| ChartError::MissingColumn {
                        dataset: dataset.id.0.to_string(),
                        column: name.to_owned(),
                    })
            })
            .transpose()?;
        let indices = (0..dataset.len())
            .filter(|index| {
                filter.include_hidden_series
                    || filter.visible_series.as_ref().is_none_or(|visible| {
                        series_column.is_none_or(|column| {
                            column.values[*index]
                                .category()
                                .is_some_and(|series| visible.contains(series))
                        })
                    })
            })
            .collect::<Vec<_>>();
        let offset = offset.min(indices.len());
        let end = offset
            .checked_add(self.limits.max_visible_table_rows)
            .unwrap_or(usize::MAX)
            .min(indices.len());
        let mut rows = Vec::with_capacity(end - offset);
        for (position, index) in indices[offset..end].iter().copied().enumerate() {
            let row = dataset.row(index).ok_or_else(|| {
                ChartError::invalid(ChartStage::Semantics, "table row disappeared")
            })?;
            let cells = dataset
                .columns()
                .iter()
                .map(|column| {
                    let value = column.values[index].clone();
                    ChartTableCell {
                        column: column.name.clone(),
                        unit: column.unit.clone(),
                        formatted: format_value(&value),
                        value,
                    }
                })
                .collect();
            rows.push(ChartTableRow {
                key: row.key().clone(),
                position: offset + position + 1,
                cells,
            });
        }
        Ok(ChartTablePage {
            dataset: dataset.id.0.clone(),
            revision: self.data.revision(),
            offset,
            total_rows: indices.len(),
            source_total_rows: dataset.len(),
            hidden_row_count: dataset.len() - indices.len(),
            rows,
            hidden_series_included: filter.include_hidden_series,
        })
    }
    pub fn next_offset(&self, page: &ChartTablePage) -> Option<usize> {
        let next = page.offset.checked_add(page.rows.len())?;
        (next < page.total_rows).then_some(next)
    }
    pub fn previous_offset(&self, page: &ChartTablePage) -> Option<usize> {
        (page.offset != 0).then_some(
            page.offset
                .saturating_sub(self.limits.max_visible_table_rows),
        )
    }
    pub fn position_label(page: &ChartTablePage, row: &ChartTableRow) -> String {
        format!("row {} of {}", row.position, page.total_rows)
    }

    /// Project one bounded page as ordinary native controls. Rows are grid
    /// cells with activation, while paging and hidden-series policy remain
    /// caller-controlled typed events.
    pub fn element<Message: Clone>(
        &self,
        page: &ChartTablePage,
        id: impl Into<UiId>,
        mut message: impl FnMut(ChartTableEvent) -> Message,
    ) -> impl Component<Message> {
        let headers = page
            .rows
            .first()
            .map(|row| {
                row.cells
                    .iter()
                    .map(|cell| {
                        cell.unit.as_deref().map_or_else(
                            || cell.column.to_string(),
                            |unit| format!("{} ({unit})", cell.column),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" · ")
            })
            .unwrap_or_else(|| "No rows".into());
        let mut rows = UiColumn::new().gap(2.0).child(
            Text::new(headers)
                .bold(true)
                .into_element()
                .semantic_role(SemanticRole::GridCell)
                .accessibility_label("Table headers"),
        );
        for row in &page.rows {
            let values = row
                .cells
                .iter()
                .map(|cell| {
                    cell.unit.as_deref().map_or_else(
                        || format!("{}: {}", cell.column, cell.formatted),
                        |unit| format!("{}: {} {unit}", cell.column, cell.formatted),
                    )
                })
                .collect::<Vec<_>>()
                .join(" · ");
            let label = format!("{} — {values}", Self::position_label(page, row));
            rows = rows.child(
                Button::new(
                    message(ChartTableEvent::Inspect {
                        dataset: page.dataset.clone(),
                        row: row.key.clone(),
                        revision: page.revision,
                    }),
                    label,
                )
                .id(format!("row-{}", stable_fragment(&row.key.0)))
                .into_element()
                .semantic_role(SemanticRole::GridCell),
            );
        }
        let mut controls = UiRow::new().gap(8.0);
        if let Some(offset) = self.previous_offset(page) {
            controls = controls.child(Button::new(
                message(ChartTableEvent::Page {
                    dataset: page.dataset.clone(),
                    offset,
                }),
                "Previous page",
            ));
        }
        if let Some(offset) = self.next_offset(page) {
            controls = controls.child(Button::new(
                message(ChartTableEvent::Page {
                    dataset: page.dataset.clone(),
                    offset,
                }),
                "Next page",
            ));
        }
        controls = controls.child(Button::new(
            message(ChartTableEvent::SetHiddenSeriesIncluded(
                !page.hidden_series_included,
            )),
            if page.hidden_series_included {
                "Exclude hidden series"
            } else {
                "Include hidden series"
            },
        ));
        UiColumn::new()
            .id(id)
            .gap(8.0)
            .child(
                rows.into_element()
                    .semantic_role(SemanticRole::Grid)
                    .accessibility_label(format!("Data table for {}", page.dataset))
                    .accessibility_state(format!(
                        "{} rows shown from {} source rows; {} hidden by filters",
                        page.total_rows, page.source_total_rows, page.hidden_row_count
                    )),
            )
            .child(controls)
    }
}

fn stable_fragment(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' => byte as char,
            _ => '_',
        })
        .take(128)
        .collect()
}

pub(crate) fn format_value(value: &DataValue) -> String {
    match value {
        DataValue::Null => "Missing".into(),
        DataValue::Number(value) => value.to_string(),
        DataValue::Timestamp(value) => format!("{value} ms since Unix epoch UTC"),
        DataValue::Category(value) => value.to_string(),
        DataValue::Boolean(value) => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, Dataset, DatasetId};
    #[test]
    fn every_row_is_reachable_through_bounded_pages() {
        let mut limits = ChartLimits::default();
        limits.max_visible_table_rows = 2;
        let dataset = Dataset::new(
            DatasetId::from("d"),
            (0..5)
                .map(|i| RowKey::from(format!("r{i}")))
                .collect::<Vec<_>>(),
            vec![Column::new(
                "value",
                (0..5)
                    .map(|i| DataValue::Number(i as f64))
                    .collect::<Vec<_>>(),
            )],
        )
        .unwrap();
        let table = ChartTable::new(
            Arc::new(ChartData::new(3, vec![dataset], &limits).unwrap()),
            limits,
        );
        let first = table.page("d", 0).unwrap();
        let second = table.page("d", table.next_offset(&first).unwrap()).unwrap();
        let third = table
            .page("d", table.next_offset(&second).unwrap())
            .unwrap();
        assert_eq!(
            (first.rows.len(), second.rows.len(), third.rows.len()),
            (2, 2, 1)
        );
        assert_eq!(
            ChartTable::position_label(&third, &third.rows[0]),
            "row 5 of 5"
        );
        assert!(table.next_offset(&third).is_none());
    }

    #[test]
    fn filtered_native_table_is_exact_bounded_and_keyboard_operable() {
        let limits = ChartLimits {
            max_visible_table_rows: 1,
            ..ChartLimits::default()
        };
        let dataset = Dataset::new(
            DatasetId::from("d"),
            (0..4)
                .map(|index| RowKey::from(format!("r{index}")))
                .collect::<Vec<_>>(),
            vec![
                Column::new(
                    "series",
                    ["a", "b", "a", "b"]
                        .into_iter()
                        .map(|value| DataValue::Category(Arc::from(value)))
                        .collect::<Vec<_>>(),
                ),
                Column::new("value", [1.0, 2.0, 3.0, 4.0].map(DataValue::Number)).with_unit("ms"),
            ],
        )
        .unwrap();
        let table = ChartTable::new(
            Arc::new(ChartData::new(8, vec![dataset], &limits).unwrap()),
            limits,
        );
        let page = table
            .page_filtered(
                "d",
                0,
                &ChartTableFilter {
                    series_column: Some(Arc::from("series")),
                    visible_series: Some([Arc::from("a")].into_iter().collect()),
                    include_hidden_series: false,
                },
            )
            .unwrap();
        assert_eq!(page.total_rows, 2);
        assert_eq!(page.source_total_rows, 4);
        assert_eq!(page.hidden_row_count, 2);
        assert_eq!(page.rows[0].key, RowKey::from("r0"));
        assert!(table.next_offset(&page).is_some());

        let frame = twinkle_ui::UiFrame::layout(
            table.element(&page, "table", |event| event),
            twinkle_ui::Rect::new(0.0, 0.0, 640.0, 240.0),
        );
        assert!(frame.semantic_nodes().iter().any(|node| {
            node.role == Some(SemanticRole::Grid)
                && node.name.as_deref() == Some("Data table for d")
        }));
        let row = frame
            .semantic_nodes()
            .into_iter()
            .find(|node| {
                node.role == Some(SemanticRole::GridCell)
                    && node.actions.contains(&twinkle_ui::ActionKind::Activate)
            })
            .unwrap();
        assert_eq!(
            frame
                .perform_semantic_action(
                    &row.id,
                    twinkle_ui::SemanticAction::Invoke(twinkle_ui::ActionKind::Activate),
                )
                .unwrap()
                .messages,
            vec![ChartTableEvent::Inspect {
                dataset: Arc::from("d"),
                row: RowKey::from("r0"),
                revision: 8,
            }]
        );
        let next = frame
            .semantic_nodes()
            .into_iter()
            .find(|node| node.name.as_deref() == Some("Next page"))
            .unwrap();
        assert_eq!(
            frame
                .perform_semantic_action(
                    &next.id,
                    twinkle_ui::SemanticAction::Invoke(twinkle_ui::ActionKind::Activate),
                )
                .unwrap()
                .messages,
            vec![ChartTableEvent::Page {
                dataset: Arc::from("d"),
                offset: 1,
            }]
        );
    }
}

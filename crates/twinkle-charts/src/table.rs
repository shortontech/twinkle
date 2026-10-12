use std::sync::Arc;

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
    pub rows: Vec<ChartTableRow>,
    pub hidden_series_included: bool,
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
        let dataset = self
            .data
            .dataset(dataset)
            .ok_or_else(|| ChartError::MissingDataset(dataset.into()))?;
        let offset = offset.min(dataset.len());
        let end = offset
            .checked_add(self.limits.max_visible_table_rows)
            .unwrap_or(usize::MAX)
            .min(dataset.len());
        let mut rows = Vec::with_capacity(end - offset);
        for index in offset..end {
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
                position: index + 1,
                cells,
            });
        }
        Ok(ChartTablePage {
            dataset: dataset.id.0.clone(),
            revision: self.data.revision(),
            offset,
            total_rows: dataset.len(),
            rows,
            hidden_series_included: true,
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
}

fn format_value(value: &DataValue) -> String {
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
}

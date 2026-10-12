use std::{collections::HashSet, sync::Arc};

use crate::{ChartError, ChartLimits, ChartStage};

/// Stable dataset identity.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DatasetId(pub Arc<str>);

impl From<&str> for DatasetId {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}
impl From<String> for DatasetId {
    fn from(value: String) -> Self {
        Self(value.into())
    }
}

/// Stable row identity, independent of vector position and display labels.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RowKey(pub Arc<str>);

impl From<&str> for RowKey {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}
impl From<String> for RowKey {
    fn from(value: String) -> Self {
        Self(value.into())
    }
}

/// One typed nullable cell.
#[derive(Clone, Debug, PartialEq)]
pub enum DataValue {
    Null,
    Number(f64),
    Timestamp(i64),
    Category(Arc<str>),
    Boolean(bool),
}

impl DataValue {
    pub fn number(&self) -> Option<f64> {
        if let Self::Number(value) = self {
            Some(*value)
        } else {
            None
        }
    }
    pub fn timestamp(&self) -> Option<i64> {
        if let Self::Timestamp(value) = self {
            Some(*value)
        } else {
            None
        }
    }
    pub fn category(&self) -> Option<&str> {
        if let Self::Category(value) = self {
            Some(value)
        } else {
            None
        }
    }
    pub const fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
}

/// Named typed column.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub name: Arc<str>,
    pub values: Arc<[DataValue]>,
    pub unit: Option<Arc<str>>,
}

impl Column {
    pub fn new(name: impl Into<Arc<str>>, values: impl Into<Arc<[DataValue]>>) -> Self {
        Self {
            name: name.into(),
            values: values.into(),
            unit: None,
        }
    }
    pub fn with_unit(mut self, unit: impl Into<Arc<str>>) -> Self {
        self.unit = Some(unit.into());
        self
    }
}

/// Row view resolved against a dataset.
#[derive(Clone, Copy, Debug)]
pub struct Row<'a> {
    dataset: &'a Dataset,
    index: usize,
}

impl<'a> Row<'a> {
    pub fn key(self) -> &'a RowKey {
        &self.dataset.keys[self.index]
    }
    pub fn get(self, name: &str) -> Option<&'a DataValue> {
        self.dataset
            .column(name)
            .map(|column| &column.values[self.index])
    }
    pub const fn index(self) -> usize {
        self.index
    }
}

/// Immutable columnar dataset.
#[derive(Clone, Debug, PartialEq)]
pub struct Dataset {
    pub id: DatasetId,
    keys: Arc<[RowKey]>,
    columns: Arc<[Column]>,
}

impl Dataset {
    pub fn new(
        id: impl Into<DatasetId>,
        keys: impl Into<Arc<[RowKey]>>,
        columns: impl Into<Arc<[Column]>>,
    ) -> Result<Self, ChartError> {
        let result = Self {
            id: id.into(),
            keys: keys.into(),
            columns: columns.into(),
        };
        result.validate()?;
        Ok(result)
    }
    fn validate(&self) -> Result<(), ChartError> {
        let mut names = HashSet::with_capacity(self.columns.len());
        for column in self.columns.iter() {
            if !names.insert(column.name.clone()) {
                return Err(ChartError::DuplicateKey(column.name.to_string()));
            }
            if column.values.len() != self.keys.len() {
                return Err(ChartError::invalid(
                    ChartStage::Validate,
                    format!(
                        "column `{}` has {} values for {} rows",
                        column.name,
                        column.values.len(),
                        self.keys.len()
                    ),
                ));
            }
        }
        let mut keys = HashSet::with_capacity(self.keys.len());
        for key in self.keys.iter() {
            if !keys.insert(key.clone()) {
                return Err(ChartError::DuplicateKey(key.0.to_string()));
            }
        }
        Ok(())
    }
    pub fn len(&self) -> usize {
        self.keys.len()
    }
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|column| &*column.name == name)
    }
    pub fn row(&self, index: usize) -> Option<Row<'_>> {
        (index < self.len()).then_some(Row {
            dataset: self,
            index,
        })
    }
    pub fn rows(&self) -> impl ExactSizeIterator<Item = Row<'_>> {
        (0..self.len()).map(|index| Row {
            dataset: self,
            index,
        })
    }
    pub fn keys(&self) -> &[RowKey] {
        &self.keys
    }
    pub fn columns(&self) -> &[Column] {
        &self.columns
    }
}

/// Versioned immutable chart data snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartData {
    revision: u64,
    datasets: Arc<[Dataset]>,
}

impl ChartData {
    pub fn new(
        revision: u64,
        datasets: impl Into<Arc<[Dataset]>>,
        limits: &ChartLimits,
    ) -> Result<Self, ChartError> {
        let datasets = datasets.into();
        let rows = datasets
            .iter()
            .try_fold(0usize, |total, data| total.checked_add(data.len()))
            .ok_or_else(|| ChartError::invalid(ChartStage::Validate, "row count overflow"))?;
        if rows > limits.max_rows {
            return Err(ChartError::Limit {
                resource: "input rows",
                actual: rows,
                limit: limits.max_rows,
            });
        }
        let columns = datasets
            .iter()
            .map(|data| data.columns.len())
            .sum::<usize>();
        if columns > limits.max_columns {
            return Err(ChartError::Limit {
                resource: "columns",
                actual: columns,
                limit: limits.max_columns,
            });
        }
        let mut ids = HashSet::with_capacity(datasets.len());
        for dataset in datasets.iter() {
            check_label(&dataset.id.0, limits)?;
            if !ids.insert(dataset.id.clone()) {
                return Err(ChartError::DuplicateKey(dataset.id.0.to_string()));
            }
            for key in dataset.keys() {
                check_label(&key.0, limits)?;
            }
            for column in dataset.columns() {
                check_label(&column.name, limits)?;
                if let Some(unit) = &column.unit {
                    check_label(unit, limits)?;
                }
                for value in column.values.iter() {
                    if let DataValue::Category(label) = value {
                        check_label(label, limits)?;
                    }
                }
            }
        }
        Ok(Self { revision, datasets })
    }
    pub const fn revision(&self) -> u64 {
        self.revision
    }
    pub fn datasets(&self) -> &[Dataset] {
        &self.datasets
    }
    pub fn dataset(&self, id: &str) -> Option<&Dataset> {
        self.datasets.iter().find(|data| &*data.id.0 == id)
    }
    pub fn row_count(&self) -> usize {
        self.datasets.iter().map(Dataset::len).sum()
    }
}

fn check_label(label: &str, limits: &ChartLimits) -> Result<(), ChartError> {
    if label.len() > limits.max_label_bytes {
        Err(ChartError::Limit {
            resource: "label bytes per item",
            actual: label.len(),
            limit: limits.max_label_bytes,
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_and_column_caps_reject_before_snapshot_publication() {
        let dataset = Dataset::new(
            "d",
            vec![RowKey::from("a"), RowKey::from("b")],
            vec![
                Column::new("x", vec![DataValue::Number(1.0), DataValue::Number(2.0)]),
                Column::new("y", vec![DataValue::Number(3.0), DataValue::Number(4.0)]),
            ],
        )
        .unwrap();

        let mut row_limits = ChartLimits::default();
        row_limits.max_rows = 1;
        assert_eq!(
            ChartData::new(1, vec![dataset.clone()], &row_limits),
            Err(ChartError::Limit {
                resource: "input rows",
                actual: 2,
                limit: 1,
            })
        );

        let mut column_limits = ChartLimits::default();
        column_limits.max_columns = 1;
        assert_eq!(
            ChartData::new(1, vec![dataset], &column_limits),
            Err(ChartError::Limit {
                resource: "columns",
                actual: 2,
                limit: 1,
            })
        );

        let labeled = Dataset::new(
            "dataset-name",
            vec![RowKey::from("a")],
            vec![Column::new("x", vec![DataValue::Number(1.0)])],
        )
        .unwrap();
        let mut label_limits = ChartLimits::default();
        label_limits.max_label_bytes = 4;
        assert_eq!(
            ChartData::new(1, vec![labeled], &label_limits),
            Err(ChartError::Limit {
                resource: "label bytes per item",
                actual: 12,
                limit: 4,
            })
        );
    }
}

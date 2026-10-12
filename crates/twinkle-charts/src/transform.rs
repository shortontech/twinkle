/// Missing-value policy for aggregation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MissingPolicy {
    Omit,
    Propagate,
}

/// Supported aggregate operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggregateOp {
    Count,
    Sum,
    Mean,
    Min,
    Max,
}

/// Deterministic histogram options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BinOptions {
    pub start: f64,
    pub end: f64,
    pub count: usize,
    pub density: bool,
}

/// Stack normalization behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StackMode {
    Raw,
    Normalized,
}

/// Finite declarative transform plan step.
#[derive(Clone, Debug, PartialEq)]
pub enum Transform {
    Sort {
        field: String,
        descending: bool,
    },
    Bin {
        field: String,
        output: String,
        options: BinOptions,
    },
    Aggregate {
        group_by: Vec<String>,
        field: Option<String>,
        output: String,
        operation: AggregateOp,
        missing: MissingPolicy,
    },
    Stack {
        category: String,
        series: String,
        value: String,
        mode: StackMode,
    },
}

impl BinOptions {
    pub fn boundaries(self) -> Option<Vec<f64>> {
        if !self.start.is_finite()
            || !self.end.is_finite()
            || self.end <= self.start
            || self.count == 0
        {
            return None;
        }
        let width = (self.end - self.start) / self.count as f64;
        Some(
            (0..=self.count)
                .map(|index| {
                    if index == self.count {
                        self.end
                    } else {
                        self.start + index as f64 * width
                    }
                })
                .collect(),
        )
    }
    /// Bins are left-inclusive/right-exclusive, except for the final upper edge.
    pub fn index(self, value: f64) -> Option<usize> {
        if !value.is_finite() || value < self.start || value > self.end || self.count == 0 {
            return None;
        }
        if value == self.end {
            return Some(self.count - 1);
        }
        Some(
            (((value - self.start) / (self.end - self.start) * self.count as f64).floor() as usize)
                .min(self.count - 1),
        )
    }
}

/// Execute an ordered, bounded transform plan into a new immutable dataset.
///
/// Stable source keys survive sort/bin steps. Aggregate keys are deterministic
/// group identities and include `__source_count`; stack steps add `<value>_raw`,
/// `<value>_start`, `<value>_end`, and `<value>_percent` columns.
pub fn execute_transforms(
    source: &Dataset,
    plan: &[Transform],
    limits: &ChartLimits,
) -> Result<Dataset, ChartError> {
    if plan.len() > limits.max_transforms {
        return Err(ChartError::Limit {
            resource: "transform steps",
            actual: plan.len(),
            limit: limits.max_transforms,
        });
    }
    let mut table = Table::from_dataset(source);
    let mut density_bins: Vec<(String, f64)> = Vec::new();
    for step in plan {
        table = match step {
            Transform::Sort { field, descending } => table.sort(field, *descending)?,
            Transform::Bin {
                field,
                output,
                options,
            } => {
                if options.density {
                    let width = (options.end - options.start) / options.count as f64;
                    if width.is_finite() && width > 0.0 {
                        density_bins.push((output.clone(), width));
                    }
                }
                table.bin(field, output, *options)?
            }
            Transform::Aggregate {
                group_by,
                field,
                output,
                operation,
                missing,
            } => {
                let mut aggregated =
                    table.aggregate(group_by, field.as_deref(), output, *operation, *missing)?;
                if *operation == AggregateOp::Count
                    && let Some((_, width)) = density_bins
                        .iter()
                        .rev()
                        .find(|(bin, _)| group_by.iter().any(|group| group == bin))
                {
                    aggregated.normalize_density(output, *width)?;
                }
                aggregated
            }
            Transform::Stack {
                category,
                series,
                value,
                mode,
            } => table.stack(category, series, value, *mode)?,
        };
        if table.rows.len() > limits.max_rows {
            return Err(ChartError::Limit {
                resource: "transformed rows",
                actual: table.rows.len(),
                limit: limits.max_rows,
            });
        }
        if table.columns.len() > limits.max_columns {
            return Err(ChartError::Limit {
                resource: "transformed columns",
                actual: table.columns.len(),
                limit: limits.max_columns,
            });
        }
    }
    table.into_dataset(source)
}

#[derive(Clone)]
struct Table {
    columns: Vec<String>,
    rows: Vec<TableRow>,
}

#[derive(Clone)]
struct TableRow {
    key: RowKey,
    values: BTreeMap<String, DataValue>,
}

impl Table {
    fn from_dataset(source: &Dataset) -> Self {
        let columns = source
            .columns()
            .iter()
            .map(|column| column.name.to_string())
            .collect();
        let rows = source
            .rows()
            .map(|row| TableRow {
                key: row.key().clone(),
                values: source
                    .columns()
                    .iter()
                    .map(|column| (column.name.to_string(), column.values[row.index()].clone()))
                    .collect(),
            })
            .collect();
        Self { columns, rows }
    }

    fn require(&self, field: &str) -> Result<(), ChartError> {
        if self.columns.iter().any(|column| column == field) {
            Ok(())
        } else {
            Err(ChartError::MissingColumn {
                dataset: "transformed".into(),
                column: field.into(),
            })
        }
    }

    fn sort(mut self, field: &str, descending: bool) -> Result<Self, ChartError> {
        self.require(field)?;
        self.rows.sort_by(|a, b| {
            let ordering =
                compare_values(&a.values[field], &b.values[field]).then_with(|| a.key.cmp(&b.key));
            if descending {
                ordering.reverse()
            } else {
                ordering
            }
        });
        Ok(self)
    }

    fn bin(mut self, field: &str, output: &str, options: BinOptions) -> Result<Self, ChartError> {
        self.require(field)?;
        let boundaries = options.boundaries().ok_or_else(|| {
            ChartError::invalid(
                ChartStage::Transform,
                "bin range and count must be finite and non-degenerate",
            )
        })?;
        for suffix in ["", "_start", "_end"] {
            self.add_column(format!("{output}{suffix}"))?;
        }
        for row in &mut self.rows {
            let Some(value) = row.values[field].number() else {
                row.values.insert(output.into(), DataValue::Null);
                row.values
                    .insert(format!("{output}_start"), DataValue::Null);
                row.values.insert(format!("{output}_end"), DataValue::Null);
                continue;
            };
            if let Some(index) = options.index(value) {
                let start = boundaries[index];
                let end = boundaries[index + 1];
                row.values
                    .insert(output.into(), DataValue::Number((start + end) * 0.5));
                row.values
                    .insert(format!("{output}_start"), DataValue::Number(start));
                row.values
                    .insert(format!("{output}_end"), DataValue::Number(end));
            } else {
                row.values.insert(output.into(), DataValue::Null);
                row.values
                    .insert(format!("{output}_start"), DataValue::Null);
                row.values.insert(format!("{output}_end"), DataValue::Null);
            }
        }
        Ok(self)
    }

    fn aggregate(
        self,
        group_by: &[String],
        field: Option<&str>,
        output: &str,
        operation: AggregateOp,
        missing: MissingPolicy,
    ) -> Result<Self, ChartError> {
        for group in group_by {
            self.require(group)?;
        }
        if let Some(field) = field {
            self.require(field)?;
        }
        if operation != AggregateOp::Count && field.is_none() {
            return Err(ChartError::invalid(
                ChartStage::Transform,
                "numeric aggregate requires a field",
            ));
        }
        let mut groups: BTreeMap<Vec<String>, Vec<&TableRow>> = BTreeMap::new();
        for row in &self.rows {
            if missing == MissingPolicy::Omit
                && group_by.iter().any(|field| row.values[field].is_null())
            {
                continue;
            }
            let key = group_by
                .iter()
                .map(|field| value_key(&row.values[field]))
                .collect();
            groups.entry(key).or_default().push(row);
        }
        let mut rows = Vec::with_capacity(groups.len());
        for (index, (_, members)) in groups.into_iter().enumerate() {
            let mut values = BTreeMap::new();
            for group in group_by {
                values.insert(group.clone(), members[0].values[group].clone());
            }
            let source_values: Vec<Option<f64>> = field.map_or_else(
                || vec![Some(1.0); members.len()],
                |field| {
                    members
                        .iter()
                        .map(|row| row.values[field].number().filter(|value| value.is_finite()))
                        .collect()
                },
            );
            let has_missing = source_values.iter().any(Option::is_none);
            let numbers: Vec<f64> = source_values.into_iter().flatten().collect();
            let result = if missing == MissingPolicy::Propagate && has_missing {
                DataValue::Null
            } else {
                aggregate_value(operation, &numbers)
            };
            values.insert(output.into(), result);
            values.insert(
                "__source_count".into(),
                DataValue::Number(members.len() as f64),
            );
            rows.push(TableRow {
                key: RowKey(Arc::from(format!("aggregate/{index}"))),
                values,
            });
        }
        let mut columns = group_by.to_vec();
        columns.push(output.into());
        columns.push("__source_count".into());
        Ok(Self { columns, rows })
    }

    fn stack(
        mut self,
        category: &str,
        series: &str,
        value: &str,
        mode: StackMode,
    ) -> Result<Self, ChartError> {
        for field in [category, series, value] {
            self.require(field)?;
        }
        for suffix in ["_raw", "_start", "_end", "_percent"] {
            self.add_column(format!("{value}{suffix}"))?;
        }
        let mut totals: BTreeMap<String, (f64, f64)> = BTreeMap::new();
        for row in &self.rows {
            let Some(number) = row.values[value].number().filter(|value| value.is_finite()) else {
                continue;
            };
            let total = totals.entry(value_key(&row.values[category])).or_default();
            if number >= 0.0 {
                total.0 += number;
            } else {
                total.1 += number.abs();
            }
        }
        let mut cursors: BTreeMap<String, (f64, f64)> = BTreeMap::new();
        // Stable series order is the incoming order; equal category/series keys retain row order.
        for row in &mut self.rows {
            let category_key = value_key(&row.values[category]);
            let Some(raw) = row.values[value].number().filter(|value| value.is_finite()) else {
                for suffix in ["_raw", "_start", "_end", "_percent"] {
                    row.values
                        .insert(format!("{value}{suffix}"), DataValue::Null);
                }
                continue;
            };
            let totals = totals[&category_key];
            let denominator = if raw >= 0.0 { totals.0 } else { totals.1 };
            let plotted = if mode == StackMode::Normalized && denominator != 0.0 {
                raw / denominator
            } else {
                raw
            };
            let cursor = cursors.entry(category_key).or_default();
            let start = if plotted >= 0.0 { cursor.0 } else { cursor.1 };
            let end = start + plotted;
            if plotted >= 0.0 {
                cursor.0 = end;
            } else {
                cursor.1 = end;
            }
            row.values
                .insert(format!("{value}_raw"), DataValue::Number(raw));
            row.values
                .insert(format!("{value}_start"), DataValue::Number(start));
            row.values
                .insert(format!("{value}_end"), DataValue::Number(end));
            row.values.insert(
                format!("{value}_percent"),
                DataValue::Number(if denominator == 0.0 {
                    0.0
                } else {
                    raw / denominator
                }),
            );
        }
        Ok(self)
    }

    fn add_column(&mut self, name: String) -> Result<(), ChartError> {
        if self.columns.iter().any(|column| column == &name) {
            return Err(ChartError::DuplicateKey(name));
        }
        self.columns.push(name);
        Ok(())
    }

    fn normalize_density(&mut self, output: &str, width: f64) -> Result<(), ChartError> {
        let total = self
            .rows
            .iter()
            .filter_map(|row| row.values.get("__source_count").and_then(DataValue::number))
            .sum::<f64>();
        if total <= 0.0 || !width.is_finite() || width <= 0.0 {
            return Err(ChartError::invalid(
                ChartStage::Transform,
                "density normalization requires a positive count and bin width",
            ));
        }
        for row in &mut self.rows {
            if let Some(value) = row.values.get(output).and_then(DataValue::number) {
                row.values
                    .insert(output.into(), DataValue::Number(value / (total * width)));
            }
        }
        Ok(())
    }
    fn into_dataset(self, source: &Dataset) -> Result<Dataset, ChartError> {
        let keys: Vec<RowKey> = self.rows.iter().map(|row| row.key.clone()).collect();
        let columns: Vec<Column> = self
            .columns
            .iter()
            .map(|name| {
                let mut column = Column::new(
                    name.clone(),
                    self.rows
                        .iter()
                        .map(|row| row.values.get(name).cloned().unwrap_or(DataValue::Null))
                        .collect::<Vec<_>>(),
                );
                if let Some(unit) = source.column(name).and_then(|column| column.unit.clone()) {
                    column.unit = Some(unit);
                }
                column
            })
            .collect();
        Dataset::new(source.id.clone(), keys, columns)
    }
}

fn compare_values(a: &DataValue, b: &DataValue) -> Ordering {
    match (a, b) {
        (DataValue::Null, DataValue::Null) => Ordering::Equal,
        (DataValue::Null, _) => Ordering::Greater,
        (_, DataValue::Null) => Ordering::Less,
        (DataValue::Number(a), DataValue::Number(b)) => a.total_cmp(b),
        (DataValue::Timestamp(a), DataValue::Timestamp(b)) => a.cmp(b),
        (DataValue::Category(a), DataValue::Category(b)) => a.cmp(b),
        (DataValue::Boolean(a), DataValue::Boolean(b)) => a.cmp(b),
        (a, b) => value_kind(a).cmp(&value_kind(b)),
    }
}
fn value_kind(value: &DataValue) -> u8 {
    match value {
        DataValue::Null => 0,
        DataValue::Number(_) => 1,
        DataValue::Timestamp(_) => 2,
        DataValue::Category(_) => 3,
        DataValue::Boolean(_) => 4,
    }
}
fn value_key(value: &DataValue) -> String {
    match value {
        DataValue::Null => "null".into(),
        DataValue::Number(value) => format!("n:{:016x}", value.to_bits()),
        DataValue::Timestamp(value) => format!("t:{value}"),
        DataValue::Category(value) => format!("c:{}:{value}", value.len()),
        DataValue::Boolean(value) => format!("b:{value}"),
    }
}
fn aggregate_value(operation: AggregateOp, values: &[f64]) -> DataValue {
    match operation {
        AggregateOp::Count => DataValue::Number(values.len() as f64),
        _ if values.is_empty() => DataValue::Null,
        AggregateOp::Sum => DataValue::Number(values.iter().sum()),
        AggregateOp::Mean => DataValue::Number(values.iter().sum::<f64>() / values.len() as f64),
        AggregateOp::Min => DataValue::Number(values.iter().copied().fold(f64::INFINITY, f64::min)),
        AggregateOp::Max => {
            DataValue::Number(values.iter().copied().fold(f64::NEG_INFINITY, f64::max))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{ChartLimits, Column, DataValue, Dataset, DatasetId, RowKey};

    use super::*;
    #[test]
    fn final_bin_includes_upper_edge() {
        let bins = BinOptions {
            start: 0.0,
            end: 10.0,
            count: 2,
            density: false,
        };
        assert_eq!(bins.index(0.0), Some(0));
        assert_eq!(bins.index(5.0), Some(1));
        assert_eq!(bins.index(10.0), Some(1));
    }

    fn dataset(values: &[f64]) -> Dataset {
        Dataset::new(
            DatasetId::from("d"),
            values
                .iter()
                .enumerate()
                .map(|(index, _)| RowKey::from(format!("r{index}")))
                .collect::<Vec<_>>(),
            vec![
                Column::new(
                    "category",
                    values
                        .iter()
                        .enumerate()
                        .map(|(index, _)| {
                            DataValue::Category(Arc::from(if index < 2 { "a" } else { "b" }))
                        })
                        .collect::<Vec<_>>(),
                ),
                Column::new(
                    "series",
                    values
                        .iter()
                        .enumerate()
                        .map(|(index, _)| {
                            DataValue::Category(Arc::from(if index % 2 == 0 {
                                "one"
                            } else {
                                "two"
                            }))
                        })
                        .collect::<Vec<_>>(),
                ),
                Column::new(
                    "value",
                    values
                        .iter()
                        .copied()
                        .map(DataValue::Number)
                        .collect::<Vec<_>>(),
                ),
            ],
        )
        .unwrap()
    }

    #[test]
    fn aggregate_retains_provenance_and_stable_groups() {
        let result = execute_transforms(
            &dataset(&[2.0, 4.0, 8.0]),
            &[Transform::Aggregate {
                group_by: vec!["category".into()],
                field: Some("value".into()),
                output: "mean".into(),
                operation: AggregateOp::Mean,
                missing: MissingPolicy::Omit,
            }],
            &ChartLimits::default(),
        )
        .unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(
            result.column("mean").unwrap().values[0],
            DataValue::Number(3.0)
        );
        assert_eq!(
            result.column("__source_count").unwrap().values[0],
            DataValue::Number(2.0)
        );
    }

    #[test]
    fn normalized_stack_separates_positive_and_negative_sides() {
        let result = execute_transforms(
            &dataset(&[2.0, -3.0, 6.0]),
            &[Transform::Stack {
                category: "category".into(),
                series: "series".into(),
                value: "value".into(),
                mode: StackMode::Normalized,
            }],
            &ChartLimits::default(),
        )
        .unwrap();
        assert_eq!(
            result.column("value_end").unwrap().values[0],
            DataValue::Number(1.0)
        );
        assert_eq!(
            result.column("value_end").unwrap().values[1],
            DataValue::Number(-1.0)
        );
        assert_eq!(
            result.column("value_raw").unwrap().values[1],
            DataValue::Number(-3.0)
        );
    }

    #[test]
    fn histogram_density_integrates_to_one_and_omits_out_of_range_rows() {
        let result = execute_transforms(
            &dataset(&[0.0, 4.0, 10.0, 20.0]),
            &[
                Transform::Bin {
                    field: "value".into(),
                    output: "bin".into(),
                    options: BinOptions {
                        start: 0.0,
                        end: 10.0,
                        count: 2,
                        density: true,
                    },
                },
                Transform::Aggregate {
                    group_by: vec!["bin".into()],
                    field: None,
                    output: "density".into(),
                    operation: AggregateOp::Count,
                    missing: MissingPolicy::Omit,
                },
            ],
            &ChartLimits::default(),
        )
        .unwrap();
        assert_eq!(result.len(), 2);
        let integral = result
            .column("density")
            .unwrap()
            .values
            .iter()
            .filter_map(DataValue::number)
            .sum::<f64>()
            * 5.0;
        assert!((integral - 1.0).abs() < 1e-12);
    }
}
use std::{cmp::Ordering, collections::BTreeMap, sync::Arc};

use crate::{ChartError, ChartLimits, ChartStage, Column, DataValue, Dataset, RowKey};

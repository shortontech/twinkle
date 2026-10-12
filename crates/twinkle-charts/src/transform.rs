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

#[cfg(test)]
mod tests {
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
}

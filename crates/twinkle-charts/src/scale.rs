use crate::{ChartError, ChartStage};

/// Forward and inverse continuous-scale behavior.
pub trait ContinuousScale {
    fn map(&self, value: f64) -> f64;
    fn invert(&self, position: f64) -> f64;
}

/// A numeric scale domain value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScaleValue {
    pub domain: f64,
    pub position: f64,
}

/// Validated linear scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearScale {
    domain: [f64; 2],
    range: [f64; 2],
    clamp: bool,
}

impl LinearScale {
    pub fn new(mut domain: [f64; 2], range: [f64; 2], clamp: bool) -> Result<Self, ChartError> {
        if domain
            .into_iter()
            .chain(range)
            .any(|value| !value.is_finite())
        {
            return Err(ChartError::invalid(
                ChartStage::Domain,
                "linear scale values must be finite",
            ));
        }
        if domain[0] == domain[1] {
            let padding = domain[0].abs().max(1.0) * 0.5;
            domain = [domain[0] - padding, domain[1] + padding];
        }
        Ok(Self {
            domain,
            range,
            clamp,
        })
    }
    pub const fn domain(&self) -> [f64; 2] {
        self.domain
    }
    pub const fn range(&self) -> [f64; 2] {
        self.range
    }
}

impl ContinuousScale for LinearScale {
    fn map(&self, value: f64) -> f64 {
        let mut t = (value - self.domain[0]) / (self.domain[1] - self.domain[0]);
        if self.clamp {
            t = t.clamp(0.0, 1.0);
        }
        self.range[0] + t * (self.range[1] - self.range[0])
    }
    fn invert(&self, position: f64) -> f64 {
        let mut t = (position - self.range[0]) / (self.range[1] - self.range[0]);
        if self.clamp {
            t = t.clamp(0.0, 1.0);
        }
        self.domain[0] + t * (self.domain[1] - self.domain[0])
    }
}

/// Validated logarithmic scale. Non-positive domains and inputs are errors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LogScale {
    domain: [f64; 2],
    range: [f64; 2],
    base: f64,
    clamp: bool,
}

impl LogScale {
    pub fn new(
        domain: [f64; 2],
        range: [f64; 2],
        base: f64,
        clamp: bool,
    ) -> Result<Self, ChartError> {
        if domain
            .into_iter()
            .chain(range)
            .chain([base])
            .any(|value| !value.is_finite())
            || domain[0] <= 0.0
            || domain[1] <= 0.0
            || domain[0] == domain[1]
            || base <= 0.0
            || base == 1.0
        {
            return Err(ChartError::invalid(
                ChartStage::Domain,
                "log scale requires a finite positive non-degenerate domain and base > 0 other than 1",
            ));
        }
        Ok(Self {
            domain,
            range,
            base,
            clamp,
        })
    }
    pub fn try_map(&self, value: f64) -> Result<f64, ChartError> {
        if !value.is_finite() || value <= 0.0 {
            Err(ChartError::invalid(
                ChartStage::Domain,
                "log scale value must be finite and positive",
            ))
        } else {
            Ok(self.map(value))
        }
    }
}

impl ContinuousScale for LogScale {
    fn map(&self, value: f64) -> f64 {
        let log = |value: f64| value.log(self.base);
        let mut t =
            (log(value) - log(self.domain[0])) / (log(self.domain[1]) - log(self.domain[0]));
        if self.clamp {
            t = t.clamp(0.0, 1.0);
        }
        self.range[0] + t * (self.range[1] - self.range[0])
    }
    fn invert(&self, position: f64) -> f64 {
        let mut t = (position - self.range[0]) / (self.range[1] - self.range[0]);
        if self.clamp {
            t = t.clamp(0.0, 1.0);
        }
        let lower = self.domain[0].log(self.base);
        self.base
            .powf(lower + t * (self.domain[1].log(self.base) - lower))
    }
}

/// Stable categorical band scale.
#[derive(Clone, Debug, PartialEq)]
pub struct BandScale {
    categories: Vec<String>,
    start: f64,
    end: f64,
    inner_padding: f64,
    outer_padding: f64,
}

impl BandScale {
    pub fn new(
        categories: impl IntoIterator<Item = impl Into<String>>,
        range: [f64; 2],
        inner_padding: f64,
        outer_padding: f64,
    ) -> Result<Self, ChartError> {
        let categories: Vec<String> = categories.into_iter().map(Into::into).collect();
        if categories.is_empty()
            || range.into_iter().any(|v| !v.is_finite())
            || !(0.0..1.0).contains(&inner_padding)
            || !(0.0..=1.0).contains(&outer_padding)
        {
            return Err(ChartError::invalid(
                ChartStage::Domain,
                "invalid band scale",
            ));
        }
        let mut unique = std::collections::HashSet::new();
        if categories.iter().any(|value| !unique.insert(value)) {
            return Err(ChartError::invalid(
                ChartStage::Domain,
                "band categories must be unique",
            ));
        }
        Ok(Self {
            categories,
            start: range[0],
            end: range[1],
            inner_padding,
            outer_padding,
        })
    }
    fn step(&self) -> f64 {
        (self.end - self.start)
            / (self.categories.len() as f64 - self.inner_padding + 2.0 * self.outer_padding)
    }
    pub fn bandwidth(&self) -> f64 {
        self.step().abs() * (1.0 - self.inner_padding)
    }
    pub fn map(&self, category: &str) -> Option<f64> {
        self.categories
            .iter()
            .position(|item| item == category)
            .map(|index| self.start + self.step() * (self.outer_padding + index as f64))
    }
    pub fn invert(&self, position: f64) -> Option<&str> {
        self.categories
            .iter()
            .enumerate()
            .min_by(|(a, _), (b, _)| {
                let da = (self.start
                    + self.step() * (self.outer_padding + *a as f64)
                    + self.bandwidth() / 2.0
                    - position)
                    .abs();
                let db = (self.start
                    + self.step() * (self.outer_padding + *b as f64)
                    + self.bandwidth() / 2.0
                    - position)
                    .abs();
                da.total_cmp(&db)
            })
            .map(|(_, value)| value.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn singleton_linear_domain_expands_and_inverts() {
        let scale = LinearScale::new([4.0, 4.0], [10.0, 110.0], false).unwrap();
        let p = scale.map(4.0);
        assert_eq!(p, 60.0);
        assert!((scale.invert(p) - 4.0).abs() < 1e-12);
    }
    #[test]
    fn log_rejects_nonpositive_domain_and_value() {
        assert!(LogScale::new([0.0, 10.0], [0.0, 1.0], 10.0, false).is_err());
        assert!(
            LogScale::new([1.0, 10.0], [0.0, 1.0], 10.0, false)
                .unwrap()
                .try_map(-1.0)
                .is_err()
        );
    }
    #[test]
    fn band_order_is_stable() {
        let scale = BandScale::new(["b", "a", "c"], [0.0, 300.0], 0.1, 0.1).unwrap();
        assert!(scale.map("b").unwrap() < scale.map("a").unwrap());
        assert_eq!(scale.invert(scale.map("c").unwrap()).unwrap(), "c");
    }
}

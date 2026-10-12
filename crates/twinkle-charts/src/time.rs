use crate::{ChartError, ChartStage};

/// Deterministic timezone capability. Named zones are rejected until an
/// optional timezone database is configured; a fixed offset is not mislabeled.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TimeZonePolicy {
    Utc,
    FixedOffsetSeconds(i32),
    Named(String),
}

/// Calendar-aware tick interval.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeTickInterval {
    Milliseconds(i64),
    Seconds(i64),
    Minutes(i64),
    Hours(i64),
    Days(i64),
    Months(u32),
    Years(u32),
}

/// One absolute tick and its deterministic display label.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalendarTick {
    pub epoch_millis: i64,
    pub label: String,
}

/// Checked absolute-instant scale using Unix epoch milliseconds.
#[derive(Clone, Debug, PartialEq)]
pub struct TimeScale {
    domain: [i64; 2],
    range: [f64; 2],
    clamp: bool,
    timezone: TimeZonePolicy,
}

impl TimeScale {
    /// ECMAScript-compatible maximum absolute epoch milliseconds.
    pub const MAX_EPOCH_MILLIS: i64 = 8_640_000_000_000_000;
    pub fn new(
        domain: [i64; 2],
        range: [f64; 2],
        clamp: bool,
        timezone: TimeZonePolicy,
    ) -> Result<Self, ChartError> {
        if domain[0] == domain[1]
            || domain
                .iter()
                .any(|value| value.unsigned_abs() > Self::MAX_EPOCH_MILLIS as u64)
            || range.iter().any(|value| !value.is_finite())
        {
            return Err(ChartError::invalid(
                ChartStage::Domain,
                "time scale requires checked, distinct instants and finite range",
            ));
        }
        validate_timezone(&timezone)?;
        Ok(Self {
            domain,
            range,
            clamp,
            timezone,
        })
    }
    pub fn map(&self, instant: i64) -> Result<f64, ChartError> {
        checked_epoch(instant)?;
        let mut t = (instant - self.domain[0]) as f64 / (self.domain[1] - self.domain[0]) as f64;
        if self.clamp {
            t = t.clamp(0.0, 1.0);
        }
        Ok(self.range[0] + t * (self.range[1] - self.range[0]))
    }
    pub fn invert(&self, position: f64) -> Result<i64, ChartError> {
        if !position.is_finite() {
            return Err(ChartError::invalid(
                ChartStage::Domain,
                "time position must be finite",
            ));
        }
        let mut t = (position - self.range[0]) / (self.range[1] - self.range[0]);
        if self.clamp {
            t = t.clamp(0.0, 1.0);
        }
        let value = self.domain[0] as f64 + t * (self.domain[1] - self.domain[0]) as f64;
        if value < i64::MIN as f64 || value > i64::MAX as f64 {
            return Err(ChartError::invalid(
                ChartStage::Domain,
                "inverted timestamp overflow",
            ));
        }
        let value = value.round() as i64;
        checked_epoch(value)?;
        Ok(value)
    }
    pub fn ticks(
        &self,
        interval: TimeTickInterval,
        max_ticks: usize,
    ) -> Result<Vec<CalendarTick>, ChartError> {
        if max_ticks == 0 {
            return Ok(Vec::new());
        }
        validate_timezone(&self.timezone)?;
        let ascending = self.domain[0] < self.domain[1];
        let start = self.domain[0].min(self.domain[1]);
        let end = self.domain[0].max(self.domain[1]);
        let offset = match self.timezone {
            TimeZonePolicy::Utc => 0,
            TimeZonePolicy::FixedOffsetSeconds(value) => value,
            TimeZonePolicy::Named(_) => unreachable!(),
        };
        let mut ticks = match interval {
            TimeTickInterval::Months(step) => {
                calendar_ticks(start, end, step.max(1), false, offset, max_ticks)?
            }
            TimeTickInterval::Years(step) => {
                calendar_ticks(start, end, step.max(1), true, offset, max_ticks)?
            }
            other => fixed_ticks(start, end, interval_millis(other)?, offset, max_ticks)?,
        };
        if !ascending {
            ticks.reverse();
        }
        Ok(ticks)
    }
}

fn validate_timezone(timezone: &TimeZonePolicy) -> Result<(), ChartError> {
    match timezone {
        TimeZonePolicy::Named(name) => Err(ChartError::Unsupported(format!(
            "named timezone `{name}` (timezone database feature is unavailable)"
        ))),
        TimeZonePolicy::FixedOffsetSeconds(offset) if offset.abs() > 86_400 => Err(
            ChartError::invalid(ChartStage::Domain, "fixed timezone offset exceeds 24 hours"),
        ),
        _ => Ok(()),
    }
}
fn checked_epoch(value: i64) -> Result<(), ChartError> {
    if value.unsigned_abs() > TimeScale::MAX_EPOCH_MILLIS as u64 {
        Err(ChartError::invalid(
            ChartStage::Domain,
            "timestamp exceeds admitted epoch-millisecond range",
        ))
    } else {
        Ok(())
    }
}
fn interval_millis(interval: TimeTickInterval) -> Result<i64, ChartError> {
    let (value, multiplier) = match interval {
        TimeTickInterval::Milliseconds(value) => (value, 1),
        TimeTickInterval::Seconds(value) => (value, 1_000),
        TimeTickInterval::Minutes(value) => (value, 60_000),
        TimeTickInterval::Hours(value) => (value, 3_600_000),
        TimeTickInterval::Days(value) => (value, 86_400_000),
        _ => unreachable!(),
    };
    value
        .checked_mul(multiplier)
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            ChartError::invalid(
                ChartStage::Domain,
                "tick interval must be positive and cannot overflow",
            )
        })
}
fn fixed_ticks(
    start: i64,
    end: i64,
    step: i64,
    offset: i32,
    max_ticks: usize,
) -> Result<Vec<CalendarTick>, ChartError> {
    let first = start
        .div_euclid(step)
        .checked_mul(step)
        .and_then(|value| {
            if value < start {
                value.checked_add(step)
            } else {
                Some(value)
            }
        })
        .ok_or_else(|| ChartError::invalid(ChartStage::Domain, "tick alignment overflow"))?;
    let mut ticks = Vec::new();
    let mut value = first;
    while value <= end {
        if ticks.len() == max_ticks {
            return Err(ChartError::Limit {
                resource: "time ticks",
                actual: ticks.len() + 1,
                limit: max_ticks,
            });
        }
        ticks.push(CalendarTick {
            epoch_millis: value,
            label: format_timestamp(value, offset),
        });
        value = match value.checked_add(step) {
            Some(next) => next,
            None => break,
        };
    }
    Ok(ticks)
}
fn calendar_ticks(
    start: i64,
    end: i64,
    step: u32,
    years: bool,
    offset: i32,
    max_ticks: usize,
) -> Result<Vec<CalendarTick>, ChartError> {
    let local_start = start
        .checked_add(i64::from(offset) * 1_000)
        .ok_or_else(|| ChartError::invalid(ChartStage::Domain, "timezone offset overflow"))?;
    let days = local_start.div_euclid(86_400_000);
    let (mut year, mut month, _) = civil_from_days(days);
    if local_start.rem_euclid(86_400_000) != 0 || civil_from_days(days).2 != 1 {
        month += 1;
        if month == 13 {
            month = 1;
            year += 1;
        }
    }
    if years {
        year = ((year.div_euclid(step as i32)) + i32::from(year.rem_euclid(step as i32) != 0))
            * step as i32;
        month = 1;
    }
    let mut ticks = Vec::new();
    loop {
        let local_days = days_from_civil(year, month, 1);
        let epoch = local_days
            .checked_mul(86_400_000)
            .and_then(|value| value.checked_sub(i64::from(offset) * 1_000))
            .ok_or_else(|| ChartError::invalid(ChartStage::Domain, "calendar tick overflow"))?;
        if epoch > end {
            break;
        }
        if epoch >= start {
            if ticks.len() == max_ticks {
                return Err(ChartError::Limit {
                    resource: "time ticks",
                    actual: ticks.len() + 1,
                    limit: max_ticks,
                });
            }
            ticks.push(CalendarTick {
                epoch_millis: epoch,
                label: format_timestamp(epoch, offset),
            });
        }
        if years {
            year = year
                .checked_add(step as i32)
                .ok_or_else(|| ChartError::invalid(ChartStage::Domain, "calendar year overflow"))?;
        } else {
            let absolute = i64::from(year) * 12 + i64::from(month - 1) + i64::from(step);
            year = absolute.div_euclid(12) as i32;
            month = absolute.rem_euclid(12) as u32 + 1;
        }
    }
    Ok(ticks)
}
fn format_timestamp(epoch: i64, offset: i32) -> String {
    let local = epoch + i64::from(offset) * 1_000;
    let days = local.div_euclid(86_400_000);
    let millis = local.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    let hour = millis / 3_600_000;
    let minute = millis % 3_600_000 / 60_000;
    let second = millis % 60_000 / 1_000;
    let sign = if offset < 0 { '-' } else { '+' };
    let absolute = offset.abs();
    format!(
        "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} {sign}{:02}:{:02}",
        absolute / 3600,
        absolute % 3600 / 60
    )
}

// Howard Hinnant's proleptic-Gregorian civil-date transformations.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let shifted = month as i32 + if month > 2 { -3 } else { 9 };
    let doy = (153 * shifted + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year as i32, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scale_maps_and_inverts_absolute_time() {
        let scale = TimeScale::new([0, 1_000], [10.0, 110.0], false, TimeZonePolicy::Utc).unwrap();
        assert_eq!(scale.map(500).unwrap(), 60.0);
        assert_eq!(scale.invert(60.0).unwrap(), 500);
    }
    #[test]
    fn month_ticks_observe_leap_february() {
        let start = days_from_civil(2024, 1, 1) * 86_400_000;
        let end = days_from_civil(2024, 4, 1) * 86_400_000;
        let scale = TimeScale::new([start, end], [0.0, 1.0], false, TimeZonePolicy::Utc).unwrap();
        let ticks = scale.ticks(TimeTickInterval::Months(1), 4).unwrap();
        let gaps: Vec<i64> = ticks
            .windows(2)
            .map(|pair| (pair[1].epoch_millis - pair[0].epoch_millis) / 86_400_000)
            .collect();
        assert_eq!(gaps, vec![31, 29, 31]);
    }
    #[test]
    fn named_zone_is_explicitly_unsupported() {
        assert!(matches!(
            TimeScale::new(
                [0, 1],
                [0.0, 1.0],
                false,
                TimeZonePolicy::Named("America/Los_Angeles".into())
            ),
            Err(ChartError::Unsupported(_))
        ));
    }
}

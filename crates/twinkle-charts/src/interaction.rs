use crate::{ChartEvent, ChartHit, Viewport};

/// Keyboard/controller operations shared by native and semantic actions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NavigationCommand {
    Previous,
    Next,
    PreviousSeries,
    NextSeries,
    Home,
    End,
    Select,
    Cancel,
    CycleOverlap,
}

/// Transient, bounded active-datum navigation state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChartNavigation {
    active: Option<usize>,
    overlap_cycle: usize,
}

impl ChartNavigation {
    pub const fn active_index(&self) -> Option<usize> {
        self.active
    }
    pub fn active<'a>(&self, hits: &'a [ChartHit]) -> Option<&'a ChartHit> {
        self.active.and_then(|index| hits.get(index))
    }
    pub fn set_active_index<'a>(
        &mut self,
        hits: &'a [ChartHit],
        index: usize,
    ) -> Option<&'a ChartHit> {
        self.active = (index < hits.len()).then_some(index);
        self.overlap_cycle = 0;
        self.active(hits)
    }
    pub fn set_nearest<'a>(
        &mut self,
        hits: &'a [ChartHit],
        point: twinkle_ui::Point,
    ) -> Option<&'a ChartHit> {
        self.active = hits
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                distance(a, point)
                    .total_cmp(&distance(b, point))
                    .then_with(|| a.dataset.cmp(&b.dataset))
                    .then_with(|| a.row.cmp(&b.row))
            })
            .map(|(index, _)| index);
        self.overlap_cycle = 0;
        self.active(hits)
    }
    pub fn command(&mut self, hits: &[ChartHit], command: NavigationCommand) -> Option<ChartEvent> {
        if hits.is_empty() {
            self.active = None;
            return None;
        }
        match command {
            NavigationCommand::Previous => {
                self.active = Some(self.active.unwrap_or(0).saturating_sub(1))
            }
            NavigationCommand::Next => {
                self.active = Some((self.active.unwrap_or(0) + 1).min(hits.len() - 1))
            }
            NavigationCommand::Home => self.active = Some(0),
            NavigationCommand::End => self.active = Some(hits.len() - 1),
            NavigationCommand::PreviousSeries | NavigationCommand::NextSeries => {
                self.move_series(hits, command == NavigationCommand::NextSeries)
            }
            NavigationCommand::CycleOverlap => self.cycle_overlap(hits),
            NavigationCommand::Cancel => {
                self.active = None;
                self.overlap_cycle = 0;
                return None;
            }
            NavigationCommand::Select => {
                let hit = self.active(hits)?;
                return Some(ChartEvent::Select {
                    dataset: hit.dataset.clone(),
                    row: hit.row.clone(),
                    series: hit.series.clone(),
                    revision: hit.revision,
                });
            }
        }
        let hit = self.active(hits)?;
        Some(ChartEvent::Inspect {
            dataset: hit.dataset.clone(),
            row: hit.row.clone(),
            revision: hit.revision,
        })
    }
    pub fn reconcile(&mut self, hits: &[ChartHit]) {
        let Some(active) = self.active else { return };
        if active >= hits.len() {
            self.active = hits.len().checked_sub(1);
        }
    }
    fn move_series(&mut self, hits: &[ChartHit], forward: bool) {
        let current = self.active.unwrap_or(0);
        let bounds = hits[current].bounds;
        let iterator: Box<dyn Iterator<Item = usize>> = if forward {
            Box::new((current + 1..hits.len()).chain(0..=current))
        } else {
            Box::new((0..current).rev().chain((current..hits.len()).rev()))
        };
        self.active = iterator
            .filter(|index| hits[*index].series != hits[current].series)
            .min_by(|a, b| {
                center_distance(hits[*a].bounds, bounds)
                    .total_cmp(&center_distance(hits[*b].bounds, bounds))
            })
            .or(Some(current));
    }
    fn cycle_overlap(&mut self, hits: &[ChartHit]) {
        let Some(current) = self.active else {
            self.active = Some(0);
            return;
        };
        let bounds = hits[current].bounds;
        let overlaps: Vec<usize> = hits
            .iter()
            .enumerate()
            .filter(|(_, hit)| intersects(bounds, hit.bounds))
            .map(|(index, _)| index)
            .collect();
        if !overlaps.is_empty() {
            self.overlap_cycle = (self.overlap_cycle + 1) % overlaps.len();
            self.active = Some(overlaps[self.overlap_cycle]);
        }
    }
}

/// Opt-in bounded continuous Cartesian pan/zoom policy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanZoomPolicy {
    pub x_extent: Option<[f64; 2]>,
    pub y_extent: Option<[f64; 2]>,
    pub minimum_x_span: f64,
    pub minimum_y_span: f64,
}
impl PanZoomPolicy {
    pub fn zoom(&self, viewport: Viewport, factor: f64, anchor: [f64; 2]) -> Option<Viewport> {
        if !factor.is_finite() || factor <= 0.0 {
            return None;
        }
        Some(Viewport {
            x: viewport.x.map(|range| {
                constrain(
                    zoom_axis(range, factor, anchor[0]),
                    self.x_extent,
                    self.minimum_x_span,
                )
            }),
            y: viewport.y.map(|range| {
                constrain(
                    zoom_axis(range, factor, anchor[1]),
                    self.y_extent,
                    self.minimum_y_span,
                )
            }),
        })
    }
    pub fn pan(&self, viewport: Viewport, delta: [f64; 2]) -> Option<Viewport> {
        if delta.iter().any(|value| !value.is_finite()) {
            return None;
        }
        Some(Viewport {
            x: viewport.x.map(|range| {
                constrain(
                    [range[0] + delta[0], range[1] + delta[0]],
                    self.x_extent,
                    self.minimum_x_span,
                )
            }),
            y: viewport.y.map(|range| {
                constrain(
                    [range[0] + delta[1], range[1] + delta[1]],
                    self.y_extent,
                    self.minimum_y_span,
                )
            }),
        })
    }
    pub const fn reset(&self) -> Viewport {
        Viewport {
            x: self.x_extent,
            y: self.y_extent,
        }
    }
}

fn zoom_axis(range: [f64; 2], factor: f64, anchor: f64) -> [f64; 2] {
    [
        anchor + (range[0] - anchor) / factor,
        anchor + (range[1] - anchor) / factor,
    ]
}
fn constrain(mut range: [f64; 2], extent: Option<[f64; 2]>, minimum_span: f64) -> [f64; 2] {
    let direction = if range[1] >= range[0] { 1.0 } else { -1.0 };
    if (range[1] - range[0]).abs() < minimum_span.max(0.0) {
        let center = (range[0] + range[1]) / 2.0;
        let half = minimum_span.max(0.0) / 2.0;
        range = [center - half * direction, center + half * direction];
    }
    if let Some(extent) = extent {
        let low = extent[0].min(extent[1]);
        let high = extent[0].max(extent[1]);
        let span = (range[1] - range[0]).abs();
        if span >= high - low {
            return if direction > 0.0 {
                [low, high]
            } else {
                [high, low]
            };
        }
        let min = range[0].min(range[1]);
        let max = range[0].max(range[1]);
        let shift = if min < low {
            low - min
        } else if max > high {
            high - max
        } else {
            0.0
        };
        range[0] += shift;
        range[1] += shift;
    }
    range
}
fn distance(hit: &ChartHit, point: twinkle_ui::Point) -> f32 {
    let x = point.x.clamp(
        hit.bounds.origin.x,
        hit.bounds.origin.x + hit.bounds.size.width,
    );
    let y = point.y.clamp(
        hit.bounds.origin.y,
        hit.bounds.origin.y + hit.bounds.size.height,
    );
    (point.x - x).hypot(point.y - y)
}
fn center_distance(a: twinkle_ui::Rect, b: twinkle_ui::Rect) -> f32 {
    let ax = a.origin.x + a.size.width / 2.0;
    let ay = a.origin.y + a.size.height / 2.0;
    let bx = b.origin.x + b.size.width / 2.0;
    let by = b.origin.y + b.size.height / 2.0;
    (ax - bx).hypot(ay - by)
}
fn intersects(a: twinkle_ui::Rect, b: twinkle_ui::Rect) -> bool {
    a.origin.x <= b.origin.x + b.size.width
        && b.origin.x <= a.origin.x + a.size.width
        && a.origin.y <= b.origin.y + b.size.height
        && b.origin.y <= a.origin.y + a.size.height
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RowKey;
    use std::sync::Arc;
    fn hit(key: &str, x: f32) -> ChartHit {
        ChartHit {
            dataset: Arc::from("d"),
            row: RowKey::from(key),
            series: None,
            bounds: twinkle_ui::Rect::new(x, 0.0, 4.0, 4.0),
            revision: 7,
            source_index: 0,
            row_index: 0,
        }
    }
    #[test]
    fn keyboard_navigation_carries_stable_revision() {
        let hits = vec![hit("a", 0.0), hit("b", 10.0)];
        let mut navigation = ChartNavigation::default();
        navigation.command(&hits, NavigationCommand::End);
        let event = navigation
            .command(&hits, NavigationCommand::Select)
            .unwrap();
        assert!(
            matches!(event, ChartEvent::Select { row, revision: 7, .. } if row == RowKey::from("b"))
        );
    }
    #[test]
    fn zoom_is_anchored_clamped_and_has_minimum_span() {
        let policy = PanZoomPolicy {
            x_extent: Some([0.0, 100.0]),
            y_extent: None,
            minimum_x_span: 10.0,
            minimum_y_span: 0.0,
        };
        let zoomed = policy
            .zoom(
                Viewport {
                    x: Some([0.0, 100.0]),
                    y: None,
                },
                100.0,
                [75.0, 0.0],
            )
            .unwrap();
        assert_eq!(zoomed.x, Some([69.75, 79.75]));
        let panned = policy.pan(zoomed, [100.0, 0.0]).unwrap();
        assert_eq!(panned.x, Some([90.0, 100.0]));
    }
}

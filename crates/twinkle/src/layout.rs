//! Backend-neutral geometry, sizing constraints, and flex/grid vocabulary.

/// A logical two-dimensional extent.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    /// Horizontal extent in logical pixels.
    pub width: f32,
    /// Vertical extent in logical pixels.
    pub height: f32,
}

impl Size {
    /// Creates a size from logical width and height.
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

/// A point in logical viewport coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    /// Horizontal coordinate from the viewport's leading edge.
    pub x: f32,
    /// Vertical coordinate from the viewport's top edge.
    pub y: f32,
}

/// An axis-aligned logical rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    /// Top-left coordinate.
    pub origin: Point,
    /// Nonnegative logical extent.
    pub size: Size,
}

impl Rect {
    /// Creates a rectangle from its top-left coordinate and extent.
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            origin: Point { x, y },
            size: Size { width, height },
        }
    }

    /// Returns the content rectangle after applying edge insets.
    ///
    /// Width and height clamp to zero when the insets exceed the original extent.
    pub fn inset(self, insets: Insets) -> Self {
        Self::new(
            self.origin.x + insets.left,
            self.origin.y + insets.top,
            (self.size.width - insets.left - insets.right).max(0.0),
            (self.size.height - insets.top - insets.bottom).max(0.0),
        )
    }
}

/// Logical spacing on the four edges of a box.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    /// Top inset.
    pub top: f32,
    /// Right inset.
    pub right: f32,
    /// Bottom inset.
    pub bottom: f32,
    /// Left inset.
    pub left: f32,
}

impl Insets {
    /// Uses the same inset on every edge.
    pub const fn all(value: f32) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }

    /// Uses one value for left/right and another for top/bottom.
    pub const fn symmetric(horizontal: f32, vertical: f32) -> Self {
        Self {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }

    /// Applies an inset to the left and right edges only.
    pub const fn horizontal(value: f32) -> Self {
        Self::symmetric(value, 0.0)
    }

    /// Applies an inset to the top and bottom edges only.
    pub const fn vertical(value: f32) -> Self {
        Self::symmetric(0.0, value)
    }

    /// Returns the combined left and right inset.
    pub const fn width(self) -> f32 {
        self.left + self.right
    }

    /// Returns the combined top and bottom inset.
    pub const fn height(self) -> f32 {
        self.top + self.bottom
    }
}

impl From<f32> for Insets {
    fn from(value: f32) -> Self {
        Self::all(value)
    }
}

/// Minimum and maximum size accepted by a layout child.
///
/// Infinite maximums represent an indefinite parent axis. Resolution normalizes invalid or
/// contradictory values into finite, nonnegative output whenever a concrete size is constrained.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Constraints {
    /// Minimum permitted size.
    pub min: Size,
    /// Maximum permitted size.
    pub max: Size,
}

impl Constraints {
    /// Creates constraints from explicit minimum and maximum sizes.
    pub const fn new(min: Size, max: Size) -> Self {
        Self { min, max }
    }

    /// Clamps a candidate size to these constraints.
    pub fn constrain(self, size: Size) -> Size {
        let max_width = self.max.width.max(self.min.width);
        let max_height = self.max.height.max(self.min.height);
        Size {
            width: finite_nonnegative(size.width).clamp(self.min.width.max(0.0), max_width),
            height: finite_nonnegative(size.height).clamp(self.min.height.max(0.0), max_height),
        }
    }

    /// Creates constraints that require exactly `size`.
    pub const fn tight(size: Size) -> Self {
        Self {
            min: size,
            max: size,
        }
    }

    /// Creates zero-minimum constraints bounded by `max`.
    pub const fn loose(max: Size) -> Self {
        Self {
            min: Size::new(0.0, 0.0),
            max,
        }
    }

    /// Creates indefinite constraints on both axes.
    pub const fn unbounded() -> Self {
        Self::loose(Size::new(f32::INFINITY, f32::INFINITY))
    }
}

/// A declarative size along one layout axis.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Length {
    /// Uses the component's intrinsic size.
    #[default]
    Auto,
    /// Uses a fixed number of logical pixels.
    Px(f32),
    /// Uses a fraction of a definite parent extent, expressed from `0.0` to `1.0`.
    Percent(f32),
    /// Fills definite available space and falls back to intrinsic size when indefinite.
    Fill,
    /// Takes a weighted share of grid or flex free space.
    Fraction(f32),
    /// Uses the minimum intrinsic content contribution.
    MinContent,
    /// Uses the maximum intrinsic content contribution.
    MaxContent,
}

impl From<f32> for Length {
    fn from(value: f32) -> Self {
        Self::Px(value)
    }
}

impl Length {
    /// Creates a percentage length expressed as a fraction of the parent.
    pub const fn percent(value: f32) -> Self {
        Self::Percent(value)
    }

    /// Creates a fractional free-space length.
    pub const fn fr(value: f32) -> Self {
        Self::Fraction(value)
    }

    /// Resolves this length against a parent extent and intrinsic fallback.
    pub fn resolve(self, parent: f32, intrinsic: f32) -> f32 {
        match self {
            Self::Auto | Self::MinContent | Self::MaxContent => intrinsic,
            Self::Px(value) => finite_nonnegative(value),
            Self::Percent(value) if parent.is_finite() => {
                finite_nonnegative(parent * value.max(0.0))
            }
            Self::Percent(_) => intrinsic,
            Self::Fill => {
                if parent.is_finite() {
                    parent.max(0.0)
                } else {
                    intrinsic
                }
            }
            Self::Fraction(_) => intrinsic,
        }
    }
}

/// Cross-axis alignment of a child within its allocated layout slot.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Align {
    /// Aligns to the cross-axis start.
    #[default]
    Start,
    /// Centers on the cross axis.
    Center,
    /// Aligns to the cross-axis end.
    End,
    /// Expands to the available cross-axis extent.
    Stretch,
    /// Aligns compatible text children by baseline.
    Baseline,
}

/// Distribution of children along a container's main axis.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Justify {
    /// Packs children at the main-axis start.
    #[default]
    Start,
    /// Packs children at the center.
    Center,
    /// Packs children at the main-axis end.
    End,
    /// Distributes free space only between children.
    SpaceBetween,
    /// Distributes equal space around every child.
    SpaceAround,
    /// Distributes equal space between children and container edges.
    SpaceEvenly,
}

/// Painting and scrolling policy when content exceeds its allocated box.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Overflow {
    /// Paints overflow without clipping or scrolling.
    #[default]
    Visible,
    /// Clips content to the allocated box.
    Clip,
    /// Clips content and always provides scroll behavior on the axis.
    Scroll,
    /// Provides scroll behavior only when content overflows.
    Auto,
}

/// A declarative grid track definition.
#[derive(Clone, Debug, PartialEq)]
pub enum Track {
    /// A fixed logical-pixel track.
    Px(f32),
    /// A track sized from its contents.
    Auto,
    /// A weighted share of remaining grid space.
    Fraction(f32),
    /// A track clamped between two track sizing functions.
    MinMax(Box<Track>, Box<Track>),
    /// A fixed number of repeated tracks.
    Repeat(usize, Box<Track>),
    /// As many repeated tracks as fit the definite grid extent.
    AutoFit(Box<Track>),
}

impl Track {
    /// Creates a fixed logical-pixel track.
    pub fn px(value: f32) -> Self {
        Self::Px(value)
    }

    /// Creates a fractional track.
    pub fn fr(value: f32) -> Self {
        Self::Fraction(value)
    }

    /// Creates a track clamped between minimum and maximum definitions.
    pub fn minmax(min: impl Into<Track>, max: impl Into<Track>) -> Self {
        Self::MinMax(Box::new(min.into()), Box::new(max.into()))
    }

    /// Repeats a track definition `count` times.
    pub fn repeat(count: usize, track: impl Into<Track>) -> Self {
        Self::Repeat(count, Box::new(track.into()))
    }

    /// Repeats a track definition to fill the available grid extent.
    pub fn repeat_auto_fit(track: impl Into<Track>) -> Self {
        Self::AutoFit(Box::new(track.into()))
    }
}

impl From<f32> for Track {
    fn from(value: f32) -> Self {
        Self::Px(value)
    }
}

/// Primary direction of a flex layout operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Axis {
    /// Places children from left to right in logical coordinates.
    Horizontal,
    /// Places children from top to bottom.
    Vertical,
}

/// One dimension's intrinsic and flexible sizing inputs for [`layout_flex`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlexItem {
    /// Preferred extent before distributing positive or negative free space.
    pub preferred: f32,
    /// Smallest permitted extent.
    pub min: f32,
    /// Largest permitted extent.
    pub max: f32,
    /// Relative weight when distributing positive free space.
    pub grow: f32,
    /// Relative weight when removing overflowing space.
    pub shrink: f32,
}

impl FlexItem {
    /// Creates a non-growing, non-shrinking item of exactly `size`.
    pub const fn fixed(size: f32) -> Self {
        Self {
            preferred: size,
            min: size,
            max: size,
            grow: 0.0,
            shrink: 0.0,
        }
    }

    /// Creates a growing item with the default shrink factor of one.
    pub const fn flexible(preferred: f32, min: f32, max: f32, grow: f32) -> Self {
        Self {
            preferred,
            min,
            max,
            grow,
            shrink: 1.0,
        }
    }

    /// Creates an item with explicit grow and shrink factors.
    pub const fn flex(preferred: f32, min: f32, max: f32, grow: f32, shrink: f32) -> Self {
        Self {
            preferred,
            min,
            max,
            grow,
            shrink,
        }
    }
}

/// Resolves one-dimensional flex items into rectangles inside `bounds`.
///
/// The result preserves item order, applies `gap` between adjacent items, and never emits negative
/// or non-finite geometry. Cross-axis placement fills the supplied bounds.
pub fn layout_flex(bounds: Rect, axis: Axis, gap: f32, items: &[FlexItem]) -> Vec<Rect> {
    if items.is_empty() {
        return Vec::new();
    }
    let available = match axis {
        Axis::Horizontal => bounds.size.width,
        Axis::Vertical => bounds.size.height,
    };
    let gap_total = gap * items.len().saturating_sub(1) as f32;
    let mut sizes: Vec<_> = items
        .iter()
        .map(|item| item.preferred.clamp(item.min, item.max))
        .collect();
    let occupied = sizes.iter().sum::<f32>() + gap_total;
    let extra = (available - occupied).max(0.0);
    let total_grow = items.iter().map(|item| item.grow.max(0.0)).sum::<f32>();
    if extra > 0.0 && total_grow > 0.0 {
        for (size, item) in sizes.iter_mut().zip(items) {
            *size = (*size + extra * item.grow.max(0.0) / total_grow).min(item.max);
        }
    }
    let mut deficit = (occupied - available).max(0.0);
    while deficit > f32::EPSILON {
        let shrink_weight = items
            .iter()
            .zip(&sizes)
            .filter(|(item, size)| item.shrink > 0.0 && **size > item.min)
            .map(|(item, size)| item.shrink * *size)
            .sum::<f32>();
        if shrink_weight <= f32::EPSILON {
            break;
        }
        let mut removed = 0.0;
        for (item, size) in items.iter().zip(&mut sizes) {
            if item.shrink <= 0.0 || *size <= item.min {
                continue;
            }
            let share = deficit * item.shrink * *size / shrink_weight;
            let next = (*size - share).max(item.min);
            removed += *size - next;
            *size = next;
        }
        if removed <= f32::EPSILON {
            break;
        }
        deficit -= removed;
    }

    let mut cursor = match axis {
        Axis::Horizontal => bounds.origin.x,
        Axis::Vertical => bounds.origin.y,
    };
    sizes
        .into_iter()
        .map(|main| {
            let rect = match axis {
                Axis::Horizontal => Rect::new(cursor, bounds.origin.y, main, bounds.size.height),
                Axis::Vertical => Rect::new(bounds.origin.x, cursor, bounds.size.width, main),
            };
            cursor += main + gap;
            rect
        })
        .collect()
}

fn finite_nonnegative(value: f32) -> f32 {
    if value.is_nan() || value.is_sign_negative() {
        0.0
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::{Axis, FlexItem, Insets, Length, Rect, Size, layout_flex};
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn valid_flex_inputs_always_produce_finite_nonnegative_geometry(
            width in 0.0_f32..4096.0,
            height in 0.0_f32..2160.0,
            gap in 0.0_f32..64.0,
            preferred in prop::collection::vec(0.0_f32..1024.0, 0..24),
        ) {
            let items = preferred
                .into_iter()
                .map(|preferred| FlexItem::flex(preferred, 0.0, 2048.0, 1.0, 1.0))
                .collect::<Vec<_>>();
            for rect in layout_flex(
                Rect::new(0.0, 0.0, width, height),
                Axis::Horizontal,
                gap,
                &items,
            ) {
                prop_assert!(rect.origin.x.is_finite());
                prop_assert!(rect.origin.y.is_finite());
                prop_assert!(rect.size.width.is_finite() && rect.size.width >= 0.0);
                prop_assert!(rect.size.height.is_finite() && rect.size.height >= 0.0);
            }
        }
    }

    #[test]
    fn row_distributes_remaining_space_to_flexible_children() {
        let layout = layout_flex(
            Rect::new(0.0, 0.0, 300.0, 40.0),
            Axis::Horizontal,
            10.0,
            &[
                FlexItem::fixed(50.0),
                FlexItem::flexible(50.0, 20.0, 500.0, 1.0),
                FlexItem::flexible(50.0, 20.0, 500.0, 1.0),
            ],
        );
        assert_eq!(layout[0], Rect::new(0.0, 0.0, 50.0, 40.0));
        assert_eq!(layout[1], Rect::new(60.0, 0.0, 115.0, 40.0));
        assert_eq!(layout[2], Rect::new(185.0, 0.0, 115.0, 40.0));
    }

    #[test]
    fn insets_never_produce_negative_content_size() {
        assert_eq!(
            Rect::new(0.0, 0.0, 10.0, 10.0).inset(Insets::all(8.0)),
            Rect::new(8.0, 8.0, 0.0, 0.0)
        );
    }

    #[test]
    fn flex_items_shrink_proportionally_without_crossing_minimums() {
        let layout = layout_flex(
            Rect::new(0.0, 0.0, 120.0, 20.0),
            Axis::Horizontal,
            0.0,
            &[
                FlexItem::flex(100.0, 80.0, 200.0, 0.0, 1.0),
                FlexItem::flex(100.0, 20.0, 200.0, 0.0, 1.0),
            ],
        );
        assert_eq!(layout[0].size.width, 80.0);
        assert_eq!(layout[1].size.width, 40.0);
    }

    #[test]
    fn percentage_and_fill_fall_back_to_intrinsic_when_parent_is_unbounded() {
        assert_eq!(Length::percent(0.5).resolve(200.0, 30.0), 100.0);
        assert_eq!(Length::percent(0.5).resolve(f32::INFINITY, 30.0), 30.0);
        assert_eq!(Length::Fill.resolve(f32::INFINITY, 30.0), 30.0);
        assert_eq!(Length::Fill.resolve(200.0, 30.0), 200.0);
    }

    #[test]
    fn contradictory_constraints_resolve_without_negative_or_nonfinite_geometry() {
        let constrained = super::Constraints::new(Size::new(80.0, 40.0), Size::new(20.0, 10.0))
            .constrain(Size::new(f32::NAN, -1.0));
        assert_eq!(constrained, Size::new(80.0, 40.0));
    }
}

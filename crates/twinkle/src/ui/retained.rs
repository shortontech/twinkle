use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use crate::{Rect, Size, UiId};

use super::{
    BoxShadow, DropdownPartStyle, Element, GridColumnSpec, InteractionPaint, Kind, Length,
    Overflow, Style, Track,
};

/// Stable native identity assigned independently of a node's current arena slot.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct RetainedNodeId(u64);

/// Independent work classes dirtied by declarative reconciliation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DirtyPhases(u8);

impl DirtyPhases {
    pub(crate) const CHILDREN: Self = Self(1 << 0);
    pub(crate) const MEASURE: Self = Self(1 << 1);
    pub(crate) const PLACE: Self = Self(1 << 2);
    pub(crate) const PAINT: Self = Self(1 << 3);
    pub(crate) const INTERACTION: Self = Self(1 << 4);
    pub(crate) const SEMANTICS: Self = Self(1 << 5);
    pub(crate) const ALL: Self = Self((1 << 6) - 1);

    pub(crate) const fn contains(self, phase: Self) -> bool {
        self.0 & phase.0 == phase.0
    }

    const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    fn insert(&mut self, other: Self) {
        *self = self.union(other);
    }

    const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Intrinsic measurements for the two scroll-content constraint variants.
type ScrollMeasurementCache = [Option<([u32; 2], Size)>; 2];

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RetainedPhaseData {
    pub(crate) measured: Option<Size>,
    measured_constraints: Option<[u32; 2]>,
    scroll_measurements: Option<Box<ScrollMeasurementCache>>,
    pub(crate) allocated: Option<Rect>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ReconcileStats {
    pub(crate) visited: usize,
    pub(crate) reused: usize,
    pub(crate) created: usize,
    pub(crate) removed: usize,
    pub(crate) moved: usize,
    pub(crate) replaced: usize,
    pub(crate) clean: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct RetainedNode {
    id: RetainedNodeId,
    parent: Option<RetainedNodeId>,
    kind: KindTag,
    dirty: DirtyPhases,
    children: Vec<RetainedNodeId>,
    subtree_dirty: DirtyPhases,
    subtree_nodes: usize,
    /// Whether resolved records can be copied without recreating auxiliary
    /// layout collections such as grids, scroll regions, or dropdown layers.
    subtree_bulk_resolved_reuse_safe: bool,
    /// Whether paint and interaction output can be translated/reused without
    /// rescanning descendants for scroll, dropdown, or geometry-dependent
    /// emission behavior.
    subtree_independent_output_reuse_safe: bool,
    /// Recomputed bottom-up during reconciliation, before layout reuse queries.
    subtree_layout_clean: bool,
    pub(crate) phases: RetainedPhaseData,
    metadata: Option<Box<RetainedNodeMetadata>>,
}

#[derive(Clone, Debug)]
pub(crate) struct RetainedNodeMetadata {
    identity: ChildIdentity,
    #[cfg_attr(not(test), allow(dead_code))]
    ui_id: UiId,
    signatures: PhaseSignatures,
}

impl std::ops::Deref for RetainedNode {
    type Target = RetainedNodeMetadata;

    fn deref(&self) -> &Self::Target {
        self.metadata
            .as_deref()
            .expect("live retained node has metadata")
    }
}

impl std::ops::DerefMut for RetainedNode {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.metadata
            .as_deref_mut()
            .expect("live retained node has metadata")
    }
}

impl RetainedNode {
    #[cfg(test)]
    pub(crate) const fn id(&self) -> RetainedNodeId {
        self.id
    }

    #[cfg(test)]
    pub(crate) const fn dirty(&self) -> DirtyPhases {
        self.dirty
    }

    pub(crate) const fn parent(&self) -> Option<RetainedNodeId> {
        self.parent
    }

    fn invalidate_dirty_phases(&mut self) {
        if self.dirty.contains(DirtyPhases::MEASURE) {
            self.phases.measured = None;
        }
        if self.dirty.contains(DirtyPhases::PLACE) {
            self.phases.allocated = None;
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RetainedNodeArena {
    next_id: u64,
    root: Option<RetainedNodeId>,
    nodes: HashMap<RetainedNodeId, RetainedNode>,
    // Identity lookup is on every retained layout/emission query. Ordering is
    // never observed; hashing avoids paying tree-comparison cost for the full
    // scoped UiId path several times per node and frame.
    by_ui_id: HashMap<UiId, RetainedNodeId>,
    last: ReconcileStats,
    pending_commit: Vec<RetainedNodeId>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum ChildIdentity {
    Root,
    Key(UiId),
    Position(usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum KindTag {
    Row,
    Column,
    Layer,
    Scroll,
    Grid,
    CustomPaint,
    Text,
    StyledText,
    Image,
    Slider,
    Dropdown,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PhaseSignatures {
    measure: u64,
    place: u64,
    paint: u64,
    interaction: u64,
    semantics: u64,
    content_revision: Option<u64>,
    measure_content_sensitive: bool,
    paint_content_sensitive: bool,
    interaction_content_sensitive: bool,
    semantics_content_sensitive: bool,
}

#[derive(Default)]
struct CompactHasher(std::collections::hash_map::DefaultHasher);

impl CompactHasher {
    fn add<T: Hash>(&mut self, value: &T) {
        value.hash(&mut self.0);
    }

    fn finish(self) -> u64 {
        self.0.finish()
    }
}

fn bits(value: f32) -> u32 {
    value.to_bits()
}

fn length(hasher: &mut CompactHasher, value: Length) {
    std::mem::discriminant(&value).hash(&mut hasher.0);
    match value {
        Length::Px(value) | Length::Percent(value) | Length::Fraction(value) => {
            hasher.add(&bits(value));
        }
        Length::Auto | Length::Fill | Length::MinContent | Length::MaxContent => {}
    }
}

fn optional_bits(hasher: &mut CompactHasher, value: Option<f32>) {
    hasher.add(&value.map(bits));
}

fn interaction_paint(hasher: &mut CompactHasher, value: &InteractionPaint) {
    hasher.add(&value.background);
    hasher.add(&value.foreground);
    hasher.add(&value.border_color);
    optional_bits(hasher, value.border_width);
    optional_bits(hasher, value.radius);
    optional_bits(hasher, value.font_size);
    optional_bits(hasher, value.line_height);
    for length_value in [value.width, value.height] {
        hasher.add(&length_value.is_some());
        if let Some(length_value) = length_value {
            length(hasher, length_value);
        }
    }
}

fn box_shadow(hasher: &mut CompactHasher, value: Option<BoxShadow>) {
    hasher.add(&value.is_some());
    if let Some(value) = value {
        hasher.add(&bits(value.offset_x));
        hasher.add(&bits(value.offset_y));
        hasher.add(&bits(value.blur));
        hasher.add(&bits(value.spread));
        hasher.add(&value.color);
    }
}

fn dropdown_part(hasher: &mut CompactHasher, value: &DropdownPartStyle) {
    for float in [
        value.width,
        value.height,
        value.padding.top,
        value.padding.right,
        value.padding.bottom,
        value.padding.left,
        value.margin.top,
        value.margin.right,
        value.margin.bottom,
        value.margin.left,
        value.border_width,
        value.radius,
        value.font_size,
        value.line_height,
    ] {
        hasher.add(&bits(float));
    }
    hasher.add(&value.background);
    hasher.add(&value.interaction_backgrounds);
    for paint in &value.interaction_paints {
        interaction_paint(hasher, paint);
    }
    hasher.add(&value.inherited_text);
    hasher.add(&value.foreground);
    hasher.add(&value.border_color);
}

fn track(hasher: &mut CompactHasher, value: &Track) {
    std::mem::discriminant(value).hash(&mut hasher.0);
    match value {
        Track::Px(value) | Track::Fraction(value) => hasher.add(&bits(*value)),
        Track::MinMax(minimum, maximum) => {
            track(hasher, minimum);
            track(hasher, maximum);
        }
        Track::Repeat(count, repeated) => {
            hasher.add(count);
            track(hasher, repeated);
        }
        Track::AutoFit(repeated) => track(hasher, repeated),
        Track::Auto => {}
    }
}

fn grid_columns(hasher: &mut CompactHasher, columns: &GridColumnSpec) {
    std::mem::discriminant(columns).hash(&mut hasher.0);
    match columns {
        GridColumnSpec::Count(count) => hasher.add(count),
        GridColumnSpec::Tracks(tracks) => {
            hasher.add(&tracks.len());
            for value in tracks {
                track(hasher, value);
            }
        }
        GridColumnSpec::AutoFit(value) => track(hasher, value),
    }
}

fn hash_layout_style(style: &Style) -> (u64, u64) {
    let mut measure = CompactHasher::default();
    for value in [
        style.padding.top,
        style.padding.right,
        style.padding.bottom,
        style.padding.left,
    ] {
        measure.add(&bits(value));
    }
    measure.add(&bits(style.gap));
    length(&mut measure, style.width);
    length(&mut measure, style.height);
    length(&mut measure, style.basis);
    for value in [
        style.min_width,
        style.min_height,
        style.max_width,
        style.max_height,
    ] {
        measure.add(&bits(value));
    }
    measure.add(&bits(style.grow));
    measure.add(&bits(style.shrink));
    measure.add(&style.align_self);
    measure.add(&style.align_items);
    measure.add(&style.justify_content);

    let mut place = CompactHasher::default();
    place.add(&style.overflow_x);
    place.add(&style.overflow_y);
    if let Some(position) = style.absolute_position {
        place.add(&bits(position.x));
        place.add(&bits(position.y));
    }
    place.add(&style.follow_scroll_end);
    (measure.finish(), place.finish())
}

fn kind_tag(kind: &Kind) -> KindTag {
    match kind {
        Kind::Flex(super::Axis::Horizontal) => KindTag::Row,
        Kind::Flex(super::Axis::Vertical) => KindTag::Column,
        Kind::Layer => KindTag::Layer,
        Kind::VerticalScroll { .. } => KindTag::Scroll,
        Kind::Grid { .. } => KindTag::Grid,
        Kind::CustomPaint { .. } | Kind::CustomPaintCommands { .. } => KindTag::CustomPaint,
        Kind::Text { .. } => KindTag::Text,
        Kind::StyledText(_) => KindTag::StyledText,
        Kind::Image { .. } => KindTag::Image,
        Kind::Slider { .. } => KindTag::Slider,
        Kind::Dropdown(_) => KindTag::Dropdown,
    }
}

fn node_independent_output_reuse_safe<Message>(element: &Element<Message>) -> bool {
    !matches!(
        element.kind,
        Kind::VerticalScroll { .. } | Kind::Dropdown(_)
    ) && !matches!(element.style.overflow_x, Overflow::Scroll | Overflow::Auto)
        && !matches!(element.style.overflow_y, Overflow::Scroll | Overflow::Auto)
        && element.style.proximity_magnification.is_none()
}

fn signatures<Message>(element: &Element<Message>) -> PhaseSignatures {
    let (style_measure, place) = hash_layout_style(&element.style);
    let mut content_measure = CompactHasher::default();
    let mut paint = CompactHasher::default();
    let mut semantics = CompactHasher::default();
    let mut measure_content_sensitive = false;
    let mut paint_content_sensitive = false;
    let interaction_content_sensitive = element.primary_message().is_some()
        || element.declared_context_message().is_some()
        || element.declared_focus_message().is_some()
        || element.declared_blur_message().is_some()
        || element.declared_drag_seed().is_some()
        || element.declared_drop_message().is_some()
        || element.text_mapper.is_some()
        || !element.option_messages.is_empty()
        || !element.inline_messages.is_empty();
    let mut semantics_content_sensitive = element.style.accessibility_label.is_some()
        || element.style.accessibility_description.is_some()
        || element.style.accessibility_role.is_some()
        || element.style.accessibility_state.is_some()
        || element.style.accessibility_controls.is_some();
    std::mem::discriminant(&element.kind).hash(&mut content_measure.0);
    std::mem::discriminant(&element.kind).hash(&mut paint.0);

    match &element.kind {
        Kind::Text {
            value,
            scale,
            bold,
            wrap,
            line_height,
            max_lines,
            ellipsis,
            details,
            ..
        } => {
            let _ = value;
            measure_content_sensitive = true;
            paint_content_sensitive = true;
            semantics_content_sensitive = true;
            content_measure.add(&bits(*scale));
            content_measure.add(bold);
            content_measure.add(wrap);
            content_measure.add(&line_height.map(bits));
            content_measure.add(max_lines);
            paint.add(ellipsis);
            paint.add(
                &details
                    .as_ref()
                    .and_then(|details| details.outline)
                    .map(|(color, width)| (color, bits(width))),
            );
            paint.add(
                &details
                    .as_ref()
                    .is_some_and(|details| details.input_value.is_some()),
            );
            paint.add(
                &details
                    .as_ref()
                    .is_some_and(|details| details.input_placeholder.is_some()),
            );
            paint.add(&details.as_ref().and_then(|details| details.input_mask));
        }
        Kind::StyledText(styled) => {
            measure_content_sensitive = true;
            paint_content_sensitive = true;
            semantics_content_sensitive = true;
            content_measure.add(&bits(styled.scale));
            content_measure.add(&styled.wrap);
            content_measure.add(&styled.line_height.map(bits));
        }
        Kind::Image {
            id,
            generation,
            presentation,
            ..
        } => {
            content_measure.add(id);
            content_measure.add(generation);
            content_measure.add(presentation);
            paint.add(id);
            paint.add(generation);
            paint.add(presentation);
        }
        Kind::VerticalScroll { offset, controlled } => {
            paint.add(&bits(*offset));
            paint.add(controlled);
        }
        Kind::Grid { columns } => grid_columns(&mut content_measure, columns),
        Kind::CustomPaint { paint: callback } => {
            paint.add(&(*callback as usize));
            paint_content_sensitive = true;
        }
        Kind::CustomPaintCommands { commands } => {
            let _ = commands;
            paint_content_sensitive = true;
        }
        Kind::Slider {
            value,
            track,
            fill,
            thumb,
            thumb_border,
            geometry,
            presentation,
        } => {
            paint.add(&bits(*value));
            paint.add(track);
            paint.add(fill);
            paint.add(thumb);
            paint.add(thumb_border);
            for value in geometry {
                paint.add(&bits(*value));
            }
            paint.add(&presentation.is_some());
            if let Some(parts) = presentation {
                for part in parts.as_ref() {
                    dropdown_part(&mut paint, part);
                }
            }
        }
        Kind::Dropdown(dropdown) => {
            let _ = (
                &dropdown.selected,
                &dropdown.options,
                dropdown
                    .details
                    .as_ref()
                    .map(|details| &details.option_presentations),
                dropdown
                    .details
                    .as_ref()
                    .map(|details| &details.resolved_options),
            );
            measure_content_sensitive = true;
            paint_content_sensitive = true;
            semantics_content_sensitive = true;
            if let Some(details) = &dropdown.details {
                for icon in
                    std::iter::once(&details.selected_icon).chain(details.option_icons.iter())
                {
                    paint.add(&icon.as_ref().map(|(id, _, generation)| (*id, *generation)));
                }
            }
            paint.add(&dropdown.expanded);
            paint.add(&dropdown.open_generation);
            paint.add(&dropdown.overlay);
            paint.add(&dropdown.background);
            paint.add(&dropdown.option_background);
            paint.add(&dropdown.foreground);
            paint.add(&dropdown.presentation.is_some());
            if let Some(parts) = &dropdown.presentation {
                for part in parts.as_ref() {
                    dropdown_part(&mut paint, part);
                }
            }
        }
        Kind::Flex(_) | Kind::Layer => {}
    }

    let fixed_leaf = element.children.is_empty()
        && matches!(element.style.width, Length::Px(_))
        && matches!(element.style.height, Length::Px(_));
    let measure = if fixed_leaf {
        measure_content_sensitive = false;
        style_measure
    } else {
        style_measure ^ content_measure.finish()
    };
    paint.add(&element.style.background);
    paint.add(&element.style.border);
    paint.add(&bits(element.style.border_width));
    paint.add(&element.style.foreground);
    box_shadow(&mut paint, element.style.box_shadow);
    optional_bits(&mut paint, element.style.backdrop_blur);
    paint.add(&bits(element.style.corner_radius));
    paint.add(&bits(element.style.top_corner_radius));
    paint.add(&element.style.interaction_paints.is_some());
    if let Some(interaction_paints) = &element.style.interaction_paints {
        for value in interaction_paints.as_ref() {
            interaction_paint(&mut paint, value);
        }
    }
    paint.add(&element.style.text_align);
    paint.add(&element.style.css_paint);
    paint.add(&element.style.editing_parts.is_some());
    if let Some(parts) = &element.style.editing_parts {
        for part in parts.as_ref() {
            dropdown_part(&mut paint, part);
        }
    }
    paint.add(&element.style.editing_menu.is_some());
    if let Some(menu) = &element.style.editing_menu {
        dropdown_part(&mut paint, &menu.frame);
        for part in menu.item.iter().chain(menu.disabled_item.iter()) {
            dropdown_part(&mut paint, part);
        }
        paint.add(&bits(menu.row_gap));
    }
    paint.add(&element.style.focus_background_tint);

    let mut interaction = CompactHasher::default();
    interaction.add(&element.virtual_navigation);
    interaction.add(&element.primary_message().is_some());
    interaction.add(&element.declared_context_message().is_some());
    interaction.add(&element.declared_focus_message().is_some());
    interaction.add(&element.declared_blur_message().is_some());
    interaction.add(&element.message_mapper.map(|callback| callback as usize));
    interaction.add(
        &element
            .seeded_value_mapper
            .map(|callback| callback as usize),
    );
    interaction.add(
        &element
            .scroll_extent_mapper
            .map(|callback| callback as usize),
    );
    interaction.add(&element.drag_mapper.map(|callback| callback as usize));
    interaction.add(&element.drop_mapper.map(|callback| callback as usize));
    interaction.add(&element.text_mapper.is_some());
    interaction.add(&element.option_messages.len());
    interaction.add(&element.inline_messages.len());

    semantics.add(&element.style.accessibility_label.is_some());
    semantics.add(&element.style.accessibility_description.is_some());
    semantics.add(&element.style.accessibility_role.is_some());
    semantics.add(&element.style.accessibility_state.is_some());
    semantics.add(&element.style.accessibility_controls.is_some());
    semantics.add(&element.style.semantic_role);
    semantics.add(&element.style.accessibility_hidden);
    semantics.add(&element.style.semantic_decorative);

    PhaseSignatures {
        measure,
        place,
        paint: paint.finish(),
        interaction: interaction.finish(),
        semantics: semantics.finish(),
        content_revision: element.content_revision,
        measure_content_sensitive,
        paint_content_sensitive,
        interaction_content_sensitive,
        semantics_content_sensitive,
    }
}

// Fingerprints reject unequal metadata cheaply, but never authorize reuse by
// themselves. This exact comparison deliberately includes a superset of the
// phase inputs: an uncommon change may invalidate more phases than strictly
// necessary, while a forced fingerprint collision can never preserve stale
// output. Content payloads with an explicit source revision remain governed by
// that revision in `phase_matches`.
fn exact_metadata_equal<Message>(current: &Element<Message>, previous: &Element<Message>) -> bool {
    fn optional_f32_equal(current: Option<f32>, previous: Option<f32>) -> bool {
        current.map(f32::to_bits) == previous.map(f32::to_bits)
    }

    fn text_mapper_equal<Message>(
        current: Option<&super::TextMessageMapper<Message>>,
        previous: Option<&super::TextMessageMapper<Message>>,
    ) -> bool {
        use super::TextMessageMapper;
        match (current, previous) {
            (None, None) => true,
            (
                Some(TextMessageMapper::Function(current)),
                Some(TextMessageMapper::Function(previous)),
            ) => *current as usize == *previous as usize,
            (
                Some(TextMessageMapper::Closure(current)),
                Some(TextMessageMapper::Closure(previous)),
            ) => std::sync::Arc::ptr_eq(current, previous),
            _ => false,
        }
    }

    fn dropdown_equal(current: &super::DropdownKind, previous: &super::DropdownKind) -> bool {
        fn icon_equal(
            current: &Option<(u16, std::sync::Arc<image::RgbaImage>, u64)>,
            previous: &Option<(u16, std::sync::Arc<image::RgbaImage>, u64)>,
        ) -> bool {
            match (current, previous) {
                (
                    Some((current_id, _, current_generation)),
                    Some((previous_id, _, previous_generation)),
                ) => current_id == previous_id && current_generation == previous_generation,
                (None, None) => true,
                _ => false,
            }
        }

        current.selected == previous.selected
            && current.options == previous.options
            && current.expanded == previous.expanded
            && current.open_generation == previous.open_generation
            && current.overlay == previous.overlay
            && current.background == previous.background
            && current.option_background == previous.option_background
            && current.foreground == previous.foreground
            && current.presentation == previous.presentation
            && match (&current.details, &previous.details) {
                (Some(current), Some(previous)) => {
                    current.option_presentations == previous.option_presentations
                        && current.resolved_options == previous.resolved_options
                        && icon_equal(&current.selected_icon, &previous.selected_icon)
                        && current.option_icons.len() == previous.option_icons.len()
                        && current
                            .option_icons
                            .iter()
                            .zip(&previous.option_icons)
                            .all(|(current, previous)| icon_equal(current, previous))
                }
                (None, None) => true,
                _ => false,
            }
    }

    fn kind_equal(current: &Kind, previous: &Kind) -> bool {
        match (current, previous) {
            (Kind::Flex(current), Kind::Flex(previous)) => current == previous,
            (Kind::Layer, Kind::Layer) => true,
            (
                Kind::VerticalScroll {
                    offset: current_offset,
                    controlled: current_controlled,
                },
                Kind::VerticalScroll {
                    offset: previous_offset,
                    controlled: previous_controlled,
                },
            ) => {
                current_offset.to_bits() == previous_offset.to_bits()
                    && current_controlled == previous_controlled
            }
            (Kind::Grid { columns: current }, Kind::Grid { columns: previous }) => {
                current == previous
            }
            (Kind::CustomPaint { paint: current }, Kind::CustomPaint { paint: previous }) => {
                *current as usize == *previous as usize
            }
            (Kind::CustomPaintCommands { .. }, Kind::CustomPaintCommands { .. }) => true,
            (
                Kind::Text {
                    scale: current_scale,
                    bold: current_bold,
                    wrap: current_wrap,
                    line_height: current_line_height,
                    max_lines: current_max_lines,
                    ellipsis: current_ellipsis,
                    details: current_details,
                    ..
                },
                Kind::Text {
                    scale: previous_scale,
                    bold: previous_bold,
                    wrap: previous_wrap,
                    line_height: previous_line_height,
                    max_lines: previous_max_lines,
                    ellipsis: previous_ellipsis,
                    details: previous_details,
                    ..
                },
            ) => {
                current_scale.to_bits() == previous_scale.to_bits()
                    && current_bold == previous_bold
                    && current_wrap == previous_wrap
                    && optional_f32_equal(*current_line_height, *previous_line_height)
                    && current_max_lines == previous_max_lines
                    && current_ellipsis == previous_ellipsis
                    && match (current_details, previous_details) {
                        (Some(current), Some(previous)) => {
                            current
                                .outline
                                .map(|(color, width)| (color, width.to_bits()))
                                == previous
                                    .outline
                                    .map(|(color, width)| (color, width.to_bits()))
                                && current
                                    .selection_x
                                    .map(|(start, end)| (start.to_bits(), end.to_bits()))
                                    == previous
                                        .selection_x
                                        .map(|(start, end)| (start.to_bits(), end.to_bits()))
                                && current
                                    .caret_position
                                    .map(|point| (point.x.to_bits(), point.y.to_bits()))
                                    == previous
                                        .caret_position
                                        .map(|point| (point.x.to_bits(), point.y.to_bits()))
                                && current.input_value.is_some() == previous.input_value.is_some()
                                && current.input_placeholder.is_some()
                                    == previous.input_placeholder.is_some()
                                && current.input_mask == previous.input_mask
                        }
                        (None, None) => true,
                        _ => false,
                    }
            }
            (Kind::StyledText(current), Kind::StyledText(previous)) => {
                current.scale.to_bits() == previous.scale.to_bits()
                    && current.wrap == previous.wrap
                    && optional_f32_equal(current.line_height, previous.line_height)
            }
            (
                Kind::Image {
                    id: current_id,
                    generation: current_generation,
                    presentation: current_presentation,
                    ..
                },
                Kind::Image {
                    id: previous_id,
                    generation: previous_generation,
                    presentation: previous_presentation,
                    ..
                },
            ) => {
                current_id == previous_id
                    && current_generation == previous_generation
                    && current_presentation == previous_presentation
            }
            (
                Kind::Slider {
                    value: current_value,
                    track: current_track,
                    fill: current_fill,
                    thumb: current_thumb,
                    thumb_border: current_thumb_border,
                    geometry: current_geometry,
                    presentation: current_presentation,
                },
                Kind::Slider {
                    value: previous_value,
                    track: previous_track,
                    fill: previous_fill,
                    thumb: previous_thumb,
                    thumb_border: previous_thumb_border,
                    geometry: previous_geometry,
                    presentation: previous_presentation,
                },
            ) => {
                current_value.to_bits() == previous_value.to_bits()
                    && current_track == previous_track
                    && current_fill == previous_fill
                    && current_thumb == previous_thumb
                    && current_thumb_border == previous_thumb_border
                    && current_geometry.map(f32::to_bits) == previous_geometry.map(f32::to_bits)
                    && current_presentation == previous_presentation
            }
            (Kind::Dropdown(current), Kind::Dropdown(previous)) => {
                dropdown_equal(current, previous)
            }
            _ => false,
        }
    }

    current.style == previous.style
        && kind_equal(&current.kind, &previous.kind)
        && current.virtual_navigation == previous.virtual_navigation
        && current.primary_message().is_some() == previous.primary_message().is_some()
        && current.declared_context_message().is_some()
            == previous.declared_context_message().is_some()
        && current.declared_focus_message().is_some() == previous.declared_focus_message().is_some()
        && current.declared_blur_message().is_some() == previous.declared_blur_message().is_some()
        && current.declared_drag_seed().is_some() == previous.declared_drag_seed().is_some()
        && current.declared_drop_message().is_some() == previous.declared_drop_message().is_some()
        && current.message_mapper.map(|mapper| mapper as usize)
            == previous.message_mapper.map(|mapper| mapper as usize)
        && current.seeded_value_mapper.map(|mapper| mapper as usize)
            == previous.seeded_value_mapper.map(|mapper| mapper as usize)
        && current.scroll_extent_mapper.map(|mapper| mapper as usize)
            == previous.scroll_extent_mapper.map(|mapper| mapper as usize)
        && current.drag_mapper.map(|mapper| mapper as usize)
            == previous.drag_mapper.map(|mapper| mapper as usize)
        && current.drop_mapper.map(|mapper| mapper as usize)
            == previous.drop_mapper.map(|mapper| mapper as usize)
        && text_mapper_equal(current.text_mapper.as_ref(), previous.text_mapper.as_ref())
        && current.option_messages.len() == previous.option_messages.len()
        && current.inline_messages.len() == previous.inline_messages.len()
        && current.navigation_scope == previous.navigation_scope
        && current.adjustment_step.to_bits() == previous.adjustment_step.to_bits()
}

#[derive(Clone, Copy)]
struct ExactPhaseMatches {
    measure: bool,
    place: bool,
    paint: bool,
    interaction: bool,
    semantics: bool,
}

impl ExactPhaseMatches {
    const ALL: Self = Self {
        measure: true,
        place: true,
        paint: true,
        interaction: true,
        semantics: true,
    };
}

fn exact_optional_f32(current: Option<f32>, previous: Option<f32>) -> bool {
    current.map(f32::to_bits) == previous.map(f32::to_bits)
}

fn exact_length(current: Length, previous: Length) -> bool {
    match (current, previous) {
        (Length::Px(current), Length::Px(previous))
        | (Length::Percent(current), Length::Percent(previous))
        | (Length::Fraction(current), Length::Fraction(previous)) => {
            current.to_bits() == previous.to_bits()
        }
        (Length::Auto, Length::Auto)
        | (Length::Fill, Length::Fill)
        | (Length::MinContent, Length::MinContent)
        | (Length::MaxContent, Length::MaxContent) => true,
        _ => false,
    }
}

fn exact_measure_style(current: &Style, previous: &Style) -> bool {
    current.padding.top.to_bits() == previous.padding.top.to_bits()
        && current.padding.right.to_bits() == previous.padding.right.to_bits()
        && current.padding.bottom.to_bits() == previous.padding.bottom.to_bits()
        && current.padding.left.to_bits() == previous.padding.left.to_bits()
        && current.gap.to_bits() == previous.gap.to_bits()
        && exact_length(current.width, previous.width)
        && exact_length(current.height, previous.height)
        && exact_length(current.basis, previous.basis)
        && current.min_width.to_bits() == previous.min_width.to_bits()
        && current.min_height.to_bits() == previous.min_height.to_bits()
        && current.max_width.to_bits() == previous.max_width.to_bits()
        && current.max_height.to_bits() == previous.max_height.to_bits()
        && current.grow.to_bits() == previous.grow.to_bits()
        && current.shrink.to_bits() == previous.shrink.to_bits()
        && current.align_self == previous.align_self
        && current.align_items == previous.align_items
        && current.justify_content == previous.justify_content
}

fn exact_measure_kind(current: &Kind, previous: &Kind) -> bool {
    match (current, previous) {
        (Kind::Flex(current), Kind::Flex(previous)) => current == previous,
        (Kind::Layer, Kind::Layer)
        | (Kind::VerticalScroll { .. }, Kind::VerticalScroll { .. })
        | (Kind::CustomPaint { .. }, Kind::CustomPaint { .. })
        | (Kind::CustomPaintCommands { .. }, Kind::CustomPaintCommands { .. })
        | (Kind::Slider { .. }, Kind::Slider { .. })
        | (Kind::Dropdown(_), Kind::Dropdown(_)) => true,
        (Kind::Grid { columns: current }, Kind::Grid { columns: previous }) => current == previous,
        (
            Kind::Text {
                scale: current_scale,
                bold: current_bold,
                wrap: current_wrap,
                line_height: current_line_height,
                max_lines: current_max_lines,
                ..
            },
            Kind::Text {
                scale: previous_scale,
                bold: previous_bold,
                wrap: previous_wrap,
                line_height: previous_line_height,
                max_lines: previous_max_lines,
                ..
            },
        ) => {
            current_scale.to_bits() == previous_scale.to_bits()
                && current_bold == previous_bold
                && current_wrap == previous_wrap
                && exact_optional_f32(*current_line_height, *previous_line_height)
                && current_max_lines == previous_max_lines
        }
        (Kind::StyledText(current), Kind::StyledText(previous)) => {
            current.scale.to_bits() == previous.scale.to_bits()
                && current.wrap == previous.wrap
                && exact_optional_f32(current.line_height, previous.line_height)
        }
        (
            Kind::Image {
                id: current_id,
                generation: current_generation,
                presentation: current_presentation,
                ..
            },
            Kind::Image {
                id: previous_id,
                generation: previous_generation,
                presentation: previous_presentation,
                ..
            },
        ) => {
            current_id == previous_id
                && current_generation == previous_generation
                && current_presentation == previous_presentation
        }
        _ => false,
    }
}

fn exact_paint_style(current: &Style, previous: &Style) -> bool {
    current.background == previous.background
        && current.border == previous.border
        && current.border_width.to_bits() == previous.border_width.to_bits()
        && current.foreground == previous.foreground
        && current.box_shadow == previous.box_shadow
        && exact_optional_f32(current.backdrop_blur, previous.backdrop_blur)
        && current.corner_radius.to_bits() == previous.corner_radius.to_bits()
        && current.top_corner_radius.to_bits() == previous.top_corner_radius.to_bits()
        && current.interaction_paints == previous.interaction_paints
        && current.text_align == previous.text_align
        && current.css_paint == previous.css_paint
        && current.editing_parts == previous.editing_parts
        && current.editing_menu == previous.editing_menu
        && current.focus_background_tint == previous.focus_background_tint
}

fn exact_paint_kind<Message>(current: &Element<Message>, previous: &Element<Message>) -> bool {
    match (&current.kind, &previous.kind) {
        (Kind::Flex(current), Kind::Flex(previous)) => current == previous,
        (Kind::Layer, Kind::Layer) | (Kind::Grid { .. }, Kind::Grid { .. }) => true,
        (
            Kind::VerticalScroll {
                offset: current_offset,
                controlled: current_controlled,
            },
            Kind::VerticalScroll {
                offset: previous_offset,
                controlled: previous_controlled,
            },
        ) => {
            current_offset.to_bits() == previous_offset.to_bits()
                && current_controlled == previous_controlled
        }
        (Kind::CustomPaint { paint: current }, Kind::CustomPaint { paint: previous }) => {
            *current as usize == *previous as usize
        }
        (Kind::CustomPaintCommands { .. }, Kind::CustomPaintCommands { .. }) => true,
        (
            Kind::Text {
                ellipsis: current_ellipsis,
                details: current_details,
                ..
            },
            Kind::Text {
                ellipsis: previous_ellipsis,
                details: previous_details,
                ..
            },
        ) => {
            current_ellipsis == previous_ellipsis
                && match (current_details, previous_details) {
                    (Some(current), Some(previous)) => {
                        current
                            .outline
                            .map(|(color, width)| (color, width.to_bits()))
                            == previous
                                .outline
                                .map(|(color, width)| (color, width.to_bits()))
                            && current
                                .selection_x
                                .map(|(start, end)| (start.to_bits(), end.to_bits()))
                                == previous
                                    .selection_x
                                    .map(|(start, end)| (start.to_bits(), end.to_bits()))
                            && current
                                .caret_position
                                .map(|point| (point.x.to_bits(), point.y.to_bits()))
                                == previous
                                    .caret_position
                                    .map(|point| (point.x.to_bits(), point.y.to_bits()))
                            && current.input_value.is_some() == previous.input_value.is_some()
                            && current.input_placeholder.is_some()
                                == previous.input_placeholder.is_some()
                            && current.input_mask == previous.input_mask
                    }
                    (None, None) => true,
                    _ => false,
                }
        }
        (Kind::StyledText(_), Kind::StyledText(_)) => true,
        (
            Kind::Image {
                id: current_id,
                generation: current_generation,
                presentation: current_presentation,
                ..
            },
            Kind::Image {
                id: previous_id,
                generation: previous_generation,
                presentation: previous_presentation,
                ..
            },
        ) => {
            current_id == previous_id
                && current_generation == previous_generation
                && current_presentation == previous_presentation
        }
        (
            Kind::Slider {
                value: current_value,
                track: current_track,
                fill: current_fill,
                thumb: current_thumb,
                thumb_border: current_thumb_border,
                geometry: current_geometry,
                presentation: current_presentation,
            },
            Kind::Slider {
                value: previous_value,
                track: previous_track,
                fill: previous_fill,
                thumb: previous_thumb,
                thumb_border: previous_thumb_border,
                geometry: previous_geometry,
                presentation: previous_presentation,
            },
        ) => {
            current_value.to_bits() == previous_value.to_bits()
                && current_track == previous_track
                && current_fill == previous_fill
                && current_thumb == previous_thumb
                && current_thumb_border == previous_thumb_border
                && current_geometry.map(f32::to_bits) == previous_geometry.map(f32::to_bits)
                && current_presentation == previous_presentation
        }
        (Kind::Dropdown(_), Kind::Dropdown(_)) => exact_metadata_equal(current, previous),
        _ => false,
    }
}

fn exact_text_mapper<Message>(
    current: Option<&super::TextMessageMapper<Message>>,
    previous: Option<&super::TextMessageMapper<Message>>,
) -> bool {
    use super::TextMessageMapper;
    match (current, previous) {
        (None, None) => true,
        (
            Some(TextMessageMapper::Function(current)),
            Some(TextMessageMapper::Function(previous)),
        ) => *current as usize == *previous as usize,
        (Some(TextMessageMapper::Closure(current)), Some(TextMessageMapper::Closure(previous))) => {
            std::sync::Arc::ptr_eq(current, previous)
        }
        _ => false,
    }
}

fn exact_phase_matches<Message>(
    current: &Element<Message>,
    previous: &Element<Message>,
) -> ExactPhaseMatches {
    let current_fixed = current.children.is_empty()
        && matches!(current.style.width, Length::Px(_))
        && matches!(current.style.height, Length::Px(_));
    let previous_fixed = previous.children.is_empty()
        && matches!(previous.style.width, Length::Px(_))
        && matches!(previous.style.height, Length::Px(_));
    ExactPhaseMatches {
        measure: current_fixed == previous_fixed
            && exact_measure_style(&current.style, &previous.style)
            && (current_fixed || exact_measure_kind(&current.kind, &previous.kind)),
        place: current.style.overflow_x == previous.style.overflow_x
            && current.style.overflow_y == previous.style.overflow_y
            && current.style.absolute_position == previous.style.absolute_position
            && current.style.follow_scroll_end == previous.style.follow_scroll_end,
        paint: exact_paint_style(&current.style, &previous.style)
            && exact_paint_kind(current, previous),
        interaction: current.virtual_navigation == previous.virtual_navigation
            && current.primary_message().is_some() == previous.primary_message().is_some()
            && current.declared_context_message().is_some()
                == previous.declared_context_message().is_some()
            && current.declared_focus_message().is_some()
                == previous.declared_focus_message().is_some()
            && current.declared_blur_message().is_some()
                == previous.declared_blur_message().is_some()
            && current.declared_drag_seed().is_some() == previous.declared_drag_seed().is_some()
            && current.declared_drop_message().is_some()
                == previous.declared_drop_message().is_some()
            && current.message_mapper.map(|mapper| mapper as usize)
                == previous.message_mapper.map(|mapper| mapper as usize)
            && current.seeded_value_mapper.map(|mapper| mapper as usize)
                == previous.seeded_value_mapper.map(|mapper| mapper as usize)
            && current.scroll_extent_mapper.map(|mapper| mapper as usize)
                == previous.scroll_extent_mapper.map(|mapper| mapper as usize)
            && current.drag_mapper.map(|mapper| mapper as usize)
                == previous.drag_mapper.map(|mapper| mapper as usize)
            && current.drop_mapper.map(|mapper| mapper as usize)
                == previous.drop_mapper.map(|mapper| mapper as usize)
            && exact_text_mapper(current.text_mapper.as_ref(), previous.text_mapper.as_ref())
            && current.option_messages.len() == previous.option_messages.len()
            && current.inline_messages.len() == previous.inline_messages.len()
            && current.navigation_scope == previous.navigation_scope
            && current.adjustment_step.to_bits() == previous.adjustment_step.to_bits(),
        semantics: current.style.accessibility_label == previous.style.accessibility_label
            && current.style.accessibility_description == previous.style.accessibility_description
            && current.style.accessibility_role == previous.style.accessibility_role
            && current.style.accessibility_state == previous.style.accessibility_state
            && current.style.accessibility_controls == previous.style.accessibility_controls
            && current.style.semantic_role == previous.style.semantic_role
            && current.style.accessibility_hidden == previous.style.accessibility_hidden
            && current.style.semantic_decorative == previous.style.semantic_decorative,
    }
}

fn phase_matches(
    current_metadata: u64,
    previous_metadata: u64,
    exact_metadata_equal: bool,
    current_sensitive: bool,
    previous_sensitive: bool,
    current_revision: Option<u64>,
    previous_revision: Option<u64>,
) -> bool {
    current_metadata == previous_metadata
        && exact_metadata_equal
        && current_sensitive == previous_sensitive
        && (!current_sensitive
            || current_revision
                .zip(previous_revision)
                .is_some_and(|(current, previous)| current == previous))
}

fn changed_phases(
    current: PhaseSignatures,
    previous: PhaseSignatures,
    exact: ExactPhaseMatches,
) -> DirtyPhases {
    let mut dirty = DirtyPhases::default();
    if !phase_matches(
        current.measure,
        previous.measure,
        exact.measure,
        current.measure_content_sensitive,
        previous.measure_content_sensitive,
        current.content_revision,
        previous.content_revision,
    ) {
        dirty.insert(DirtyPhases::MEASURE.union(DirtyPhases::PLACE));
    }
    if current.place != previous.place || !exact.place {
        dirty.insert(DirtyPhases::PLACE);
    }
    if !phase_matches(
        current.paint,
        previous.paint,
        exact.paint,
        current.paint_content_sensitive,
        previous.paint_content_sensitive,
        current.content_revision,
        previous.content_revision,
    ) {
        dirty.insert(DirtyPhases::PAINT);
    }
    if !phase_matches(
        current.interaction,
        previous.interaction,
        exact.interaction,
        current.interaction_content_sensitive,
        previous.interaction_content_sensitive,
        current.content_revision,
        previous.content_revision,
    ) {
        dirty.insert(DirtyPhases::INTERACTION);
    }
    if !phase_matches(
        current.semantics,
        previous.semantics,
        exact.semantics,
        current.semantics_content_sensitive,
        previous.semantics_content_sensitive,
        current.content_revision,
        previous.content_revision,
    ) {
        dirty.insert(DirtyPhases::SEMANTICS);
    }
    dirty
}

impl RetainedNodeArena {
    pub(crate) fn child_ui_id(&self, parent: &UiId, index: usize) -> Option<UiId> {
        let parent = self.by_ui_id.get(parent)?;
        let child = self.nodes.get(parent)?.children.get(index)?;
        self.nodes.get(child).map(|node| node.ui_id.clone())
    }

    pub(crate) fn phase_is_clean(&self, id: &UiId, phase: DirtyPhases) -> bool {
        self.by_ui_id
            .get(id)
            .and_then(|node| self.nodes.get(node))
            .is_some_and(|node| !node.dirty.contains(phase))
    }

    pub(crate) fn subtree_phase_is_clean(&self, id: &UiId, phase: DirtyPhases) -> bool {
        let Some(node) = self.by_ui_id.get(id).and_then(|node| self.nodes.get(node)) else {
            return false;
        };
        !node.subtree_dirty.contains(phase)
    }

    pub(crate) fn subtree_is_clean(&self, id: &UiId) -> bool {
        self.by_ui_id
            .get(id)
            .and_then(|node| self.nodes.get(node))
            .is_some_and(|node| node.subtree_dirty.is_empty())
    }

    pub(crate) fn subtree_bulk_resolved_reuse_safe(&self, id: &UiId) -> bool {
        self.by_ui_id
            .get(id)
            .and_then(|node| self.nodes.get(node))
            .is_some_and(|node| node.subtree_bulk_resolved_reuse_safe)
    }

    pub(crate) fn subtree_independent_output_reuse_safe(&self, id: &UiId) -> bool {
        self.by_ui_id
            .get(id)
            .and_then(|node| self.nodes.get(node))
            .is_some_and(|node| node.subtree_independent_output_reuse_safe)
    }

    pub(crate) fn subtree_node_count(&self, id: &UiId) -> Option<usize> {
        self.by_ui_id
            .get(id)
            .and_then(|node| self.nodes.get(node))
            .map(|node| node.subtree_nodes)
    }

    pub(crate) fn is_descendant_or_self(&self, ancestor: &UiId, candidate: &UiId) -> bool {
        let (Some(&ancestor), Some(mut candidate)) = (
            self.by_ui_id.get(ancestor),
            self.by_ui_id.get(candidate).copied(),
        ) else {
            return false;
        };
        loop {
            if candidate == ancestor {
                return true;
            }
            let Some(parent) = self.nodes.get(&candidate).and_then(|node| node.parent) else {
                return false;
            };
            candidate = parent;
        }
    }

    pub(crate) fn reconcile_against<Message>(
        &mut self,
        root: &Element<Message>,
        previous: Option<&Element<Message>>,
    ) -> ReconcileStats {
        self.reconcile_impl(root, previous, None, false)
    }

    pub(crate) fn reconcile_changed_against<Message>(
        &mut self,
        root: &Element<Message>,
        previous: Option<&Element<Message>>,
        changed: &[UiId],
    ) -> ReconcileStats {
        let mut ancestry = HashSet::new();
        let mut changed_nodes = HashSet::new();
        for id in changed {
            let Some(mut node) = self.by_ui_id.get(id).copied() else {
                return self.reconcile_against(root, previous);
            };
            changed_nodes.insert(node);
            loop {
                ancestry.insert(node);
                let Some(parent) = self.nodes.get(&node).and_then(|node| node.parent) else {
                    break;
                };
                node = parent;
            }
        }
        let Some(root_id) = self.root else {
            return self.reconcile_against(root, previous);
        };
        let root_ui_id = root.id.as_ref().map_or_else(
            || UiId::from("root"),
            |id| UiId::from("root").scoped(id.as_str()),
        );
        if !self.changed_shape_matches(root, root_id, &root_ui_id, &ancestry) {
            return self.reconcile_against(root, previous);
        }
        self.last = ReconcileStats::default();
        self.pending_commit.clear();
        self.reconcile_changed_node(root, previous, root_id, &ancestry, &changed_nodes, false);
        self.last.clone()
    }

    #[cfg(test)]
    fn reconcile<Message>(&mut self, root: &Element<Message>) -> ReconcileStats {
        self.reconcile_impl(root, None, None, true)
    }

    #[cfg(test)]
    fn reconcile_changed<Message>(
        &mut self,
        root: &Element<Message>,
        changed: &[UiId],
    ) -> ReconcileStats {
        // Existing arena-focused tests exercise topology and work accounting
        // without retaining declaration fixtures. Production always supplies
        // the prior declaration through `reconcile_changed_against`.
        let mut ancestry = HashSet::new();
        let mut changed_nodes = HashSet::new();
        for id in changed {
            let Some(mut node) = self.by_ui_id.get(id).copied() else {
                return self.reconcile(root);
            };
            changed_nodes.insert(node);
            loop {
                ancestry.insert(node);
                let Some(parent) = self.nodes.get(&node).and_then(|node| node.parent) else {
                    break;
                };
                node = parent;
            }
        }
        let Some(root_id) = self.root else {
            return self.reconcile(root);
        };
        let root_ui_id = root.id.as_ref().map_or_else(
            || UiId::from("root"),
            |id| UiId::from("root").scoped(id.as_str()),
        );
        if !self.changed_shape_matches(root, root_id, &root_ui_id, &ancestry) {
            return self.reconcile(root);
        }
        self.last = ReconcileStats::default();
        self.pending_commit.clear();
        self.reconcile_changed_node(root, None, root_id, &ancestry, &changed_nodes, true);
        self.last.clone()
    }

    fn changed_shape_matches<Message>(
        &self,
        element: &Element<Message>,
        id: RetainedNodeId,
        ui_id: &UiId,
        ancestry: &HashSet<RetainedNodeId>,
    ) -> bool {
        let Some(node) = self.nodes.get(&id) else {
            return false;
        };
        if node.kind != kind_tag(&element.kind)
            || node.ui_id != *ui_id
            || node.children.len() != element.children.len()
        {
            return false;
        }
        for (index, (child, child_id)) in element.children.iter().zip(&node.children).enumerate() {
            let identity = child
                .id
                .clone()
                .map_or(ChildIdentity::Position(index), ChildIdentity::Key);
            let child_ui_id = child.id.as_ref().map_or_else(
                || ui_id.scoped(format!("#{index}")),
                |id| ui_id.scoped(id.as_str()),
            );
            let Some(retained) = self.nodes.get(child_id) else {
                return false;
            };
            if retained.parent != Some(id)
                || retained.identity != identity
                || retained.ui_id != child_ui_id
                || retained.kind != kind_tag(&child.kind)
            {
                return false;
            }
            if ancestry.contains(child_id)
                && !self.changed_shape_matches(child, *child_id, &child_ui_id, ancestry)
            {
                return false;
            }
        }
        true
    }

    fn reconcile_changed_node<Message>(
        &mut self,
        element: &Element<Message>,
        previous_element: Option<&Element<Message>>,
        id: RetainedNodeId,
        ancestry: &HashSet<RetainedNodeId>,
        changed_nodes: &HashSet<RetainedNodeId>,
        trust_fingerprint_for_test: bool,
    ) {
        self.last.visited += 1;
        self.last.reused += 1;
        let (previous, children) = {
            let node = &self.nodes[&id];
            (node.signatures, node.children.clone())
        };
        let current = signatures(element);
        let exact = if trust_fingerprint_for_test || !changed_nodes.contains(&id) {
            ExactPhaseMatches::ALL
        } else {
            previous_element.map_or(
                ExactPhaseMatches {
                    measure: false,
                    place: false,
                    paint: false,
                    interaction: false,
                    semantics: false,
                },
                |previous| exact_phase_matches(element, previous),
            )
        };
        let mut dirty = changed_phases(current, previous, exact);
        for (index, (child, child_id)) in element.children.iter().zip(&children).enumerate() {
            if ancestry.contains(child_id) {
                let previous_child =
                    previous_element.and_then(|previous| previous.children.get(index));
                self.reconcile_changed_node(
                    child,
                    previous_child,
                    *child_id,
                    ancestry,
                    changed_nodes,
                    trust_fingerprint_for_test,
                );
            }
        }
        let mut descendants = DirtyPhases::default();
        for child in &children {
            descendants.insert(self.nodes[child].subtree_dirty);
        }
        if descendants.contains(DirtyPhases::MEASURE) {
            dirty.insert(
                DirtyPhases::MEASURE
                    .union(DirtyPhases::PLACE)
                    .union(DirtyPhases::PAINT)
                    .union(DirtyPhases::INTERACTION)
                    .union(DirtyPhases::SEMANTICS),
            );
        } else if descendants.contains(DirtyPhases::PLACE) {
            dirty.insert(
                DirtyPhases::PLACE
                    .union(DirtyPhases::PAINT)
                    .union(DirtyPhases::INTERACTION)
                    .union(DirtyPhases::SEMANTICS),
            );
        }
        let subtree_dirty = dirty.union(descendants);
        let children_layout_clean = children
            .iter()
            .all(|child| self.nodes[child].subtree_layout_clean);
        let subtree_independent_output_reuse_safe = node_independent_output_reuse_safe(element)
            && children
                .iter()
                .all(|child| self.nodes[child].subtree_independent_output_reuse_safe);
        let node = self.nodes.get_mut(&id).expect("validated retained node");
        node.signatures = current;
        node.dirty = dirty;
        node.subtree_dirty = subtree_dirty;
        node.subtree_layout_clean = children_layout_clean
            && !dirty.contains(DirtyPhases::MEASURE)
            && !dirty.contains(DirtyPhases::PLACE)
            && node.phases.measured.is_some()
            && node.phases.allocated.is_some();
        node.subtree_independent_output_reuse_safe = subtree_independent_output_reuse_safe;
        node.invalidate_dirty_phases();
        if subtree_dirty.is_empty() {
            self.last.clean += 1;
        } else {
            self.pending_commit.push(id);
        }
    }

    /// Mark the phases executed by a successfully published frame clean while
    /// retaining their cached products for the next sparse reconciliation.
    pub(crate) fn commit_frame(&mut self) {
        for id in std::mem::take(&mut self.pending_commit) {
            let children_layout_clean = self.nodes[&id]
                .children
                .iter()
                .all(|child| self.nodes[child].subtree_layout_clean);
            let node = self.nodes.get_mut(&id).expect("pending retained node");
            node.dirty = DirtyPhases::default();
            node.subtree_dirty = DirtyPhases::default();
            node.subtree_layout_clean = children_layout_clean
                && node.phases.measured.is_some()
                && node.phases.allocated.is_some();
        }
    }

    fn reconcile_impl<Message>(
        &mut self,
        root: &Element<Message>,
        previous_root: Option<&Element<Message>>,
        changed_ancestry: Option<&HashSet<RetainedNodeId>>,
        trust_fingerprint_for_test: bool,
    ) -> ReconcileStats {
        let mut old_nodes = std::mem::take(&mut self.nodes);
        let old_count = old_nodes.len();
        let old_root = self.root.take();
        self.last = ReconcileStats::default();
        self.pending_commit.clear();
        if changed_ancestry.is_none() {
            self.by_ui_id.clear();
        }
        let mut consumed = HashSet::new();
        let root_id = self.reconcile_node(
            root,
            previous_root,
            None,
            ChildIdentity::Root,
            root.id.as_ref().map_or_else(
                || UiId::from("root"),
                |id| UiId::from("root").scoped(id.as_str()),
            ),
            0,
            old_root,
            &mut old_nodes,
            &mut consumed,
            changed_ancestry,
            trust_fingerprint_for_test,
        );
        self.root = Some(root_id);
        self.last.removed = old_count.saturating_sub(consumed.len());
        self.last.clone()
    }

    #[allow(clippy::too_many_arguments)]
    fn reconcile_node<Message>(
        &mut self,
        element: &Element<Message>,
        previous_element: Option<&Element<Message>>,
        parent: Option<RetainedNodeId>,
        identity: ChildIdentity,
        ui_id: UiId,
        position: usize,
        candidate: Option<RetainedNodeId>,
        old_nodes: &mut HashMap<RetainedNodeId, RetainedNode>,
        consumed: &mut HashSet<RetainedNodeId>,
        changed_ancestry: Option<&HashSet<RetainedNodeId>>,
        trust_fingerprint_for_test: bool,
    ) -> RetainedNodeId {
        self.last.visited += 1;
        let kind = kind_tag(&element.kind);
        if let (Some(changed_ancestry), Some(candidate)) = (changed_ancestry, candidate)
            && !changed_ancestry.contains(&candidate)
            && old_nodes.get(&candidate).is_some_and(|node| {
                node.kind == kind
                    && node.identity == identity
                    && node.parent == parent
                    && node.ui_id == ui_id
            })
        {
            self.reuse_unchanged_subtree(candidate, old_nodes, consumed);
            return candidate;
        }
        let mut old = candidate.and_then(|id| old_nodes.remove(&id));
        let had_old = old.is_some();
        let mut reusable = old
            .as_mut()
            .filter(|node| node.kind == kind && node.identity == identity);
        let current = signatures(element);
        let (id, mut phases, mut metadata, mut dirty) = if let Some(old) = reusable.as_deref_mut() {
            consumed.insert(old.id);
            self.last.reused += 1;
            let exact = if trust_fingerprint_for_test {
                ExactPhaseMatches::ALL
            } else {
                previous_element.map_or(
                    ExactPhaseMatches {
                        measure: false,
                        place: false,
                        paint: false,
                        interaction: false,
                        semantics: false,
                    },
                    |previous| exact_phase_matches(element, previous),
                )
            };
            let dirty = changed_phases(current, old.signatures, exact);
            // The prior arena is already owned by this reconciliation. Move
            // retained resources instead of cloning buffers before invalidation.
            (
                old.id,
                std::mem::take(&mut old.phases),
                old.metadata
                    .take()
                    .expect("reusable retained node has metadata"),
                dirty,
            )
        } else {
            if had_old {
                self.last.replaced += 1;
            }
            self.next_id = self.next_id.max(1);
            let id = RetainedNodeId(self.next_id);
            self.next_id += 1;
            self.last.created += 1;
            (
                id,
                RetainedPhaseData::default(),
                Box::new(RetainedNodeMetadata {
                    identity: identity.clone(),
                    ui_id: ui_id.clone(),
                    signatures: current,
                }),
                DirtyPhases::ALL,
            )
        };

        let old_children = reusable
            .as_ref()
            .map_or(&[][..], |node| node.children.as_slice());
        // Stable child lists are the common case. Match their positional
        // prefix directly and pay for a keyed lookup table only after the
        // first insertion, removal, or move.
        let mut keyed = None;
        let mut children = Vec::with_capacity(element.children.len());
        for (index, child) in element.children.iter().enumerate() {
            let child_identity = child
                .id
                .clone()
                .map_or(ChildIdentity::Position(index), ChildIdentity::Key);
            let child_ui_id = child.id.as_ref().map_or_else(
                || ui_id.scoped(format!("#{index}")),
                |id| ui_id.scoped(id.as_str()),
            );
            let aligned = old_children.get(index).copied().filter(|candidate| {
                !consumed.contains(candidate)
                    && old_nodes
                        .get(candidate)
                        .is_some_and(|node| node.identity == child_identity)
            });
            let old_child = aligned.or_else(|| {
                let keyed = keyed.get_or_insert_with(|| {
                    old_children
                        .iter()
                        .filter_map(|child_id| {
                            old_nodes
                                .get(child_id)
                                .map(|node| (node.identity.clone(), *child_id))
                        })
                        .collect::<HashMap<_, _>>()
                });
                keyed
                    .get(&child_identity)
                    .copied()
                    .filter(|candidate| !consumed.contains(candidate))
            });
            if let Some(old_child) = old_child
                && old_children.get(index).copied() != Some(old_child)
            {
                self.last.moved += 1;
            }
            children.push(self.reconcile_node(
                child,
                previous_element.and_then(|previous| {
                    let aligned = previous.children.get(index).filter(|previous_child| {
                        previous_child.id == child.id
                            || (previous_child.id.is_none() && child.id.is_none())
                    });
                    aligned.or_else(|| {
                        child.id.as_ref().and_then(|id| {
                            previous
                                .children
                                .iter()
                                .find(|previous_child| previous_child.id.as_ref() == Some(id))
                        })
                    })
                }),
                Some(id),
                child_identity,
                child_ui_id,
                index,
                old_child,
                old_nodes,
                consumed,
                changed_ancestry,
                trust_fingerprint_for_test,
            ));
        }
        if reusable.is_some_and(|old| old.children != children) {
            dirty.insert(DirtyPhases::CHILDREN);
        }
        let mut descendants = DirtyPhases::default();
        for child in &children {
            descendants.insert(self.nodes[child].subtree_dirty);
        }
        if dirty.contains(DirtyPhases::CHILDREN) || descendants.contains(DirtyPhases::MEASURE) {
            dirty.insert(
                DirtyPhases::MEASURE
                    .union(DirtyPhases::PLACE)
                    .union(DirtyPhases::PAINT)
                    .union(DirtyPhases::INTERACTION)
                    .union(DirtyPhases::SEMANTICS),
            );
        } else if descendants.contains(DirtyPhases::PLACE) {
            dirty.insert(
                DirtyPhases::PLACE
                    .union(DirtyPhases::PAINT)
                    .union(DirtyPhases::INTERACTION)
                    .union(DirtyPhases::SEMANTICS),
            );
        }
        let subtree_dirty = dirty.union(descendants);
        if dirty.contains(DirtyPhases::MEASURE) {
            phases.scroll_measurements = None;
        }
        let subtree_nodes = 1 + children
            .iter()
            .map(|child| self.nodes[child].subtree_nodes)
            .sum::<usize>();
        let subtree_bulk_resolved_reuse_safe =
            !matches!(kind, KindTag::Grid | KindTag::Scroll | KindTag::Dropdown)
                && children
                    .iter()
                    .all(|child| self.nodes[child].subtree_bulk_resolved_reuse_safe);
        let subtree_independent_output_reuse_safe = node_independent_output_reuse_safe(element)
            && children
                .iter()
                .all(|child| self.nodes[child].subtree_independent_output_reuse_safe);
        let subtree_layout_clean = children
            .iter()
            .all(|child| self.nodes[child].subtree_layout_clean)
            && !dirty.contains(DirtyPhases::MEASURE)
            && !dirty.contains(DirtyPhases::PLACE)
            && phases.measured.is_some()
            && phases.allocated.is_some();
        self.by_ui_id.entry(ui_id.clone()).or_insert(id);
        metadata.identity = identity;
        metadata.ui_id = ui_id;
        metadata.signatures = current;
        let mut node = RetainedNode {
            id,
            parent,
            kind,
            dirty,
            children,
            subtree_dirty,
            subtree_nodes,
            subtree_bulk_resolved_reuse_safe,
            subtree_independent_output_reuse_safe,
            subtree_layout_clean,
            phases,
            metadata: Some(metadata),
        };
        node.invalidate_dirty_phases();
        if !node.subtree_dirty.is_empty() {
            self.pending_commit.push(id);
        }
        if node.dirty.is_empty() {
            self.last.clean += 1;
        }
        let _ = position;
        self.nodes.insert(id, node);
        id
    }

    fn reuse_unchanged_subtree(
        &mut self,
        id: RetainedNodeId,
        old_nodes: &mut HashMap<RetainedNodeId, RetainedNode>,
        consumed: &mut HashSet<RetainedNodeId>,
    ) {
        let mut node = old_nodes
            .remove(&id)
            .expect("unchanged retained subtree root exists");
        for child in node.children.clone() {
            self.reuse_unchanged_subtree(child, old_nodes, consumed);
        }
        consumed.insert(id);
        self.last.reused += 1;
        self.last.clean += 1;
        node.dirty = DirtyPhases::default();
        node.subtree_dirty = DirtyPhases::default();
        self.nodes.insert(id, node);
    }

    #[cfg(test)]
    pub(crate) fn nodes(&self) -> impl Iterator<Item = &RetainedNode> {
        self.nodes.values()
    }

    #[cfg(test)]
    pub(crate) fn retained_id(&self, id: &UiId) -> Option<RetainedNodeId> {
        self.by_ui_id.get(id).copied()
    }

    pub(crate) fn capture_layout(&mut self, id: &UiId, measured: Size, allocated: Rect) {
        if let Some(node) = self
            .by_ui_id
            .get(id)
            .and_then(|key| self.nodes.get_mut(key))
        {
            node.phases.measured = Some(measured);
            node.phases.measured_constraints = Some([
                allocated.size.width.to_bits(),
                allocated.size.height.to_bits(),
            ]);
            node.phases.allocated = Some(allocated);
        }
    }

    pub(crate) fn measured_for(&self, id: &UiId, constraints: Size) -> Option<Size> {
        let node = self.nodes.get(self.by_ui_id.get(id)?)?;
        (!node.dirty.contains(DirtyPhases::MEASURE)
            && node.phases.measured_constraints
                == Some([constraints.width.to_bits(), constraints.height.to_bits()]))
        .then_some(node.phases.measured)
        .flatten()
    }

    pub(crate) fn scroll_measurement_for(&self, id: &UiId, width: f32) -> Option<Size> {
        let node = self.nodes.get(self.by_ui_id.get(id)?)?;
        let constraints = [width.to_bits(), f32::INFINITY.to_bits()];
        node.phases
            .scroll_measurements
            .as_ref()?
            .iter()
            .flatten()
            .find_map(|(key, size)| (*key == constraints).then_some(*size))
    }

    pub(crate) fn cache_scroll_measurement(&mut self, id: &UiId, width: f32, size: Size) {
        let Some(key) = self.by_ui_id.get(id) else {
            return;
        };
        let node = self.nodes.get_mut(key).expect("indexed retained node");
        let entries = node
            .phases
            .scroll_measurements
            .get_or_insert_with(|| Box::new([None, None]));
        let constraints = [width.to_bits(), f32::INFINITY.to_bits()];
        let slot = entries
            .iter()
            .position(|entry| entry.is_none_or(|(key, _)| key == constraints))
            .unwrap_or_else(|| {
                entries.swap(0, 1);
                0
            });
        entries[slot] = Some((constraints, size));
    }

    /// Returns whether measurement and placement records for the complete
    /// declaration subtree remain valid. Callers may reuse geometry only when
    /// the incoming parent allocation also matches the recorded allocation.
    pub(crate) fn subtree_layout_is_clean(&self, id: &UiId) -> bool {
        self.by_ui_id
            .get(id)
            .and_then(|key| self.nodes.get(key))
            .is_some_and(|node| node.subtree_layout_clean)
    }

    #[cfg(test)]
    fn subtree_layout_is_clean_from(&self, id: RetainedNodeId) -> bool {
        let node = &self.nodes[&id];
        !node.dirty.contains(DirtyPhases::MEASURE)
            && !node.dirty.contains(DirtyPhases::PLACE)
            && node.phases.measured.is_some()
            && node.phases.allocated.is_some()
            && node
                .children
                .iter()
                .all(|child| self.subtree_layout_is_clean_from(*child))
    }

    pub(crate) fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub(crate) fn metadata_sidecars(&self) -> (usize, usize) {
        (
            self.nodes.len(),
            self.nodes
                .len()
                .saturating_mul(std::mem::size_of::<RetainedNodeMetadata>()),
        )
    }

    pub(crate) fn last(&self) -> ReconcileStats {
        self.last.clone()
    }

    pub(crate) fn estimated_bytes(&self) -> usize {
        self.nodes.capacity()
            * (std::mem::size_of::<RetainedNodeId>() + std::mem::size_of::<RetainedNode>())
            + self.nodes.len() * std::mem::size_of::<RetainedNodeMetadata>()
            + self
                .nodes
                .values()
                .map(|node| {
                    let _parent = node.parent();
                    node.children.capacity() * std::mem::size_of::<RetainedNodeId>()
                        + node
                            .phases
                            .scroll_measurements
                            .as_ref()
                            .map_or(0, |cache| std::mem::size_of_val(cache.as_ref()))
                })
                .sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PaintCommand;

    #[test]
    fn declarative_element_inline_size_does_not_regress() {
        let small = std::mem::size_of::<Element<usize>>();
        let large = std::mem::size_of::<Element<[u8; 64]>>();
        eprintln!(
            "element_inline_bytes small_message={small} large_message={large} styled_text_sidecar={}",
            std::mem::size_of::<super::super::StyledTextKind>()
        );
        assert_eq!(std::mem::size_of::<RetainedNode>(), 128);
        assert_eq!(std::mem::size_of::<RetainedPhaseData>(), 56);
        assert_eq!(std::mem::size_of::<RetainedNodeMetadata>(), 88);
        assert_eq!(std::mem::size_of::<Kind>(), 56);
        assert_eq!(small, 448);
        assert!(
            small <= 1_032,
            "small-message Element grew to {small} bytes"
        );
        assert!(
            large <= 1_032,
            "large-message Element grew to {large} bytes"
        );
    }

    #[test]
    fn declarative_event_sidecar_is_absent_until_needed_and_maps_messages() {
        let empty = Element::<u32>::flex(super::super::Axis::Vertical);
        assert!(empty.events.is_none());

        let mapped = empty
            .message(7)
            .context_message(9)
            .focus_message(11)
            .map_message(|message| message.to_string());
        assert_eq!(mapped.primary_message().map(String::as_str), Some("7"));
        assert_eq!(
            mapped.declared_context_message().map(String::as_str),
            Some("9")
        );
        assert_eq!(
            mapped.declared_focus_message().map(String::as_str),
            Some("11")
        );

        let mut cleared = Element::<u32>::flex(super::super::Axis::Vertical).message(1);
        cleared.clear_primary_message();
        assert!(cleared.events.is_none());
    }

    #[test]
    fn paint_signature_covers_style_inputs_consumed_by_emission() {
        let base = Element::<()>::flex(super::super::Axis::Vertical);
        let baseline = signatures(&base).paint;
        let assert_changes = |element: Element<()>| {
            assert_ne!(
                signatures(&element).paint,
                baseline,
                "paint-affecting style mutation must invalidate paint reuse"
            );
        };

        let mut element = base.clone();
        element.style.border_width = 4.0;
        assert_changes(element);

        let mut element = base.clone();
        element.style.backdrop_blur = Some(8.0);
        assert_changes(element);

        let mut element = base.clone();
        element.style.text_align = super::super::TextAlign::Center;
        assert_changes(element);

        let mut element = base.clone();
        element.style.css_paint = true;
        assert_changes(element);

        let mut element = base.clone();
        element.style.focus_background_tint = Some(0xff12_3456);
        assert_changes(element);

        let mut element = base.clone();
        element.style.editing_parts =
            Some(Box::new([super::super::DropdownPartStyle::default(); 2]));
        assert_changes(element);

        let mut element = base;
        element.style.editing_menu = Some(Box::default());
        assert_changes(element);
    }

    #[test]
    fn forced_phase_fingerprint_collisions_cannot_authorize_reuse() {
        let baseline = Element::<()>::flex(super::super::Axis::Vertical);
        let fingerprints = signatures(&baseline);
        let forced_dirty = |mutated: &Element<()>| {
            changed_phases(
                fingerprints,
                fingerprints,
                exact_phase_matches(mutated, &baseline),
            )
        };

        let mut measure = baseline.clone();
        measure.style.width = Length::Px(42.0);
        assert!(forced_dirty(&measure).contains(DirtyPhases::MEASURE));

        let mut place = baseline.clone();
        place.style.overflow_x = Overflow::Clip;
        assert!(forced_dirty(&place).contains(DirtyPhases::PLACE));

        let mut paint = baseline.clone();
        paint.style.background = Some(0xff12_3456.into());
        assert!(forced_dirty(&paint).contains(DirtyPhases::PAINT));

        let interaction = baseline.clone().message(());
        assert!(
            forced_dirty(&interaction).contains(DirtyPhases::INTERACTION),
            "exact handler metadata must defeat a colliding interaction fingerprint"
        );

        let mut semantics = baseline.clone();
        semantics.style.semantic_role = Some(super::super::SemanticRole::Group);
        assert!(forced_dirty(&semantics).contains(DirtyPhases::SEMANTICS));
    }
    use crate::{
        Column, Component, ComponentBuilderExt, CustomPaint, FrameRequest, Grid, Rect, Row,
        StyledText, StyledTextSpan, Text, TextUnderlineStyle, Track, UiFrame, UiStateStore,
    };
    #[cfg(not(debug_assertions))]
    use crate::{Container, SoftwareRenderer};
    use proptest::prelude::*;

    fn keyed_text(id: &str, value: &str) -> Element<()> {
        let mut revision = std::collections::hash_map::DefaultHasher::new();
        value.hash(&mut revision);
        Text::new(value)
            .id(id)
            .content_revision(revision.finish())
            .width(80.0)
            .height(20.0)
            .into_element()
    }

    fn ids(arena: &RetainedNodeArena) -> HashMap<UiId, RetainedNodeId> {
        arena
            .nodes()
            .filter_map(|node| match &node.identity {
                ChildIdentity::Key(key) => Some((key.clone(), node.id)),
                ChildIdentity::Root | ChildIdentity::Position(_) => None,
            })
            .collect()
    }

    fn dirty_for(arena: &RetainedNodeArena, id: &str) -> DirtyPhases {
        arena
            .nodes()
            .find(|node| node.ui_id.as_str().ends_with(id))
            .expect("retained node")
            .dirty()
    }

    #[test]
    fn equal_length_middle_text_mutation_without_revision_cannot_authorize_measurement_reuse() {
        let prefix = "p".repeat(96);
        let suffix = "s".repeat(96);
        let before = format!("{prefix}iiiiiiii{suffix}");
        let after = format!("{prefix}ＷＷＷＷＷＷＷＷ{suffix}");
        assert_eq!(before.chars().count(), after.chars().count());

        let view = |value: &str| {
            Column::<()>::new()
                .child(Text::<()>::new(value).id("value"))
                .into_element()
        };
        let bounds = Rect::new(0.0, 0.0, 800.0, 200.0);
        let mut retained_state = UiStateStore::default();
        let first = UiFrame::resolve(
            view(&before),
            FrameRequest::new(bounds, &mut retained_state),
        );
        let next = UiFrame::resolve_against(
            view(&after),
            FrameRequest::new(bounds, &mut retained_state),
            &first,
        );
        assert!(next.resource_diagnostics().nodes_measured > 0);

        let mut cold_state = UiStateStore::default();
        let cold = UiFrame::resolve(view(&after), FrameRequest::new(bounds, &mut cold_state));
        assert_eq!(next.resolved_layout(), cold.resolved_layout());
        assert_eq!(next.commands(), cold.commands());
        assert_eq!(next.semantic_nodes(), cold.semantic_nodes());
    }

    #[test]
    fn moved_keyed_subtrees_translate_paint_and_interaction_records() {
        fn view(order: [&str; 2]) -> Element<&'static str> {
            Column::new()
                .children(order.map(|id| {
                    Text::new(id)
                        .id(id)
                        .content_revision(if id == "a" { 1 } else { 2 })
                        .width(80.0)
                        .height(20.0)
                        .message(if id == "a" { "a" } else { "b" })
                }))
                .into_element()
        }

        let bounds = Rect::new(0.0, 0.0, 80.0, 40.0);
        let mut retained_state = UiStateStore::default();
        let first = UiFrame::resolve(
            view(["a", "b"]),
            FrameRequest::new(bounds, &mut retained_state),
        );
        let moved = UiFrame::resolve_against(
            view(["b", "a"]),
            FrameRequest::new(bounds, &mut retained_state),
            &first,
        );
        let work = moved.resource_diagnostics();
        assert_eq!(work.nodes_measured, 1);
        assert_eq!(work.nodes_placed, 1);
        assert_eq!(work.paint_nodes_executed, 1);
        assert_eq!(work.paint_nodes_reused, 2);
        assert_eq!(work.interaction_nodes_executed, 1);
        assert_eq!(work.interaction_nodes_reused, 2);
        assert_eq!(
            moved.message_at(crate::Point { x: 10.0, y: 10.0 }),
            Some(&"b")
        );
        assert_eq!(
            moved.message_at(crate::Point { x: 10.0, y: 30.0 }),
            Some(&"a")
        );

        let mut cold_state = UiStateStore::default();
        let cold = UiFrame::resolve(view(["b", "a"]), FrameRequest::new(bounds, &mut cold_state));
        assert_frame_oracle(&moved, &cold);
    }

    #[test]
    fn styled_payload_revision_invalidates_equal_cardinality_span_changes() {
        let span = |bold| StyledTextSpan {
            range: 0..8,
            bold,
            italic: false,
            monospace: false,
            font_family: None,
            strikethrough: false,
            underline: TextUnderlineStyle::None,
            color: None,
            background: None,
        };
        let view = |bold, revision| {
            StyledText::<()>::new("abcdefgh", vec![span(bold)])
                .id("styled")
                .content_revision(revision)
        };
        let mut arena = RetainedNodeArena::default();
        arena.reconcile(&view(false, 1));
        arena.reconcile(&view(true, 2));
        let dirty = dirty_for(&arena, "styled");
        assert!(dirty.contains(DirtyPhases::MEASURE));
        assert!(dirty.contains(DirtyPhases::PAINT));
    }

    #[test]
    fn custom_paint_revision_detects_equal_length_middle_command_mutation() {
        let commands = |middle| {
            vec![
                PaintCommand::Fill {
                    rect: Rect::new(0.0, 0.0, 2.0, 2.0),
                    color: 1,
                },
                PaintCommand::Fill {
                    rect: Rect::new(2.0, 0.0, 2.0, 2.0),
                    color: middle,
                },
                PaintCommand::Fill {
                    rect: Rect::new(4.0, 0.0, 2.0, 2.0),
                    color: 3,
                },
            ]
        };
        let view = |middle, revision| {
            CustomPaint::<()>::commands(commands(middle))
                .id("paint")
                .content_revision(revision)
        };
        let mut arena = RetainedNodeArena::default();
        arena.reconcile(&view(2, 10));
        arena.reconcile(&view(9, 11));
        let dirty = dirty_for(&arena, "paint");
        assert!(dirty.contains(DirtyPhases::PAINT));
        assert!(!dirty.contains(DirtyPhases::MEASURE));
    }

    #[test]
    fn unchanged_declaration_reuses_every_node_and_phase() {
        let view = Column::new()
            .child(keyed_text("a", "A"))
            .child(keyed_text("b", "B"))
            .into_element();
        let mut arena = RetainedNodeArena::default();
        assert_eq!(arena.reconcile(&view).created, 3);
        let before = ids(&arena);
        let stats = arena.reconcile(&view);
        assert_eq!(stats.reused, 3);
        assert_eq!(stats.clean, 3);
        assert_eq!(ids(&arena), before);
        assert!(arena.nodes().all(|node| node.dirty().is_empty()));
    }

    #[test]
    fn authoritative_leaf_changes_reconcile_only_their_ancestry() {
        let view = |changed: bool| {
            Column::<()>::new()
                .children((0..200).map(|index| {
                    Text::new(if changed && index == 137 { "B" } else { "A" })
                        .id(format!("cell-{index}"))
                        .content_revision(u64::from(changed && index == 137))
                }))
                .into_element()
        };
        let before = view(false);
        let after = view(true);
        let mut sparse = RetainedNodeArena::default();
        sparse.reconcile(&before);
        sparse.commit_frame();
        let mut full = sparse.clone();

        let target = UiId::from("root").scoped("cell-137");
        let sparse_stats = sparse.reconcile_changed(&after, &[target]);
        let full_stats = full.reconcile(&after);

        assert_eq!(sparse_stats.visited, 2);
        assert_eq!(full_stats.visited, 201);
        for (id, sparse_node) in &sparse.nodes {
            let full_node = &full.nodes[id];
            assert_eq!(sparse_node.signatures, full_node.signatures);
            assert_eq!(sparse_node.dirty, full_node.dirty);
            assert_eq!(sparse_node.subtree_dirty, full_node.subtree_dirty);
        }
    }

    #[test]
    fn reconciliation_moves_retained_scroll_cache_instead_of_cloning_it() {
        let mut arena = RetainedNodeArena::default();
        let view = keyed_text("leaf", "same");
        arena.reconcile(&view);
        let bounds = Rect::new(0.0, 0.0, 80.0, 20.0);
        let leaf = arena.nodes.values_mut().next().unwrap();
        leaf.phases.measured = Some(bounds.size);
        leaf.phases.allocated = Some(bounds);
        leaf.phases.scroll_measurements = Some(Box::new([
            Some(([80.0f32.to_bits(), f32::INFINITY.to_bits()], bounds.size)),
            None,
        ]));
        let metadata_storage = leaf.metadata.as_deref().unwrap() as *const RetainedNodeMetadata;
        let measurement_storage = leaf.phases.scroll_measurements.as_deref().unwrap() as *const _;
        let stats = arena.reconcile(&view);
        assert_eq!(stats.reused, 1);
        assert_eq!(stats.removed, 0);
        let leaf = arena.nodes().next().unwrap();
        assert_eq!(
            leaf.metadata.as_deref().unwrap() as *const RetainedNodeMetadata,
            metadata_storage
        );
        assert_eq!(
            leaf.phases.scroll_measurements.as_deref().unwrap() as *const _,
            measurement_storage
        );
    }

    #[test]
    fn paint_only_change_does_not_dirty_layout_or_semantics() {
        let mut arena = RetainedNodeArena::default();
        arena.reconcile(
            &Text::<()>::new("same")
                .id("leaf")
                .content_revision(1)
                .foreground(0xff01_0203)
                .into_element(),
        );
        arena.reconcile(
            &Text::<()>::new("same")
                .id("leaf")
                .content_revision(1)
                .foreground(0xff03_0201)
                .into_element(),
        );
        let leaf = arena
            .nodes()
            .find(|node| matches!(node.identity, ChildIdentity::Root))
            .unwrap();
        assert!(leaf.dirty().contains(DirtyPhases::PAINT));
        assert!(!leaf.dirty().contains(DirtyPhases::MEASURE));
        assert!(!leaf.dirty().contains(DirtyPhases::PLACE));
        assert!(!leaf.dirty().contains(DirtyPhases::SEMANTICS));
    }

    #[test]
    fn fixed_size_leaf_content_reuses_measurement_and_placement() {
        let mut arena = RetainedNodeArena::default();
        arena.reconcile(&keyed_text("leaf", "short"));
        let cached_size = Size::new(80.0, 20.0);
        let cached_bounds = Rect::new(4.0, 8.0, 80.0, 20.0);
        let leaf = arena.nodes.values_mut().next().unwrap();
        leaf.phases.measured = Some(cached_size);
        leaf.phases.allocated = Some(cached_bounds);
        arena.reconcile(&keyed_text("leaf", "a much longer label"));
        let leaf = arena.nodes().next().unwrap();
        assert!(!leaf.dirty().contains(DirtyPhases::MEASURE));
        assert!(!leaf.dirty().contains(DirtyPhases::PLACE));
        assert!(leaf.dirty().contains(DirtyPhases::PAINT));
        assert!(leaf.dirty().contains(DirtyPhases::SEMANTICS));
        assert_eq!(leaf.phases.measured, Some(cached_size));
        assert_eq!(leaf.phases.allocated, Some(cached_bounds));
    }

    #[test]
    fn indexed_layout_records_follow_keys_and_retire_removed_nodes() {
        let mut arena = RetainedNodeArena::default();
        let view = |keys: &[&str]| {
            Row::new()
                .children(keys.iter().map(|key| keyed_text(key, key)))
                .into_element()
        };
        let first = view(&["a", "b", "c"]);
        arena.reconcile(&first);
        let bounds = Rect::new(0.0, 0.0, 80.0, 20.0);
        for id in arena.by_ui_id.keys().cloned().collect::<Vec<_>>() {
            arena.capture_layout(&id, bounds.size, bounds);
        }
        arena.reconcile(&first);
        let removed = arena
            .nodes()
            .find(|node| node.ui_id.as_str().ends_with("/b"))
            .unwrap()
            .ui_id
            .clone();
        assert_eq!(arena.measured_for(&removed, bounds.size), Some(bounds.size));
        assert_eq!(arena.measured_for(&removed, Size::new(81.0, 20.0)), None);
        for keys in [&["c", "a", "x"][..], &["a"][..], &[][..]] {
            arena.reconcile(&view(keys));
            assert_eq!(arena.by_ui_id.len(), arena.nodes.len());
            assert_eq!(arena.measured_for(&removed, bounds.size), None);
            assert!(!arena.subtree_layout_is_clean(&removed));
            arena.capture_layout(&removed, bounds.size, bounds);
            assert!(!arena.by_ui_id.contains_key(&removed));
            for node in arena.nodes() {
                assert_eq!(
                    arena.subtree_layout_is_clean(&node.ui_id),
                    arena.subtree_layout_is_clean_from(node.id)
                );
            }
        }
    }

    #[test]
    fn cached_layout_cleanliness_matches_recursive_oracle_at_scale() {
        for count in [100, 1_000, 10_000] {
            let mut arena = RetainedNodeArena::default();
            let view = Column::new()
                .children((0..count).map(|index| keyed_text(&format!("item-{index}"), "item")))
                .into_element();
            arena.reconcile(&view);
            let bounds = Rect::new(0.0, 0.0, 80.0, 20.0);
            for id in arena.by_ui_id.keys().cloned().collect::<Vec<_>>() {
                arena.capture_layout(&id, bounds.size, bounds);
            }
            arena.reconcile(&view);
            for node in arena.nodes() {
                assert_eq!(
                    arena.measured_for(&node.ui_id, bounds.size),
                    Some(bounds.size)
                );
                assert!(arena.subtree_layout_is_clean(&node.ui_id));
                assert_eq!(
                    node.subtree_layout_clean,
                    arena.subtree_layout_is_clean_from(node.id)
                );
            }
            // A missing descendant record must invalidate the cached ancestor result.
            let leaf = *arena
                .by_ui_id
                .values()
                .find(|id| arena.nodes[id].children.is_empty())
                .unwrap();
            arena.nodes.get_mut(&leaf).unwrap().phases.allocated = None;
            arena.reconcile(&view);
            for node in arena.nodes() {
                assert_eq!(
                    node.subtree_layout_clean,
                    arena.subtree_layout_is_clean_from(node.id)
                );
            }
            assert!(!arena.nodes[&arena.root.unwrap()].subtree_layout_clean);
        }
    }

    #[test]
    fn keyed_insertion_and_reorder_preserve_identity_and_cleanup_removed_nodes() {
        let mut arena = RetainedNodeArena::default();
        let first = Row::new()
            .children([
                keyed_text("a", "A"),
                keyed_text("b", "B"),
                keyed_text("c", "C"),
            ])
            .into_element();
        arena.reconcile(&first);
        let original = ids(&arena);
        let reordered = Row::new()
            .children([
                keyed_text("x", "X"),
                keyed_text("c", "C"),
                keyed_text("a", "A"),
            ])
            .into_element();
        let stats = arena.reconcile(&reordered);
        let current = ids(&arena);
        assert_eq!(current[&UiId::from("a")], original[&UiId::from("a")]);
        assert_eq!(current[&UiId::from("c")], original[&UiId::from("c")]);
        assert!(!current.contains_key(&UiId::from("b")));
        assert_eq!(stats.created, 1);
        assert_eq!(stats.removed, 1);
        assert!(stats.moved >= 2);
        assert_eq!(arena.nodes().count(), 4);
    }

    #[test]
    fn positional_nodes_reuse_only_their_documented_slot() {
        let mut arena = RetainedNodeArena::default();
        let first = Column::new()
            .child(Text::<()>::new("A"))
            .child(Text::<()>::new("B"))
            .into_element();
        arena.reconcile(&first);
        let before = arena.nodes().map(RetainedNode::id).collect::<HashSet<_>>();
        let second = Column::new()
            .child(Text::<()>::new("B"))
            .child(Text::<()>::new("A"))
            .into_element();
        arena.reconcile(&second);
        assert_eq!(
            arena.nodes().map(RetainedNode::id).collect::<HashSet<_>>(),
            before
        );
    }

    #[test]
    fn semantic_and_paint_only_change_executes_no_measurement_or_placement() {
        let bounds = Rect::new(0.0, 0.0, 320.0, 120.0);
        let make_view = |label: &str, color| {
            Column::new()
                .width(320.0)
                .height(120.0)
                .child(
                    Text::<()>::new(label)
                        .id("label")
                        .width(120.0)
                        .height(24.0)
                        .foreground(color),
                )
                .into_element()
        };
        let mut retained_state = UiStateStore::default();
        let first = UiFrame::resolve(
            make_view("before", 0xff11_2233),
            FrameRequest::new(bounds, &mut retained_state),
        );
        assert_eq!(first.resource_diagnostics().nodes_measured, 2);
        assert_eq!(first.resource_diagnostics().nodes_placed, 2);

        let next = UiFrame::resolve_against(
            make_view("after", 0xff33_2211),
            FrameRequest::new(bounds, &mut retained_state),
            &first,
        );
        let work = next.resource_diagnostics();
        assert_eq!(work.nodes_measured, 0);
        assert_eq!(work.nodes_placed, 0);

        let mut cold_state = UiStateStore::default();
        let cold = UiFrame::resolve(
            make_view("after", 0xff33_2211),
            FrameRequest::new(bounds, &mut cold_state),
        );
        assert_eq!(next.resolved_layout(), cold.resolved_layout());
        assert_eq!(next.commands(), cold.commands());
        assert_eq!(next.semantic_nodes(), cold.semantic_nodes());
    }

    #[test]
    fn border_width_change_invalidates_paint_and_matches_cold_frame() {
        let bounds = Rect::new(0.0, 0.0, 160.0, 80.0);
        let view = |border_width| {
            Column::<()>::new()
                .id("panel")
                .width(120.0)
                .height(48.0)
                .into_element()
                .background(0xff22_3344)
                .border(0xffaa_bbcc, border_width)
        };
        let mut state = UiStateStore::default();
        let first = UiFrame::resolve(view(1.0), FrameRequest::new(bounds, &mut state));
        let next =
            UiFrame::resolve_against(view(6.0), FrameRequest::new(bounds, &mut state), &first);
        assert!(next.resource_diagnostics().paint_nodes_executed > 0);

        let mut cold_state = UiStateStore::default();
        let cold = UiFrame::resolve(view(6.0), FrameRequest::new(bounds, &mut cold_state));
        assert_frame_oracle(&next, &cold);
        assert_ne!(first.commands(), next.commands());
    }

    #[test]
    fn backdrop_blur_change_invalidates_paint_and_matches_cold_frame() {
        let bounds = Rect::new(0.0, 0.0, 160.0, 80.0);
        let view = |blur: Option<f32>| {
            let panel = Column::<()>::new()
                .id("panel")
                .width(120.0)
                .height(48.0)
                .into_element()
                .background(0xff22_3344);
            match blur {
                Some(radius) => panel.backdrop_blur(radius),
                None => panel,
            }
        };
        let mut state = UiStateStore::default();
        let first = UiFrame::resolve(view(None), FrameRequest::new(bounds, &mut state));
        let next = UiFrame::resolve_against(
            view(Some(8.0)),
            FrameRequest::new(bounds, &mut state),
            &first,
        );
        assert!(next.resource_diagnostics().paint_nodes_executed > 0);

        let mut cold_state = UiStateStore::default();
        let cold = UiFrame::resolve(view(Some(8.0)), FrameRequest::new(bounds, &mut cold_state));
        assert_frame_oracle(&next, &cold);
        assert_ne!(first.commands(), next.commands());
    }

    #[test]
    fn authoritative_change_bulk_reuses_clean_resolved_sibling_subtree() {
        let bounds = Rect::new(0.0, 0.0, 400.0, 200.0);
        let make_view = |changed: &str| {
            Column::new()
                .width(400.0)
                .height(200.0)
                .child(Text::<()>::new(changed).id("changed").height(24.0))
                .child(Column::new().id("stable").children([
                    keyed_text("one", "One"),
                    keyed_text("two", "Two"),
                    keyed_text("three", "Three"),
                ]))
                .into_element()
        };
        let mut state = UiStateStore::default();
        let mut previous =
            UiFrame::resolve(make_view("before"), FrameRequest::new(bounds, &mut state));
        let next = UiFrame::resolve_against_reusing_changed(
            make_view("after"),
            FrameRequest::new(bounds, &mut state),
            &mut previous,
            &[UiId::from("root/changed")],
        );
        assert_eq!(next.resource_diagnostics().resolved_nodes_reused, 4);

        let mut cold_state = UiStateStore::default();
        let cold = UiFrame::resolve(
            make_view("after"),
            FrameRequest::new(bounds, &mut cold_state),
        );
        assert_frame_oracle(&next, &cold);
    }

    #[test]
    fn unchanged_grid_reuses_geometry_tracks_and_emission() {
        let bounds = Rect::new(0.0, 0.0, 520.0, 160.0);
        let view = || {
            Grid::auto_fit(Track::minmax(Track::px(120.0), Track::fr(1.0)))
                .id("grid")
                .width(520.0)
                .height(160.0)
                .gap(8.0)
                .children((0..6).map(|index| {
                    Text::<()>::new(format!("Item {index}"))
                        .id(format!("item-{index}"))
                        .content_revision(index)
                }))
                .into_element()
        };
        let mut state = UiStateStore::default();
        let first = UiFrame::resolve(view(), FrameRequest::new(bounds, &mut state));
        let next = UiFrame::resolve_against(view(), FrameRequest::new(bounds, &mut state), &first);
        let work = next.resource_diagnostics();
        assert_eq!(work.nodes_measured, 0);
        assert_eq!(work.nodes_placed, 0);
        assert_eq!(work.paint_nodes_executed, 0);
        assert_eq!(work.interaction_nodes_executed, 0);
        assert_eq!(next.resolved_grids(), first.resolved_grids());
        assert_eq!(next.resolved_layout(), first.resolved_layout());
        assert_eq!(next.commands(), first.commands());
        assert_eq!(next.semantic_nodes(), first.semantic_nodes());
        assert_eq!(
            next.interaction_record_counts(),
            first.interaction_record_counts()
        );
    }

    #[test]
    fn changed_constraints_reject_geometry_and_measurement_reuse() {
        let view = || {
            Column::new()
                .child(keyed_text("a", "A"))
                .child(keyed_text("b", "B"))
                .into_element()
        };
        let mut state = UiStateStore::default();
        let first = UiFrame::resolve(
            view(),
            FrameRequest::new(Rect::new(0.0, 0.0, 240.0, 120.0), &mut state),
        );
        let next = UiFrame::resolve_against(
            view(),
            FrameRequest::new(Rect::new(0.0, 0.0, 280.0, 120.0), &mut state),
            &first,
        );
        let work = next.resource_diagnostics();
        assert!(work.nodes_measured > 0);
        assert!(work.nodes_placed > 0);
    }

    fn assert_frame_oracle<Message: Clone + PartialEq + std::fmt::Debug>(
        incremental: &UiFrame<Message>,
        cold: &UiFrame<Message>,
    ) {
        assert_eq!(incremental.resolved_layout(), cold.resolved_layout());
        assert_eq!(incremental.commands(), cold.commands());
        assert_eq!(incremental.semantic_nodes(), cold.semantic_nodes());
        assert_eq!(
            incremental.accessibility_nodes(),
            cold.accessibility_nodes()
        );
        assert_eq!(
            incremental.interaction_record_counts(),
            cold.interaction_record_counts()
        );
    }

    #[test]
    fn output_phases_reuse_independently_from_proven_clean_authority() {
        let bounds = Rect::new(0.0, 0.0, 240.0, 80.0);
        let paint_view = |color| {
            Column::new()
                .child(keyed_text("label", "stable").foreground(color))
                .child(keyed_text("sibling", "unchanged"))
                .into_element()
        };
        let mut state = UiStateStore::default();
        let first = UiFrame::resolve(
            paint_view(0xff11_2233),
            FrameRequest::new(bounds, &mut state),
        );
        let painted = UiFrame::resolve_against(
            paint_view(0xff33_2211),
            FrameRequest::new(bounds, &mut state),
            &first,
        );
        let work = painted.resource_diagnostics();
        assert_eq!(work.paint_nodes_executed, 2);
        assert_eq!(work.paint_nodes_reused, 1);
        assert_eq!(work.interaction_nodes_executed, 0);
        assert_eq!(work.interaction_nodes_reused, 3);
        assert_eq!(work.semantic_nodes_executed, 0);
        assert_eq!(work.semantic_nodes_reused, 3);
        let mut cold_state = UiStateStore::default();
        let cold = UiFrame::resolve(
            paint_view(0xff33_2211),
            FrameRequest::new(bounds, &mut cold_state),
        );
        assert_frame_oracle(&painted, &cold);

        let interaction_view = |interactive| {
            let label = keyed_text("label", "stable");
            Column::new()
                .child(if interactive {
                    label.message(())
                } else {
                    label
                })
                .child(keyed_text("sibling", "unchanged"))
                .into_element()
        };
        let first = UiFrame::resolve(
            interaction_view(false),
            FrameRequest::new(bounds, &mut state),
        );
        let interactive = UiFrame::resolve_against(
            interaction_view(true),
            FrameRequest::new(bounds, &mut state),
            &first,
        );
        let work = interactive.resource_diagnostics();
        assert_eq!(work.paint_nodes_executed, 0);
        assert_eq!(work.paint_nodes_reused, 3);
        assert_eq!(work.interaction_nodes_executed, 2);
        assert_eq!(work.interaction_nodes_reused, 1);
        assert!(work.semantic_nodes_executed > 0);
        assert_eq!(work.semantic_nodes_reused, 2);
        let mut cold_state = UiStateStore::default();
        let cold = UiFrame::resolve(
            interaction_view(true),
            FrameRequest::new(bounds, &mut cold_state),
        );
        assert_frame_oracle(&interactive, &cold);

        let semantic_view = |role| {
            let view = Column::new()
                .child(keyed_text("label", "stable"))
                .child(keyed_text("sibling", "unchanged"));
            if role {
                view.accessibility_role("group").into_element()
            } else {
                view.into_element()
            }
        };
        let first = UiFrame::resolve(semantic_view(false), FrameRequest::new(bounds, &mut state));
        let semantic = UiFrame::resolve_against(
            semantic_view(true),
            FrameRequest::new(bounds, &mut state),
            &first,
        );
        let work = semantic.resource_diagnostics();
        assert_eq!(work.paint_nodes_executed, 0);
        assert_eq!(work.interaction_nodes_executed, 0);
        assert!(work.semantic_nodes_executed > 0);
        assert_eq!(work.paint_nodes_reused, 3);
        assert_eq!(work.interaction_nodes_reused, 3);
        assert_eq!(work.semantic_nodes_reused, 2);
        let mut cold_state = UiStateStore::default();
        let cold = UiFrame::resolve(
            semantic_view(true),
            FrameRequest::new(bounds, &mut cold_state),
        );
        assert_frame_oracle(&semantic, &cold);
    }

    proptest! {
        #[test]
        fn incremental_reconciliation_matches_a_cold_frame(
            operations in prop::collection::vec((0u8..4, 0u8..12), 1..48)
        ) {
            let bounds = Rect::new(0.0, 0.0, 420.0, 320.0);
            let mut order = vec![0u8, 1, 2, 3];
            let mut values = (0u8..12).map(|key| (key, key as u16)).collect::<HashMap<_, _>>();
            let make_view = |order: &[u8], values: &HashMap<u8, u16>| {
                Column::new()
                    .children(order.iter().map(|key| {
                        keyed_text(
                            &format!("item-{key}"),
                            &format!("value-{}", values[key]),
                        )
                    }))
                    .into_element()
            };

            let mut retained_state = UiStateStore::default();
            let mut retained = UiFrame::resolve(
                make_view(&order, &values),
                FrameRequest::new(bounds, &mut retained_state),
            );
            for (operation, key) in operations {
                let order_before = order.clone();
                match operation {
                    0 if !order.contains(&key) => order.insert((key as usize) % (order.len() + 1), key),
                    1 if order.len() > 1 => order.retain(|candidate| *candidate != key),
                    2 if order.contains(&key) => {
                        order.retain(|candidate| *candidate != key);
                        order.insert((key as usize * 7) % (order.len() + 1), key);
                    }
                    _ => *values.entry(key).or_default() += 1,
                }
                if order.is_empty() {
                    order.push(key);
                }

                let mut next_state = retained_state.clone();
                let next = UiFrame::resolve_against(
                    make_view(&order, &values),
                    FrameRequest::new(bounds, &mut next_state),
                    &retained,
                );
                if order == order_before {
                    let work = next.resource_diagnostics();
                    prop_assert_eq!(work.nodes_measured, 0);
                    prop_assert_eq!(work.nodes_placed, 0);
                    prop_assert_eq!(work.interaction_nodes_executed, 0);
                    prop_assert_eq!(work.interaction_nodes_reused, order.len() + 1);
                }
                let mut cold_state = retained_state.clone();
                let cold = UiFrame::resolve(
                    make_view(&order, &values),
                    FrameRequest::new(bounds, &mut cold_state),
                );
                prop_assert_eq!(next.resolved_layout(), cold.resolved_layout());
                prop_assert_eq!(next.commands(), cold.commands());
                prop_assert_eq!(next.semantic_nodes(), cold.semantic_nodes());
                prop_assert_eq!(next.interaction_record_counts(), cold.interaction_record_counts());
                prop_assert!(next.resource_diagnostics().estimated_retained_bytes <= 512 * 1024);
                retained = next;
                retained_state = next_state;
            }
        }
    }

    #[cfg(not(debug_assertions))]
    #[test]
    #[ignore = "release-profile retained layout admission benchmark"]
    fn retained_layout_release_admission_skips_work_and_beats_cold_resolution() {
        use crate::release_admission::AdmissionReport;

        const NODES: usize = 2_000;
        const CHANGED_LABEL: usize = NODES / 2;
        const ITERATIONS: usize = 2;
        const SAMPLES: usize = 5;

        fn view(changed_label: Option<char>) -> Element<()> {
            Column::new()
                .children((0..NODES).map(|index| {
                    let suffix = if index == CHANGED_LABEL {
                        changed_label.unwrap_or('a')
                    } else {
                        'a'
                    };
                    keyed_text(
                        &format!("item-{index}"),
                        &format!("retained admission item {index:04} value {suffix}"),
                    )
                }))
                .into_element()
        }

        let bounds = Rect::new(0.0, 0.0, 900.0, 800.0);
        let mut retained_state = UiStateStore::default();
        let mut retained =
            UiFrame::resolve(view(None), FrameRequest::new(bounds, &mut retained_state));
        let mut unchanged_samples = Vec::with_capacity(SAMPLES * ITERATIONS);
        let mut one_label_samples = Vec::with_capacity(SAMPLES * ITERATIONS);
        let mut cold_samples = Vec::with_capacity(SAMPLES);
        let mut unchanged_work = None;
        let mut label_work = None;
        for _ in 0..SAMPLES {
            for _ in 0..ITERATIONS {
                let started = std::time::Instant::now();
                let next = UiFrame::resolve_against(
                    view(None),
                    FrameRequest::new(bounds, &mut retained_state),
                    &retained,
                );
                let work = next.resource_diagnostics();
                assert_eq!(work.nodes_measured, 0);
                assert_eq!(work.nodes_placed, 0);
                assert_eq!(work.paint_nodes_executed, 0);
                assert_eq!(work.interaction_nodes_executed, 0);
                assert_eq!(work.semantic_nodes_executed, 0);
                unchanged_work.get_or_insert(work);
                assert_eq!(unchanged_work, Some(work));
                retained = next;
                unchanged_samples.push(started.elapsed());
            }

            for iteration in 0..ITERATIONS {
                let suffix = if iteration % 2 == 0 { 'b' } else { 'a' };
                let started = std::time::Instant::now();
                let next = UiFrame::resolve_against(
                    view(Some(suffix)),
                    FrameRequest::new(bounds, &mut retained_state),
                    &retained,
                );
                let work = next.resource_diagnostics();
                // The equal-width label mutation is paint-only: cached intrinsic
                // geometry remains authoritative while its fragment changes.
                assert_eq!(work.nodes_measured, 0);
                assert_eq!(work.nodes_placed, 0);
                assert_eq!(work.paint_nodes_executed, 2);
                assert_eq!(work.paint_nodes_reused, NODES - 1);
                assert_eq!(work.interaction_nodes_executed, 0);
                assert_eq!(work.interaction_nodes_reused, NODES + 1);
                label_work.get_or_insert(work);
                assert_eq!(label_work, Some(work));
                retained = next;
                one_label_samples.push(started.elapsed());
            }

            let mut state = UiStateStore::default();
            let started = std::time::Instant::now();
            let cold = UiFrame::resolve(view(None), FrameRequest::new(bounds, &mut state));
            cold_samples.push(started.elapsed());
            assert_eq!(cold.resource_diagnostics().nodes_measured, NODES + 1);
            std::hint::black_box(cold.commands());
        }

        let unchanged_work = unchanged_work.expect("unchanged work sample");
        let label_work = label_work.expect("label work sample");
        AdmissionReport::new("retained_layout", "2000_node_unchanged_and_one_label")
            .metadata("nodes", NODES)
            .metadata("iterations_per_sample", ITERATIONS)
            .metadata("samples", SAMPLES)
            .work("unchanged_nodes_measured", unchanged_work.nodes_measured)
            .work("unchanged_nodes_placed", unchanged_work.nodes_placed)
            .work(
                "unchanged_paint_nodes_executed",
                unchanged_work.paint_nodes_executed,
            )
            .work(
                "unchanged_interaction_nodes_executed",
                unchanged_work.interaction_nodes_executed,
            )
            .work(
                "unchanged_semantic_nodes_executed",
                unchanged_work.semantic_nodes_executed,
            )
            .work("one_label_nodes_measured", label_work.nodes_measured)
            .work("one_label_nodes_placed", label_work.nodes_placed)
            .work(
                "one_label_paint_nodes_executed",
                label_work.paint_nodes_executed,
            )
            .work(
                "one_label_interaction_nodes_executed",
                label_work.interaction_nodes_executed,
            )
            .work(
                "one_label_semantic_nodes_executed",
                label_work.semantic_nodes_executed,
            )
            .timings("unchanged", &unchanged_samples)
            .timings("one_label_same_size", &one_label_samples)
            .timings("cold", &cold_samples)
            .emit();

        let retained =
            crate::release_admission::DurationDistribution::from_samples(&unchanged_samples).p95;
        let cold = crate::release_admission::DurationDistribution::from_samples(&cold_samples).p95;
        assert!(
            retained.as_nanos().saturating_mul(5) <= cold.as_nanos().saturating_mul(4),
            "retained layout p95 must be at least 20% faster: retained={retained:?}, cold={cold:?}"
        );
    }

    #[cfg(not(debug_assertions))]
    #[derive(Clone, Copy)]
    struct FlexGridFixture {
        badge: char,
        expanded: bool,
    }

    #[cfg(not(debug_assertions))]
    fn nested_flex_grid_view(state: FlexGridFixture) -> impl Component<()> {
        const CARDS: usize = 24;
        const CHANGED_CARD: usize = CARDS / 2;
        Column::new().id("layout-root").padding(12.0).child(
            Grid::auto_fit(Track::minmax(Track::px(210.0), Track::fr(1.0)))
                .id("responsive-grid")
                .gap(10.0)
                .children((0..CARDS).map(|index| {
                    let badge = if index == CHANGED_CARD {
                        state.badge
                    } else {
                        'A'
                    };
                    let description = if index == CHANGED_CARD && state.expanded {
                        "A deliberately longer description that wraps across several lines when the responsive grid narrows."
                    } else {
                        "Short description."
                    };
                    Container::new()
                        .id(format!("card-{index}"))
                        .padding(8.0)
                        .child(
                            Column::new()
                                .id("stack")
                                .gap(6.0)
                                .child(
                                    Row::new()
                                        .id("heading")
                                        .gap(4.0)
                                        .child(
                                            Text::new(format!("Card {index:02}"))
                                                .id("title")
                                                .content_revision(index as u64),
                                        )
                                        .child(
                                            Text::new(badge.to_string())
                                                .id("badge")
                                                .width(28.0)
                                                .height(20.0)
                                                .content_revision(badge as u64),
                                        ),
                                )
                                .child(
                                    Grid::fixed(2)
                                        .id("facts")
                                        .gap(4.0)
                                        .child(
                                            Text::new("Status")
                                                .id("status-label")
                                                .content_revision(1),
                                        )
                                        .child(
                                            Text::new("Ready")
                                                .id("status-value")
                                                .content_revision(1),
                                        )
                                        .child(
                                            Text::new("Owner")
                                                .id("owner-label")
                                                .content_revision(1),
                                        )
                                        .child(
                                            Text::new(format!("Team {}", index % 4))
                                                .id("owner-value")
                                                .content_revision((index % 4) as u64),
                                        ),
                                )
                                .child(
                                    Text::new(description)
                                        .id("description")
                                        .wrap(true)
                                        .content_revision(
                                            (index == CHANGED_CARD && state.expanded) as u64,
                                        ),
                                ),
                        )
                })),
        )
    }

    #[cfg(not(debug_assertions))]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct FlexGridWork {
        retained: usize,
        reused: usize,
        created: usize,
        removed: usize,
        moved: usize,
        measured: usize,
        placed: usize,
        paint_executed: usize,
        paint_reused: usize,
        interaction_executed: usize,
        interaction_reused: usize,
        semantic_executed: usize,
        semantic_reused: usize,
    }

    #[cfg(not(debug_assertions))]
    fn flex_grid_work(frame: &UiFrame<()>) -> FlexGridWork {
        let work = frame.resource_diagnostics();
        FlexGridWork {
            retained: work.retained_node_count,
            reused: work.retained_nodes_reused,
            created: work.retained_nodes_created,
            removed: work.retained_nodes_removed,
            moved: work.retained_nodes_moved,
            measured: work.nodes_measured,
            placed: work.nodes_placed,
            paint_executed: work.paint_nodes_executed,
            paint_reused: work.paint_nodes_reused,
            interaction_executed: work.interaction_nodes_executed,
            interaction_reused: work.interaction_nodes_reused,
            semantic_executed: work.semantic_nodes_executed,
            semantic_reused: work.semantic_nodes_reused,
        }
    }

    #[cfg(not(debug_assertions))]
    fn flex_grid_bounds(width: f32) -> Rect {
        Rect::new(0.0, 0.0, width, 720.0)
    }

    #[cfg(not(debug_assertions))]
    fn assert_flex_grid_cold_equivalence(
        incremental: &UiFrame<()>,
        cold: &UiFrame<()>,
        scale: f32,
        width: f32,
        raster: bool,
    ) {
        assert_eq!(incremental.resolved_layout(), cold.resolved_layout());
        assert_eq!(incremental.commands().len(), cold.commands().len());
        assert_eq!(incremental.semantic_nodes(), cold.semantic_nodes());
        assert_eq!(
            incremental.accessibility_nodes(),
            cold.accessibility_nodes()
        );
        assert_eq!(
            incremental.interaction_record_counts(),
            cold.interaction_record_counts()
        );
        if !raster {
            return;
        }
        let mut incremental_renderer =
            SoftwareRenderer::new((width * scale).ceil() as u32, (720.0 * scale) as u32, scale);
        let mut cold_renderer =
            SoftwareRenderer::new((width * scale).ceil() as u32, (720.0 * scale) as u32, scale);
        incremental_renderer.render(incremental.commands());
        cold_renderer.render(cold.commands());
        assert_eq!(incremental_renderer.pixels(), cold_renderer.pixels());
    }

    #[cfg(not(debug_assertions))]
    fn measure_flex_grid_transition(
        scale: f32,
        from: FlexGridFixture,
        from_width: f32,
        to: FlexGridFixture,
        to_width: f32,
    ) -> (Vec<std::time::Duration>, FlexGridWork) {
        const SAMPLES: usize = 20;
        let mut samples = Vec::with_capacity(SAMPLES);
        let mut expected = None;
        for sample in 0..SAMPLES {
            let mut state = UiStateStore::default();
            let retained = UiFrame::resolve(
                nested_flex_grid_view(from),
                FrameRequest::new(flex_grid_bounds(from_width), &mut state),
            );
            let started = std::time::Instant::now();
            let next = UiFrame::resolve_against(
                nested_flex_grid_view(to),
                FrameRequest::new(flex_grid_bounds(to_width), &mut state),
                &retained,
            );
            samples.push(started.elapsed());
            expected.get_or_insert_with(|| flex_grid_work(&next));
            assert_eq!(expected, Some(flex_grid_work(&next)));

            let mut cold_state = UiStateStore::default();
            let cold = UiFrame::resolve(
                nested_flex_grid_view(to),
                FrameRequest::new(flex_grid_bounds(to_width), &mut cold_state),
            );
            assert_flex_grid_cold_equivalence(&next, &cold, scale, to_width, sample == 0);
        }
        (samples, expected.expect("transition work"))
    }

    #[cfg(not(debug_assertions))]
    #[test]
    #[ignore = "release-profile nested wrapping flex/grid scale admission benchmark"]
    fn nested_wrapping_flex_grid_release_admission() {
        use crate::release_admission::AdmissionReport;

        const WIDE: f32 = 960.0;
        const NARROW: f32 = 700.0;
        let baseline = FlexGridFixture {
            badge: 'A',
            expanded: false,
        };
        for scale in [1.0_f32, 1.25, 2.0] {
            let (unchanged_time, unchanged) =
                measure_flex_grid_transition(scale, baseline, WIDE, baseline, WIDE);
            let (same_size_time, same_size) = measure_flex_grid_transition(
                scale,
                baseline,
                WIDE,
                FlexGridFixture {
                    badge: 'B',
                    expanded: false,
                },
                WIDE,
            );
            let (size_change_time, size_change) = measure_flex_grid_transition(
                scale,
                baseline,
                WIDE,
                FlexGridFixture {
                    badge: 'A',
                    expanded: true,
                },
                WIDE,
            );
            let (resize_time, resize) =
                measure_flex_grid_transition(scale, baseline, WIDE, baseline, NARROW);

            assert_eq!(unchanged.retained, same_size.retained);
            assert_eq!(unchanged.retained, size_change.retained);
            assert_eq!(unchanged.retained, resize.retained);
            assert_eq!(
                unchanged,
                FlexGridWork {
                    retained: 266,
                    reused: 266,
                    created: 0,
                    removed: 0,
                    moved: 0,
                    measured: 0,
                    placed: 0,
                    paint_executed: 0,
                    paint_reused: 266,
                    interaction_executed: 0,
                    interaction_reused: 266,
                    semantic_executed: 0,
                    semantic_reused: 266,
                }
            );
            assert_eq!(
                same_size,
                FlexGridWork {
                    retained: 266,
                    reused: 266,
                    created: 0,
                    removed: 0,
                    moved: 0,
                    measured: 0,
                    placed: 0,
                    paint_executed: 6,
                    paint_reused: 260,
                    interaction_executed: 0,
                    interaction_reused: 266,
                    semantic_executed: 1,
                    semantic_reused: 265,
                }
            );
            assert_eq!(
                size_change,
                FlexGridWork {
                    retained: 266,
                    reused: 266,
                    created: 0,
                    removed: 0,
                    moved: 0,
                    measured: 8,
                    placed: 8,
                    paint_executed: 8,
                    paint_reused: 258,
                    interaction_executed: 8,
                    interaction_reused: 258,
                    semantic_executed: 96,
                    semantic_reused: 170,
                }
            );
            assert_eq!(
                resize,
                FlexGridWork {
                    retained: 266,
                    reused: 266,
                    created: 0,
                    removed: 0,
                    moved: 0,
                    measured: 218,
                    placed: 218,
                    paint_executed: 218,
                    paint_reused: 48,
                    interaction_executed: 218,
                    interaction_reused: 48,
                    semantic_executed: 264,
                    semantic_reused: 2,
                }
            );

            AdmissionReport::new("retained_layout", "nested_wrapping_flex_grid")
                .metadata("cards", 24)
                .metadata("samples", 20)
                .metadata("scale_milli", (scale * 1_000.0) as usize)
                .metadata("wide_width", WIDE as usize)
                .metadata("narrow_width", NARROW as usize)
                .work("unchanged_retained", unchanged.retained)
                .work("unchanged_reused", unchanged.reused)
                .work("unchanged_created", unchanged.created)
                .work("unchanged_removed", unchanged.removed)
                .work("unchanged_moved", unchanged.moved)
                .work("unchanged_measured", unchanged.measured)
                .work("unchanged_placed", unchanged.placed)
                .work("unchanged_paint_executed", unchanged.paint_executed)
                .work("unchanged_paint_reused", unchanged.paint_reused)
                .work(
                    "unchanged_interaction_executed",
                    unchanged.interaction_executed,
                )
                .work("unchanged_interaction_reused", unchanged.interaction_reused)
                .work("unchanged_semantic_executed", unchanged.semantic_executed)
                .work("unchanged_semantic_reused", unchanged.semantic_reused)
                .work("same_size_retained", same_size.retained)
                .work("same_size_reused", same_size.reused)
                .work("same_size_created", same_size.created)
                .work("same_size_removed", same_size.removed)
                .work("same_size_moved", same_size.moved)
                .work("same_size_measured", same_size.measured)
                .work("same_size_placed", same_size.placed)
                .work("same_size_paint_executed", same_size.paint_executed)
                .work("same_size_paint_reused", same_size.paint_reused)
                .work(
                    "same_size_interaction_executed",
                    same_size.interaction_executed,
                )
                .work("same_size_interaction_reused", same_size.interaction_reused)
                .work("same_size_semantic_executed", same_size.semantic_executed)
                .work("same_size_semantic_reused", same_size.semantic_reused)
                .work("size_change_retained", size_change.retained)
                .work("size_change_reused", size_change.reused)
                .work("size_change_created", size_change.created)
                .work("size_change_removed", size_change.removed)
                .work("size_change_moved", size_change.moved)
                .work("size_change_measured", size_change.measured)
                .work("size_change_placed", size_change.placed)
                .work("size_change_paint_executed", size_change.paint_executed)
                .work("size_change_paint_reused", size_change.paint_reused)
                .work(
                    "size_change_interaction_executed",
                    size_change.interaction_executed,
                )
                .work(
                    "size_change_interaction_reused",
                    size_change.interaction_reused,
                )
                .work(
                    "size_change_semantic_executed",
                    size_change.semantic_executed,
                )
                .work("size_change_semantic_reused", size_change.semantic_reused)
                .work("resize_retained", resize.retained)
                .work("resize_reused", resize.reused)
                .work("resize_created", resize.created)
                .work("resize_removed", resize.removed)
                .work("resize_moved", resize.moved)
                .work("resize_measured", resize.measured)
                .work("resize_placed", resize.placed)
                .work("resize_paint_executed", resize.paint_executed)
                .work("resize_paint_reused", resize.paint_reused)
                .work("resize_interaction_executed", resize.interaction_executed)
                .work("resize_interaction_reused", resize.interaction_reused)
                .work("resize_semantic_executed", resize.semantic_executed)
                .work("resize_semantic_reused", resize.semantic_reused)
                .timings("unchanged", &unchanged_time)
                .timings("same_size_leaf", &same_size_time)
                .timings("size_changing_leaf", &size_change_time)
                .timings("resize", &resize_time)
                .emit();
        }
    }
}

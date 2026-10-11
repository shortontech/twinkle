use super::*;

fn measure_scroll_content<Message: Clone>(
    child: &Element<Message>,
    id: &UiId,
    width: f32,
    tree: &mut UiFrame<Message>,
) -> Size {
    if let Some(size) = tree.retained_nodes.scroll_measurement_for(id, width) {
        tree.scroll_content_measurements_reused += 1;
        return size;
    }
    tree.scroll_content_measurements += 1;
    let size = measure_element(child, Constraints::loose(Size::new(width, f32::INFINITY)));
    tree.retained_nodes
        .cache_scroll_measurement(id, width, size);
    size
}

pub(super) fn layout_element<Message: Clone>(
    element: &Element<Message>,
    id: &UiId,
    bounds: Rect,
    inherited_foreground: Option<Color>,
    inherited_clip: Option<Rect>,
    overlay: Option<&TransientStateJournal>,
    tree: &mut UiFrame<Message>,
    previous: Option<&UiFrame<Message>>,
) -> usize {
    let style = overlay.map_or(&element.style, |overlay| overlay.style(id, element));
    let kind = overlay.map_or(&element.kind, |overlay| overlay.kind(id, element));
    let reusable = geometry_reuse_safe(element)
        .then(|| tree.reusable_geometry(id, bounds, inherited_clip))
        .flatten()
        .cloned();
    if reusable
        .as_ref()
        .is_some_and(|geometry| geometry.allocated == bounds)
        && !tree.diagnostics_enabled
        && tree.retained_nodes.subtree_bulk_resolved_reuse_safe(id)
        && tree.retained_nodes.subtree_is_clean(id)
        && let Some(previous) = previous
        && let Some(&old_start) = previous.resolved_indices.get(id)
        && let Some(count) = tree.retained_nodes.subtree_node_count(id)
        && count >= 4
        && let Some(old_nodes) = previous.resolved.nodes.get(old_start..old_start + count)
    {
        let new_start = tree.resolved.nodes.len();
        let delta = new_start as isize - old_start as isize;
        tree.resolved
            .nodes
            .extend(old_nodes.iter().cloned().map(|mut node| {
                if delta != 0 {
                    for child in &mut node.children {
                        *child = (*child as isize + delta) as usize;
                    }
                }
                node
            }));
        tree.resolved_nodes_reused = tree.resolved_nodes_reused.saturating_add(count);
        return new_start;
    }
    let rect = bounds;
    let preferred = reusable.as_ref().map_or_else(
        || {
            tree.retained_nodes
                .measured_for(id, bounds.size)
                .unwrap_or_else(|| {
                    tree.nodes_measured = tree.nodes_measured.saturating_add(1);
                    overlay.map_or_else(
                        || measure_element(element, Constraints::loose(bounds.size)),
                        |overlay| {
                            measure_element_with_overlay(
                                element,
                                id,
                                overlay,
                                Constraints::loose(bounds.size),
                            )
                        },
                    )
                })
        },
        |node| node.preferred,
    );
    if reusable.is_none() {
        tree.nodes_placed = tree.nodes_placed.saturating_add(1);
    }
    let node_index = tree.resolved.nodes.len();
    let interaction = InteractionState {
        interactive: (element.primary_message().is_some()
            || element.declared_context_message().is_some())
            && !is_scroll_container(element)
            || element.text_mapper.is_some(),
        ..InteractionState::default()
    };
    let derived_accessible_name = derive_accessible_name(element);
    let missing_accessible_name = style.semantic_role.is_some()
        && derived_accessible_name.as_deref().is_none_or(str::is_empty)
        && !style.semantic_decorative;
    let semantic_role = (!missing_accessible_name && !style.semantic_decorative)
        .then_some(style.semantic_role)
        .flatten();
    let semantic_hidden =
        style.accessibility_hidden || style.semantic_decorative || missing_accessible_name;
    let semantic_actions = {
        let mut actions = Vec::new();
        if !missing_accessible_name {
            if let Kind::Slider { value, .. } = kind
                && (element.message_mapper.is_some() || element.seeded_value_mapper.is_some())
            {
                if *value < 1.0 {
                    actions.push(ActionKind::Increment);
                }
                if *value > 0.0 {
                    actions.push(ActionKind::Decrement);
                }
                actions.push(ActionKind::SetValue);
            } else if element.text_mapper.is_some() {
                actions.push(ActionKind::SetValue);
            } else if element.primary_message().is_some() && !is_scroll_container(element) {
                actions.push(ActionKind::Activate);
            }
            if element.declared_context_message().is_some() {
                actions.push(ActionKind::ContextMenu);
            }
        }
        actions
    };
    let semantic_value = match kind {
        Kind::Slider { value, .. } => Some(SemanticValueSnapshot::Number {
            value: f64::from(*value),
            minimum: 0.0,
            maximum: 1.0,
            step: f64::from(element.adjustment_step),
        }),
        Kind::Text {
            details: Some(details),
            ..
        } if details.input_value.is_some() && details.input_mask.is_some() => {
            Some(SemanticValueSnapshot::ProtectedText {
                character_count: details.input_value.as_ref().unwrap().chars().count(),
            })
        }
        Kind::Text {
            details: Some(details),
            ..
        } if details.input_value.is_some() => Some(SemanticValueSnapshot::Text(
            details.input_value.as_ref().unwrap().clone(),
        )),
        Kind::Dropdown(dropdown) => Some(SemanticValueSnapshot::Text(dropdown.selected.clone())),
        _ => None,
    };
    tree.resolved.nodes.push(ResolvedNode {
        component: kind.name(),
        id: id.clone(),
        source: element.source,
        allocated: rect,
        padding_box: rect,
        border_box: rect,
        content: rect.inset(style.padding),
        constraints: Constraints::new(
            Size::new(style.min_width, style.min_height),
            Size::new(style.max_width, style.max_height),
        ),
        preferred,
        flex_basis: style.basis,
        flex_grow: style.grow,
        flex_shrink: style.shrink,
        clip: inherited_clip,
        interaction,
        children: Vec::new(),
        details: ResolvedNodeDetails::compact(ResolvedNodeDetails {
            auto_focus: style.auto_focus,
            navigation_scope: element.navigation_scope.as_deref().cloned(),
            virtual_navigation: element.virtual_navigation.clone(),
            adjustment_step: element.adjustment_step,
            controller_value: match kind {
                Kind::Slider { value, .. } => Some(*value),
                _ => None,
            },
            accessibility_label: derived_accessible_name,
            accessibility_description: style.accessibility_description.clone(),
            accessibility_role: style.accessibility_role.clone(),
            accessibility_state: style.accessibility_state.clone(),
            accessibility_controls: style.accessibility_controls.clone(),
            accessibility_hidden: semantic_hidden,
            semantic_role,
            semantic_actions,
            semantic_value,
            ..ResolvedNodeDetails::default()
        }),
    });
    if missing_accessible_name {
        tree.diagnostic(
            DiagnosticKind::MissingAccessibleName,
            id,
            "semantic role requires an accessible name or an explicit decorative exemption",
        );
    }
    if tree.diagnostics_enabled && !tree.seen_ids.insert(id.clone()) {
        tree.diagnostic(
            DiagnosticKind::DuplicateIdentity,
            id,
            "stable identity occurs more than once in this surface",
        );
    }
    if style.min_width > style.max_width || style.min_height > style.max_height {
        tree.diagnostic(
            DiagnosticKind::ContradictoryConstraints,
            id,
            format!(
                "min=({:.2},{:.2}) max=({:.2},{:.2})",
                style.min_width, style.min_height, style.max_width, style.max_height
            ),
        );
    }
    if !rect.origin.x.is_finite()
        || !rect.origin.y.is_finite()
        || !rect.size.width.is_finite()
        || !rect.size.height.is_finite()
        || rect.size.width < 0.0
        || rect.size.height < 0.0
    {
        tree.diagnostic(
            DiagnosticKind::InvalidGeometry,
            id,
            format!("allocated rectangle is {rect:?}"),
        );
    }
    if matches!(style.width, Length::Percent(_)) && !rect.size.width.is_finite()
        || matches!(style.height, Length::Percent(_)) && !rect.size.height.is_finite()
    {
        tree.diagnostic(
            DiagnosticKind::IndefinitePercentage,
            id,
            "percentage length resolved against indefinite parent space",
        );
    }
    if element.primary_message().is_some()
        && !is_scroll_container(element)
        && inherited_clip.is_some_and(|clip| {
            intersection(rect, clip).is_none_or(|visible| !approximately_same_rect(visible, rect))
        })
    {
        tree.diagnostic(
            DiagnosticKind::ClippedInteraction,
            id,
            format!("interactive rectangle {rect:?} is clipped by {inherited_clip:?}"),
        );
    }
    // Content-overflow and missing-asset checks are diagnostic observations,
    // not layout inputs. Avoid a second shaping pass for every text node on
    // ordinary production frames where diagnostics were not requested.
    if tree.diagnostics_enabled {
        let unconstrained_content = match kind {
            Kind::Text {
                value,
                scale,
                bold,
                wrap,
                line_height,
                max_lines,
                ..
            } => {
                tree.diagnostic_text_measurements =
                    tree.diagnostic_text_measurements.saturating_add(1);
                Some(measure_text(
                    value,
                    *scale,
                    *bold,
                    *wrap,
                    *line_height,
                    max_lines.map(std::num::NonZeroUsize::get),
                    if *wrap {
                        rect.size.width.max(0.0)
                    } else {
                        f32::INFINITY
                    },
                ))
            }
            // Images deliberately map intrinsic pixels through their presentation
            // policy, so a different allocated size is not unsatisfied content.
            Kind::Image { .. } => None,
            _ => None,
        };
        if unconstrained_content.is_some_and(|content| {
            content.width > rect.size.width + 0.01 || content.height > rect.size.height + 0.01
        }) {
            tree.diagnostic(
                DiagnosticKind::UnsatisfiedContent,
                id,
                format!(
                    "intrinsic {:?} exceeds allocated {:?}",
                    unconstrained_content.unwrap_or_default(),
                    rect.size
                ),
            );
        }
        if matches!(kind, Kind::Image { image, .. } if image.width() == 0 || image.height() == 0) {
            tree.diagnostic(
                DiagnosticKind::MissingAsset,
                id,
                "image has no pixels; using a bounded 16x16 fallback measurement",
            );
        }
    }
    let mut child_indices = Vec::new();
    let foreground = style.foreground.or(inherited_foreground);
    let clips_descendants =
        style.overflow_x != Overflow::Visible || style.overflow_y != Overflow::Visible;
    let descendant_clip = if clips_descendants {
        Some(inherited_clip.map_or(rect, |parent| {
            intersection(parent, rect).unwrap_or_else(|| {
                Rect::new(
                    rect.origin.x.max(parent.origin.x),
                    rect.origin.y.max(parent.origin.y),
                    0.0,
                    0.0,
                )
            })
        }))
    } else {
        inherited_clip
    };
    if let Some(previous_geometry) = &reusable {
        let translation = Point {
            x: bounds.origin.x - previous_geometry.allocated.origin.x,
            y: bounds.origin.y - previous_geometry.allocated.origin.y,
        };
        if matches!(kind, Kind::Grid { .. }) {
            tree.resolved.nodes[node_index]
                .grid_tracks
                .clone_from(&previous_geometry.grid_tracks);
            tree.grids.push(ResolvedGrid {
                rect: rect.inset(style.padding),
                columns: previous_geometry.grid_tracks.len().max(1),
            });
        }
        for (index, child) in element.children.iter().enumerate() {
            let child_id = retained_child_id(tree, id, child, index);
            let Some(allocated) = tree
                .retained_layout
                .get(&child_id)
                .map(|node| translate_rect(node.allocated, translation))
            else {
                // The arena's clean-subtree proof requires every child to have
                // a phase record, so an absent resolved node is corruption.
                debug_assert!(false, "clean retained child lacks resolved geometry");
                continue;
            };
            child_indices.push(layout_element(
                child,
                &child_id,
                allocated,
                foreground,
                descendant_clip,
                overlay,
                tree,
                previous,
            ));
        }
        tree.resolved.nodes[node_index].children = child_indices;
        return node_index;
    }
    match kind {
        Kind::Text { .. }
        | Kind::StyledText(_)
        | Kind::CustomPaint { .. }
        | Kind::CustomPaintCommands { .. }
        | Kind::Image { .. }
        | Kind::Slider { .. }
        | Kind::Dropdown(_) => {}
        Kind::Layer => {
            let content = rect.inset(style.padding);
            for (index, child) in element.children.iter().enumerate() {
                let child_context = overlay.map(|overlay| (id, overlay));
                let position = effective_child_style(child, child_context, index)
                    .absolute_position
                    .unwrap_or_default();
                let size = measure_child_effective(
                    child,
                    child_context,
                    index,
                    Constraints::loose(content.size),
                );
                let bounds = Rect::new(
                    content.origin.x + position.x,
                    content.origin.y + position.y,
                    size.width.min((content.size.width - position.x).max(0.0)),
                    size.height.min((content.size.height - position.y).max(0.0)),
                );
                let child_id = retained_child_id(tree, id, child, index);
                child_indices.push(layout_element(
                    child,
                    &child_id,
                    bounds,
                    foreground,
                    descendant_clip,
                    overlay,
                    tree,
                    previous,
                ));
            }
        }
        Kind::Flex(axis) => {
            let scroll_rect = rect.inset(style.padding);
            let scrollable_y = *axis == Axis::Vertical
                && matches!(style.overflow_y, Overflow::Scroll | Overflow::Auto);
            let scrollable_x = *axis == Axis::Horizontal
                && matches!(style.overflow_x, Overflow::Scroll | Overflow::Auto);
            let intrinsic_content_width = if scrollable_x {
                element
                    .children
                    .iter()
                    .enumerate()
                    .map(|(index, child)| {
                        measure_child_effective(
                            child,
                            overlay.map(|overlay| (id, overlay)),
                            index,
                            Constraints::loose(Size::new(f32::INFINITY, scroll_rect.size.height)),
                        )
                        .width
                    })
                    .sum::<f32>()
                    + style.gap * element.children.len().saturating_sub(1) as f32
            } else {
                scroll_rect.size.width
            };
            let horizontal_scrollbar_gutter = match style.overflow_x {
                Overflow::Scroll => scrollbar::scrollbar_gutter(&style),
                Overflow::Auto if intrinsic_content_width > scroll_rect.size.width + 0.01 => {
                    scrollbar::scrollbar_gutter(&style)
                }
                _ => 0.0,
            }
            .min(scroll_rect.size.height);
            let content = Rect::new(
                scroll_rect.origin.x,
                scroll_rect.origin.y,
                scroll_rect.size.width,
                (scroll_rect.size.height - horizontal_scrollbar_gutter).max(0.0),
            );
            let minimum_total = element
                .children
                .iter()
                .enumerate()
                .map(|(index, child)| match axis {
                    Axis::Horizontal => {
                        effective_child_style(child, overlay.map(|overlay| (id, overlay)), index)
                            .min_width
                    }
                    Axis::Vertical => {
                        effective_child_style(child, overlay.map(|overlay| (id, overlay)), index)
                            .min_height
                    }
                })
                .sum::<f32>()
                + style.gap * element.children.len().saturating_sub(1) as f32;
            let available = match axis {
                Axis::Horizontal => content.size.width,
                Axis::Vertical => content.size.height,
            };
            if minimum_total > available + 0.01 {
                tree.diagnostic(
                    DiagnosticKind::FlexOverflow,
                    id,
                    format!("minimum total {minimum_total:.2} exceeds {available:.2}"),
                );
            }
            let intrinsic_content_height = if scrollable_y {
                element
                    .children
                    .iter()
                    .enumerate()
                    .map(|(index, child)| {
                        measure_child_effective(
                            child,
                            overlay.map(|overlay| (id, overlay)),
                            index,
                            Constraints::loose(Size::new(content.size.width, f32::INFINITY)),
                        )
                        .height
                    })
                    .sum::<f32>()
                    + style.gap * element.children.len().saturating_sub(1) as f32
            } else {
                content.size.height
            };
            let content_extent = Size::new(
                intrinsic_content_width.max(content.size.width),
                intrinsic_content_height.max(content.size.height),
            );
            let maximum_offset_x = (content_extent.width - content.size.width).max(0.0);
            let scroll_offset_x = if scrollable_x {
                style.scroll_offset_x.clamp(0.0, maximum_offset_x)
            } else {
                0.0
            };
            if scrollable_x && (style.scroll_offset_x - scroll_offset_x).abs() > f32::EPSILON {
                tree.diagnostic(
                    DiagnosticKind::ScrollOffsetClamped,
                    id,
                    format!(
                        "horizontal offset {:.2} clamped to {:.2}",
                        style.scroll_offset_x, scroll_offset_x
                    ),
                );
            }
            let maximum_offset = (content_extent.height - content.size.height).max(0.0);
            let scroll_offset = if scrollable_y {
                let clamped = style.scroll_offset.clamp(0.0, maximum_offset);
                if (style.scroll_offset - clamped).abs() > f32::EPSILON {
                    tree.diagnostic(
                        DiagnosticKind::ScrollOffsetClamped,
                        id,
                        format!(
                            "offset {:.2} clamped to {:.2}",
                            style.scroll_offset, clamped
                        ),
                    );
                }
                let extent = ScrollExtent {
                    viewport: content.size,
                    content: content_extent,
                    offset_x: scroll_offset_x,
                    offset: clamped,
                };
                tree.resolved.nodes[node_index].scroll = Some(extent);
                configure_scroll_semantics(&mut tree.resolved.nodes[node_index], extent);
                tree.scrolls.push(ScrollRegion {
                    id: id.clone(),
                    message: element.primary_message().cloned(),
                    offset_mapper: element.message_mapper,
                    extent_mapper: element.scroll_extent_mapper,
                    rect: scroll_rect,
                    clip: descendant_clip.unwrap_or(scroll_rect),
                    extent,
                    scrollbar: style.scrollbar_palette,
                    parts: style.scrollbar_parts.clone(),
                });
                clamped
            } else {
                0.0
            };
            if scrollable_x {
                let extent = ScrollExtent {
                    viewport: content.size,
                    content: content_extent,
                    offset_x: scroll_offset_x,
                    offset: scroll_offset,
                };
                tree.resolved.nodes[node_index].scroll = Some(extent);
                configure_scroll_semantics(&mut tree.resolved.nodes[node_index], extent);
                tree.scrolls.push(ScrollRegion {
                    id: id.clone(),
                    message: element.primary_message().cloned(),
                    offset_mapper: element.message_mapper,
                    extent_mapper: element.scroll_extent_mapper,
                    rect: scroll_rect,
                    clip: descendant_clip.unwrap_or(scroll_rect),
                    extent,
                    scrollbar: style.scrollbar_palette,
                    parts: style.scrollbar_parts.clone(),
                });
            }
            let layout_content = Rect::new(
                content.origin.x - scroll_offset_x,
                content.origin.y - scroll_offset,
                content_extent.width,
                content_extent.height,
            );
            let child_bounds = flex_bounds(
                layout_content,
                *axis,
                style.gap,
                style.align_items,
                style.justify_content,
                &element.children,
            );
            let child_clip = if scrollable_x || scrollable_y {
                Some(descendant_clip.map_or(content, |parent| {
                    intersection(parent, content)
                        .unwrap_or_else(|| Rect::new(content.origin.x, content.origin.y, 0.0, 0.0))
                }))
            } else {
                descendant_clip
            };
            for (index, (child, bounds)) in element.children.iter().zip(child_bounds).enumerate() {
                let child_id = retained_child_id(tree, id, child, index);
                child_indices.push(layout_element(
                    child, &child_id, bounds, foreground, child_clip, overlay, tree, previous,
                ));
            }
        }
        Kind::VerticalScroll { offset, .. } => {
            let requested_offset = *offset;
            let viewport = rect.inset(style.padding);
            let scroll_child_id = element
                .children
                .first()
                .map(|child| retained_child_id(tree, id, child, 0));
            let initial_size =
                element
                    .children
                    .first()
                    .zip(scroll_child_id.as_ref())
                    .map(|(child, child_id)| {
                        measure_scroll_content(child, child_id, viewport.size.width, tree)
                    });
            let reserves_scrollbar =
                initial_size.is_some_and(|content| content.height > viewport.size.height + 0.01);
            let content_viewport = Rect::new(
                viewport.origin.x,
                viewport.origin.y,
                (viewport.size.width
                    - if reserves_scrollbar {
                        scrollbar::scrollbar_gutter(&style)
                    } else {
                        0.0
                    })
                .max(0.0),
                viewport.size.height,
            );
            let content_clip = descendant_clip.map_or(content_viewport, |parent| {
                intersection(parent, content_viewport)
                    .unwrap_or_else(|| Rect::new(viewport.origin.x, viewport.origin.y, 0.0, 0.0))
            });
            let viewport_clip = descendant_clip.map_or(viewport, |parent| {
                intersection(parent, viewport)
                    .unwrap_or_else(|| Rect::new(viewport.origin.x, viewport.origin.y, 0.0, 0.0))
            });
            if let Some(child) = element.children.first() {
                // Only a changed width can make the second measurement differ.
                // This also covers zero-width scrollbar gutters: reserving a
                // scrollbar must not force a duplicate whole-content walk.
                let content_size = if content_viewport.size.width == viewport.size.width {
                    tree.scroll_content_measurements_reused += 1;
                    initial_size.expect("the existing child was measured above")
                } else {
                    measure_scroll_content(
                        child,
                        scroll_child_id.as_ref().expect("scroll child identity"),
                        content_viewport.size.width,
                        tree,
                    )
                };
                let content_height = content_size.height.max(viewport.size.height);
                let offset =
                    requested_offset.clamp(0.0, (content_height - viewport.size.height).max(0.0));
                if (requested_offset - offset).abs() > f32::EPSILON {
                    tree.diagnostic(
                        DiagnosticKind::ScrollOffsetClamped,
                        id,
                        format!("offset {requested_offset:.2} clamped to {offset:.2}"),
                    );
                }
                let extent = ScrollExtent {
                    viewport: viewport.size,
                    content: Size::new(
                        content_size.width.max(content_viewport.size.width),
                        content_height,
                    ),
                    offset_x: 0.0,
                    offset,
                };
                tree.resolved.nodes[node_index].scroll = Some(extent);
                configure_scroll_semantics(&mut tree.resolved.nodes[node_index], extent);
                if let Some(message) = element.primary_message() {
                    tree.scrolls.push(ScrollRegion {
                        id: id.clone(),
                        message: Some(message.clone()),
                        offset_mapper: element.message_mapper,
                        extent_mapper: element.scroll_extent_mapper,
                        rect: viewport,
                        clip: viewport_clip,
                        extent,
                        scrollbar: style.scrollbar_palette,
                        parts: style.scrollbar_parts.clone(),
                    });
                }
                child_indices.push(layout_element(
                    child,
                    scroll_child_id.as_ref().expect("scroll child identity"),
                    Rect::new(
                        viewport.origin.x,
                        viewport.origin.y - offset,
                        content_viewport.size.width,
                        content_height,
                    ),
                    foreground,
                    Some(content_clip),
                    overlay,
                    tree,
                    previous,
                ));
            }
        }
        Kind::Grid { columns } => {
            let scroll_rect = rect.inset(style.padding);
            let horizontal_scrollbar_gutter =
                if matches!(style.overflow_x, Overflow::Scroll | Overflow::Auto) {
                    scrollbar::scrollbar_gutter(&style).min(scroll_rect.size.height)
                } else {
                    0.0
                };
            let content = Rect::new(
                scroll_rect.origin.x,
                scroll_rect.origin.y,
                scroll_rect.size.width,
                (scroll_rect.size.height - horizontal_scrollbar_gutter).max(0.0),
            );
            let contributions = element
                .children
                .iter()
                .enumerate()
                .map(|(index, child)| {
                    measure_child_effective(
                        child,
                        overlay.map(|overlay| (id, overlay)),
                        index,
                        Constraints::loose(content.size),
                    )
                })
                .collect::<Vec<_>>();
            let widths = resolve_grid_columns(
                columns,
                content.size.width,
                style.gap,
                &contributions,
                element.children.len(),
            );
            let column_count = widths.len().max(1);
            let measured = element
                .children
                .iter()
                .enumerate()
                .map(|(index, child)| {
                    measure_child_effective(
                        child,
                        overlay.map(|overlay| (id, overlay)),
                        index,
                        Constraints::loose(Size::new(
                            widths[index % column_count],
                            content.size.height,
                        )),
                    )
                })
                .collect::<Vec<_>>();
            tree.resolved.nodes[node_index]
                .grid_tracks
                .clone_from(&widths);
            tree.grids.push(ResolvedGrid {
                rect: content,
                columns: column_count,
            });
            let rows = element.children.len().div_ceil(column_count);
            let row_heights = (0..rows)
                .map(|row| {
                    measured[row * column_count..measured.len().min((row + 1) * column_count)]
                        .iter()
                        .map(|size| size.height)
                        .fold(0.0, f32::max)
                })
                .collect::<Vec<_>>();
            let track_width =
                widths.iter().sum::<f32>() + style.gap * widths.len().saturating_sub(1) as f32;
            let (leading, extra_gap) = justify_offsets(
                (content.size.width - track_width).max(0.0),
                widths.len(),
                style.justify_content,
            );
            let content_extent = Size::new(
                track_width.max(content.size.width),
                (row_heights.iter().sum::<f32>() + style.gap * rows.saturating_sub(1) as f32)
                    .max(content.size.height),
            );
            let scrollable_x = matches!(style.overflow_x, Overflow::Scroll | Overflow::Auto);
            let scrollable_y = matches!(style.overflow_y, Overflow::Scroll | Overflow::Auto);
            let offset_x = if scrollable_x {
                element
                    .style
                    .scroll_offset_x
                    .clamp(0.0, (content_extent.width - content.size.width).max(0.0))
            } else {
                0.0
            };
            let offset_y = if scrollable_y {
                element
                    .style
                    .scroll_offset
                    .clamp(0.0, (content_extent.height - content.size.height).max(0.0))
            } else {
                0.0
            };
            if scrollable_x && (offset_x - style.scroll_offset_x).abs() > f32::EPSILON
                || scrollable_y && (offset_y - style.scroll_offset).abs() > f32::EPSILON
            {
                tree.diagnostic(
                    DiagnosticKind::ScrollOffsetClamped,
                    id,
                    format!(
                        "grid offset ({:.2},{:.2}) clamped to ({offset_x:.2},{offset_y:.2})",
                        style.scroll_offset_x, style.scroll_offset
                    ),
                );
            }
            if scrollable_x || scrollable_y {
                let extent = ScrollExtent {
                    viewport: content.size,
                    content: content_extent,
                    offset_x,
                    offset: offset_y,
                };
                tree.resolved.nodes[node_index].scroll = Some(extent);
                tree.scrolls.push(ScrollRegion {
                    id: id.clone(),
                    message: element.primary_message().cloned(),
                    offset_mapper: element.message_mapper,
                    extent_mapper: element.scroll_extent_mapper,
                    rect: scroll_rect,
                    clip: descendant_clip.unwrap_or(scroll_rect),
                    extent,
                    scrollbar: style.scrollbar_palette,
                    parts: style.scrollbar_parts.clone(),
                });
            }
            for (index, child) in element.children.iter().enumerate() {
                let column = index % column_count;
                let row = index / column_count;
                let x = content.origin.x - offset_x
                    + leading
                    + widths[..column].iter().sum::<f32>()
                    + (style.gap + extra_gap) * column as f32;
                let row_y = content.origin.y - offset_y
                    + row_heights[..row].iter().sum::<f32>()
                    + style.gap * row as f32;
                let alignment =
                    effective_child_style(child, overlay.map(|overlay| (id, overlay)), index)
                        .align_self
                        .unwrap_or(style.align_items);
                let item_height = measured[index].height.min(row_heights[row]);
                let row_baseline = if alignment == Align::Baseline {
                    measured[row * column_count..measured.len().min((row + 1) * column_count)]
                        .iter()
                        .map(|size| size.height * 0.8)
                        .fold(0.0, f32::max)
                } else {
                    0.0
                };
                let (y, height) = match alignment {
                    Align::Stretch => (row_y, row_heights[row]),
                    Align::Center => (row_y + (row_heights[row] - item_height) / 2.0, item_height),
                    Align::End => (row_y + row_heights[row] - item_height, item_height),
                    Align::Baseline => (row_y + row_baseline - item_height * 0.8, item_height),
                    Align::Start => (row_y, item_height),
                };
                let child_id = retained_child_id(tree, id, child, index);
                child_indices.push(layout_element(
                    child,
                    &child_id,
                    Rect::new(x, y, widths[column], height),
                    foreground,
                    descendant_clip,
                    overlay,
                    tree,
                    previous,
                ));
            }
        }
    }
    tree.resolved.nodes[node_index].children = child_indices;
    node_index
}

fn geometry_reuse_safe<Message>(element: &Element<Message>) -> bool {
    let owns_auxiliary_layout_records = matches!(element.kind, Kind::VerticalScroll { .. })
        || matches!(element.kind, Kind::Dropdown(_))
        || matches!(element.style.overflow_x, Overflow::Scroll | Overflow::Auto)
        || matches!(element.style.overflow_y, Overflow::Scroll | Overflow::Auto);
    // Reused geometry still recursively lays out every child. A descendant
    // therefore recreates its own scroll/dropdown records when reached; only
    // the current node must decline reuse when it owns auxiliary records.
    !owns_auxiliary_layout_records
}

pub(super) fn resolved_child_id<Message>(
    parent: &UiId,
    child: &Element<Message>,
    index: usize,
) -> UiId {
    child.id.as_ref().map_or_else(
        || parent.scoped(format!("#{index}")),
        |id| parent.scoped(id.as_str()),
    )
}

fn retained_child_id<Message>(
    tree: &UiFrame<Message>,
    parent: &UiId,
    child: &Element<Message>,
    index: usize,
) -> UiId {
    tree.retained_nodes
        .child_ui_id(parent, index)
        .unwrap_or_else(|| resolved_child_id(parent, child, index))
}

fn mask_text(value: &str, mask: char) -> String {
    value
        .chars()
        .map(|character| if character == '\n' { '\n' } else { mask })
        .collect()
}

pub(super) fn apply_transient_state<Message>(
    element: &mut Element<Message>,
    id: &UiId,
    state: &mut UiStateStore,
) {
    apply_transient_state_with_parent(element, id, state, None);
}

#[derive(Default)]
pub(super) struct TransientStateJournal {
    styles: std::collections::HashMap<UiId, Style>,
    kinds: std::collections::HashMap<UiId, Kind>,
    phases: std::collections::HashMap<UiId, super::super::retained::DirtyPhases>,
}

impl TransientStateJournal {
    pub(super) fn is_empty(&self) -> bool {
        self.styles.is_empty() && self.kinds.is_empty()
    }

    pub(super) fn style_count(&self) -> usize {
        self.styles.len()
    }

    pub(super) fn kind_count(&self) -> usize {
        self.kinds.len()
    }

    pub(super) fn affected_phases(
        &self,
    ) -> impl Iterator<Item = (&UiId, super::super::retained::DirtyPhases)> {
        self.phases.iter().map(|(id, phases)| (id, *phases))
    }

    pub(super) fn restore<Message>(mut self, element: &mut Element<Message>, id: &UiId) {
        self.restore_tree(element, id);
        debug_assert!(self.styles.is_empty());
        debug_assert!(self.kinds.is_empty());
    }

    /// Restores the canonical declaration while retaining the effective
    /// transient payloads in this side table. This creates the ownership
    /// boundary needed for resolution to read through the table without
    /// modifying the declaration it will publish.
    pub(super) fn detach_effective<Message>(&mut self, element: &mut Element<Message>, id: &UiId) {
        self.detach_effective_tree(element, id);
    }

    pub(super) fn attach_effective<Message>(&mut self, element: &mut Element<Message>, id: &UiId) {
        self.detach_effective_tree(element, id);
    }

    fn detach_effective_tree<Message>(&mut self, element: &mut Element<Message>, id: &UiId) {
        for (index, child) in element.children.iter_mut().enumerate() {
            let child_id = resolved_child_id(id, child, index);
            self.detach_effective_tree(child, &child_id);
        }
        if let Some(canonical) = self.styles.get_mut(id) {
            std::mem::swap(canonical, &mut element.style);
        }
        if self
            .styles
            .get(id)
            .is_some_and(|effective| *effective == element.style)
        {
            self.styles.remove(id);
        }
        if let Some(canonical) = self.kinds.get_mut(id) {
            std::mem::swap(canonical, &mut element.kind);
        }
        if self.kinds.get(id).is_some_and(|effective| {
            super::super::retained::exact_kind_metadata_equal(effective, &element.kind)
        }) {
            self.kinds.remove(id);
        }
        let effective_style = self.styles.get(id).unwrap_or(&element.style);
        let effective_kind = self.kinds.get(id).unwrap_or(&element.kind);
        let dirty = super::super::retained::transient_dirty_phases(
            element,
            effective_style,
            effective_kind,
        );
        if dirty == super::super::retained::DirtyPhases::default() {
            self.phases.remove(id);
        } else {
            self.phases.insert(id.clone(), dirty);
        }
        compact_kind_capacity(&mut element.kind);
    }

    pub(super) fn style<'a, Message>(
        &'a self,
        id: &UiId,
        element: &'a Element<Message>,
    ) -> &'a Style {
        self.styles.get(id).unwrap_or(&element.style)
    }

    pub(super) fn kind<'a, Message>(
        &'a self,
        id: &UiId,
        element: &'a Element<Message>,
    ) -> &'a Kind {
        self.kinds.get(id).unwrap_or(&element.kind)
    }

    fn restore_tree<Message>(&mut self, element: &mut Element<Message>, id: &UiId) {
        for (index, child) in element.children.iter_mut().enumerate() {
            let child_id = resolved_child_id(id, child, index);
            self.restore_tree(child, &child_id);
        }
        self.restore_node(element, id);
    }

    fn restore_node<Message>(&mut self, element: &mut Element<Message>, id: &UiId) {
        if let Some(style) = self.styles.remove(id) {
            element.style = style;
        }
        if let Some(kind) = self.kinds.remove(id) {
            element.kind = kind;
        }
        compact_kind_capacity(&mut element.kind);
    }
}

pub(super) fn apply_transient_state_staged<Message>(
    element: &mut Element<Message>,
    id: &UiId,
    state: &mut UiStateStore,
    overlay: &mut TransientStateJournal,
) {
    capture_transient_state(element, id, state, None, false, false, overlay);
    apply_transient_state(element, id, state);
}

fn capture_transient_state<Message>(
    element: &mut Element<Message>,
    id: &UiId,
    state: &UiStateStore,
    parent_interaction: Option<usize>,
    inherited_text_paint: bool,
    inherited_geometry: bool,
    overlay: &mut TransientStateJournal,
) {
    let direct_interaction = if state.pressed() == Some(id) {
        Some(1)
    } else if state.window_focused()
        && (state.focused() == Some(id) || state.navigation().controller_selected() == Some(id))
    {
        Some(2)
    } else if state.hovered() == Some(id) {
        Some(0)
    } else {
        None
    };
    let interaction_index = if element.style.parent_interaction {
        parent_interaction.or(direct_interaction)
    } else {
        direct_interaction
    };
    let owns_state = element.style.parent_interaction
        || element.primary_message().is_some()
        || element.navigation_scope.is_some()
        || element.text_mapper.is_some()
        || matches!(element.style.overflow_x, Overflow::Scroll | Overflow::Auto)
        || matches!(element.style.overflow_y, Overflow::Scroll | Overflow::Auto)
        || matches!(
            element.kind,
            Kind::VerticalScroll { .. } | Kind::Dropdown(_)
        );
    if owns_state || inherited_text_paint || inherited_geometry {
        if !overlay.styles.contains_key(id) {
            let original = std::mem::take(&mut element.style);
            element.style = original.clone();
            overlay.styles.insert(id.clone(), original);
        }
    }
    if element.text_mapper.is_some()
        || matches!(
            element.kind,
            Kind::VerticalScroll { .. }
                | Kind::Slider { .. }
                | Kind::Dropdown(_)
                | Kind::Text { .. }
        ) && (owns_state || inherited_text_paint)
    {
        if !overlay.kinds.contains_key(id) {
            let original = std::mem::replace(&mut element.kind, Kind::Layer);
            element.kind = original.clone();
            overlay.kinds.insert(id.clone(), original);
        }
    }
    let child_text_paint = inherited_text_paint || interaction_index.is_some();
    let child_geometry = inherited_geometry || element.style.proximity_magnification.is_some();
    for (index, child) in element.children.iter_mut().enumerate() {
        let child_id = resolved_child_id(id, child, index);
        capture_transient_state(
            child,
            &child_id,
            state,
            interaction_index,
            child_text_paint,
            child_geometry,
            overlay,
        );
    }
}

fn apply_transient_state_with_parent<Message>(
    element: &mut Element<Message>,
    id: &UiId,
    state: &mut UiStateStore,
    parent_interaction: Option<usize>,
) {
    let direct_interaction = if state.pressed() == Some(id) {
        Some(1)
    } else if state.window_focused()
        && (state.focused() == Some(id) || state.navigation().controller_selected() == Some(id))
    {
        Some(2)
    } else if state.hovered() == Some(id) {
        Some(0)
    } else {
        None
    };
    let interaction_index = if element.style.parent_interaction {
        parent_interaction.or(direct_interaction)
    } else {
        direct_interaction
    };
    let owns_state = element.style.parent_interaction
        || element.primary_message().is_some()
        || element.navigation_scope.is_some()
        || element.text_mapper.is_some()
        || matches!(element.style.overflow_x, Overflow::Scroll | Overflow::Auto)
        || matches!(element.style.overflow_y, Overflow::Scroll | Overflow::Auto)
        || matches!(
            element.kind,
            Kind::VerticalScroll { .. } | Kind::Dropdown(_)
        );
    if owns_state {
        let focus_foreground = element.style.foreground.or_else(|| {
            element
                .children
                .iter()
                .find_map(|child| focus_participating_foreground(child))
        });
        let scope_background_active = state.window_focused()
            && state.input_modality() == InputModality::Controller
            && element.navigation_scope.is_some()
            && (state.navigation().controller_selected() == Some(id)
                || state.navigation().controller_scope() == Some(id)
                || (element
                    .navigation_scope
                    .as_ref()
                    .is_some_and(|scope| scope.pane)
                    && state.navigation().controller_pane() == Some(id)))
            && element.style.controller_scope_background.is_some();
        if scope_background_active
            && let Some(background) = element.style.controller_scope_background
        {
            element.style.background = Some(background);
        }
        if element
            .navigation_scope
            .as_ref()
            .is_some_and(|scope| scope.pane)
            && state.navigation().controller_pane().is_none()
            && element
                .navigation_scope
                .as_ref()
                .is_some_and(|scope| scope.default_pane)
        {
            state.navigation_mut().set_controller_pane(Some(id.clone()));
        }
        if state.pressed() == Some(id) {
            if let Some(background) = element.style.pressed_background {
                element.style.background = Some(background);
            }
        } else if state.hovered() == Some(id)
            && let Some(background) = element.style.hover_background
        {
            element.style.background = Some(background);
        }
        let exact_focus_background = (!scope_background_active
            && state.window_focused()
            && state.pressed() != Some(id)
            && (state.focused() == Some(id)
                || state.navigation().controller_selected() == Some(id)))
        .then_some(element.style.focus_background)
        .flatten();
        if let Some(background) = exact_focus_background {
            element.style.background = Some(background);
        }
        if let Some(index) = interaction_index {
            let paint = element
                .style
                .interaction_paints
                .as_ref()
                .map_or(InteractionPaint::default(), |paints| paints[index]);
            if let Some(color) = paint.background {
                element.style.background = (color != 0).then_some(Background::Solid(color));
            }
            if let Some(color) = paint.border_color {
                element.style.border = (color != 0).then_some(color);
            }
            if let Some(width) = paint.border_width {
                element.style.border_width = width;
            }
            if let Some(radius) = paint.radius {
                element.style.corner_radius = radius;
            }
            if let Some(width) = paint.width {
                element.style.width = width;
            }
            if let Some(height) = paint.height {
                element.style.height = height;
            }
            fn text_paint<Message>(
                element: &mut Element<Message>,
                paint: InteractionPaint,
                owner: bool,
            ) {
                if let Some(color) = paint.foreground
                    && (owner || element.style.inherited_state_text[0])
                {
                    element.style.foreground = Some(color);
                }
                if let Kind::Text {
                    scale, line_height, ..
                } = &mut element.kind
                {
                    if let Some(size) = paint.font_size
                        && (owner || element.style.inherited_state_text[1])
                    {
                        *scale = -size;
                    }
                    if let Some(height) = paint.line_height
                        && (owner || element.style.inherited_state_text[2])
                    {
                        *line_height = Some(height);
                    }
                }
                for child in &mut element.children {
                    text_paint(child, paint, false);
                }
            }
            text_paint(element, paint, true);
        }
        // A transparent editor already exposes keyboard focus through its caret.
        // Filling its entire text node with the generic fallback tint makes a
        // blue/purple strip appear and disappear as input modality changes.
        let transparent_editor = element.text_mapper.is_some()
            && matches!(element.kind, Kind::Text { .. })
            && element.style.background.is_none();
        let css_dropdown =
            matches!(&element.kind, Kind::Dropdown(dropdown) if dropdown.presentation.is_some());
        let active_focus_tint = if scope_background_active
            || exact_focus_background.is_some()
            || css_dropdown
            || !element.style.automatic_focus_tint
        {
            None
        } else {
            if state.window_focused() && state.navigation().controller_selected() == Some(id) {
                element
                    .style
                    .controller_focus_background_tint
                    .or(element.style.focus_background_tint)
                    .or(Some(crate::theme::FALLBACK_CONTROLLER_FOCUS_CUE))
            } else if state.window_focused()
                && state.focused() == Some(id)
                && matches!(
                    state.input_modality(),
                    InputModality::Keyboard | InputModality::Accessibility
                )
            {
                element.style.focus_background_tint.or_else(|| {
                    (!transparent_editor).then_some(crate::theme::FALLBACK_KEYBOARD_FOCUS_CUE)
                })
            } else {
                None
            }
        };
        if let Some(tint) = active_focus_tint {
            let transform = |color| {
                focus_foreground.map_or_else(
                    || crate::focused_surface(color, tint),
                    |foreground| crate::focused_surface_with_foreground(color, tint, foreground),
                )
            };
            element.style.background = Some(match element.style.background {
                Some(Background::Solid(color)) => Background::Solid(transform(color)),
                Some(Background::LinearGradient(mut gradient)) => {
                    gradient.start = transform(gradient.start);
                    gradient.end = transform(gradient.end);
                    Background::LinearGradient(gradient)
                }
                None => Background::Solid(transform(crate::theme::FALLBACK_FOCUS_SURFACE)),
            });
        }
        let requested_open_generation = match &element.kind {
            Kind::Dropdown(dropdown) => Some(dropdown.open_generation),
            _ => None,
        };
        let (scroll_offset_x, scroll_offset, scroll_at_end, dropdown_open) = {
            let transient = state.touch(id.clone());
            if requested_open_generation.is_some_and(|generation| {
                if generation == transient.dropdown_open_generation {
                    false
                } else {
                    transient.dropdown_open_generation = generation;
                    true
                }
            }) {
                transient.dropdown_open = true;
            }
            (
                transient.scroll_offset_x,
                transient.scroll_offset,
                transient.scroll_at_end,
                transient.dropdown_open,
            )
        };
        match &mut element.kind {
            Kind::VerticalScroll {
                offset,
                controlled: false,
                ..
            } => *offset = scroll_offset.max(0.0),
            Kind::Slider {
                presentation: Some(parts),
                ..
            } => {
                let index = if state.pressed() == Some(id) {
                    Some(1)
                } else if state.window_focused()
                    && (state.focused() == Some(id)
                        || state.navigation().controller_selected() == Some(id))
                {
                    Some(2)
                } else if state.hovered() == Some(id) {
                    Some(0)
                } else {
                    None
                };
                if let Some(index) = index {
                    for part in parts.iter_mut() {
                        *part = part.with_interaction(index);
                    }
                }
            }
            Kind::Dropdown(dropdown) => {
                dropdown.expanded = dropdown_open;
                let owner_paint = interaction_index
                    .map(|index| {
                        element
                            .style
                            .interaction_paints
                            .as_ref()
                            .map_or(InteractionPaint::default(), |paints| paints[index])
                    })
                    .unwrap_or_default();
                if let Some(parts) = dropdown.presentation.as_mut() {
                    for part in parts.iter_mut() {
                        *part = part.inherited_paint(owner_paint);
                    }
                    let resolved = |mut part: DropdownPartStyle, part_id: &UiId| {
                        let index = if state.pressed() == Some(part_id) {
                            Some(1)
                        } else if state.window_focused()
                            && (state.focused() == Some(part_id)
                                || state.navigation().controller_selected() == Some(part_id))
                        {
                            Some(2)
                        } else if state.hovered() == Some(part_id) {
                            Some(0)
                        } else {
                            None
                        };
                        if let Some(index) = index {
                            part = part.with_interaction(index);
                        }
                        part
                    };
                    parts[0] = resolved(parts[0], id);
                    parts[2] = resolved(parts[2], id);
                    let details = dropdown
                        .details
                        .get_or_insert_with(|| Box::new(DropdownDetails::default()));
                    details.resolved_options = (0..dropdown.options.len())
                        .map(|index| {
                            resolved(
                                details
                                    .option_presentations
                                    .get(index)
                                    .copied()
                                    .unwrap_or(parts[1])
                                    .inherited_paint(owner_paint),
                                &id.scoped(format!("option-{index}")),
                            )
                        })
                        .collect();
                }
                let height = dropdown.presentation.as_ref().map(|parts| {
                    parts[0].height
                        + parts[0].margin.height()
                        + if dropdown.expanded && !dropdown.overlay {
                            dropdown
                                .details
                                .as_ref()
                                .into_iter()
                                .flat_map(|details| details.resolved_options.iter())
                                .map(|part| part.height + part.margin.height())
                                .sum::<f32>()
                        } else {
                            0.0
                        }
                });
                if !element.style.css_paint {
                    element.style.height = Length::Px(height.unwrap_or({
                        if dropdown.overlay {
                            30.0
                        } else if dropdown.expanded {
                            42.0 + dropdown.options.len() as f32 * 36.0
                        } else {
                            42.0
                        }
                    }));
                }
            }
            _ => {}
        }
        if element.text_mapper.is_some()
            && let Kind::Text {
                value,
                scale,
                bold,
                line_height,
                details,
                ..
            } = &mut element.kind
        {
            let details = details
                .get_or_insert_with(|| Box::new(TextDetails::default()))
                .as_mut();
            // A text field may paint a placeholder while its editable value is
            // intentionally empty. Reconciliation must seed the editor from
            // that canonical input value, not from presentation text.
            let initial = details.input_value.clone().unwrap_or_else(|| value.clone());
            let focused = state.focused() == Some(id);
            let caret_visible = state.caret_visible();
            let editor = state.editor(id.clone(), &initial);
            if editor.text() != initial {
                editor.set_text(initial);
            }
            details.input_value = Some(editor.text().to_owned());
            details.selection_x = focused
                .then(|| editor.selection())
                .flatten()
                .map(|selection| {
                    let width = |end| {
                        let visible = details.input_mask.map_or_else(
                            || editor.text()[..end].to_owned(),
                            |mask| mask_text(&editor.text()[..end], mask),
                        );
                        measure_text_with_retention(
                            &visible,
                            *scale,
                            *bold,
                            false,
                            None,
                            Some(1),
                            f32::INFINITY,
                            details
                                .input_mask
                                .map_or(TextRetention::Public, TextRetention::Masked),
                        )
                        .width
                    };
                    (width(selection.start), width(selection.end))
                });
            details.caret_position = (focused && caret_visible).then(|| {
                let prefix = details.input_mask.map_or_else(
                    || editor.display_caret_prefix(),
                    |mask| mask_text(&editor.display_caret_prefix(), mask),
                );
                let (line_index, line) =
                    prefix
                        .rsplit_once('\n')
                        .map_or((0, prefix.as_str()), |(before, line)| {
                            (
                                before.bytes().filter(|byte| *byte == b'\n').count() + 1,
                                line,
                            )
                        });
                let x = measure_text_with_retention(
                    line,
                    *scale,
                    *bold,
                    false,
                    None,
                    Some(1),
                    f32::INFINITY,
                    details
                        .input_mask
                        .map_or(TextRetention::Public, TextRetention::Masked),
                )
                .width;
                let height = line_height.unwrap_or_else(|| text_font_size(*scale) * 1.3);
                Point {
                    x,
                    y: line_index as f32 * height,
                }
            });
            *value = if editor.text().is_empty() && editor.preedit().is_empty() {
                details.input_placeholder.clone().unwrap_or_default()
            } else if let Some(mask) = details.input_mask {
                mask_text(&editor.display_text_with_caret(""), mask)
            } else {
                editor.display_text_with_caret("")
            };
        }
        if matches!(element.style.overflow_y, Overflow::Scroll | Overflow::Auto) {
            element.style.scroll_offset = if element.style.follow_scroll_end && scroll_at_end {
                f32::MAX
            } else {
                scroll_offset.max(0.0)
            };
        }
        if matches!(element.style.overflow_x, Overflow::Scroll | Overflow::Auto) {
            element.style.scroll_offset_x = scroll_offset_x.max(0.0);
        }
    }
    let magnified_child = element.style.proximity_magnification.and_then(|_| {
        let hovered = state.hovered()?.as_str();
        element
            .children
            .iter()
            .enumerate()
            .find_map(|(index, child)| {
                let child_id = child.id.as_ref().map_or_else(
                    || id.scoped(format!("#{index}")),
                    |child_id| id.scoped(child_id.as_str()),
                );
                (hovered == child_id.as_str()
                    || hovered.starts_with(&format!("{}/", child_id.as_str())))
                .then_some(index)
            })
    });
    for (index, child) in element.children.iter_mut().enumerate() {
        let child_id = child.id.as_ref().map_or_else(
            || id.scoped(format!("#{index}")),
            |child_id| id.scoped(child_id.as_str()),
        );
        if let Some(magnification) = element.style.proximity_magnification {
            let target = magnified_child.map_or(1.0, |hovered| {
                let pointer_index = hovered as f32 + state.hover_fraction() - 0.5;
                let distance = (index as f32 - pointer_index).abs();
                if distance > magnification.radius as f32 + 0.5 {
                    1.0
                } else {
                    let influence = 1.0 - distance / (magnification.radius as f32 + 1.0);
                    1.0 + (magnification.maximum_scale - 1.0) * influence
                }
            });
            let scale = state.animate_geometry_scale(
                child_id.clone(),
                target,
                element.style.transition_duration_ms,
            );
            scale_explicit_geometry(child, scale);
        }
        apply_transient_state_with_parent(
            child,
            &child_id,
            state,
            direct_interaction.or(parent_interaction),
        );
    }
    // The application just built this declaration and relinquishes it after
    // resolution. Normalize common builder over-allocation in the traversal
    // already required for transient state instead of walking the tree again.
    element.children.shrink_to_fit();
    element.option_messages.shrink_to_fit();
    element.inline_messages.shrink_to_fit();
    compact_kind_capacity(&mut element.kind);
}

pub(super) fn scale_explicit_geometry<Message>(element: &mut Element<Message>, scale: f32) {
    if let Length::Px(width) = element.style.width {
        element.style.width = Length::Px(width * scale);
    }
    if let Length::Px(height) = element.style.height {
        element.style.height = Length::Px(height * scale);
    }
    for child in &mut element.children {
        scale_explicit_geometry(child, scale);
    }
}

fn focus_participating_foreground<Message>(element: &Element<Message>) -> Option<Color> {
    element.style.foreground.or_else(|| {
        element
            .children
            .iter()
            .find_map(focus_participating_foreground)
    })
}

pub(super) fn resolve_grid_columns(
    definition: &GridColumnSpec,
    available: f32,
    gap: f32,
    children: &[Size],
    child_count: usize,
) -> Vec<f32> {
    let tracks = match definition {
        GridColumnSpec::Count(count) => {
            let count = (*count).max(1);
            if available.is_finite() {
                let width = ((available - gap.max(0.0) * count.saturating_sub(1) as f32)
                    / count as f32)
                    .max(0.0);
                return vec![width; count];
            }
            let width = children.iter().map(|size| size.width).fold(0.0, f32::max);
            return vec![width; count];
        }
        GridColumnSpec::Tracks(tracks) => {
            if let [Track::AutoFit(track)] = tracks.as_slice() {
                let minimum = track_minimum(track).max(1.0);
                let count = if available.is_finite() {
                    (((available + gap.max(0.0)) / (minimum + gap.max(0.0))).floor() as usize)
                        .max(1)
                        .min(child_count.max(1))
                } else {
                    child_count.max(1)
                };
                vec![track.as_ref().clone(); count]
            } else {
                expand_tracks(tracks)
            }
        }
        GridColumnSpec::AutoFit(track) => {
            let minimum = track_minimum(track).max(1.0);
            let count = if available.is_finite() {
                (((available + gap.max(0.0)) / (minimum + gap.max(0.0))).floor() as usize)
                    .max(1)
                    .min(child_count.max(1))
            } else {
                child_count.max(1)
            };
            vec![track.clone(); count]
        }
    };
    if tracks.is_empty() {
        return vec![available.max(0.0)];
    }
    let mut widths = vec![0.0; tracks.len()];
    let mut fractions = vec![0.0; tracks.len()];
    for (index, track) in tracks.iter().enumerate() {
        let contribution = children
            .iter()
            .skip(index)
            .step_by(tracks.len())
            .map(|size| size.width)
            .fold(0.0, f32::max);
        let (base, fraction) = resolve_track(track, contribution);
        widths[index] = if available.is_finite() || fraction == 0.0 {
            base
        } else {
            base.max(contribution)
        };
        fractions[index] = fraction;
    }
    if available.is_finite() {
        let gap_total = gap.max(0.0) * tracks.len().saturating_sub(1) as f32;
        let free = (available - gap_total - widths.iter().sum::<f32>()).max(0.0);
        let fraction_total = fractions.iter().sum::<f32>();
        if fraction_total > 0.0 {
            for ((width, fraction), track) in widths.iter_mut().zip(&fractions).zip(&tracks) {
                if *fraction > 0.0 {
                    *width += free * *fraction / fraction_total;
                    if let Track::MinMax(_, max) = track
                        && let Track::Px(maximum) = max.as_ref()
                    {
                        *width = width.min(maximum.max(0.0));
                    }
                }
            }
        }
    }
    widths
}

fn expand_tracks(tracks: &[Track]) -> Vec<Track> {
    let mut expanded = Vec::new();
    for track in tracks {
        match track {
            Track::Repeat(count, track) => {
                expanded.extend(std::iter::repeat_n(track.as_ref().clone(), *count));
            }
            Track::AutoFit(track) => expanded.push(track.as_ref().clone()),
            _ => expanded.push(track.clone()),
        }
    }
    expanded
}

fn track_minimum(track: &Track) -> f32 {
    match track {
        Track::Px(value) => value.max(0.0),
        Track::MinMax(min, _) => track_minimum(min),
        Track::Repeat(_, track) | Track::AutoFit(track) => track_minimum(track),
        Track::Auto | Track::Fraction(_) => 0.0,
    }
}

fn resolve_track(track: &Track, contribution: f32) -> (f32, f32) {
    match track {
        Track::Px(value) => (value.max(0.0), 0.0),
        Track::Auto => (contribution.max(0.0), 0.0),
        Track::Fraction(fraction) => (0.0, fraction.max(0.0)),
        Track::MinMax(min, max) => {
            let minimum = match min.as_ref() {
                Track::Auto => contribution,
                other => track_minimum(other),
            };
            match max.as_ref() {
                Track::Fraction(fraction) => (minimum, fraction.max(0.0)),
                Track::Px(maximum) => (minimum.min(maximum.max(0.0)), 0.0),
                Track::Auto => (minimum.max(contribution), 0.0),
                other => (minimum.max(track_minimum(other)), 0.0),
            }
        }
        Track::Repeat(_, track) | Track::AutoFit(track) => resolve_track(track, contribution),
    }
}

fn justify_offsets(free: f32, count: usize, justify: Justify) -> (f32, f32) {
    if count == 0 {
        return (0.0, 0.0);
    }
    match justify {
        Justify::Start => (0.0, 0.0),
        Justify::Center => (free / 2.0, 0.0),
        Justify::End => (free, 0.0),
        Justify::SpaceBetween if count > 1 => (0.0, free / (count - 1) as f32),
        Justify::SpaceAround => {
            let space = free / count as f32;
            (space / 2.0, space)
        }
        Justify::SpaceEvenly => {
            let space = free / (count + 1) as f32;
            (space, space)
        }
        _ => (0.0, 0.0),
    }
}

fn flex_bounds<Message>(
    content: Rect,
    axis: Axis,
    gap: f32,
    align_items: Align,
    justify: Justify,
    children: &[Element<Message>],
) -> Vec<Rect> {
    if children.is_empty() {
        return Vec::new();
    }
    let measured = children
        .iter()
        .map(|child| {
            measure_element(
                child,
                Constraints::loose(match axis {
                    Axis::Horizontal => Size::new(f32::INFINITY, content.size.height),
                    Axis::Vertical => Size::new(content.size.width, f32::INFINITY),
                }),
            )
        })
        .collect::<Vec<_>>();
    let items = children
        .iter()
        .zip(&measured)
        .map(|(child, measured)| {
            let (intrinsic, min, max) = match axis {
                Axis::Horizontal => (measured.width, child.style.min_width, child.style.max_width),
                Axis::Vertical => (
                    measured.height,
                    child.style.min_height,
                    child.style.max_height,
                ),
            };
            let parent = match axis {
                Axis::Horizontal => content.size.width,
                Axis::Vertical => content.size.height,
            };
            let preferred = child.style.basis.resolve(
                parent,
                match axis {
                    Axis::Horizontal => child.style.width.resolve(parent, intrinsic),
                    Axis::Vertical => child.style.height.resolve(parent, intrinsic),
                },
            );
            FlexItem::flex(
                preferred,
                min.max(0.0),
                max.max(min.max(0.0)),
                child.style.grow,
                child.style.shrink,
            )
        })
        .collect::<Vec<_>>();
    let mut rects = layout_flex(content, axis, gap.max(0.0), &items);
    let resolved_measured = children
        .iter()
        .zip(&rects)
        .map(|(child, rect)| match axis {
            Axis::Horizontal => measure_element(
                child,
                Constraints::loose(Size::new(rect.size.width, f32::INFINITY)),
            ),
            Axis::Vertical => measure_element(
                child,
                Constraints::loose(Size::new(content.size.width, rect.size.height)),
            ),
        })
        .collect::<Vec<_>>();
    let occupied = rects
        .iter()
        .map(|rect| match axis {
            Axis::Horizontal => rect.size.width,
            Axis::Vertical => rect.size.height,
        })
        .sum::<f32>()
        + gap.max(0.0) * rects.len().saturating_sub(1) as f32;
    let available = match axis {
        Axis::Horizontal => content.size.width,
        Axis::Vertical => content.size.height,
    };
    let free = (available - occupied).max(0.0);
    let (leading, extra_gap) = justify_offsets(free, rects.len(), justify);
    let shared_baseline = if axis == Axis::Horizontal {
        children
            .iter()
            .zip(&resolved_measured)
            .filter(|(child, _)| child.style.align_self.unwrap_or(align_items) == Align::Baseline)
            .map(|(_, size)| size.height * 0.8)
            .fold(0.0, f32::max)
    } else {
        0.0
    };
    for (index, ((rect, child), measured)) in rects
        .iter_mut()
        .zip(children)
        .zip(&resolved_measured)
        .enumerate()
    {
        let main_offset = leading + index as f32 * extra_gap;
        let alignment = child.style.align_self.unwrap_or(align_items);
        match axis {
            Axis::Horizontal => {
                rect.origin.x += main_offset;
                let cross = measured.height.min(content.size.height);
                if alignment != Align::Stretch
                    || (child.style.css_paint && child.style.height != Length::Auto)
                {
                    rect.size.height = cross;
                    rect.origin.y += match alignment {
                        Align::Center => (content.size.height - cross) / 2.0,
                        Align::End => content.size.height - cross,
                        Align::Baseline => shared_baseline - cross * 0.8,
                        _ => 0.0,
                    };
                }
            }
            Axis::Vertical => {
                rect.origin.y += main_offset;
                let cross = measured.width.min(content.size.width);
                if alignment != Align::Stretch
                    || (child.style.css_paint && child.style.width != Length::Auto)
                {
                    rect.size.width = cross;
                    rect.origin.x += match alignment {
                        Align::Center => (content.size.width - cross) / 2.0,
                        Align::End => content.size.width - cross,
                        _ => 0.0,
                    };
                }
            }
        }
    }
    rects
}

pub(super) fn explicit_px(length: Length) -> Option<f32> {
    match length {
        Length::Px(value) => Some(value.max(0.0)),
        _ => None,
    }
}

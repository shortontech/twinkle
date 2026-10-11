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
    let style = overlay.map_or_else(
        || EffectiveStyle::canonical(&element.style),
        |overlay| overlay.style(id, element),
    );
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
            let input_value = overlay.map_or(details.input_value.as_deref(), |overlay| {
                overlay.text_input_value(id, details.input_value.as_deref())
            });
            Some(SemanticValueSnapshot::ProtectedText {
                character_count: input_value.unwrap_or_default().chars().count(),
            })
        }
        Kind::Text {
            details: Some(details),
            ..
        } if details.input_value.is_some() => {
            let input_value = overlay.map_or(details.input_value.as_deref(), |overlay| {
                overlay.text_input_value(id, details.input_value.as_deref())
            });
            Some(SemanticValueSnapshot::Text(
                input_value.unwrap_or_default().to_owned(),
            ))
        }
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
    if matches!(style.width(), Length::Percent(_)) && !rect.size.width.is_finite()
        || matches!(style.height(), Length::Percent(_)) && !rect.size.height.is_finite()
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
                let value = overlay.map_or(value.as_str(), |overlay| overlay.text_value(id, value));
                let scale = overlay.map_or(*scale, |overlay| overlay.text_scale(id, *scale));
                let line_height = overlay.map_or(*line_height, |overlay| {
                    overlay.text_line_height(id, *line_height)
                });
                tree.diagnostic_text_measurements =
                    tree.diagnostic_text_measurements.saturating_add(1);
                Some(measure_text(
                    value,
                    scale,
                    *bold,
                    *wrap,
                    line_height,
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
    let foreground = style.foreground().or(inherited_foreground);
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
                style.scroll_offset_x().clamp(0.0, maximum_offset_x)
            } else {
                0.0
            };
            if scrollable_x && (style.scroll_offset_x() - scroll_offset_x).abs() > f32::EPSILON {
                tree.diagnostic(
                    DiagnosticKind::ScrollOffsetClamped,
                    id,
                    format!(
                        "horizontal offset {:.2} clamped to {:.2}",
                        style.scroll_offset_x(),
                        scroll_offset_x
                    ),
                );
            }
            let maximum_offset = (content_extent.height - content.size.height).max(0.0);
            let scroll_offset = if scrollable_y {
                let clamped = style.scroll_offset().clamp(0.0, maximum_offset);
                if (style.scroll_offset() - clamped).abs() > f32::EPSILON {
                    tree.diagnostic(
                        DiagnosticKind::ScrollOffsetClamped,
                        id,
                        format!(
                            "offset {:.2} clamped to {:.2}",
                            style.scroll_offset(),
                            clamped
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
                overlay.map(|overlay| (id, overlay)),
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
            let requested_offset = overlay.map_or(*offset, |overlay| {
                overlay.vertical_scroll_offset(id, *offset)
            });
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
            if scrollable_x && (offset_x - style.scroll_offset_x()).abs() > f32::EPSILON
                || scrollable_y && (offset_y - style.scroll_offset()).abs() > f32::EPSILON
            {
                tree.diagnostic(
                    DiagnosticKind::ScrollOffsetClamped,
                    id,
                    format!(
                        "grid offset ({:.2},{:.2}) clamped to ({offset_x:.2},{offset_y:.2})",
                        style.scroll_offset_x(),
                        style.scroll_offset()
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

fn text_projection_needed<Message>(
    element: &Element<Message>,
    id: &UiId,
    state: &UiStateStore,
) -> bool {
    if element.text_mapper.is_none() {
        return false;
    }
    let Kind::Text { value, details, .. } = &element.kind else {
        return false;
    };
    let Some(details) = details.as_ref() else {
        return true;
    };
    let Some(initial) = details.input_value.as_ref() else {
        return true;
    };
    if state.focused() == Some(id) {
        return true;
    }
    let Some(editor) = state
        .state(id)
        .and_then(|transient| transient.editor.as_ref())
    else {
        let expected = if initial.is_empty() {
            details.input_placeholder.as_deref().unwrap_or_default()
        } else {
            initial.as_str()
        };
        return value != expected;
    };
    editor.text() != initial || !editor.preedit().is_empty() || editor.selection().is_some()
}

fn project_text_state<Message>(
    element: &Element<Message>,
    id: &UiId,
    state: &mut UiStateStore,
) -> Option<TransientTextPatch> {
    let Kind::Text {
        value,
        scale,
        bold,
        line_height,
        details,
        ..
    } = &element.kind
    else {
        return None;
    };
    let details = details.as_deref();
    let initial = details
        .and_then(|details| details.input_value.as_ref())
        .cloned()
        .unwrap_or_else(|| value.clone());
    let focused = state.focused() == Some(id);
    let caret_visible = state.caret_visible();
    let editor = state.editor(id.clone(), &initial);
    if editor.text() != initial {
        editor.set_text(initial);
    }
    let input_mask = details.and_then(|details| details.input_mask);
    let retention = input_mask.map_or(TextRetention::Public, TextRetention::Masked);
    let selection_x = focused
        .then(|| editor.selection())
        .flatten()
        .map(|selection| {
            let width = |end| {
                let visible = input_mask.map_or_else(
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
                    retention,
                )
                .width
            };
            (width(selection.start), width(selection.end))
        });
    let caret_position = (focused && caret_visible).then(|| {
        let prefix = input_mask.map_or_else(
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
            retention,
        )
        .width;
        let height = line_height.unwrap_or_else(|| text_font_size(*scale) * 1.3);
        Point {
            x,
            y: line_index as f32 * height,
        }
    });
    let projected_value = if editor.text().is_empty() && editor.preedit().is_empty() {
        details
            .and_then(|details| details.input_placeholder.clone())
            .unwrap_or_default()
    } else if let Some(mask) = input_mask {
        mask_text(&editor.display_text_with_caret(""), mask)
    } else {
        editor.display_text_with_caret("")
    };
    Some(TransientTextPatch {
        value: projected_value,
        input_value: editor.text().to_owned(),
        selection_x,
        caret_position,
    })
}

fn synchronize_text_editor_state<Message>(
    element: &Element<Message>,
    id: &UiId,
    state: &mut UiStateStore,
) {
    if element.text_mapper.is_some()
        && let Kind::Text { value, details, .. } = &element.kind
    {
        let initial = details
            .as_ref()
            .and_then(|details| details.input_value.clone())
            .unwrap_or_else(|| value.clone());
        let editor = state.editor(id.clone(), &initial);
        if editor.text() != initial {
            editor.set_text(initial);
        }
    }
}

fn dropdown_projection_needed<Message>(
    element: &Element<Message>,
    id: &UiId,
    state: &UiStateStore,
    interaction_index: Option<usize>,
) -> bool {
    let Kind::Dropdown(dropdown) = &element.kind else {
        return false;
    };
    let retained = state.state(id);
    let generation_opens = retained.map_or(dropdown.open_generation != 0, |transient| {
        transient.dropdown_open_generation != dropdown.open_generation
    });
    let expanded = generation_opens || retained.is_some_and(|transient| transient.dropdown_open);
    if expanded != dropdown.expanded {
        return true;
    }
    if dropdown.presentation.is_none() {
        return false;
    }
    interaction_index.is_some()
        || (0..dropdown.options.len()).any(|index| {
            let option = id.scoped(format!("option-{index}"));
            state.pressed() == Some(&option)
                || state.hovered() == Some(&option)
                || state.focused() == Some(&option)
                || state.navigation().controller_selected() == Some(&option)
        })
}

fn vertical_scroll_projection_needed<Message>(
    element: &Element<Message>,
    id: &UiId,
    state: &UiStateStore,
) -> bool {
    let Kind::VerticalScroll {
        offset,
        controlled: false,
    } = element.kind
    else {
        return false;
    };
    let effective = state
        .state(id)
        .map_or(0.0, |transient| transient.scroll_offset)
        .max(0.0);
    effective.to_bits() != offset.to_bits()
}

pub(super) fn apply_transient_state<Message>(
    element: &mut Element<Message>,
    id: &UiId,
    state: &mut UiStateStore,
) {
    apply_transient_state_with_parent(element, id, state, None, None);
}

fn style_owned_bytes(style: &Style) -> usize {
    style.details().map_or(0, |details| {
        std::mem::size_of::<StyleDetails>()
            + details
                .accessibility_label
                .as_ref()
                .map_or(0, String::capacity)
            + details
                .accessibility_description
                .as_ref()
                .map_or(0, String::capacity)
            + details
                .accessibility_role
                .as_ref()
                .map_or(0, String::capacity)
            + details
                .accessibility_state
                .as_ref()
                .map_or(0, String::capacity)
            + details
                .selection_run_id
                .as_ref()
                .map_or(0, String::capacity)
            + details
                .interaction_paints
                .as_ref()
                .map_or(0, |value| std::mem::size_of_val(value.as_ref()))
            + details
                .editing_parts
                .as_ref()
                .map_or(0, |value| std::mem::size_of_val(value.as_ref()))
            + details
                .editing_menu
                .as_ref()
                .map_or(0, |value| std::mem::size_of_val(value.as_ref()))
            + details
                .scrollbar_parts
                .as_ref()
                .map_or(0, |value| std::mem::size_of_val(value.as_ref()))
    })
}

fn style_clone_cost(style: &Style) -> (usize, usize) {
    let allocations = style.details().map_or(0, |details| {
        1 + [
            details.accessibility_label.as_ref(),
            details.accessibility_description.as_ref(),
            details.accessibility_role.as_ref(),
            details.accessibility_state.as_ref(),
            details.selection_run_id.as_ref(),
        ]
        .into_iter()
        .flatten()
        .filter(|value| value.capacity() != 0)
        .count()
            + usize::from(details.interaction_paints.is_some())
            + usize::from(details.editing_parts.is_some())
            + usize::from(details.editing_menu.is_some())
            + usize::from(details.scrollbar_parts.is_some())
    });
    (
        std::mem::size_of::<Style>() + style_owned_bytes(style),
        allocations,
    )
}

fn kind_owned_bytes(kind: &Kind) -> usize {
    match kind {
        Kind::Grid {
            columns: GridColumnSpec::Tracks(tracks),
        } => tracks.capacity() * std::mem::size_of::<Track>(),
        Kind::CustomPaintCommands { commands } => {
            commands.capacity() * std::mem::size_of::<PaintCommand>()
        }
        Kind::Text { value, details, .. } => {
            value.capacity()
                + details.as_ref().map_or(0, |details| {
                    std::mem::size_of::<TextDetails>()
                        + details.input_value.as_ref().map_or(0, String::capacity)
                        + details
                            .input_placeholder
                            .as_ref()
                            .map_or(0, String::capacity)
                })
        }
        Kind::StyledText(styled) => {
            std::mem::size_of::<StyledTextKind>()
                + styled.value.capacity()
                + styled.spans.capacity() * std::mem::size_of::<StyledTextSpan>()
        }
        Kind::Slider { presentation, .. } => presentation
            .as_ref()
            .map_or(0, |value| std::mem::size_of_val(value.as_ref())),
        Kind::Dropdown(dropdown) => {
            std::mem::size_of::<DropdownKind>()
                + dropdown.selected.capacity()
                + dropdown.options.capacity() * std::mem::size_of::<String>()
                + dropdown.options.iter().map(String::capacity).sum::<usize>()
                + dropdown
                    .presentation
                    .as_ref()
                    .map_or(0, |value| std::mem::size_of_val(value.as_ref()))
                + dropdown.details.as_ref().map_or(0, |details| {
                    std::mem::size_of::<DropdownDetails>()
                        + details.option_presentations.capacity()
                            * std::mem::size_of::<DropdownPartStyle>()
                        + details.resolved_options.capacity()
                            * std::mem::size_of::<DropdownPartStyle>()
                        + details.option_icons.capacity()
                            * std::mem::size_of::<Option<(u16, Arc<RgbaImage>, u64)>>()
                })
        }
        Kind::Flex(_)
        | Kind::Layer
        | Kind::VerticalScroll { .. }
        | Kind::Grid { .. }
        | Kind::CustomPaint { .. }
        | Kind::Image { .. } => 0,
    }
}

fn kind_clone_cost(kind: &Kind) -> (usize, usize) {
    let allocated = |capacity: usize| usize::from(capacity != 0);
    let allocations = match kind {
        Kind::Grid {
            columns: GridColumnSpec::Tracks(tracks),
        } => allocated(tracks.capacity()),
        Kind::CustomPaintCommands { commands } => allocated(commands.capacity()),
        Kind::Text { value, details, .. } => {
            allocated(value.capacity())
                + details.as_ref().map_or(0, |details| {
                    1 + details
                        .input_value
                        .as_ref()
                        .map_or(0, |value| allocated(value.capacity()))
                        + details
                            .input_placeholder
                            .as_ref()
                            .map_or(0, |value| allocated(value.capacity()))
                })
        }
        Kind::StyledText(styled) => {
            1 + allocated(styled.value.capacity()) + allocated(styled.spans.capacity())
        }
        Kind::Slider { presentation, .. } => usize::from(presentation.is_some()),
        Kind::Dropdown(dropdown) => {
            1 + allocated(dropdown.selected.capacity())
                + allocated(dropdown.options.capacity())
                + dropdown
                    .options
                    .iter()
                    .map(|value| allocated(value.capacity()))
                    .sum::<usize>()
                + usize::from(dropdown.presentation.is_some())
                + dropdown.details.as_ref().map_or(0, |details| {
                    1 + allocated(details.option_presentations.capacity())
                        + allocated(details.resolved_options.capacity())
                        + allocated(details.option_icons.capacity())
                })
        }
        Kind::Flex(_)
        | Kind::Layer
        | Kind::VerticalScroll { .. }
        | Kind::Grid { .. }
        | Kind::CustomPaint { .. }
        | Kind::Image { .. } => 0,
    };
    (
        std::mem::size_of::<Kind>() + kind_owned_bytes(kind),
        allocations,
    )
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct TransientStylePatch {
    background: Option<Option<Background>>,
    border: Option<Option<Color>>,
    border_width: Option<f32>,
    foreground: Option<Option<Color>>,
    corner_radius: Option<f32>,
    width: Option<Length>,
    height: Option<Length>,
    scroll_offset_x: Option<f32>,
    scroll_offset: Option<f32>,
}

impl TransientStylePatch {
    fn between(canonical: &Style, effective: &Style) -> Option<Self> {
        let patch = Self {
            background: (canonical.background != effective.background)
                .then_some(effective.background),
            border: (canonical.border != effective.border).then_some(effective.border),
            border_width: (canonical.border_width != effective.border_width)
                .then_some(effective.border_width),
            foreground: (canonical.foreground != effective.foreground)
                .then_some(effective.foreground),
            corner_radius: (canonical.corner_radius != effective.corner_radius)
                .then_some(effective.corner_radius),
            width: (canonical.width != effective.width).then_some(effective.width),
            height: (canonical.height != effective.height).then_some(effective.height),
            scroll_offset_x: (canonical.scroll_offset_x != effective.scroll_offset_x)
                .then_some(effective.scroll_offset_x),
            scroll_offset: (canonical.scroll_offset != effective.scroll_offset)
                .then_some(effective.scroll_offset),
        };
        (patch != Self::default()).then_some(patch)
    }

    fn merge(&mut self, newer: Self) {
        if newer.background.is_some() {
            self.background = newer.background;
        }
        if newer.border.is_some() {
            self.border = newer.border;
        }
        if newer.border_width.is_some() {
            self.border_width = newer.border_width;
        }
        if newer.foreground.is_some() {
            self.foreground = newer.foreground;
        }
        if newer.corner_radius.is_some() {
            self.corner_radius = newer.corner_radius;
        }
        if newer.width.is_some() {
            self.width = newer.width;
        }
        if newer.height.is_some() {
            self.height = newer.height;
        }
        if newer.scroll_offset_x.is_some() {
            self.scroll_offset_x = newer.scroll_offset_x;
        }
        if newer.scroll_offset.is_some() {
            self.scroll_offset = newer.scroll_offset;
        }
    }
}

#[derive(Clone, Copy)]
struct TransientStyleValues {
    background: Option<Background>,
    border: Option<Color>,
    border_width: f32,
    foreground: Option<Color>,
    corner_radius: f32,
    width: Length,
    height: Length,
    scroll_offset_x: f32,
    scroll_offset: f32,
}

impl TransientStyleValues {
    fn canonical(style: &Style) -> Self {
        Self {
            background: style.background,
            border: style.border,
            border_width: style.border_width,
            foreground: style.foreground,
            corner_radius: style.corner_radius,
            width: style.width,
            height: style.height,
            scroll_offset_x: style.scroll_offset_x,
            scroll_offset: style.scroll_offset,
        }
    }

    fn patch(self, style: &Style) -> Option<TransientStylePatch> {
        let patch = TransientStylePatch {
            background: (style.background != self.background).then_some(self.background),
            border: (style.border != self.border).then_some(self.border),
            border_width: (style.border_width != self.border_width).then_some(self.border_width),
            foreground: (style.foreground != self.foreground).then_some(self.foreground),
            corner_radius: (style.corner_radius != self.corner_radius)
                .then_some(self.corner_radius),
            width: (style.width != self.width).then_some(self.width),
            height: (style.height != self.height).then_some(self.height),
            scroll_offset_x: (style.scroll_offset_x.to_bits() != self.scroll_offset_x.to_bits())
                .then_some(self.scroll_offset_x),
            scroll_offset: (style.scroll_offset.to_bits() != self.scroll_offset.to_bits())
                .then_some(self.scroll_offset),
        };
        (patch != TransientStylePatch::default()).then_some(patch)
    }
}

#[derive(Clone, Copy)]
pub(super) struct EffectiveStyle<'a> {
    canonical: &'a Style,
    patch: Option<&'a TransientStylePatch>,
}

impl<'a> EffectiveStyle<'a> {
    pub(super) const fn canonical(style: &'a Style) -> Self {
        Self {
            canonical: style,
            patch: None,
        }
    }

    fn patched(style: &'a Style, patch: Option<&'a TransientStylePatch>) -> Self {
        Self {
            canonical: style,
            patch,
        }
    }

    pub(super) fn background(self) -> Option<Background> {
        self.patch
            .and_then(|patch| patch.background)
            .unwrap_or(self.canonical.background)
    }

    pub(super) fn border(self) -> Option<Color> {
        self.patch
            .and_then(|patch| patch.border)
            .unwrap_or(self.canonical.border)
    }

    pub(super) fn border_width(self) -> f32 {
        self.patch
            .and_then(|patch| patch.border_width)
            .unwrap_or(self.canonical.border_width)
    }

    pub(super) fn foreground(self) -> Option<Color> {
        self.patch
            .and_then(|patch| patch.foreground)
            .unwrap_or(self.canonical.foreground)
    }

    pub(super) fn corner_radius(self) -> f32 {
        self.patch
            .and_then(|patch| patch.corner_radius)
            .unwrap_or(self.canonical.corner_radius)
    }

    pub(super) fn width(self) -> Length {
        self.patch
            .and_then(|patch| patch.width)
            .unwrap_or(self.canonical.width)
    }

    pub(super) fn height(self) -> Length {
        self.patch
            .and_then(|patch| patch.height)
            .unwrap_or(self.canonical.height)
    }

    pub(super) fn scroll_offset_x(self) -> f32 {
        self.patch
            .and_then(|patch| patch.scroll_offset_x)
            .unwrap_or(self.canonical.scroll_offset_x)
    }

    pub(super) fn scroll_offset(self) -> f32 {
        self.patch
            .and_then(|patch| patch.scroll_offset)
            .unwrap_or(self.canonical.scroll_offset)
    }
}

impl std::ops::Deref for EffectiveStyle<'_> {
    type Target = Style;

    fn deref(&self) -> &Self::Target {
        self.canonical
    }
}

#[derive(Default)]
pub(super) struct TransientStateJournal {
    styles: std::collections::HashMap<UiId, Style>,
    style_patches: std::collections::HashMap<UiId, TransientStylePatch>,
    kinds: std::collections::HashMap<UiId, Kind>,
    vertical_scroll_offsets: std::collections::HashMap<UiId, f32>,
    dropdown_expanded: std::collections::HashMap<UiId, bool>,
    text_patches: std::collections::HashMap<UiId, TransientTextPatch>,
    text_format_patches: std::collections::HashMap<UiId, TransientTextFormatPatch>,
    slider_presentations: std::collections::HashMap<UiId, [ResolvedPartStyle; 3]>,
    phases: std::collections::HashMap<UiId, super::super::retained::DirtyPhases>,
    whole_payload_clone_bytes: usize,
    whole_payload_clone_allocations: usize,
    style_clone_bytes: usize,
    style_clone_allocations: usize,
    kind_clone_bytes: usize,
    kind_clone_allocations: usize,
}

#[derive(Debug)]
struct TransientTextPatch {
    value: String,
    input_value: String,
    selection_x: Option<(f32, f32)>,
    caret_position: Option<Point>,
}

#[derive(Clone, Copy, Debug, Default)]
struct TransientTextFormatPatch {
    scale: Option<f32>,
    line_height: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ResolvedPartStyle {
    pub(super) width: f32,
    pub(super) height: f32,
    pub(super) padding: Insets,
    pub(super) margin: Insets,
    pub(super) background: Option<Color>,
    pub(super) foreground: Option<Color>,
    pub(super) border_color: Option<Color>,
    pub(super) border_width: f32,
    pub(super) radius: f32,
    pub(super) font_size: f32,
    pub(super) line_height: f32,
}

impl From<DropdownPartStyle> for ResolvedPartStyle {
    fn from(style: DropdownPartStyle) -> Self {
        Self {
            width: style.width,
            height: style.height,
            padding: style.padding,
            margin: style.margin,
            background: style.background,
            foreground: style.foreground,
            border_color: style.border_color,
            border_width: style.border_width,
            radius: style.radius,
            font_size: style.font_size,
            line_height: style.line_height,
        }
    }
}

impl TransientStateJournal {
    pub(super) fn style_count(&self) -> usize {
        self.style_patches.len()
    }

    pub(super) fn kind_count(&self) -> usize {
        self.kinds.len()
            + self.vertical_scroll_offsets.len()
            + self.dropdown_expanded.len()
            + self.text_patches.len()
            + self.text_format_patches.len()
            + self.slider_presentations.len()
    }

    pub(super) fn whole_payload_clone_bytes(&self) -> usize {
        self.whole_payload_clone_bytes
    }

    pub(super) fn whole_payload_clone_allocations(&self) -> usize {
        self.whole_payload_clone_allocations
    }

    pub(super) fn style_clone_bytes(&self) -> usize {
        self.style_clone_bytes
    }

    pub(super) fn style_clone_allocations(&self) -> usize {
        self.style_clone_allocations
    }

    pub(super) fn kind_clone_bytes(&self) -> usize {
        self.kind_clone_bytes
    }

    pub(super) fn kind_clone_allocations(&self) -> usize {
        self.kind_clone_allocations
    }

    pub(super) fn storage_bytes(&self) -> usize {
        self.styles.capacity() * std::mem::size_of::<(UiId, Style)>()
            + self.style_patches.capacity() * std::mem::size_of::<(UiId, TransientStylePatch)>()
            + self.kinds.capacity() * std::mem::size_of::<(UiId, Kind)>()
            + self.vertical_scroll_offsets.capacity() * std::mem::size_of::<(UiId, f32)>()
            + self.dropdown_expanded.capacity() * std::mem::size_of::<(UiId, bool)>()
            + self.text_patches.capacity() * std::mem::size_of::<(UiId, TransientTextPatch)>()
            + self.text_format_patches.capacity()
                * std::mem::size_of::<(UiId, TransientTextFormatPatch)>()
            + self.slider_presentations.capacity()
                * std::mem::size_of::<(UiId, [ResolvedPartStyle; 3])>()
            + self.phases.capacity()
                * std::mem::size_of::<(UiId, super::super::retained::DirtyPhases)>()
            + self.styles.values().map(style_owned_bytes).sum::<usize>()
            + self.kinds.values().map(kind_owned_bytes).sum::<usize>()
            + self
                .text_patches
                .values()
                .map(|patch| patch.value.capacity() + patch.input_value.capacity())
                .sum::<usize>()
    }

    pub(super) fn table_allocations(&self) -> usize {
        usize::from(self.styles.capacity() != 0)
            + usize::from(self.style_patches.capacity() != 0)
            + usize::from(self.kinds.capacity() != 0)
            + usize::from(self.vertical_scroll_offsets.capacity() != 0)
            + usize::from(self.dropdown_expanded.capacity() != 0)
            + usize::from(self.text_patches.capacity() != 0)
            + usize::from(self.text_format_patches.capacity() != 0)
            + usize::from(self.slider_presentations.capacity() != 0)
            + usize::from(self.phases.capacity() != 0)
    }

    pub(super) fn affected_phases(
        &self,
    ) -> impl Iterator<Item = (&UiId, super::super::retained::DirtyPhases)> {
        self.phases.iter().map(|(id, phases)| (id, *phases))
    }

    /// Restores the canonical declaration while retaining the effective
    /// transient payloads in this side table. This creates the ownership
    /// boundary needed for resolution to read through the table without
    /// modifying the declaration it will publish.
    pub(super) fn detach_effective<Message>(&mut self, element: &mut Element<Message>, id: &UiId) {
        self.detach_effective_tree(element, id, true);
    }

    pub(super) fn attach_effective<Message>(&mut self, element: &mut Element<Message>, id: &UiId) {
        self.detach_effective_tree(element, id, false);
    }

    /// Drops the Stage A whole-style bridge after the final detach. All later
    /// consumers read the compact field patch against the canonical style.
    pub(super) fn release_staged_styles(&mut self) {
        self.styles = std::collections::HashMap::new();
    }

    fn detach_effective_tree<Message>(
        &mut self,
        element: &mut Element<Message>,
        id: &UiId,
        derive_patches: bool,
    ) {
        for (index, child) in element.children.iter_mut().enumerate() {
            let child_id = resolved_child_id(id, child, index);
            self.detach_effective_tree(child, &child_id, derive_patches);
        }
        let had_staged_style = self.styles.contains_key(id);
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
        if derive_patches && had_staged_style {
            if let Some(patch) = TransientStylePatch::between(&element.style, effective_style) {
                self.style_patches.insert(id.clone(), patch);
            } else {
                self.style_patches.remove(id);
            }
        }
        let effective_kind = self.kinds.get(id).unwrap_or(&element.kind);
        let mut dirty = super::super::retained::transient_dirty_phases(
            element,
            effective_style,
            effective_kind,
        );
        if let Some(patch) = self.style_patches.get(id) {
            use super::super::retained::DirtyPhases;
            if patch.width.is_some() || patch.height.is_some() {
                dirty.insert(DirtyPhases::MEASURE);
            }
            if patch.background.is_some()
                || patch.border.is_some()
                || patch.border_width.is_some()
                || patch.foreground.is_some()
                || patch.corner_radius.is_some()
            {
                dirty.insert(DirtyPhases::PAINT);
            }
            if patch.scroll_offset_x.is_some() || patch.scroll_offset.is_some() {
                dirty.insert(DirtyPhases::PLACE);
            }
        }
        if self.vertical_scroll_offsets.contains_key(id) {
            use super::super::retained::DirtyPhases;
            dirty.insert(
                DirtyPhases::PLACE
                    .union(DirtyPhases::PAINT)
                    .union(DirtyPhases::INTERACTION)
                    .union(DirtyPhases::SEMANTICS),
            );
        }
        if self.dropdown_expanded.contains_key(id) {
            use super::super::retained::DirtyPhases;
            dirty.insert(
                DirtyPhases::PAINT
                    .union(DirtyPhases::INTERACTION)
                    .union(DirtyPhases::SEMANTICS),
            );
        }
        if let Some(patch) = self.text_patches.get(id)
            && let Kind::Text { value, details, .. } = &element.kind
        {
            use super::super::retained::DirtyPhases;
            let canonical_details = details.as_deref();
            if patch.value != *value {
                dirty.insert(
                    DirtyPhases::MEASURE
                        .union(DirtyPhases::PLACE)
                        .union(DirtyPhases::PAINT),
                );
            }
            if canonical_details.and_then(|details| details.input_value.as_deref())
                != Some(patch.input_value.as_str())
            {
                dirty.insert(DirtyPhases::INTERACTION.union(DirtyPhases::SEMANTICS));
            }
            if canonical_details.and_then(|details| details.selection_x) != patch.selection_x
                || canonical_details.and_then(|details| details.caret_position)
                    != patch.caret_position
            {
                dirty.insert(DirtyPhases::PAINT);
            }
        }
        if self.text_format_patches.contains_key(id) {
            use super::super::retained::DirtyPhases;
            dirty.insert(
                DirtyPhases::MEASURE
                    .union(DirtyPhases::PLACE)
                    .union(DirtyPhases::PAINT),
            );
        }
        if self.slider_presentations.contains_key(id) {
            use super::super::retained::DirtyPhases;
            dirty.insert(
                DirtyPhases::MEASURE
                    .union(DirtyPhases::PLACE)
                    .union(DirtyPhases::PAINT)
                    .union(DirtyPhases::INTERACTION),
            );
        }
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
    ) -> EffectiveStyle<'a> {
        EffectiveStyle::patched(&element.style, self.style_patches.get(id))
    }

    pub(super) fn kind<'a, Message>(
        &'a self,
        id: &UiId,
        element: &'a Element<Message>,
    ) -> &'a Kind {
        self.kinds.get(id).unwrap_or(&element.kind)
    }

    pub(super) fn vertical_scroll_offset(&self, id: &UiId, canonical: f32) -> f32 {
        self.vertical_scroll_offsets
            .get(id)
            .copied()
            .unwrap_or(canonical)
    }

    pub(super) fn dropdown_expanded(&self, id: &UiId, canonical: bool) -> bool {
        self.dropdown_expanded.get(id).copied().unwrap_or(canonical)
    }

    pub(super) fn text_value<'a>(&'a self, id: &UiId, canonical: &'a str) -> &'a str {
        self.text_patches
            .get(id)
            .map_or(canonical, |patch| patch.value.as_str())
    }

    pub(super) fn text_input_value<'a>(
        &'a self,
        id: &UiId,
        canonical: Option<&'a str>,
    ) -> Option<&'a str> {
        self.text_patches
            .get(id)
            .map_or(canonical, |patch| Some(patch.input_value.as_str()))
    }

    pub(super) fn text_selection_x(
        &self,
        id: &UiId,
        canonical: Option<(f32, f32)>,
    ) -> Option<(f32, f32)> {
        self.text_patches
            .get(id)
            .map_or(canonical, |patch| patch.selection_x)
    }

    pub(super) fn text_caret_position(&self, id: &UiId, canonical: Option<Point>) -> Option<Point> {
        self.text_patches
            .get(id)
            .map_or(canonical, |patch| patch.caret_position)
    }

    pub(super) fn text_scale(&self, id: &UiId, canonical: f32) -> f32 {
        self.text_format_patches
            .get(id)
            .and_then(|patch| patch.scale)
            .unwrap_or(canonical)
    }

    pub(super) fn text_line_height(&self, id: &UiId, canonical: Option<f32>) -> Option<f32> {
        self.text_format_patches
            .get(id)
            .and_then(|patch| patch.line_height)
            .or(canonical)
    }

    pub(super) fn slider_presentation(&self, id: &UiId) -> Option<&[ResolvedPartStyle; 3]> {
        self.slider_presentations.get(id)
    }

    fn has_direct_slider_presentation(&self, id: &UiId) -> bool {
        self.slider_presentations.contains_key(id)
    }

    fn has_direct_text_patch(&self, id: &UiId) -> bool {
        self.text_patches.contains_key(id)
    }

    fn has_direct_dropdown_patch(&self, id: &UiId) -> bool {
        self.dropdown_expanded.contains_key(id)
    }

    fn has_direct_style_patch(&self, id: &UiId) -> bool {
        self.styles.get(id).is_none() && self.style_patches.contains_key(id)
    }
}

pub(super) fn apply_transient_state_staged<Message>(
    element: &mut Element<Message>,
    id: &UiId,
    state: &mut UiStateStore,
    overlay: &mut TransientStateJournal,
) {
    capture_transient_state(element, id, state, None, None, 1.0, overlay);
    apply_transient_state_with_parent(element, id, state, None, Some(overlay));
}

fn local_transient_style_patch<Message>(
    element: &Element<Message>,
    id: &UiId,
    state: &UiStateStore,
    direct_interaction: Option<usize>,
    interaction_index: Option<usize>,
    scope_background_active: bool,
) -> Option<TransientStylePatch> {
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
    if !owns_state {
        return None;
    }
    let mut effective = TransientStyleValues::canonical(&element.style);
    if scope_background_active && let Some(background) = element.style.controller_scope_background {
        effective.background = Some(background);
    }
    if direct_interaction == Some(1) {
        if let Some(background) = element.style.pressed_background {
            effective.background = Some(background);
        }
    } else if direct_interaction == Some(0)
        && let Some(background) = element.style.hover_background
    {
        effective.background = Some(background);
    }
    let exact_focus_background =
        (!scope_background_active && state.window_focused() && direct_interaction == Some(2))
            .then_some(element.style.focus_background)
            .flatten();
    if let Some(background) = exact_focus_background {
        effective.background = Some(background);
    }
    if let Some(index) = interaction_index {
        let paint = element
            .style
            .interaction_paints
            .as_ref()
            .map_or(InteractionPaint::default(), |paints| paints[index]);
        if let Some(color) = paint.background {
            effective.background = (color != 0).then_some(Background::Solid(color));
        }
        if let Some(color) = paint.border_color {
            effective.border = (color != 0).then_some(color);
        }
        if let Some(width) = paint.border_width {
            effective.border_width = width;
        }
        if let Some(radius) = paint.radius {
            effective.corner_radius = radius;
        }
        if let Some(width) = paint.width {
            effective.width = width;
        }
        if let Some(height) = paint.height {
            effective.height = height;
        }
        if let Some(color) = paint.foreground {
            effective.foreground = Some(color);
        }
    }
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
    } else if state.window_focused() && state.navigation().controller_selected() == Some(id) {
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
        element
            .style
            .focus_background_tint
            .or_else(|| (!transparent_editor).then_some(crate::theme::FALLBACK_KEYBOARD_FOCUS_CUE))
    } else {
        None
    };
    if let Some(tint) = active_focus_tint {
        let focus_foreground = element.style.foreground.or_else(|| {
            element
                .children
                .iter()
                .find_map(|child| focus_participating_foreground(child))
        });
        let transform = |color| {
            focus_foreground.map_or_else(
                || crate::focused_surface(color, tint),
                |foreground| crate::focused_surface_with_foreground(color, tint, foreground),
            )
        };
        effective.background = Some(match effective.background {
            Some(Background::Solid(color)) => Background::Solid(transform(color)),
            Some(Background::LinearGradient(mut gradient)) => {
                gradient.start = transform(gradient.start);
                gradient.end = transform(gradient.end);
                Background::LinearGradient(gradient)
            }
            None => Background::Solid(transform(crate::theme::FALLBACK_FOCUS_SURFACE)),
        });
    }
    if matches!(element.style.overflow_y, Overflow::Scroll | Overflow::Auto) {
        let retained = state.state(id);
        let scroll_at_end = retained.is_none_or(|state| state.scroll_at_end);
        effective.scroll_offset = if element.style.follow_scroll_end && scroll_at_end {
            f32::MAX
        } else {
            retained.map_or(0.0, |state| state.scroll_offset).max(0.0)
        };
    }
    if matches!(element.style.overflow_x, Overflow::Scroll | Overflow::Auto) {
        effective.scroll_offset_x = state
            .state(id)
            .map_or(0.0, |state| state.scroll_offset_x)
            .max(0.0);
    }
    effective.patch(&element.style)
}

fn merged_text_paint(
    inherited: Option<InteractionPaint>,
    local: Option<InteractionPaint>,
) -> Option<InteractionPaint> {
    match (inherited, local) {
        (None, None) => None,
        (Some(paint), None) | (None, Some(paint)) => Some(paint),
        (Some(inherited), Some(local)) => Some(InteractionPaint {
            background: local.background.or(inherited.background),
            foreground: local.foreground.or(inherited.foreground),
            border_color: local.border_color.or(inherited.border_color),
            border_width: local.border_width.or(inherited.border_width),
            radius: local.radius.or(inherited.radius),
            font_size: local.font_size.or(inherited.font_size),
            line_height: local.line_height.or(inherited.line_height),
            width: local.width.or(inherited.width),
            height: local.height.or(inherited.height),
        }),
    }
}

fn capture_transient_state<Message>(
    element: &mut Element<Message>,
    id: &UiId,
    state: &mut UiStateStore,
    parent_interaction: Option<usize>,
    inherited_text_paint: Option<InteractionPaint>,
    inherited_geometry_scale: f32,
    overlay: &mut TransientStateJournal,
) {
    synchronize_text_editor_state(element, id, state);
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
    let direct_dropdown_supported =
        matches!(&element.kind, Kind::Dropdown(dropdown) if dropdown.presentation.is_none());
    let direct_dropdown = if let Kind::Dropdown(dropdown) = &element.kind
        && direct_dropdown_supported
    {
        let transient = state.touch(id.clone());
        if dropdown.open_generation != transient.dropdown_open_generation {
            transient.dropdown_open_generation = dropdown.open_generation;
            transient.dropdown_open = true;
        }
        let expanded = transient.dropdown_open;
        if expanded != dropdown.expanded {
            overlay.dropdown_expanded.insert(id.clone(), expanded);
            if !element.style.css_paint {
                overlay.style_patches.entry(id.clone()).or_default().height =
                    Some(Length::Px(if dropdown.overlay {
                        30.0
                    } else if expanded {
                        42.0 + dropdown.options.len() as f32 * 36.0
                    } else {
                        42.0
                    }));
            }
            true
        } else {
            overlay.dropdown_expanded.remove(id);
            false
        }
    } else {
        false
    };
    let dropdown_height_changes = matches!(element.kind, Kind::Dropdown(_))
        && !element.style.css_paint
        && !direct_dropdown_supported;
    if inherited_geometry_scale.to_bits() != 1.0f32.to_bits() {
        let patch = overlay.style_patches.entry(id.clone()).or_default();
        if let Length::Px(width) = element.style.width {
            patch.width = Some(Length::Px(width * inherited_geometry_scale));
        }
        if let Length::Px(height) = element.style.height {
            patch.height = Some(Length::Px(height * inherited_geometry_scale));
        }
    }
    if let Some(local_patch) = local_transient_style_patch(
        element,
        id,
        state,
        direct_interaction,
        interaction_index,
        scope_background_active,
    ) {
        overlay
            .style_patches
            .entry(id.clone())
            .or_default()
            .merge(local_patch);
    }
    let local_text_paint = interaction_index.map(|index| {
        element
            .style
            .interaction_paints
            .as_ref()
            .map_or(InteractionPaint::default(), |paints| paints[index])
    });
    let effective_text_paint = merged_text_paint(inherited_text_paint, local_text_paint);
    if let Kind::Text {
        scale, line_height, ..
    } = &element.kind
    {
        let mut effective_scale = *scale;
        let mut effective_line_height = *line_height;
        let mut effective_foreground = element.style.foreground;
        if let Some(paint) = inherited_text_paint {
            if element.style.inherited_state_text[0]
                && let Some(color) = paint.foreground
            {
                effective_foreground = Some(color);
            }
            if element.style.inherited_state_text[1]
                && let Some(size) = paint.font_size
            {
                effective_scale = -size;
            }
            if element.style.inherited_state_text[2]
                && let Some(height) = paint.line_height
            {
                effective_line_height = Some(height);
            }
        }
        if let Some(paint) = local_text_paint {
            if let Some(color) = paint.foreground {
                effective_foreground = Some(color);
            }
            if let Some(size) = paint.font_size {
                effective_scale = -size;
            }
            if let Some(height) = paint.line_height {
                effective_line_height = Some(height);
            }
        }
        if effective_foreground != element.style.foreground {
            overlay
                .style_patches
                .entry(id.clone())
                .or_default()
                .foreground = Some(effective_foreground);
        }
        let format_patch = TransientTextFormatPatch {
            scale: (*scale != effective_scale).then_some(effective_scale),
            line_height: (*line_height != effective_line_height)
                .then_some(effective_line_height.unwrap_or_default()),
        };
        if format_patch.scale.is_some() || format_patch.line_height.is_some() {
            overlay.text_format_patches.insert(id.clone(), format_patch);
        } else {
            overlay.text_format_patches.remove(id);
        }
    } else {
        overlay.text_format_patches.remove(id);
        let mut effective_foreground = element.style.foreground;
        if let Some(paint) = inherited_text_paint
            && element.style.inherited_state_text[0]
            && let Some(color) = paint.foreground
        {
            effective_foreground = Some(color);
        }
        if let Some(paint) = local_text_paint
            && let Some(color) = paint.foreground
        {
            effective_foreground = Some(color);
        }
        if effective_foreground != element.style.foreground {
            overlay
                .style_patches
                .entry(id.clone())
                .or_default()
                .foreground = Some(effective_foreground);
        }
    }
    if direct_dropdown
        && !element.style.css_paint
        && let Kind::Dropdown(dropdown) = &element.kind
    {
        let expanded = overlay.dropdown_expanded(id, dropdown.expanded);
        overlay.style_patches.entry(id.clone()).or_default().height =
            Some(Length::Px(if dropdown.overlay {
                30.0
            } else if expanded {
                42.0 + dropdown.options.len() as f32 * 36.0
            } else {
                42.0
            }));
    }
    let style_can_change = dropdown_height_changes;
    if style_can_change {
        if !overlay.styles.contains_key(id) {
            let original = std::mem::take(&mut element.style);
            element.style = original.clone();
            let (bytes, allocations) = style_clone_cost(&element.style);
            overlay.whole_payload_clone_bytes =
                overlay.whole_payload_clone_bytes.saturating_add(bytes);
            overlay.whole_payload_clone_allocations = overlay
                .whole_payload_clone_allocations
                .saturating_add(allocations);
            overlay.style_clone_bytes = overlay.style_clone_bytes.saturating_add(bytes);
            overlay.style_clone_allocations =
                overlay.style_clone_allocations.saturating_add(allocations);
            overlay.styles.insert(id.clone(), original);
        }
    }
    let text_projection_changes = text_projection_needed(element, id, state);
    if text_projection_changes && let Some(patch) = project_text_state(element, id, state) {
        overlay.text_patches.insert(id.clone(), patch);
    } else {
        overlay.text_patches.remove(id);
    }
    let dropdown_projection_changes =
        !direct_dropdown && dropdown_projection_needed(element, id, state, interaction_index);
    let vertical_scroll_projection_changes = vertical_scroll_projection_needed(element, id, state);
    if vertical_scroll_projection_changes {
        let offset = state
            .state(id)
            .map_or(0.0, |transient| transient.scroll_offset)
            .max(0.0);
        overlay.vertical_scroll_offsets.insert(id.clone(), offset);
    } else {
        overlay.vertical_scroll_offsets.remove(id);
    }
    if let Kind::Slider {
        presentation: Some(parts),
        ..
    } = &element.kind
        && let Some(index) = direct_interaction
    {
        overlay.slider_presentations.insert(
            id.clone(),
            (*parts.as_ref()).map(|part| part.with_interaction(index).into()),
        );
    } else {
        overlay.slider_presentations.remove(id);
    }
    if dropdown_projection_changes {
        if !overlay.kinds.contains_key(id) {
            let original = std::mem::replace(&mut element.kind, Kind::Layer);
            element.kind = original.clone();
            let (bytes, allocations) = kind_clone_cost(&element.kind);
            overlay.whole_payload_clone_bytes =
                overlay.whole_payload_clone_bytes.saturating_add(bytes);
            overlay.whole_payload_clone_allocations = overlay
                .whole_payload_clone_allocations
                .saturating_add(allocations);
            overlay.kind_clone_bytes = overlay.kind_clone_bytes.saturating_add(bytes);
            overlay.kind_clone_allocations =
                overlay.kind_clone_allocations.saturating_add(allocations);
            overlay.kinds.insert(id.clone(), original);
        }
    }
    let child_text_paint = effective_text_paint;
    let magnified_child = element.style.proximity_magnification.and_then(|_| {
        let hovered = state.hovered()?.as_str();
        element
            .children
            .iter()
            .enumerate()
            .find_map(|(index, child)| {
                let child_id = resolved_child_id(id, child, index);
                (hovered == child_id.as_str()
                    || hovered.starts_with(&format!("{}/", child_id.as_str())))
                .then_some(index)
            })
    });
    for (index, child) in element.children.iter_mut().enumerate() {
        let child_id = resolved_child_id(id, child, index);
        let local_scale = element
            .style
            .proximity_magnification
            .map_or(1.0, |magnification| {
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
                state.animate_geometry_scale(
                    child_id.clone(),
                    target,
                    element.style.transition_duration_ms,
                )
            });
        capture_transient_state(
            child,
            &child_id,
            state,
            interaction_index,
            child_text_paint,
            inherited_geometry_scale * local_scale,
            overlay,
        );
    }
}

fn apply_transient_state_with_parent<Message>(
    element: &mut Element<Message>,
    id: &UiId,
    state: &mut UiStateStore,
    parent_interaction: Option<usize>,
    compact_overlay: Option<&TransientStateJournal>,
) {
    let direct_style_patch =
        compact_overlay.is_some_and(|overlay| overlay.has_direct_style_patch(id));
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
        if !direct_style_patch
            && scope_background_active
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
        if !direct_style_patch && state.pressed() == Some(id) {
            if let Some(background) = element.style.pressed_background {
                element.style.background = Some(background);
            }
        } else if !direct_style_patch
            && state.hovered() == Some(id)
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
        if !direct_style_patch && let Some(background) = exact_focus_background {
            element.style.background = Some(background);
        }
        if let Some(index) = interaction_index {
            let paint = element
                .style
                .interaction_paints
                .as_ref()
                .map_or(InteractionPaint::default(), |paints| paints[index]);
            if !direct_style_patch && let Some(color) = paint.background {
                element.style.background = (color != 0).then_some(Background::Solid(color));
            }
            if !direct_style_patch && let Some(color) = paint.border_color {
                element.style.border = (color != 0).then_some(color);
            }
            if !direct_style_patch && let Some(width) = paint.border_width {
                element.style.border_width = width;
            }
            if !direct_style_patch && let Some(radius) = paint.radius {
                element.style.corner_radius = radius;
            }
            if !direct_style_patch && let Some(width) = paint.width {
                element.style.width = width;
            }
            if !direct_style_patch && let Some(height) = paint.height {
                element.style.height = height;
            }
            fn text_paint<Message>(
                element: &mut Element<Message>,
                paint: InteractionPaint,
                owner: bool,
                mutate_owner_style: bool,
            ) {
                if let Some(color) = paint.foreground
                    && (owner || element.style.inherited_state_text[0])
                    && (!owner || mutate_owner_style)
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
                    text_paint(child, paint, false, true);
                }
            }
            if compact_overlay.is_none() {
                text_paint(element, paint, true, true);
            }
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
        if !direct_style_patch && let Some(tint) = active_focus_tint {
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
        synchronize_text_editor_state(element, id, state);
        match &mut element.kind {
            Kind::VerticalScroll {
                offset,
                controlled: false,
                ..
            } if compact_overlay.is_none() => *offset = scroll_offset.max(0.0),
            Kind::VerticalScroll { .. } => {}
            Kind::Slider { .. }
                if compact_overlay
                    .is_some_and(|overlay| overlay.has_direct_slider_presentation(id)) => {}
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
            Kind::Dropdown(_)
                if compact_overlay.is_some_and(|overlay| overlay.has_direct_dropdown_patch(id)) => {
            }
            Kind::Dropdown(dropdown) => {
                dropdown.expanded = dropdown_open;
                let resolve_presentation = interaction_index.is_some()
                    || (0..dropdown.options.len()).any(|index| {
                        let option = id.scoped(format!("option-{index}"));
                        state.pressed() == Some(&option)
                            || state.hovered() == Some(&option)
                            || state.focused() == Some(&option)
                            || state.navigation().controller_selected() == Some(&option)
                    });
                let owner_paint = interaction_index
                    .map(|index| {
                        element
                            .style
                            .interaction_paints
                            .as_ref()
                            .map_or(InteractionPaint::default(), |paints| paints[index])
                    })
                    .unwrap_or_default();
                if resolve_presentation && let Some(parts) = dropdown.presentation.as_mut() {
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
                if !direct_style_patch && !element.style.css_paint {
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
        if text_projection_needed(element, id, state)
            && !compact_overlay.is_some_and(|overlay| overlay.has_direct_text_patch(id))
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
        if !direct_style_patch
            && matches!(element.style.overflow_y, Overflow::Scroll | Overflow::Auto)
        {
            element.style.scroll_offset = if element.style.follow_scroll_end && scroll_at_end {
                f32::MAX
            } else {
                scroll_offset.max(0.0)
            };
        }
        if !direct_style_patch
            && matches!(element.style.overflow_x, Overflow::Scroll | Overflow::Auto)
        {
            element.style.scroll_offset_x = scroll_offset_x.max(0.0);
        }
    }
    let magnified_child = compact_overlay.is_none().then(|| ()).and_then(|_| {
        element.style.proximity_magnification?;
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
        if compact_overlay.is_none()
            && let Some(magnification) = element.style.proximity_magnification
        {
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
            compact_overlay,
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
    context: Option<(&UiId, &TransientStateJournal)>,
) -> Vec<Rect> {
    if children.is_empty() {
        return Vec::new();
    }
    let measured = children
        .iter()
        .enumerate()
        .map(|(index, child)| {
            measure_child_effective(
                child,
                context,
                index,
                Constraints::loose(match axis {
                    Axis::Horizontal => Size::new(f32::INFINITY, content.size.height),
                    Axis::Vertical => Size::new(content.size.width, f32::INFINITY),
                }),
            )
        })
        .collect::<Vec<_>>();
    let items = children
        .iter()
        .enumerate()
        .zip(&measured)
        .map(|((index, child), measured)| {
            let style = effective_child_style(child, context, index);
            let (intrinsic, min, max) = match axis {
                Axis::Horizontal => (measured.width, style.min_width, style.max_width),
                Axis::Vertical => (measured.height, style.min_height, style.max_height),
            };
            let parent = match axis {
                Axis::Horizontal => content.size.width,
                Axis::Vertical => content.size.height,
            };
            let preferred = style.basis.resolve(
                parent,
                match axis {
                    Axis::Horizontal => style.width().resolve(parent, intrinsic),
                    Axis::Vertical => style.height().resolve(parent, intrinsic),
                },
            );
            FlexItem::flex(
                preferred,
                min.max(0.0),
                max.max(min.max(0.0)),
                style.grow,
                style.shrink,
            )
        })
        .collect::<Vec<_>>();
    let mut rects = layout_flex(content, axis, gap.max(0.0), &items);
    let resolved_measured = children
        .iter()
        .enumerate()
        .zip(&rects)
        .map(|((index, child), rect)| match axis {
            Axis::Horizontal => measure_child_effective(
                child,
                context,
                index,
                Constraints::loose(Size::new(rect.size.width, f32::INFINITY)),
            ),
            Axis::Vertical => measure_child_effective(
                child,
                context,
                index,
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
            .enumerate()
            .zip(&resolved_measured)
            .filter(|((index, child), _)| {
                effective_child_style(child, context, *index)
                    .align_self
                    .unwrap_or(align_items)
                    == Align::Baseline
            })
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
        let style = effective_child_style(child, context, index);
        let main_offset = leading + index as f32 * extra_gap;
        let alignment = style.align_self.unwrap_or(align_items);
        match axis {
            Axis::Horizontal => {
                rect.origin.x += main_offset;
                let cross = measured.height.min(content.size.height);
                if alignment != Align::Stretch
                    || (style.css_paint && style.height() != Length::Auto)
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
                if alignment != Align::Stretch || (style.css_paint && style.width() != Length::Auto)
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

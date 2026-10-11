//! Bounded CSS subset with inherited custom properties and typed native styles.
//!
//! This is not browser CSS. See the crate's [CSS subset reference](https://github.com/shortontech/twinkle/blob/main/crates/twinkle-presentation/docs/css.md)
//! for supported selectors, properties, values, limits, and host integration.

#![warn(missing_docs)]

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    sync::Arc,
};

use cssparser::{
    AtRuleParser, CowRcStr, DeclarationParser, ParseError, Parser, ParserState,
    QualifiedRuleParser, RuleBodyItemParser, RuleBodyParser, StyleSheetParser,
};
use twinkle::{
    Align, BoxShadow, Insets, Justify, Length, ProximityMagnification, ReadingDirection, TextAlign,
    Track,
};
use twinkle_theme::{Appearance, ThemePalette};

/// Maximum CSS source accepted by the generic parser; hosts may impose lower limits.
pub const MAX_CSS_SOURCE_BYTES: usize = 256 * 1024;

/// Generic native box display strategy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Display {
    /// Ordinary vertical/block flow for generic native boxes.
    #[default]
    Block,
    /// Flex layout, with direction selected by [`FlexDirection`].
    Flex,
    /// Grid layout using the supported grid track subset.
    Grid,
}

/// Main-axis direction for flex layout.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FlexDirection {
    /// Lay children along the horizontal axis.
    #[default]
    Row,
    /// Lay children along the vertical axis.
    Column,
}

/// Typed values resolved from matching CSS rules for one native component.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ControlStyle {
    /// Generic box layout mode.
    pub display: Option<Display>,
    /// Flex child direction.
    pub flex_direction: Option<FlexDirection>,
    /// Grid column tracks.
    pub grid_columns: Option<Vec<Track>>,
    /// Preferred width.
    pub width: Option<Length>,
    /// Preferred height.
    pub height: Option<Length>,
    /// Minimum width in logical pixels.
    pub min_width: Option<f32>,
    /// Maximum width in logical pixels.
    pub max_width: Option<f32>,
    /// Minimum height in logical pixels.
    pub min_height: Option<f32>,
    /// Maximum height in logical pixels.
    pub max_height: Option<f32>,
    /// Cross-axis child alignment.
    pub align_items: Option<Align>,
    /// Main-axis child distribution.
    pub justify_content: Option<Justify>,
    /// Flex shrink factor.
    pub shrink: Option<f32>,
    /// Flex basis.
    pub basis: Option<Length>,
    /// Content inset in logical pixels.
    pub padding: Option<Insets>,
    /// Outside spacing in logical pixels.
    pub margin: Option<Insets>,
    /// Border width in logical pixels.
    pub border_width: Option<f32>,
    /// Border color in Twinkle's `0xAARRGGBB` representation.
    pub border_color: Option<u32>,
    /// Native shadow parameters.
    pub box_shadow: Option<BoxShadow>,
    /// Native backdrop blur radius in logical pixels.
    pub backdrop_blur: Option<f32>,
    /// Sibling pointer-proximity magnification parameters.
    pub proximity_magnification: Option<ProximityMagnification>,
    /// Interaction transition duration in milliseconds.
    pub transition_duration_ms: Option<f32>,
    /// Corner radius in logical pixels.
    pub radius: Option<f32>,
    /// Text size in logical pixels.
    pub font_size: Option<f32>,
    /// Text line height in logical pixels.
    pub line_height: Option<f32>,
    /// Inline text alignment.
    pub text_align: Option<TextAlign>,
    /// Background color.
    pub background: Option<u32>,
    /// Foreground text color.
    pub color: Option<u32>,
    /// Tint for supported icon content.
    pub icon_color: Option<u32>,
    /// Flex grow factor.
    pub grow: Option<f32>,
    /// Spacing between children in logical pixels.
    pub gap: Option<f32>,
    /// Distance from the bottom edge for a granted native window surface.
    pub bottom: Option<f32>,
    /// Distance from the top edge for a granted native window surface.
    pub top: Option<f32>,
    pub(crate) inherited_text: [bool; 3],
    // Computed once for this immutable resolved style, shared by cached returns
    // and inherited text context. Never mutate a cached parent's variable map.
    pub(crate) custom_properties: Arc<HashMap<String, String>>,
    pub(crate) ancestors: Arc<Vec<(String, Option<String>, Option<String>)>>,
    pub(crate) context_token: StyleContextToken,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) struct StyleContextToken(u64);

#[derive(Clone, Debug)]
struct Selector {
    ancestors: Vec<Selector>,
    root: bool,
    kind: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    state: Option<InteractionState>,
}

/// Interaction state used to resolve hover/active/focus styles.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InteractionState {
    /// Pointer is hovering the interactive control.
    Hover,
    /// Control is actively pressed.
    Active,
    /// Control has focus.
    Focus,
}

impl InteractionState {
    const fn index(self) -> usize {
        match self {
            Self::Hover => 0,
            Self::Active => 1,
            Self::Focus => 2,
        }
    }
}

impl Selector {
    fn parse(source: &str) -> Result<Self, String> {
        let parts = source.split_ascii_whitespace().collect::<Vec<_>>();
        if parts.len() > 8 {
            return Err("CSS descendant selector exceeds eight parts".into());
        }
        if parts.len() > 1 {
            let mut selector = Self::parse(parts.last().unwrap())?;
            selector.ancestors = parts[..parts.len() - 1]
                .iter()
                .map(|part| Self::parse(part))
                .collect::<Result<_, _>>()?;
            return Ok(selector);
        }
        let mut parser = Parser::new(source);
        let mut selector = Self {
            ancestors: Vec::new(),
            root: false,
            kind: None,
            id: None,
            classes: Vec::new(),
            state: None,
        };
        while let Ok(token) = parser.next() {
            match token {
                cssparser::Token::Ident(name) if selector.kind.is_none() => {
                    selector.kind = Some(name.to_ascii_lowercase())
                }
                cssparser::Token::IDHash(name) if selector.id.is_none() => {
                    selector.id = Some(name.to_string())
                }
                cssparser::Token::Delim('.') => {
                    let name = parser
                        .expect_ident()
                        .map_err(|_| "class selector needs an identifier")?;
                    selector.classes.push(name.to_string());
                }
                cssparser::Token::Colon if selector.state.is_none() => {
                    let pseudo = parser.expect_ident();
                    if matches!(&pseudo, Ok(name) if name.eq_ignore_ascii_case("root"))
                        && selector.kind.is_none()
                        && selector.id.is_none()
                        && selector.classes.is_empty()
                    {
                        selector.root = true;
                        continue;
                    }
                    selector.state = Some(match pseudo {
                        Ok(name) if name.eq_ignore_ascii_case("hover") => InteractionState::Hover,
                        Ok(name) if name.eq_ignore_ascii_case("active") => InteractionState::Active,
                        Ok(name) if name.eq_ignore_ascii_case("focus") => InteractionState::Focus,
                        _ => return Err("unsupported plugin CSS pseudo-class".into()),
                    });
                }
                _ => return Err("unsupported plugin CSS selector".into()),
            }
        }
        if selector.root
            && (selector.kind.is_some()
                || selector.id.is_some()
                || !selector.classes.is_empty()
                || selector.state.is_some())
        {
            return Err(":root must be a standalone selector".into());
        }
        if !selector.root
            && selector.kind.is_none()
            && selector.id.is_none()
            && selector.classes.is_empty()
        {
            return Err("empty plugin CSS selector".into());
        }
        Ok(selector)
    }

    fn matches(
        &self,
        kind: &str,
        id: Option<&str>,
        class_name: Option<&str>,
        state: Option<InteractionState>,
        ancestors: &[(String, Option<String>, Option<String>)],
    ) -> bool {
        let leaf_matches = !self.root
            && self.state == state
            && self.kind.as_deref().is_none_or(|selector| selector == kind)
            && self
                .id
                .as_deref()
                .is_none_or(|selector| Some(selector) == id)
            && self.classes.iter().all(|class| {
                class_name
                    .is_some_and(|names| names.split_ascii_whitespace().any(|name| name == class))
            });
        if !leaf_matches {
            return false;
        }
        let mut remaining = ancestors;
        for required in self.ancestors.iter().rev() {
            let Some(index) = remaining.iter().rposition(|(kind, id, classes)| {
                required.matches(kind, id.as_deref(), classes.as_deref(), None, &[])
            }) else {
                return false;
            };
            remaining = &remaining[..index];
        }
        true
    }
}

#[derive(Clone, Debug)]
struct Rule {
    color_schemes: u8,
    selectors: Vec<Selector>,
    declarations: Vec<ParsedDeclaration>,
}

#[derive(Clone, Debug)]
enum ParsedDeclaration {
    Custom(String, String),
    DeferredProperty(String, String),
    Typed(Declaration),
}

#[derive(Clone, Debug)]
enum Declaration {
    Display(Display),
    FlexDirection(FlexDirection),
    GridColumns(Vec<Track>),
    Width(Length),
    Height(Length),
    MinWidth(f32),
    MaxWidth(f32),
    MinHeight(f32),
    MaxHeight(f32),
    AlignItems(Align),
    JustifyContent(Justify),
    Shrink(f32),
    Basis(Length),
    Flex(f32),
    Padding(Insets),
    Margin(Insets),
    BorderWidth(f32),
    BorderColor(u32),
    Radius(f32),
    FontSize(f32),
    LineHeight(f32),
    TextAlign(TextAlign),
    Background(u32),
    Color(u32),
    IconColor(u32),
    Grow(f32),
    Gap(f32),
    Bottom(f32),
    Top(f32),
    Border(f32, u32),
    BoxShadow(BoxShadow),
    BackdropBlur(f32),
    ProximityMagnification(ProximityMagnification),
    TransitionDuration(f32),
}

impl Declaration {
    fn allowed_in_interaction(&self) -> bool {
        matches!(
            self,
            Self::Background(_)
                | Self::Color(_)
                | Self::Border(_, _)
                | Self::BorderColor(_)
                | Self::BorderWidth(_)
                | Self::Radius(_)
                | Self::FontSize(_)
                | Self::LineHeight(_)
                | Self::Width(_)
                | Self::Height(_)
        )
    }

    const fn is_window_offset(&self) -> bool {
        matches!(self, Self::Bottom(_) | Self::Top(_))
    }

    fn apply(&self, style: &mut ControlStyle) {
        match self {
            Self::Display(value) => style.display = Some(*value),
            Self::FlexDirection(value) => style.flex_direction = Some(*value),
            Self::GridColumns(value) => style.grid_columns = Some(value.clone()),
            Self::Width(value) => style.width = Some(*value),
            Self::Height(value) => style.height = Some(*value),
            Self::MinWidth(value) => style.min_width = Some(*value),
            Self::MaxWidth(value) => style.max_width = Some(*value),
            Self::MinHeight(value) => style.min_height = Some(*value),
            Self::MaxHeight(value) => style.max_height = Some(*value),
            Self::AlignItems(value) => style.align_items = Some(*value),
            Self::JustifyContent(value) => style.justify_content = Some(*value),
            Self::Shrink(value) => style.shrink = Some(*value),
            Self::Basis(value) => style.basis = Some(*value),
            Self::Flex(value) => {
                style.grow = Some(*value);
                style.shrink = Some(1.0);
                style.basis = Some(Length::Percent(0.0));
            }
            Self::Padding(value) => style.padding = Some(*value),
            Self::Margin(value) => style.margin = Some(*value),
            Self::BorderWidth(value) => style.border_width = Some(*value),
            Self::BorderColor(value) => style.border_color = Some(*value),
            Self::BoxShadow(value) => style.box_shadow = Some(*value),
            Self::BackdropBlur(value) => style.backdrop_blur = Some(*value),
            Self::ProximityMagnification(value) => style.proximity_magnification = Some(*value),
            Self::TransitionDuration(value) => style.transition_duration_ms = Some(*value),
            Self::Radius(value) => style.radius = Some(*value),
            Self::FontSize(value) => style.font_size = Some(*value),
            Self::LineHeight(value) => style.line_height = Some(*value),
            Self::TextAlign(value) => style.text_align = Some(*value),
            Self::Background(value) => style.background = Some(*value),
            Self::Color(value) => style.color = Some(*value),
            Self::IconColor(value) => style.icon_color = Some(*value),
            Self::Grow(value) => style.grow = Some(*value),
            Self::Gap(value) => style.gap = Some(*value),
            Self::Bottom(value) => style.bottom = Some(*value),
            Self::Top(value) => style.top = Some(*value),
            Self::Border(width, color) => {
                style.border_width = Some(*width);
                style.border_color = Some(*color);
            }
        }
    }
}

impl ParsedDeclaration {
    fn apply(&self, properties: &HashMap<String, String>, style: &mut ControlStyle) {
        match self {
            Self::Typed(declaration) => declaration.apply(style),
            Self::DeferredProperty(name, value) => {
                if let Ok(value) = resolve_value(value, properties, &mut HashSet::new())
                    && let Ok(declaration) = declaration(name, &value)
                {
                    declaration.apply(style);
                }
            }
            Self::Custom(..) => {}
        }
    }

    fn allowed_in_interaction(&self) -> bool {
        match self {
            Self::Typed(declaration) => declaration.allowed_in_interaction(),
            Self::DeferredProperty(name, _) => matches!(
                name.as_str(),
                "background"
                    | "background-color"
                    | "color"
                    | "border"
                    | "border-color"
                    | "border-width"
                    | "border-radius"
                    | "font-size"
                    | "line-height"
                    | "width"
                    | "height"
            ),
            Self::Custom(..) => false,
        }
    }

    fn is_window_offset(&self) -> bool {
        match self {
            Self::Typed(declaration) => declaration.is_window_offset(),
            Self::DeferredProperty(name, _) => name == "bottom" || name == "top",
            Self::Custom(..) => false,
        }
    }
}

fn px(source: &str, maximum: f32) -> Result<f32, String> {
    let number = source.trim().strip_suffix("px").unwrap_or(source.trim());
    let value: f32 = number.parse().map_err(|_| "expected a CSS pixel length")?;
    if !value.is_finite()
        || !(0.0..=maximum).contains(&value)
        || (!source.trim().ends_with("px") && value != 0.0)
    {
        return Err("CSS length is outside the supported range".into());
    }
    Ok(value)
}

fn signed_px(source: &str, maximum: f32) -> Result<f32, String> {
    let number = source.trim().strip_suffix("px").unwrap_or(source.trim());
    let value: f32 = number.parse().map_err(|_| "expected a CSS pixel length")?;
    if !value.is_finite()
        || !(-maximum..=maximum).contains(&value)
        || (!source.trim().ends_with("px") && value != 0.0)
    {
        return Err("CSS length is outside the supported range".into());
    }
    Ok(value)
}

fn bounded_number(source: &str, maximum: f32, name: &str) -> Result<f32, String> {
    let value: f32 = source.parse().map_err(|_| format!("invalid {name}"))?;
    if !value.is_finite() || !(0.0..=maximum).contains(&value) {
        return Err(format!("{name} is outside the supported range"));
    }
    Ok(value)
}

fn length(source: &str) -> Result<Length, String> {
    let source = source.trim();
    match source {
        "auto" => Ok(Length::Auto),
        "min-content" => Ok(Length::MinContent),
        "max-content" => Ok(Length::MaxContent),
        _ if source.ends_with('%') => {
            let percent = bounded_number(&source[..source.len() - 1], 100.0, "percentage")?;
            Ok(Length::Percent(percent / 100.0))
        }
        _ => Ok(Length::Px(px(source, 8192.0)?)),
    }
}

fn top_level_parts(source: &str, delimiter: char) -> Result<Vec<&str>, String> {
    let mut depth = 0_u8;
    let mut start = 0;
    let mut parts = Vec::new();
    for (index, character) in source.char_indices() {
        match character {
            '(' => {
                depth = depth
                    .checked_add(1)
                    .filter(|depth| *depth <= 4)
                    .ok_or("CSS function nesting is too deep")?
            }
            ')' => depth = depth.checked_sub(1).ok_or("unbalanced CSS function")?,
            _ if depth == 0
                && (character == delimiter
                    || (delimiter == ' ' && character.is_ascii_whitespace())) =>
            {
                let part = source[start..index].trim();
                if !part.is_empty() {
                    parts.push(part);
                }
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err("unbalanced CSS function".into());
    }
    let last = source[start..].trim();
    if !last.is_empty() {
        parts.push(last);
    }
    Ok(parts)
}

fn track(source: &str) -> Result<Track, String> {
    let source = source.trim();
    if source == "auto" {
        return Ok(Track::Auto);
    }
    if let Some(number) = source.strip_suffix("fr") {
        return Ok(Track::Fraction(bounded_number(
            number,
            100.0,
            "grid fraction",
        )?));
    }
    if let Some(arguments) = source
        .strip_prefix("repeat(")
        .and_then(|value| value.strip_suffix(')'))
    {
        let parts = top_level_parts(arguments, ',')?;
        if parts.len() != 2 {
            return Err("repeat() needs a count and one track".into());
        }
        if parts[0] == "auto-fit" {
            return Ok(Track::repeat_auto_fit(track(parts[1])?));
        }
        let count: usize = parts[0].parse().map_err(|_| "invalid repeat() count")?;
        if !(1..=32).contains(&count) {
            return Err("repeat() count must be 1 to 32".into());
        }
        return Ok(Track::repeat(count, track(parts[1])?));
    }
    if let Some(arguments) = source
        .strip_prefix("minmax(")
        .and_then(|value| value.strip_suffix(')'))
    {
        let parts = top_level_parts(arguments, ',')?;
        if parts.len() != 2 {
            return Err("minmax() needs two tracks".into());
        }
        return Ok(Track::minmax(track(parts[0])?, track(parts[1])?));
    }
    Ok(Track::Px(px(source, 8192.0)?))
}

fn grid_columns(source: &str) -> Result<Vec<Track>, String> {
    let parts = top_level_parts(source, ' ')?;
    if parts.is_empty() || parts.len() > 32 {
        return Err("grid needs 1 to 32 column tracks".into());
    }
    let tracks = parts
        .into_iter()
        .map(track)
        .collect::<Result<Vec<_>, _>>()?;
    fn expanded_count(track: &Track) -> usize {
        match track {
            Track::Repeat(count, nested) => count.saturating_mul(expanded_count(nested)),
            _ => 1,
        }
    }
    if tracks.iter().map(expanded_count).sum::<usize>() > 32 {
        return Err("grid expands beyond 32 columns".into());
    }
    Ok(tracks)
}

fn insets(source: &str) -> Result<Insets, String> {
    let values = source
        .split_ascii_whitespace()
        .map(|value| px(value, 512.0))
        .collect::<Result<Vec<_>, _>>()?;
    let [top, right, bottom, left] = match values.as_slice() {
        [all] => [*all; 4],
        [vertical, horizontal] => [*vertical, *horizontal, *vertical, *horizontal],
        [top, horizontal, bottom] => [*top, *horizontal, *bottom, *horizontal],
        [top, right, bottom, left] => [*top, *right, *bottom, *left],
        _ => return Err("CSS inset shorthand needs one to four lengths".into()),
    };
    Ok(Insets {
        top,
        right,
        bottom,
        left,
    })
}

pub(crate) fn color(source: &str) -> Result<u32, String> {
    let source = source.trim();
    if source.eq_ignore_ascii_case("transparent") {
        return Ok(0);
    }
    if let Some(args) = source
        .strip_prefix("rgba(")
        .and_then(|args| args.strip_suffix(')'))
    {
        let values = args.split(',').map(str::trim).collect::<Vec<_>>();
        let [red, green, blue, alpha] = values.as_slice() else {
            return Err("rgba() needs four arguments".into());
        };
        let channel = |value: &str| -> Result<u32, String> {
            value
                .parse::<u8>()
                .map(u32::from)
                .map_err(|_| "invalid rgba() channel".into())
        };
        let alpha: f32 = alpha.parse().map_err(|_| "invalid rgba() alpha")?;
        if !alpha.is_finite() || !(0.0..=1.0).contains(&alpha) {
            return Err("rgba() alpha must be 0 to 1".into());
        }
        return Ok(((alpha * 255.0).round() as u32) << 24
            | channel(red)? << 16
            | channel(green)? << 8
            | channel(blue)?);
    }
    let hex = source
        .strip_prefix('#')
        .ok_or("CSS color must be #RGB, #RRGGBB, #RRGGBBAA, or transparent")?;
    let expand = |n: u32| (n << 4) | n;
    match hex.len() {
        3 => {
            let n = u32::from_str_radix(hex, 16).map_err(|_| "invalid CSS color")?;
            Ok(0xff00_0000
                | (expand((n >> 8) & 0xf) << 16)
                | (expand((n >> 4) & 0xf) << 8)
                | expand(n & 0xf))
        }
        6 => Ok(0xff00_0000 | u32::from_str_radix(hex, 16).map_err(|_| "invalid CSS color")?),
        8 => {
            let n = u32::from_str_radix(hex, 16).map_err(|_| "invalid CSS color")?;
            Ok(((n & 0xff) << 24) | (n >> 8))
        }
        _ => Err("invalid CSS color".into()),
    }
}

fn declaration(name: &str, value: &str) -> Result<Declaration, String> {
    #[cfg(test)]
    CSS_DECLARATION_PARSES.with(|count| count.set(count.get().saturating_add(1)));
    let value = value.trim();
    Ok(match name.to_ascii_lowercase().as_str() {
        "display" => Declaration::Display(match value {
            "block" => Display::Block,
            "flex" => Display::Flex,
            "grid" => Display::Grid,
            _ => return Err("display must be block, flex, or grid".into()),
        }),
        "flex-direction" => Declaration::FlexDirection(match value {
            "row" => FlexDirection::Row,
            "column" => FlexDirection::Column,
            _ => return Err("flex-direction must be row or column".into()),
        }),
        "grid-template-columns" => Declaration::GridColumns(grid_columns(value)?),
        "width" => Declaration::Width(length(value)?),
        "height" => Declaration::Height(length(value)?),
        "min-width" => Declaration::MinWidth(px(value, 8192.0)?),
        "max-width" => Declaration::MaxWidth(px(value, 8192.0)?),
        "min-height" => Declaration::MinHeight(px(value, 8192.0)?),
        "max-height" => Declaration::MaxHeight(px(value, 8192.0)?),
        "align-items" => Declaration::AlignItems(match value {
            "flex-start" | "start" => Align::Start,
            "center" => Align::Center,
            "flex-end" | "end" => Align::End,
            "stretch" => Align::Stretch,
            "baseline" => Align::Baseline,
            _ => return Err("unsupported align-items value".into()),
        }),
        "justify-content" => Declaration::JustifyContent(match value {
            "flex-start" | "start" => Justify::Start,
            "center" => Justify::Center,
            "flex-end" | "end" => Justify::End,
            "space-between" => Justify::SpaceBetween,
            "space-around" => Justify::SpaceAround,
            "space-evenly" => Justify::SpaceEvenly,
            _ => return Err("unsupported justify-content value".into()),
        }),
        "flex-shrink" => Declaration::Shrink(bounded_number(value, 100.0, "flex-shrink")?),
        "flex-basis" => Declaration::Basis(length(value)?),
        "flex" => Declaration::Flex(bounded_number(value, 100.0, "flex")?),
        "padding" => Declaration::Padding(insets(value)?),
        "margin" => Declaration::Margin(insets(value)?),
        "border-width" => Declaration::BorderWidth(px(value, 64.0)?),
        "border-color" => Declaration::BorderColor(color(value)?),
        "border" => {
            let mut parts = value.split_ascii_whitespace();
            let width = px(parts.next().ok_or("border needs a width")?, 64.0)?;
            if parts.next() != Some("solid") {
                return Err("only solid borders are supported".into());
            }
            let color = color(parts.next().ok_or("border needs a color")?)?;
            if parts.next().is_some() {
                return Err("border has too many values".into());
            }
            Declaration::Border(width, color)
        }
        "box-shadow" => {
            let mut parts = value.split_ascii_whitespace();
            let offset_x = signed_px(parts.next().ok_or("box-shadow needs an x offset")?, 256.0)?;
            let offset_y = signed_px(parts.next().ok_or("box-shadow needs a y offset")?, 256.0)?;
            let blur = px(parts.next().ok_or("box-shadow needs a blur radius")?, 128.0)?;
            let spread = px(
                parts.next().ok_or("box-shadow needs a spread radius")?,
                128.0,
            )?;
            let color = color(parts.next().ok_or("box-shadow needs a color")?)?;
            if parts.next().is_some() {
                return Err("box-shadow has too many values".into());
            }
            Declaration::BoxShadow(BoxShadow {
                offset_x,
                offset_y,
                blur,
                spread,
                color,
            })
        }
        "backdrop-filter" => {
            let inner = value
                .strip_prefix("blur(")
                .and_then(|value| value.strip_suffix(')'))
                .ok_or("backdrop-filter currently supports blur(<length>)")?;
            Declaration::BackdropBlur(px(inner.trim(), 128.0)?)
        }
        "-twinkle-proximity-magnification" => {
            let mut parts = value.split_ascii_whitespace();
            let maximum_scale = bounded_number(
                parts
                    .next()
                    .ok_or("proximity magnification needs a maximum scale")?,
                3.0,
                "proximity magnification",
            )?;
            if maximum_scale < 1.0 {
                return Err("proximity magnification scale must be at least 1".into());
            }
            let radius = parts
                .next()
                .ok_or("proximity magnification needs a sibling radius")?
                .parse::<usize>()
                .map_err(|_| "invalid proximity magnification radius")?;
            if radius > 8 || parts.next().is_some() {
                return Err("proximity magnification radius must be 0 to 8".into());
            }
            Declaration::ProximityMagnification(ProximityMagnification {
                maximum_scale,
                radius,
            })
        }
        "transition-duration" => {
            let milliseconds = value
                .strip_suffix("ms")
                .ok_or("transition-duration must use ms")?
                .trim()
                .parse::<f32>()
                .map_err(|_| "invalid transition-duration")?;
            if !milliseconds.is_finite() || !(0.0..=2_000.0).contains(&milliseconds) {
                return Err("transition-duration is outside the supported range".into());
            }
            Declaration::TransitionDuration(milliseconds)
        }
        "border-radius" => Declaration::Radius(px(value, 512.0)?),
        "font-size" => Declaration::FontSize(px(value, 256.0)?),
        "line-height" => Declaration::LineHeight(px(value, 512.0)?),
        "text-align" => Declaration::TextAlign(match value {
            "start" => TextAlign::Start,
            "center" => TextAlign::Center,
            "end" => TextAlign::End,
            _ => return Err("text-align must be start, center, or end".into()),
        }),
        "background-color" | "background" => Declaration::Background(color(value)?),
        "color" => Declaration::Color(color(value)?),
        "icon-color" => Declaration::IconColor(color(value)?),
        "flex-grow" => {
            let n: f32 = value.parse().map_err(|_| "invalid flex-grow")?;
            if !n.is_finite() || !(0.0..=100.0).contains(&n) {
                return Err("flex-grow is outside the supported range".into());
            }
            Declaration::Grow(n)
        }
        "gap" => Declaration::Gap(px(value, 512.0)?),
        "bottom" => Declaration::Bottom(px(value, 8192.0)?),
        "top" => Declaration::Top(px(value, 8192.0)?),
        _ => return Err(format!("unsupported plugin CSS property {name:?}")),
    })
}

struct CssRuleParser {
    depth: u8,
}

impl<'i> AtRuleParser<'i> for CssRuleParser {
    type Prelude = u8;
    type AtRule = Vec<Rule>;
    type Error = String;

    fn parse_prelude(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i>,
    ) -> Result<u8, ParseError<String>> {
        if !name.eq_ignore_ascii_case("media") || self.depth >= 8 {
            return Err(ParseError::custom(
                "unsupported or excessively nested CSS at-rule",
            ));
        }
        let mut schemes = 0;
        loop {
            input.expect_parenthesis_block()?;
            schemes |= input.parse_nested_block(|query| {
                query.expect_ident_matching("prefers-color-scheme")?;
                query.expect_colon()?;
                let value = query.expect_ident()?;
                let scheme = if value.eq_ignore_ascii_case("light") {
                    1
                } else if value.eq_ignore_ascii_case("dark") {
                    2
                } else {
                    return Err(ParseError::custom("unsupported color scheme"));
                };
                query.expect_exhausted()?;
                Ok(scheme)
            })?;
            if input.is_exhausted() {
                break;
            }
            input.expect_comma()?;
        }
        Ok(schemes)
    }

    fn parse_block(
        &mut self,
        schemes: u8,
        _start: &ParserState,
        input: &mut Parser<'i>,
    ) -> Result<Vec<Rule>, ParseError<String>> {
        let mut nested = CssRuleParser {
            depth: self.depth + 1,
        };
        let mut rules = Vec::new();
        for result in StyleSheetParser::new(input, &mut nested) {
            let mut children = result.map_err(|(error, ..)| error)?;
            for rule in &mut children {
                rule.color_schemes &= schemes;
            }
            rules.extend(children);
            if rules.len() > 512 {
                return Err(ParseError::custom(
                    "plugin stylesheet has more than 512 rules",
                ));
            }
        }
        Ok(rules)
    }
}

impl<'i> QualifiedRuleParser<'i> for CssRuleParser {
    type Prelude = Vec<Selector>;
    type QualifiedRule = Vec<Rule>;
    type Error = String;

    fn parse_prelude(
        &mut self,
        input: &mut Parser<'i>,
    ) -> Result<Self::Prelude, ParseError<Self::Error>> {
        let start = input.position();
        while input.next_including_whitespace_and_comments().is_ok() {}
        let selectors = input
            .slice_from(start)
            .split(',')
            .map(Selector::parse)
            .collect::<Result<Vec<_>, _>>()
            .map_err(ParseError::custom)?;
        if selectors.len() > 16 {
            return Err(ParseError::custom("too many selectors"));
        }
        Ok(selectors)
    }

    fn parse_block(
        &mut self,
        selectors: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i>,
    ) -> Result<Self::QualifiedRule, ParseError<Self::Error>> {
        let mut parser = CssDeclarationParser;
        let mut declarations = Vec::new();
        for result in RuleBodyParser::new(input, &mut parser) {
            match result {
                Ok(declaration) => declarations.push(declaration),
                Err((error, ..)) => return Err(error),
            }
            if declarations.len() > 64 {
                return Err(ParseError::custom("too many declarations"));
            }
        }
        if selectors.iter().any(|selector| selector.state.is_some()) {
            if !selectors.iter().all(|selector| selector.state.is_some()) {
                return Err(ParseError::custom(
                    "state selectors cannot share a rule with ordinary selectors",
                ));
            }
            if selectors.iter().any(|selector| {
                !matches!(
                    selector.kind.as_deref(),
                    Some(
                        "button"
                            | "text-field"
                            | "select"
                            | "select-header"
                            | "option"
                            | "menu-item"
                            | "menu-shortcut"
                            | "menu-indicator"
                            | "text-field-menu-shortcut"
                            | "text-field-menu-indicator"
                            | "select-indicator"
                            | "switch"
                            | "checkbox"
                            | "text-field-menu-item"
                            | "color-swatch"
                            | "color-swatch-fill"
                            | "color-swatch-label"
                            | "switch-track"
                            | "switch-thumb"
                            | "checkbox-box"
                            | "checkbox-mark"
                            | "slider"
                            | "slider-track"
                            | "slider-fill"
                            | "slider-thumb"
                            | "scrollbar-track"
                            | "scrollbar-thumb"
                    )
                )
            }) {
                return Err(ParseError::custom(
                    "plugin CSS state selectors require an interactive control part",
                ));
            }
            if declarations
                .iter()
                .any(|parsed| !parsed.allowed_in_interaction())
            {
                return Err(ParseError::custom(
                    "CSS interaction declarations require paint, typography, width, or height properties",
                ));
            }
        }
        if declarations.iter().any(ParsedDeclaration::is_window_offset)
            && selectors
                .iter()
                .any(|selector| selector.kind.as_deref() != Some("window"))
        {
            return Err(ParseError::custom(
                "top and bottom are supported only on window selectors",
            ));
        }
        Ok(vec![Rule {
            color_schemes: 3,
            selectors,
            declarations,
        }])
    }
}

struct CssDeclarationParser;
impl<'i> AtRuleParser<'i> for CssDeclarationParser {
    type Prelude = ();
    type AtRule = ParsedDeclaration;
    type Error = String;
}
impl<'i> DeclarationParser<'i> for CssDeclarationParser {
    type Declaration = ParsedDeclaration;
    type Error = String;
    fn parse_value(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i>,
        _start: &ParserState,
    ) -> Result<Self::Declaration, ParseError<Self::Error>> {
        let start = input.position();
        while input.next_including_whitespace_and_comments().is_ok() {}
        let name = name.to_string();
        let value = input.slice_from(start).trim().to_owned();
        if name.starts_with("--") {
            if name.len() <= 2 || name.len() > 128 || value.is_empty() || value.len() > 4096 {
                return Err(ParseError::custom("invalid CSS custom property"));
            }
            validate_var_syntax(&value).map_err(ParseError::custom)?;
            Ok(ParsedDeclaration::Custom(name, value))
        } else {
            let name = name.to_ascii_lowercase();
            validate_var_syntax(&value).map_err(ParseError::custom)?;
            if !value.contains("var(") {
                declaration(&name, &value).map_err(ParseError::custom)?;
            }
            if value.contains("var(") {
                Ok(ParsedDeclaration::DeferredProperty(name, value))
            } else {
                declaration(&name, &value)
                    .map(ParsedDeclaration::Typed)
                    .map_err(ParseError::custom)
            }
        }
    }
}
impl<'i> QualifiedRuleParser<'i> for CssDeclarationParser {
    type Prelude = ();
    type QualifiedRule = ParsedDeclaration;
    type Error = String;
}
impl<'i> RuleBodyItemParser<'i, ParsedDeclaration, String> for CssDeclarationParser {
    fn parse_declarations(&self) -> bool {
        true
    }
    fn parse_qualified(&self) -> bool {
        false
    }
}

fn var_call(source: &str, offset: usize) -> Result<(usize, &str, Option<&str>), String> {
    let arguments_start = offset + 4;
    let mut depth = 1_u8;
    let mut comma = None;
    for (relative, character) in source[arguments_start..].char_indices() {
        match character {
            '(' => {
                depth = depth
                    .checked_add(1)
                    .ok_or("CSS function nesting is too deep")?
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    let end = arguments_start + relative;
                    let split = comma.unwrap_or(end);
                    let name = source[arguments_start..split].trim();
                    if !name.starts_with("--") || name.len() <= 2 {
                        return Err("var() needs a custom property name".into());
                    }
                    let fallback = comma.map(|comma| source[comma + 1..end].trim());
                    if fallback.is_some_and(str::is_empty) {
                        return Err("var() fallback cannot be empty".into());
                    }
                    return Ok((end + 1, name, fallback));
                }
            }
            ',' if depth == 1 && comma.is_none() => comma = Some(arguments_start + relative),
            _ => {}
        }
        if depth > 8 {
            return Err("CSS function nesting is too deep".into());
        }
    }
    Err("unclosed var()".into())
}

fn validate_var_syntax(source: &str) -> Result<(), String> {
    let mut cursor = 0;
    while let Some(relative) = source[cursor..].find("var(") {
        let offset = cursor + relative;
        let (end, _, fallback) = var_call(source, offset)?;
        if let Some(fallback) = fallback {
            validate_var_syntax(fallback)?;
        }
        cursor = end;
    }
    Ok(())
}

fn resolve_value(
    source: &str,
    properties: &HashMap<String, String>,
    resolving: &mut HashSet<String>,
) -> Result<String, String> {
    if resolving.len() >= 32 {
        return Err("CSS custom property chain is too deep".into());
    }
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0;
    while let Some(relative) = source[cursor..].find("var(") {
        let offset = cursor + relative;
        output.push_str(&source[cursor..offset]);
        let (end, name, fallback) = var_call(source, offset)?;
        let replacement = if let Some(value) = properties.get(name) {
            if !resolving.insert(name.to_owned()) {
                return Err(format!("cyclic CSS custom property {name}"));
            }
            let result = resolve_value(value, properties, resolving);
            resolving.remove(name);
            result?
        } else if let Some(fallback) = fallback {
            resolve_value(fallback, properties, resolving)?
        } else {
            return Err(format!("undefined CSS custom property {name}"));
        };
        output.push_str(&replacement);
        if output.len() > 16 * 1024 {
            return Err("resolved CSS value is too large".into());
        }
        cursor = end;
    }
    output.push_str(&source[cursor..]);
    if output.len() > 16 * 1024 {
        return Err("resolved CSS value is too large".into());
    }
    Ok(output)
}

// A bounded cache belongs to one immutable rule set. Include inherited variables
// and the complete ancestor chain, so descendant selectors cannot leak styles.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
struct StyleCacheKey {
    kind: String,
    id: Option<String>,
    classes: Option<String>,
    inherited: Vec<(String, String)>,
    ancestors: Vec<(String, Option<String>, Option<String>)>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct StyleCacheFingerprint(u64);

struct StyleCacheLookup<'a> {
    kind: &'a str,
    id: Option<&'a str>,
    classes: Option<&'a str>,
    inherited: &'a HashMap<String, String>,
    ancestors: &'a [(String, Option<String>, Option<String>)],
    context_token: StyleContextToken,
}

impl<'a> StyleCacheLookup<'a> {
    #[cfg(test)]
    fn new(
        kind: &'a str,
        id: Option<&'a str>,
        classes: Option<&'a str>,
        properties: &'a HashMap<String, String>,
        ancestors: &'a [(String, Option<String>, Option<String>)],
    ) -> Self {
        Self::with_context(kind, id, classes, properties, ancestors, None)
    }

    fn with_context(
        kind: &'a str,
        id: Option<&'a str>,
        classes: Option<&'a str>,
        properties: &'a HashMap<String, String>,
        ancestors: &'a [(String, Option<String>, Option<String>)],
        context_token: Option<StyleContextToken>,
    ) -> Self {
        Self {
            kind,
            id,
            classes,
            inherited: properties,
            ancestors,
            context_token: context_token
                .unwrap_or_else(|| style_context_token(properties, ancestors)),
        }
    }

    fn fingerprint(&self) -> StyleCacheFingerprint {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        self.kind.hash(&mut hash);
        self.id.hash(&mut hash);
        self.classes.hash(&mut hash);
        self.context_token.hash(&mut hash);
        StyleCacheFingerprint(hash.finish())
    }

    fn to_owned(&self) -> StyleCacheKey {
        StyleCacheKey {
            kind: self.kind.into(),
            id: self.id.map(str::to_owned),
            classes: self.classes.map(str::to_owned),
            inherited: sorted_properties(self.inherited),
            ancestors: self.ancestors.to_vec(),
        }
    }
}

impl StyleCacheKey {
    fn matches(&self, lookup: &StyleCacheLookup<'_>) -> bool {
        self.kind == lookup.kind
            && self.id.as_deref() == lookup.id
            && self.classes.as_deref() == lookup.classes
            && self.ancestors.as_slice() == lookup.ancestors
            && self.inherited.len() == lookup.inherited.len()
            && self
                .inherited
                .iter()
                .all(|(key, value)| lookup.inherited.get(key) == Some(value))
    }

    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.kind.capacity()
            + self.id.as_ref().map_or(0, String::capacity)
            + self.classes.as_ref().map_or(0, String::capacity)
            + self.inherited.capacity() * std::mem::size_of::<(String, String)>()
            + self
                .inherited
                .iter()
                .map(|(k, v)| k.capacity() + v.capacity())
                .sum::<usize>()
            + self.ancestors.capacity()
                * std::mem::size_of::<(String, Option<String>, Option<String>)>()
            + self
                .ancestors
                .iter()
                .map(|(kind, id, classes)| {
                    kind.capacity()
                        + id.as_ref().map_or(0, String::capacity)
                        + classes.as_ref().map_or(0, String::capacity)
                })
                .sum::<usize>()
    }
}

fn sorted_properties(properties: &HashMap<String, String>) -> Vec<(String, String)> {
    #[cfg(test)]
    CSS_KEY_SORTS.with(|count| count.set(count.get().saturating_add(1)));
    let mut inherited = properties
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Vec<_>>();
    inherited.sort_unstable();
    inherited
}

fn style_context_token(
    properties: &HashMap<String, String>,
    ancestors: &[(String, Option<String>, Option<String>)],
) -> StyleContextToken {
    #[cfg(test)]
    CSS_FULL_CONTEXT_HASHES.with(|count| count.set(count.get().saturating_add(1)));
    let inherited = sorted_properties(properties);
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    inherited.hash(&mut hash);
    ancestors.hash(&mut hash);
    StyleContextToken(hash.finish())
}

#[cfg(test)]
thread_local! {
    static CSS_KEY_SORTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static CSS_FULL_CONTEXT_HASHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static CSS_DECLARATION_PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

const INTERACTION_CACHE_ENTRIES: usize = 2048;
const INTERACTION_CACHE_BYTES: usize = 2 * 1024 * 1024;

/// Opt-in work and timing counters for native CSS cache-key resolution.
/// Counters use interior `Cell` storage owned by the stylesheet; disabled
/// resolution performs no clock reads or atomic operations.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StyleCacheDiagnostics {
    /// Borrowed lookup keys constructed.
    pub key_builds: usize,
    /// Inherited-property collections sorted for owned/exact keys.
    pub key_sorts: usize,
    /// Complete inherited-property and ancestor contexts hashed.
    pub full_context_hashes: usize,
    /// Fingerprint table probes performed.
    pub cache_probes: usize,
    /// Probes accepted after exact key validation.
    pub cache_hits: usize,
    /// Candidate fingerprint matches validated against exact inputs.
    pub exact_checks: usize,
    /// Nanoseconds spent constructing borrowed keys and fingerprints.
    pub key_build_ns: u64,
    /// Nanoseconds spent probing fingerprint tables.
    pub cache_probe_ns: u64,
    /// Nanoseconds spent validating exact candidate keys.
    pub exact_check_ns: u64,
}

#[derive(Clone, Debug, Default)]
struct InteractionCache {
    // A fingerprint selects a candidate, never authorizes reuse by itself.
    // Collisions replace the candidate after exact input comparison.
    entries: HashMap<
        (StyleCacheFingerprint, InteractionState),
        (StyleCacheKey, twinkle::InteractionPaint),
    >,
    bytes: usize,
}

/// A compiled stylesheet and its bounded native style-resolution caches.
#[derive(Clone, Debug, Default)]
pub struct StyleSheet {
    rules: Vec<Rule>,
    palette: Option<ThemePalette>,
    uses_palette: bool,
    host_properties: HashMap<String, String>,
    reading_direction: ReadingDirection,
    resolved: RefCell<HashMap<StyleCacheFingerprint, (StyleCacheKey, ControlStyle)>>,
    interaction_rules: [Vec<usize>; 3],
    interactions: RefCell<InteractionCache>,
    diagnostics_enabled: std::cell::Cell<bool>,
    diagnostics: std::cell::Cell<StyleCacheDiagnostics>,
    #[cfg(test)]
    interaction_rule_visits: std::cell::Cell<usize>,
}

impl StyleSheet {
    /// Enables or disables CSS cache diagnostics and clears prior counters.
    pub fn set_cache_diagnostics_enabled(&self, enabled: bool) {
        self.diagnostics.set(StyleCacheDiagnostics::default());
        self.diagnostics_enabled.set(enabled);
    }

    /// Returns the counters accumulated since diagnostics were enabled/reset.
    pub fn cache_diagnostics(&self) -> StyleCacheDiagnostics {
        self.diagnostics.get()
    }

    fn update_cache_diagnostics(&self, update: impl FnOnce(&mut StyleCacheDiagnostics)) {
        if self.diagnostics_enabled.get() {
            let mut diagnostics = self.diagnostics.get();
            update(&mut diagnostics);
            self.diagnostics.set(diagnostics);
        }
    }

    /// Compile CSS using the default appearance palette.
    pub fn compile(source: &str) -> Result<Self, String> {
        Self::compile_with_palette(source, ThemePalette::from_appearance(Appearance::default()))
    }

    /// Compile CSS using an explicit palette for light/dark media evaluation.
    pub fn compile_with_palette(source: &str, palette: ThemePalette) -> Result<Self, String> {
        Self::compile_with_context(source, palette, HashMap::new())
    }

    /// Compile CSS with host-owned root custom properties.
    pub fn compile_with_host_properties(
        source: &str,
        properties: HashMap<String, String>,
    ) -> Result<Self, String> {
        Self::compile_with_context(
            source,
            ThemePalette::from_appearance(Appearance::default()),
            properties,
        )
    }

    fn compile_with_context(
        source: &str,
        palette: ThemePalette,
        host_properties: HashMap<String, String>,
    ) -> Result<Self, String> {
        if source.len() > MAX_CSS_SOURCE_BYTES {
            return Err("plugin stylesheet exceeds 256 KiB".into());
        }
        let mut parser = Parser::new(source);
        let mut rule_parser = CssRuleParser { depth: 0 };
        let mut rules = Vec::new();
        for result in StyleSheetParser::new(&mut parser, &mut rule_parser) {
            let rule = result.map_err(|(error, _, location)| {
                format!(
                    "plugin CSS at {}:{}: {error:?}",
                    location.line, location.column
                )
            })?;
            rules.extend(rule);
            if rules.len() > 512 {
                return Err("plugin stylesheet has more than 512 rules".into());
            }
        }
        let mut interaction_rules: [Vec<usize>; 3] = Default::default();
        for (index, rule) in rules.iter().enumerate() {
            for state in [
                InteractionState::Hover,
                InteractionState::Active,
                InteractionState::Focus,
            ] {
                if rule
                    .selectors
                    .iter()
                    .any(|selector| selector.state == Some(state))
                {
                    interaction_rules[state.index()].push(index);
                }
            }
        }
        let uses_palette = rules.iter().any(|rule| rule.color_schemes != 3);
        let sheet = Self {
            rules,
            palette: Some(palette),
            uses_palette,
            host_properties,
            reading_direction: ReadingDirection::LeftToRight,
            resolved: RefCell::default(),
            interaction_rules,
            interactions: RefCell::default(),
            diagnostics_enabled: std::cell::Cell::new(false),
            diagnostics: std::cell::Cell::new(StyleCacheDiagnostics::default()),
            #[cfg(test)]
            interaction_rule_visits: Default::default(),
        };
        sheet.validate()?;
        let mut other = sheet.clone();
        other.palette = Some(ThemePalette::from_appearance(Appearance {
            mode: if palette.mode == twinkle_theme::ThemeMode::Light {
                twinkle_theme::ThemeMode::Dark
            } else {
                twinkle_theme::ThemeMode::Light
            },
            ..Appearance::default()
        }));
        other.validate()?;
        Ok(sheet)
    }

    /// Return the currently selected reading direction.
    pub fn reading_direction(&self) -> ReadingDirection {
        self.reading_direction
    }

    /// Select reading direction and clear resolved-style caches if it changed.
    pub fn set_reading_direction(&mut self, direction: ReadingDirection) -> bool {
        let changed = self.reading_direction != direction;
        self.reading_direction = direction;
        if changed {
            self.resolved.get_mut().clear();
            *self.interactions.get_mut() = InteractionCache::default();
        }
        changed
    }

    /// Replace the palette and revalidate declarations under its color scheme.
    pub fn set_palette(&mut self, palette: ThemePalette) -> Result<bool, String> {
        if self.palette == Some(palette) {
            return Ok(false);
        }
        self.palette = Some(palette);
        self.resolved.get_mut().clear();
        *self.interactions.get_mut() = InteractionCache::default();
        Ok(self.uses_palette)
    }

    /// Replace host-owned root custom properties without assigning them a
    /// toolkit namespace or meaning, after validating all CSS uses.
    pub fn set_host_properties(
        &mut self,
        properties: HashMap<String, String>,
    ) -> Result<bool, String> {
        if properties.len() > 128
            || properties.iter().any(|(name, value)| {
                !name.starts_with("--")
                    || name.len() <= 2
                    || name.len() > 128
                    || value.is_empty()
                    || value.len() > 4096
                    || validate_var_syntax(value).is_err()
            })
        {
            return Err("invalid host CSS custom properties".into());
        }
        if self.host_properties == properties {
            return Ok(false);
        }
        let prior = std::mem::replace(&mut self.host_properties, properties);
        if let Err(error) = self.validate() {
            self.host_properties = prior;
            return Err(error);
        }
        self.resolved.get_mut().clear();
        *self.interactions.get_mut() = InteractionCache::default();
        Ok(true)
    }

    fn rule_active(&self, rule: &Rule) -> bool {
        let palette = self
            .palette
            .unwrap_or_else(|| ThemePalette::from_appearance(Appearance::default()));
        let scheme = if palette.mode == twinkle_theme::ThemeMode::Light {
            1
        } else {
            2
        };
        rule.color_schemes & scheme != 0
    }

    fn validate(&self) -> Result<(), String> {
        let mut root_properties = HashMap::new();
        let mut declared = HashSet::new();
        for rule in self.rules.iter().filter(|rule| self.rule_active(rule)) {
            for parsed in &rule.declarations {
                if let ParsedDeclaration::Custom(name, value) = parsed {
                    declared.insert(name.clone());
                    if rule.selectors.iter().any(|selector| selector.root) {
                        root_properties.insert(name.clone(), value.clone());
                    }
                }
            }
        }
        root_properties.extend(self.host_properties.clone());
        for rule in self.rules.iter().filter(|rule| self.rule_active(rule)) {
            if rule.selectors.iter().any(|selector| selector.root)
                && rule
                    .declarations
                    .iter()
                    .any(|value| !matches!(value, ParsedDeclaration::Custom(..)))
            {
                return Err(":root accepts only CSS custom properties".into());
            }
            let mut properties = root_properties.clone();
            for parsed in &rule.declarations {
                if let ParsedDeclaration::Custom(name, value) = parsed {
                    properties.insert(name.clone(), value.clone());
                }
            }
            for parsed in &rule.declarations {
                let (name, value) = match parsed {
                    ParsedDeclaration::Custom(name, _) => (None, format!("var({name})")),
                    ParsedDeclaration::DeferredProperty(name, value) => (Some(name), value.clone()),
                    ParsedDeclaration::Typed(_) => continue,
                };
                match resolve_value(&value, &properties, &mut HashSet::new()) {
                    Ok(value) => {
                        if let Some(name) = name {
                            declaration(name, &value)?;
                        }
                    }
                    // A variable supplied by a matching ancestor is checked and typed
                    // when the real component tree resolves this declaration.
                    Err(error)
                        if error
                            .strip_prefix("undefined CSS custom property ")
                            .is_some_and(|name| declared.contains(name)) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(())
    }

    /// Resolve ordinary style declarations for a native component identity.
    pub fn resolve(&self, kind: &str, id: Option<&str>, class_name: Option<&str>) -> ControlStyle {
        self.resolve_with_custom_properties(kind, id, class_name, &HashMap::new())
    }

    pub(crate) fn resolve_with_custom_properties(
        &self,
        kind: &str,
        id: Option<&str>,
        class_name: Option<&str>,
        inherited: &HashMap<String, String>,
    ) -> ControlStyle {
        self.resolve_with_ancestors(kind, id, class_name, inherited, &[])
    }

    pub(crate) fn resolve_with_ancestors(
        &self,
        kind: &str,
        id: Option<&str>,
        class_name: Option<&str>,
        inherited: &HashMap<String, String>,
        ancestors: &[(String, Option<String>, Option<String>)],
    ) -> ControlStyle {
        self.resolve_with_context(kind, id, class_name, inherited, ancestors, None)
    }

    pub(crate) fn resolve_with_context(
        &self,
        kind: &str,
        id: Option<&str>,
        class_name: Option<&str>,
        inherited: &HashMap<String, String>,
        ancestors: &[(String, Option<String>, Option<String>)],
        context_token: Option<StyleContextToken>,
    ) -> ControlStyle {
        let diagnostics_enabled = self.diagnostics_enabled.get();
        let key_started = diagnostics_enabled.then(std::time::Instant::now);
        if context_token.is_none() {
            self.update_cache_diagnostics(|diagnostics| {
                diagnostics.key_sorts = diagnostics.key_sorts.saturating_add(1);
                diagnostics.full_context_hashes = diagnostics.full_context_hashes.saturating_add(1);
            });
        }
        let lookup = StyleCacheLookup::with_context(
            kind,
            id,
            class_name,
            inherited,
            ancestors,
            context_token,
        );
        let fingerprint = lookup.fingerprint();
        self.update_cache_diagnostics(|diagnostics| {
            diagnostics.key_builds = diagnostics.key_builds.saturating_add(1);
            diagnostics.key_build_ns = diagnostics.key_build_ns.saturating_add(
                key_started.map_or(0, |started| started.elapsed().as_nanos() as u64),
            );
        });
        let probe_started = diagnostics_enabled.then(std::time::Instant::now);
        let cached = self.resolved.borrow();
        let candidate = cached.get(&fingerprint);
        self.update_cache_diagnostics(|diagnostics| {
            diagnostics.cache_probes = diagnostics.cache_probes.saturating_add(1);
            diagnostics.cache_probe_ns = diagnostics.cache_probe_ns.saturating_add(
                probe_started.map_or(0, |started| started.elapsed().as_nanos() as u64),
            );
        });
        if let Some((key, style)) = candidate {
            let exact_started = diagnostics_enabled.then(std::time::Instant::now);
            let matches = key.matches(&lookup);
            self.update_cache_diagnostics(|diagnostics| {
                diagnostics.exact_checks = diagnostics.exact_checks.saturating_add(1);
                diagnostics.exact_check_ns = diagnostics.exact_check_ns.saturating_add(
                    exact_started.map_or(0, |started| started.elapsed().as_nanos() as u64),
                );
                if matches {
                    diagnostics.cache_hits = diagnostics.cache_hits.saturating_add(1);
                }
            });
            if matches {
                return style.clone();
            }
        }
        drop(cached);
        let mut style = ControlStyle {
            ancestors: Arc::new(ancestors.to_vec()),
            ..ControlStyle::default()
        };
        Arc::make_mut(&mut style.ancestors).push((
            kind.into(),
            id.map(str::to_owned),
            class_name.map(str::to_owned),
        ));
        let mut properties = HashMap::new();
        for rule in self.rules.iter().filter(|rule| self.rule_active(rule)) {
            if rule.selectors.iter().any(|selector| selector.root) {
                for declaration in &rule.declarations {
                    if let ParsedDeclaration::Custom(name, value) = declaration {
                        properties.insert(name.clone(), value.clone());
                    }
                }
            }
        }
        properties.extend(self.host_properties.clone());
        properties.extend(inherited.clone());
        for rule in self.rules.iter().filter(|rule| self.rule_active(rule)) {
            if rule
                .selectors
                .iter()
                .any(|selector| selector.matches(kind, id, class_name, None, ancestors))
            {
                for declaration in &rule.declarations {
                    if let ParsedDeclaration::Custom(name, value) = declaration {
                        properties.insert(name.clone(), value.clone());
                    }
                }
            }
        }
        for rule in self.rules.iter().filter(|rule| self.rule_active(rule)) {
            if rule
                .selectors
                .iter()
                .any(|selector| selector.matches(kind, id, class_name, None, ancestors))
            {
                for parsed in &rule.declarations {
                    parsed.apply(&properties, &mut style);
                }
            }
        }
        style.custom_properties = properties
            .iter()
            .filter_map(|(name, value)| {
                resolve_value(value, &properties, &mut HashSet::new())
                    .ok()
                    .map(|value| (name.clone(), value))
            })
            .collect::<HashMap<_, _>>()
            .into();
        style.context_token = style_context_token(&style.custom_properties, &style.ancestors);
        self.update_cache_diagnostics(|diagnostics| {
            diagnostics.key_sorts = diagnostics.key_sorts.saturating_add(2);
            diagnostics.full_context_hashes = diagnostics.full_context_hashes.saturating_add(1);
        });
        let mut cache = self.resolved.borrow_mut();
        if cache.len() >= 256 {
            cache.clear();
        }
        cache.insert(fingerprint, (lookup.to_owned(), style.clone()));
        style
    }

    pub(crate) fn resolve_interaction_paint(
        &self,
        kind: &str,
        id: Option<&str>,
        class_name: Option<&str>,
        state: InteractionState,
        properties: &HashMap<String, String>,
        ancestors: &[(String, Option<String>, Option<String>)],
    ) -> twinkle::InteractionPaint {
        self.resolve_interaction_paint_with_context(
            kind, id, class_name, state, properties, ancestors, None,
        )
    }

    pub(crate) fn resolve_interaction_paint_with_context(
        &self,
        kind: &str,
        id: Option<&str>,
        class_name: Option<&str>,
        state: InteractionState,
        properties: &HashMap<String, String>,
        ancestors: &[(String, Option<String>, Option<String>)],
        context_token: Option<StyleContextToken>,
    ) -> twinkle::InteractionPaint {
        if self.interaction_rules[state.index()].is_empty() {
            return twinkle::InteractionPaint::default();
        }
        let diagnostics_enabled = self.diagnostics_enabled.get();
        let key_started = diagnostics_enabled.then(std::time::Instant::now);
        if context_token.is_none() {
            self.update_cache_diagnostics(|diagnostics| {
                diagnostics.key_sorts = diagnostics.key_sorts.saturating_add(1);
                diagnostics.full_context_hashes = diagnostics.full_context_hashes.saturating_add(1);
            });
        }
        let lookup = StyleCacheLookup::with_context(
            kind,
            id,
            class_name,
            properties,
            ancestors,
            context_token,
        );
        let fingerprint = (lookup.fingerprint(), state);
        self.update_cache_diagnostics(|diagnostics| {
            diagnostics.key_builds = diagnostics.key_builds.saturating_add(1);
            diagnostics.key_build_ns = diagnostics.key_build_ns.saturating_add(
                key_started.map_or(0, |started| started.elapsed().as_nanos() as u64),
            );
        });
        let probe_started = diagnostics_enabled.then(std::time::Instant::now);
        let interactions = self.interactions.borrow();
        let candidate = interactions.entries.get(&fingerprint);
        self.update_cache_diagnostics(|diagnostics| {
            diagnostics.cache_probes = diagnostics.cache_probes.saturating_add(1);
            diagnostics.cache_probe_ns = diagnostics.cache_probe_ns.saturating_add(
                probe_started.map_or(0, |started| started.elapsed().as_nanos() as u64),
            );
        });
        if let Some((key, paint)) = candidate {
            let exact_started = diagnostics_enabled.then(std::time::Instant::now);
            let matches = key.matches(&lookup);
            self.update_cache_diagnostics(|diagnostics| {
                diagnostics.exact_checks = diagnostics.exact_checks.saturating_add(1);
                diagnostics.exact_check_ns = diagnostics.exact_check_ns.saturating_add(
                    exact_started.map_or(0, |started| started.elapsed().as_nanos() as u64),
                );
                if matches {
                    diagnostics.cache_hits = diagnostics.cache_hits.saturating_add(1);
                }
            });
            if matches {
                return *paint;
            }
        }
        drop(interactions);
        let mut style = ControlStyle::default();
        for &index in &self.interaction_rules[state.index()] {
            let rule = &self.rules[index];
            if !self.rule_active(rule) {
                continue;
            }
            #[cfg(test)]
            self.interaction_rule_visits
                .set(self.interaction_rule_visits.get() + 1);
            if rule.selectors.iter().any(|selector| {
                selector.state == Some(state)
                    && selector.matches(kind, id, class_name, Some(state), ancestors)
            }) {
                for parsed in &rule.declarations {
                    parsed.apply(properties, &mut style);
                }
            }
        }
        let paint = twinkle::InteractionPaint {
            background: style.background,
            foreground: style.color,
            border_color: style.border_color,
            border_width: style.border_width,
            radius: style.radius,
            font_size: style.font_size,
            line_height: style.line_height,
            width: style.width,
            height: style.height,
        };
        let key = lookup.to_owned();
        self.update_cache_diagnostics(|diagnostics| {
            diagnostics.key_sorts = diagnostics.key_sorts.saturating_add(1);
        });
        let bytes = key.retained_bytes()
            + std::mem::size_of::<u64>()
            + std::mem::size_of::<InteractionState>()
            + std::mem::size_of::<twinkle::InteractionPaint>();
        if bytes <= INTERACTION_CACHE_BYTES {
            let mut cache = self.interactions.borrow_mut();
            if let Some((previous, _)) = cache.entries.remove(&fingerprint) {
                cache.bytes -= previous.retained_bytes()
                    + std::mem::size_of::<u64>()
                    + std::mem::size_of::<InteractionState>()
                    + std::mem::size_of::<twinkle::InteractionPaint>();
            }
            if cache.entries.len() >= INTERACTION_CACHE_ENTRIES
                || cache.bytes + bytes > INTERACTION_CACHE_BYTES
            {
                *cache = InteractionCache::default();
            }
            cache.bytes += bytes;
            cache.entries.insert(fingerprint, (key, paint));
        }
        paint
    }

    /// Resolve the interaction background for one control state.
    pub fn resolve_interaction_background(
        &self,
        kind: &str,
        id: Option<&str>,
        class_name: Option<&str>,
        state: InteractionState,
    ) -> Option<u32> {
        self.resolve_interaction_background_with_properties(
            kind,
            id,
            class_name,
            state,
            &self.resolve(kind, id, class_name).custom_properties,
            &[],
        )
    }

    pub(crate) fn resolve_interaction_background_with_properties(
        &self,
        kind: &str,
        id: Option<&str>,
        class_name: Option<&str>,
        state: InteractionState,
        properties: &HashMap<String, String>,
        ancestors: &[(String, Option<String>, Option<String>)],
    ) -> Option<u32> {
        self.resolve_interaction_paint(kind, id, class_name, state, properties, ancestors)
            .background
    }

    /// Estimate bytes retained by rules and interaction caches.
    pub fn estimated_retained_bytes(&self) -> u64 {
        let mut bytes = self.rules.capacity() * std::mem::size_of::<Rule>();
        bytes += self.interactions.borrow().bytes;
        bytes += self
            .interaction_rules
            .iter()
            .map(|rules| rules.capacity() * std::mem::size_of::<usize>())
            .sum::<usize>();
        for rule in &self.rules {
            bytes += rule.selectors.capacity() * std::mem::size_of::<Selector>();
            bytes += rule.declarations.capacity() * std::mem::size_of::<ParsedDeclaration>();
            for selector in &rule.selectors {
                bytes += selector.kind.as_ref().map_or(0, String::capacity);
                bytes += selector.id.as_ref().map_or(0, String::capacity);
                bytes += selector.classes.capacity() * std::mem::size_of::<String>();
                bytes += selector.classes.iter().map(String::capacity).sum::<usize>();
            }
            for declaration in &rule.declarations {
                match declaration {
                    ParsedDeclaration::Custom(name, value)
                    | ParsedDeclaration::DeferredProperty(name, value) => {
                        bytes += name.capacity() + value.capacity();
                    }
                    ParsedDeclaration::Typed(Declaration::GridColumns(tracks)) => {
                        bytes += tracks.capacity() * std::mem::size_of::<Track>();
                    }
                    ParsedDeclaration::Typed(_) => {}
                }
            }
        }
        bytes as u64
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn static_declarations_are_typed_once_while_variables_remain_deferred() {
        use super::*;

        let css = StyleSheet::compile(
            ":root { --space: 7px; } button { width: 40px; padding: var(--space); color: #123456; }",
        )
        .unwrap();
        assert!(css.rules.iter().any(|rule| {
            rule.declarations
                .iter()
                .any(|declaration| matches!(declaration, ParsedDeclaration::Typed(_)))
        }));
        assert!(css.rules.iter().any(|rule| {
            rule.declarations.iter().any(|declaration| {
                matches!(declaration, ParsedDeclaration::DeferredProperty(name, _) if name == "padding")
            })
        }));

        CSS_DECLARATION_PARSES.with(|count| count.set(0));
        let first = css.resolve("button", None, None);
        let first_parses = CSS_DECLARATION_PARSES.with(std::cell::Cell::get);
        assert_eq!(first.width, Some(Length::Px(40.0)));
        assert_eq!(first.padding, Some(Insets::all(7.0)));
        assert_eq!(first_parses, 1, "only the var()-dependent value reparses");

        let _ = css.resolve("button", None, None);
        assert_eq!(
            CSS_DECLARATION_PARSES.with(std::cell::Cell::get),
            first_parses,
            "a stable cache hit parses no declarations"
        );
    }

    #[test]
    fn cached_styles_share_immutable_variables_without_sibling_leaks_or_retention() {
        use super::*;
        let css = StyleSheet::compile(
            ":root { --ink: #123456; } .local { --ink: #abcdef; } text { color: var(--ink); }",
        )
        .unwrap();
        let parent = css.resolve("div", None, None);
        let again = css.resolve("div", None, None);
        assert!(Arc::ptr_eq(
            &parent.custom_properties,
            &again.custom_properties
        ));
        let ordinary =
            css.resolve_with_custom_properties("text", None, None, &parent.custom_properties);
        let local = css.resolve_with_custom_properties(
            "text",
            None,
            Some("local"),
            &parent.custom_properties,
        );
        assert_eq!(ordinary.color, Some(0xff12_3456));
        assert_eq!(local.color, Some(0xffab_cdef));
        assert_eq!(
            parent.custom_properties.get("--ink").map(String::as_str),
            Some("#123456")
        );
        assert!(!Arc::ptr_eq(
            &ordinary.custom_properties,
            &local.custom_properties
        ));
        let lifetime = Arc::downgrade(&parent.custom_properties);
        drop(css);
        assert_eq!(
            again.custom_properties.get("--ink"),
            parent.custom_properties.get("--ink")
        );
        drop(parent);
        drop(again);
        assert!(
            lifetime.upgrade().is_none(),
            "retired style variables must not leak through a global cache"
        );
    }

    #[test]
    fn borrowed_cache_lookup_rejects_collisions_and_accounts_replacements() {
        use super::*;
        let source = ":root { --ink: #000; } button { color: var(--ink); } button:hover { background: var(--ink); }";
        let css = StyleSheet::compile(source).unwrap();
        let before = HashMap::from([("--ink".into(), "#123456".into())]);
        let after = HashMap::from([("--ink".into(), "#fff".into())]);
        let old = StyleCacheLookup::new("button", None, None, &before, &[]).fingerprint();
        let new = StyleCacheLookup::new("button", None, None, &after, &[]).fingerprint();
        css.resolve_with_custom_properties("button", None, None, &before);
        // Force a candidate collision without relying on finding a hash collision.
        let entry = css.resolved.borrow_mut().remove(&old).unwrap();
        css.resolved.borrow_mut().insert(new, entry);
        let cold = StyleSheet::compile(source).unwrap();
        assert_eq!(
            css.resolve_with_custom_properties("button", None, None, &after),
            cold.resolve_with_custom_properties("button", None, None, &after)
        );
        assert_eq!(css.resolved.borrow().len(), 1);

        let state = InteractionState::Hover;
        css.resolve_interaction_paint("button", None, None, state, &before, &[]);
        let entry = css
            .interactions
            .borrow_mut()
            .entries
            .remove(&(old, state))
            .unwrap();
        css.interactions
            .borrow_mut()
            .entries
            .insert((new, state), entry);
        assert_eq!(
            css.resolve_interaction_paint("button", None, None, state, &after, &[]),
            cold.resolve_interaction_paint("button", None, None, state, &after, &[])
        );
        assert_eq!(css.interactions.borrow().entries.len(), 1);
        assert_eq!(
            css.interactions.borrow().bytes,
            cold.interactions.borrow().bytes
        );

        let forward = HashMap::from([("--a".into(), "1".into()), ("--b".into(), "2".into())]);
        let reverse = HashMap::from([("--b".into(), "2".into()), ("--a".into(), "1".into())]);
        let first = StyleCacheLookup::new("button", Some("id"), Some("class"), &forward, &[]);
        let second = StyleCacheLookup::new("button", Some("id"), Some("class"), &reverse, &[]);
        assert_eq!(first.fingerprint(), second.fingerprint());
        assert!(first.to_owned().matches(&second));
    }

    #[test]
    fn inherited_context_token_avoids_stable_hit_sort_and_ancestor_rehash() {
        CSS_KEY_SORTS.with(|count| count.set(0));
        CSS_FULL_CONTEXT_HASHES.with(|count| count.set(0));
        let css = StyleSheet::compile(
            ":root { --ink: #123456; } .parent button { color: var(--ink); } button:hover { background: var(--ink); }",
        )
        .unwrap();
        let parent = css.resolve("div", None, Some("parent"));
        let first = css.resolve_with_context(
            "button",
            Some("save"),
            None,
            &parent.custom_properties,
            &parent.ancestors,
            Some(parent.context_token),
        );
        let before = (
            CSS_KEY_SORTS.with(std::cell::Cell::get),
            CSS_FULL_CONTEXT_HASHES.with(std::cell::Cell::get),
        );
        css.set_cache_diagnostics_enabled(true);
        let second = css.resolve_with_context(
            "button",
            Some("save"),
            None,
            &parent.custom_properties,
            &parent.ancestors,
            Some(parent.context_token),
        );
        let after = (
            CSS_KEY_SORTS.with(std::cell::Cell::get),
            CSS_FULL_CONTEXT_HASHES.with(std::cell::Cell::get),
        );

        assert_eq!(second, first);
        assert_eq!(after, before);
        let diagnostics = css.cache_diagnostics();
        assert_eq!(diagnostics.key_builds, 1);
        assert_eq!(diagnostics.key_sorts, 0);
        assert_eq!(diagnostics.full_context_hashes, 0);
        assert_eq!(diagnostics.cache_probes, 1);
        assert_eq!(diagnostics.cache_hits, 1);
        assert_eq!(diagnostics.exact_checks, 1);

        let first_paint = css.resolve_interaction_paint_with_context(
            "button",
            Some("save"),
            None,
            InteractionState::Hover,
            &parent.custom_properties,
            &parent.ancestors,
            Some(parent.context_token),
        );
        let before = (
            CSS_KEY_SORTS.with(std::cell::Cell::get),
            CSS_FULL_CONTEXT_HASHES.with(std::cell::Cell::get),
        );
        css.set_cache_diagnostics_enabled(true);
        let second_paint = css.resolve_interaction_paint_with_context(
            "button",
            Some("save"),
            None,
            InteractionState::Hover,
            &parent.custom_properties,
            &parent.ancestors,
            Some(parent.context_token),
        );
        let after = (
            CSS_KEY_SORTS.with(std::cell::Cell::get),
            CSS_FULL_CONTEXT_HASHES.with(std::cell::Cell::get),
        );
        assert_eq!(second_paint, first_paint);
        assert_eq!(after, before);
        let diagnostics = css.cache_diagnostics();
        assert_eq!(diagnostics.key_builds, 1);
        assert_eq!(diagnostics.key_sorts, 0);
        assert_eq!(diagnostics.full_context_hashes, 0);
        assert_eq!(diagnostics.cache_probes, 1);
        assert_eq!(diagnostics.cache_hits, 1);
        assert_eq!(diagnostics.exact_checks, 1);
    }

    #[test]
    fn interaction_cache_preserves_inputs_and_avoids_repeated_rule_visits() {
        let source = ":root { --ink: #123456; } .parent button:hover { color: var(--ink); } button:active { background: #abcdef; } button:hover { width: 76px; } text { color: #ffffff; }";
        let mut css = super::StyleSheet::compile(source).unwrap();
        let parent = vec![("div".into(), None, Some("parent".into()))];
        let mut properties = std::collections::HashMap::from([("--ink".into(), "#334455".into())]);
        for state in [
            super::InteractionState::Hover,
            super::InteractionState::Active,
            super::InteractionState::Focus,
        ] {
            for ancestors in [&parent[..], &[][..]] {
                for id in [None, Some("one")] {
                    for classes in [None, Some("special")] {
                        let expected = super::StyleSheet::compile(source)
                            .unwrap()
                            .resolve_interaction_paint(
                                "button",
                                id,
                                classes,
                                state,
                                &properties,
                                ancestors,
                            );
                        let actual = css.resolve_interaction_paint(
                            "button",
                            id,
                            classes,
                            state,
                            &properties,
                            ancestors,
                        );
                        assert_eq!(actual, expected);
                        let visits = css.interaction_rule_visits.get();
                        assert_eq!(
                            css.resolve_interaction_paint(
                                "button",
                                id,
                                classes,
                                state,
                                &properties,
                                ancestors
                            ),
                            expected
                        );
                        assert_eq!(css.interaction_rule_visits.get(), visits);
                    }
                }
            }
        }
        properties.insert("--ink".into(), "#778899".into());
        let changed = css.resolve_interaction_paint(
            "button",
            None,
            None,
            super::InteractionState::Hover,
            &properties,
            &parent,
        );
        assert_eq!(changed.foreground, Some(0xff778899));
        assert_eq!(changed.width, Some(twinkle::Length::Px(76.0)));
        assert_eq!(
            css.resolve_interaction_paint(
                "text",
                None,
                None,
                super::InteractionState::Hover,
                &properties,
                &parent
            ),
            twinkle::InteractionPaint::default()
        );
        assert!(css.set_reading_direction(twinkle::ReadingDirection::RightToLeft));
        assert!(css.interactions.borrow().entries.is_empty());
        css.resolve_interaction_paint(
            "button",
            None,
            None,
            super::InteractionState::Hover,
            &properties,
            &parent,
        );
        css.set_palette(twinkle_theme::ThemePalette::from_appearance(
            twinkle_theme::Appearance {
                mode: twinkle_theme::ThemeMode::Light,
                ..Default::default()
            },
        ))
        .unwrap();
        assert!(css.interactions.borrow().entries.is_empty());
    }

    #[test]
    fn interaction_cache_is_count_and_payload_bounded() {
        let css = super::StyleSheet::compile("button:hover { background: #123456; }").unwrap();
        let mut properties = std::collections::HashMap::new();
        for index in 0..super::INTERACTION_CACHE_ENTRIES * 2 {
            let paint = css.resolve_interaction_paint(
                "button",
                Some(&index.to_string()),
                None,
                super::InteractionState::Hover,
                &properties,
                &[],
            );
            assert_eq!(paint.background, Some(0xff123456));
            assert!(css.interactions.borrow().entries.len() <= super::INTERACTION_CACHE_ENTRIES);
            assert!(css.interactions.borrow().bytes <= super::INTERACTION_CACHE_BYTES);
        }
        let entries = css.interactions.borrow().entries.len();
        properties.insert(
            "--oversized".into(),
            "x".repeat(super::INTERACTION_CACHE_BYTES + 1),
        );
        assert_eq!(
            css.resolve_interaction_paint(
                "button",
                None,
                None,
                super::InteractionState::Hover,
                &properties,
                &[]
            )
            .background,
            Some(0xff123456)
        );
        assert_eq!(
            css.interactions.borrow().entries.len(),
            entries,
            "oversized keys must not be retained"
        );
    }

    #[test]
    fn descendant_rules_match_ordered_ancestors_without_leaking() {
        let css = super::StyleSheet::compile("window.launcher button { background: #123456; } window.settings button { background: #abcdef; }").unwrap();
        let launcher = vec![
            ("window".into(), None, Some("launcher".into())),
            ("div".into(), None, None),
        ];
        let settings = vec![("window".into(), None, Some("settings".into()))];
        assert_eq!(
            css.resolve_with_ancestors("button", None, None, &Default::default(), &launcher)
                .background,
            Some(0xff123456)
        );
        assert_eq!(
            css.resolve_with_ancestors("button", None, None, &Default::default(), &settings)
                .background,
            Some(0xffabcdef)
        );
        assert_eq!(css.resolve("button", None, None).background, None);
    }
    use super::*;

    #[test]
    fn host_color_tokens_recompile_without_toolkit_semantics() {
        let mut properties = HashMap::from([
            ("--host-panel".into(), "#112233".into()),
            ("--host-muted".into(), "#445566".into()),
            ("--host-text".into(), "#778899".into()),
        ]);
        let mut sheet = StyleSheet::compile_with_host_properties(
            "window { background: var(--host-panel); border: 1px solid var(--host-muted); } text { color: var(--host-text); }",
            properties.clone(),
        )
        .unwrap();
        assert_eq!(
            sheet.resolve("window", None, None).background,
            Some(0xff11_2233)
        );
        assert_eq!(sheet.resolve("text", None, None).color, Some(0xff77_8899));
        assert!(sheet.set_reading_direction(ReadingDirection::RightToLeft));
        properties.insert("--host-panel".into(), "#abcdef".into());
        assert!(sheet.set_host_properties(properties.clone()).unwrap());
        assert_eq!(sheet.reading_direction(), ReadingDirection::RightToLeft);
        assert_eq!(
            sheet.resolve("window", None, None).background,
            Some(0xffab_cdef)
        );
        assert!(!sheet.set_host_properties(properties).unwrap());
        assert!(StyleSheet::compile("text { color: var(--unknown); }").is_err());
    }

    #[test]
    fn custom_properties_cascade_and_inherit_into_typed_declarations() {
        let css = StyleSheet::compile(
            ":root { --theme-text: #112233; --space: 4px; }
             div.parent { --theme-text: #445566; --space: 12px; }
             text { color: var(--theme-text); padding: var(--space); }
             text.local { --theme-text: #778899; }",
        )
        .unwrap();

        let root_text = css.resolve("text", None, None);
        assert_eq!(root_text.color, Some(0xff11_2233));
        assert_eq!(root_text.padding, Some(Insets::all(4.0)));

        let parent = css.resolve("div", None, Some("parent"));
        let child =
            css.resolve_with_custom_properties("text", None, None, &parent.custom_properties);
        assert_eq!(child.color, Some(0xff44_5566));
        assert_eq!(child.padding, Some(Insets::all(12.0)));

        let local = css.resolve_with_custom_properties(
            "text",
            None,
            Some("local"),
            &parent.custom_properties,
        );
        assert_eq!(local.color, Some(0xff77_8899));
    }

    #[test]
    fn var_fallbacks_nest_and_semantic_defaults_remain_overridable() {
        let default = StyleSheet::compile(
            ":root { --host-text: #123456; } text { color: var(--missing, var(--host-text)); }",
        )
        .unwrap();
        assert_eq!(default.resolve("text", None, None).color, Some(0xff12_3456));

        let overridden = StyleSheet::compile(
            ":root { --host-text: #abcdef; }
             text { color: var(--missing, var(--host-text)); }",
        )
        .unwrap();
        assert_eq!(
            overridden.resolve("text", None, None).color,
            Some(0xffab_cdef)
        );
    }

    #[test]
    fn computed_variables_and_interaction_colors_inherit_without_sibling_leaks() {
        let css = StyleSheet::compile(
            "div { --base: #112233; --alias: var(--base); }
             div.other { --base: 12px; }
             button { --base: #445566; color: var(--alias); }
             button:hover { background: var(--alias); }",
        )
        .unwrap();
        let parent = css.resolve("div", None, None);
        let child =
            css.resolve_with_custom_properties("button", None, None, &parent.custom_properties);
        assert_eq!(child.color, Some(0xff11_2233));
        assert_eq!(
            css.resolve_interaction_background_with_properties(
                "button",
                None,
                None,
                InteractionState::Hover,
                &child.custom_properties,
                &[],
            ),
            Some(0xff11_2233)
        );
    }

    #[test]
    fn semantic_metrics_and_variable_chain_limits_are_available() {
        let css = StyleSheet::compile(
            ":root { --host-font-size: 14px; --host-line-height: 20px; --host-spacing-control: 8px; --host-radius-control: 8px; }
             text { font-size: var(--host-font-size); line-height: var(--host-line-height); }
             button { padding: var(--host-spacing-control); border-radius: var(--host-radius-control); }",
        ).unwrap();
        assert_eq!(css.resolve("text", None, None).font_size, Some(14.0));
        assert_eq!(css.resolve("button", None, None).radius, Some(8.0));
        let mut source = String::from(":root { --v0: #112233;");
        for index in 1..40 {
            source.push_str(&format!("--v{index}: var(--v{});", index - 1));
        }
        source.push_str("} text { color: var(--v39); }");
        assert!(StyleSheet::compile(&source).is_err());
    }

    #[test]
    fn invalid_custom_property_resolution_is_rejected() {
        for css in [
            ":root { --a: var(--b); --b: var(--a); } text { color: var(--a); }",
            ":root { --size: 9000px; } div { width: var(--size); }",
            ":root { --color: 12px; } text { color: var(--color); }",
            "text { color: var(--missing); }",
            "text { color: var(--missing, var(--also-missing)); }",
        ] {
            assert!(StyleSheet::compile(css).is_err(), "{css}");
        }
    }

    #[test]
    fn classes_and_type_selectors_apply_in_order() {
        let css = StyleSheet::compile("button { padding: 2px 4px; color: #abc; } button.primary { padding: 8px; border-radius: 6px; }").unwrap();
        let style = css.resolve("button", None, Some("primary compact"));
        assert_eq!(style.padding, Some(Insets::all(8.0)));
        assert_eq!(style.color, Some(0xffaabbcc));
        assert_eq!(style.radius, Some(6.0));
        assert_eq!(
            css.resolve("text-field", None, Some("primary")).padding,
            None
        );
    }

    #[test]
    fn invalid_and_unbounded_css_is_rejected() {
        for source in [
            "button { unknown: 4px }",
            "button { padding: 10000px }",
            "@import 'remote.css';",
            "div:hover { background: #fff }",
        ] {
            assert!(StyleSheet::compile(source).is_err(), "{source}");
        }
    }

    #[test]
    fn interaction_backgrounds_are_separate_from_base_styles() {
        let css = StyleSheet::compile(
            "button { background: #111; } button.primary:hover { background: #222; } \
             button.primary:active { background: #333; } \
             button.primary:focus { background: #444; } \
             text-field.entry:focus { background: #555; }",
        )
        .unwrap();
        assert_eq!(
            css.resolve("button", None, Some("primary")).background,
            Some(0xff111111)
        );
        assert_eq!(
            css.resolve_interaction_background(
                "button",
                None,
                Some("primary"),
                InteractionState::Hover
            ),
            Some(0xff222222)
        );
        assert_eq!(
            css.resolve_interaction_background(
                "button",
                None,
                Some("primary"),
                InteractionState::Active
            ),
            Some(0xff333333)
        );
        assert_eq!(
            css.resolve_interaction_background(
                "button",
                None,
                Some("primary"),
                InteractionState::Focus
            ),
            Some(0xff444444)
        );
        assert_eq!(
            css.resolve_interaction_background(
                "text-field",
                None,
                Some("entry"),
                InteractionState::Focus
            ),
            Some(0xff555555)
        );
        assert_eq!(
            css.resolve_interaction_background(
                "button",
                None,
                Some("other"),
                InteractionState::Hover
            ),
            None
        );
    }

    #[test]
    fn interaction_geometry_and_rounded_shadow_compile_to_typed_styles() {
        let css = StyleSheet::compile(
            ".dock { box-shadow: -2px 10px 24px 0px #00000059; backdrop-filter: blur(12px); \
             -twinkle-proximity-magnification: 1.38 2; transition-duration: 150ms; } \
             button.icon:hover { width: 76px; height: 76px; }",
        )
        .unwrap();
        let shadow = css.resolve("div", None, Some("dock")).box_shadow.unwrap();
        assert_eq!(shadow.offset_x, -2.0);
        assert_eq!(shadow.offset_y, 10.0);
        assert_eq!(shadow.blur, 24.0);
        assert_eq!(shadow.spread, 0.0);
        assert_eq!(shadow.color, 0x5900_0000);
        let dock = css.resolve("div", None, Some("dock"));
        assert_eq!(dock.backdrop_blur, Some(12.0));
        assert_eq!(dock.proximity_magnification.unwrap().maximum_scale, 1.38);
        assert_eq!(dock.proximity_magnification.unwrap().radius, 2);
        assert_eq!(dock.transition_duration_ms, Some(150.0));
        let hover = css.resolve_interaction_paint(
            "button",
            None,
            Some("icon"),
            InteractionState::Hover,
            &HashMap::new(),
            &[],
        );
        assert_eq!(hover.width, Some(Length::Px(76.0)));
        assert_eq!(hover.height, Some(Length::Px(76.0)));
    }

    #[test]
    fn rgba_and_border_shorthand_compile_to_typed_colors() {
        let css = StyleSheet::compile(
            ".card { background: rgba(10, 20, 30, 0.5); border: 2px solid #abc; gap: 12px; }",
        )
        .unwrap();
        let style = css.resolve("row", None, Some("card"));
        assert_eq!(style.background, Some(0x800a141e));
        assert_eq!(style.border_color, Some(0xffaabbcc));
        assert_eq!(style.border_width, Some(2.0));
        assert_eq!(style.gap, Some(12.0));
        assert!(css.estimated_retained_bytes() > 0);
    }

    #[test]
    fn familiar_flex_grid_and_size_declarations_map_to_twinkle_layout() {
        let css = StyleSheet::compile(
            ".row { display: flex; flex-direction: row; width: 100%; gap: 8px; align-items: center; justify-content: space-between; } \
             .item { flex: 1; min-width: 20px; max-width: 120px; } \
             .grid { display: grid; grid-template-columns: repeat(2, minmax(40px, 1fr)); }",
        )
        .unwrap();
        let row = css.resolve("div", None, Some("row"));
        assert_eq!(row.display, Some(Display::Flex));
        assert_eq!(row.flex_direction, Some(FlexDirection::Row));
        assert_eq!(row.width, Some(Length::Percent(1.0)));
        assert_eq!(row.align_items, Some(Align::Center));
        assert_eq!(row.justify_content, Some(Justify::SpaceBetween));
        let item = css.resolve("div", None, Some("item"));
        assert_eq!(item.grow, Some(1.0));
        assert_eq!(item.min_width, Some(20.0));
        assert_eq!(item.max_width, Some(120.0));
        let grid = css.resolve("div", None, Some("grid"));
        assert_eq!(grid.display, Some(Display::Grid));
        assert_eq!(
            grid.grid_columns,
            Some(vec![Track::repeat(
                2,
                Track::minmax(Track::Px(40.0), Track::Fraction(1.0))
            )])
        );
        let auto = StyleSheet::compile(
            ".grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(110px, 1fr)); }",
        )
        .unwrap();
        assert_eq!(
            auto.resolve("div", None, Some("grid")).grid_columns,
            Some(vec![Track::repeat_auto_fit(Track::minmax(
                Track::Px(110.0),
                Track::Fraction(1.0)
            ))])
        );
    }

    #[test]
    fn invalid_layout_values_are_rejected() {
        for css in [
            ".x { width: 200%; }",
            ".x { grid-template-columns: repeat(100, 1fr); }",
            ".x { grid-template-columns: repeat(32, repeat(32, 1fr)); }",
            ".x { flex-direction: diagonal; }",
            ".x { grid-template-columns: minmax(1fr, 40px; }",
        ] {
            assert!(StyleSheet::compile(css).is_err(), "{css}");
        }
    }

    #[test]
    fn positioned_window_distances_are_bounded_and_require_window_selector() {
        let stylesheet = StyleSheet::compile("window.dock { bottom: 20px; top: 12px; }").unwrap();
        assert_eq!(
            stylesheet.resolve("window", None, Some("dock")).bottom,
            Some(20.0)
        );
        assert_eq!(
            stylesheet.resolve("window", None, Some("dock")).top,
            Some(12.0)
        );
        for css in [
            ".dock { bottom: 20px; }",
            "button { bottom: 20px; }",
            ".notice { top: 12px; }",
            "text { top: 12px; }",
            "window.dock { bottom: -1px; }",
            "window.dock { bottom: 9000px; }",
            "window.notice { top: -1px; }",
            "window.notice { top: 9000px; }",
        ] {
            assert!(StyleSheet::compile(css).is_err(), "{css}");
        }
    }
}

#[cfg(test)]
mod color_scheme_tests {
    use super::*;
    #[test]
    fn media_queries_switch_root_tokens_cascade_and_hover_without_reload() {
        let mut sheet = StyleSheet::compile("@media (prefers-color-scheme: light) { :root { --ink: #112233; } button:hover { background: #abcdef; } } @media (prefers-color-scheme: dark) { :root { --ink: #ddeeff; } button:hover { background: #123456; } } text { color: var(--ink); }").unwrap();
        for (mode, ink, hover) in [
            (twinkle_theme::ThemeMode::Light, 0xff112233, 0xffabcdef),
            (twinkle_theme::ThemeMode::Dark, 0xffddeeff, 0xff123456),
            (twinkle_theme::ThemeMode::Light, 0xff112233, 0xffabcdef),
        ] {
            sheet
                .set_palette(ThemePalette::from_appearance(Appearance {
                    mode,
                    ..Appearance::default()
                }))
                .unwrap();
            assert_eq!(sheet.resolve("text", None, None).color, Some(ink));
            let style = sheet.resolve("button", None, None);
            assert_eq!(
                sheet
                    .resolve_interaction_paint(
                        "button",
                        None,
                        None,
                        InteractionState::Hover,
                        &style.custom_properties,
                        &[]
                    )
                    .background,
                Some(hover)
            );
        }
    }
    #[test]
    fn dormant_media_rules_are_validated() {
        assert!(
            StyleSheet::compile(
                "@media (prefers-color-scheme: light) { text { color: rubbish; } }"
            )
            .is_err()
        );
        assert!(
            StyleSheet::compile("@media (prefers-color-scheme: sepia) { text { color: white; } }")
                .is_err()
        );
    }
}

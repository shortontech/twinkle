# Twinkle's CSS subset

The `twinkle-presentation::css` module compiles a bounded, native style language. It is not a web
browser CSS engine. Styles apply to the admitted native component tree; they do not create DOM
behavior, network resources, or arbitrary layout and paint operations.

```rust
use twinkle_presentation::css::StyleSheet;

let css = StyleSheet::compile(r#"
    :root { --panel: #20252c; --ink: #f4f5f6; }
    window { background: var(--panel); }
    button.primary { color: var(--ink); padding: 8px 12px; }
    button.primary:hover { background: #405878; }
"#)?;
# Ok::<(), String>(())
```

Use `StyleSheet::compile` for the default palette, `compile_with_palette` to choose light or dark
color-scheme evaluation, or `compile_with_host_properties` to seed host-owned custom properties.
`set_palette`, `set_reading_direction`, and `set_host_properties` update those inputs on a compiled
sheet. Invalid CSS returns an error with a source line and column when available. A host should
compile and validate the stylesheet before admitting its rendered tree.

## Selectors and rule behavior

Supported simple selectors are native element kinds, `#id`, `.class`, and combinations such as
`button.primary#save`. Descendant selectors (separated by whitespace), comma-separated selector
lists, and the standalone `:root` selector are supported. The only pseudo-classes are `:hover`,
`:active`, and `:focus`; they are limited to interactive native controls and accept only the
properties `background`, `background-color`, `color`, `border`, `border-color`, `border-width`,
`border-radius`, `font-size`, `line-height`, `width`, and `height`. A state selector must name an
interactive kind (for example, `button.action:hover`; `.action:hover` alone is rejected), and a
rule cannot mix state selectors with ordinary selectors.

Matching rules apply in source order, so a later matching declaration replaces an earlier value.
Browser selector specificity is not implemented. `:root` accepts custom-property declarations
only. `top` and `bottom` may be used only with `window` selectors and are still checked against the
host's declared surface grant.

`@media (prefers-color-scheme: light)` and `@media (prefers-color-scheme: dark)` are supported,
including comma-separated light/dark queries. No other at-rules or media features are supported.
Color, font size, line height, and text alignment inherit through the native presentation tree;
custom properties inherit as well. Other properties apply only to the matching component.

## Properties

| Property | Accepted values |
| --- | --- |
| `display` | `block`, `flex`, `grid` |
| `flex-direction` | `row`, `column` |
| `grid-template-columns` | Space-separated tracks: pixel lengths up to 8192px, `auto`, `Nfr` (0 to 100), `minmax(a, b)`, `repeat(count, track)` (count 1 to 32), or `repeat(auto-fit, track)`; at most 32 expanded columns |
| `width`, `height`, `flex-basis` | `auto`, `min-content`, `max-content`, `0` / pixel lengths up to 8192px, or `0%` to `100%` |
| `min-width`, `max-width`, `min-height`, `max-height` | Nonnegative pixel lengths up to 8192px |
| `align-items` | `start`/`flex-start`, `center`, `end`/`flex-end`, `stretch`, `baseline` |
| `justify-content` | `start`/`flex-start`, `center`, `end`/`flex-end`, `space-between`, `space-around`, `space-evenly` |
| `flex`, `flex-grow`, `flex-shrink` | Finite numbers from 0 to 100; `flex` sets grow with shrink 1 and a zero-percent basis |
| `padding`, `margin` | One to four nonnegative pixel lengths up to 512px, using the CSS top/right/bottom/left shorthand order |
| `gap` | Nonnegative pixel length up to 512px |
| `background`, `background-color`, `color`, `icon-color`, `border-color` | `#RGB`, `#RRGGBB`, `#RRGGBBAA`, `rgba(r,g,b,a)` (channels 0–255, alpha 0–1), or `transparent` |
| `border` | `<width> solid <color>`; width 0 to 64px |
| `border-width` | Nonnegative pixel length up to 64px |
| `border-radius` | Nonnegative pixel length up to 512px |
| `box-shadow` | `<x> <y> <blur> <spread> <color>`; x/y range from -256px to 256px, blur/spread from 0 to 128px |
| `backdrop-filter` | `blur(<pixel length>)`, 0 to 128px |
| `font-size` | Nonnegative pixel length up to 256px |
| `line-height` | Nonnegative pixel length up to 512px |
| `text-align` | `start`, `center`, `end` |
| `top`, `bottom` | Nonnegative pixel lengths up to 8192px; `window` selectors only |
| `transition-duration` | Milliseconds, from `0ms` to `2000ms` |
| `-twinkle-proximity-magnification` | `<maximum-scale> <sibling-radius>`; scale 1 to 3, radius 0 to 8 |

Lengths use logical pixels. Except for percentages on `width`, `height`, and `flex-basis`, units
other than `px` are unsupported. A unit may be omitted only for zero. Grid tracks allow up to 32
expanded columns; repeat counts are from 1 to 32.

## Custom properties

Custom properties use the standard `--name` spelling and `var(--name[, fallback])` references.
Values resolve from `:root`, then host-supplied properties, inherited ancestors, and finally a
matching rule. A matching component can override an inherited value. Cycles, undefined names
without a fallback, malformed references, and oversized values are errors.

```css
:root { --surface: #20252c; --foreground: #f4f5f6; }
.panel { background: var(--surface); color: var(--foreground); }
@media (prefers-color-scheme: light) {
  :root { --surface: #ffffff; --foreground: #20252c; }
}
```

## Limits and unsupported browser CSS

The defaults are at most 256 KiB of source, 512 rules, 64 declarations per rule, 16 selectors per
rule, eight selector parts, eight nested media blocks, and bounded custom-property names, values,
and expansion depth. Hosts may enforce lower limits.

There is no browser DOM, stylesheet cascade specificity, sibling or child combinator, attribute
selector, arbitrary pseudo-class, animation/keyframe engine, `calc()`, external stylesheet/font
loading, URL or image-valued CSS, transform, positioning model, or general-purpose CSS variable
substitution outside supported declarations. Unsupported properties and values fail compilation
instead of being silently ignored.

For the Rust host call sequence and how styles reach native components, see the
[JSX host guide](jsx-host.md).

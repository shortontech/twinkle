use twinkle_ui::{Component, Constraints, Size, Text, measure_text};

#[test]
fn direct_measurement_matches_native_text_layout() {
    for font_size in [12.0, 18.0, 27.0] {
        for width in [90.0, 240.0, 600.0] {
            for bold in [false, true] {
                let text = "Regional revenue — February: $12,345.67 · 北部";
                let native = Text::<()>::new(text)
                    .font_size(font_size)
                    .bold(bold)
                    .wrap(true)
                    .into_element()
                    .measure(Constraints::loose(Size::new(width, f32::INFINITY)));
                let direct = measure_text(text, -font_size, bold, true, None, None, width);
                // Native layout constrains the shaped extent to its allocation;
                // a single glyph may otherwise exceed a narrow wrap width.
                let allocated =
                    Constraints::loose(Size::new(width, f32::INFINITY)).constrain(direct);
                assert_eq!(
                    allocated, native,
                    "font={font_size}, width={width}, bold={bold}"
                );
            }
        }
    }
}

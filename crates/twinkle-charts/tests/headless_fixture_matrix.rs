use std::sync::Arc;

use twinkle_charts::{
    BinOptions, Channel, ChartData, ChartLimits, ChartModel, ChartSpec, ChartTable, ChartTheme,
    ChartView, Column, DataValue, Dataset, DatasetId, Encoding, Facet, FacetScalePolicy, FieldRef,
    Interpolation, Layer, Mark, Recipes, RowKey,
};
use twinkle_ui::{
    Rect, SemanticRole,
    retained_surface::{RetainedSurfaceInput, RetainedSurfaceRevisions},
};

fn field(name: &str) -> FieldRef {
    FieldRef::new("fixture", name)
}

fn fixture_data(revision: u64) -> Arc<ChartData> {
    let categories = [
        "A label deliberately long enough to stress guide placement",
        "B",
        "C",
        "D",
        "E",
        "F",
        "G",
        "H",
        "I",
        "J",
        "K",
        "L",
    ];
    let keys = (0..categories.len())
        .map(|index| RowKey::from(format!("row-{index}")))
        .collect::<Vec<_>>();
    let dataset = Dataset::new(
        DatasetId::from("fixture"),
        keys,
        vec![
            Column::new(
                "x",
                (0..categories.len())
                    .map(|index| DataValue::Number(if index == 2 { 1.0 } else { index as f64 }))
                    .collect::<Vec<_>>(),
            ),
            Column::new(
                "value",
                [
                    -8.0, 3.0, 3.0, 1.0e9, 0.0, 7.0, -2.0, 9.0, 4.0, 6.0, 1.0, 8.0,
                ]
                .into_iter()
                .map(DataValue::Number)
                .collect::<Vec<_>>(),
            ),
            Column::new(
                "positive",
                (1..=categories.len())
                    .map(|value| DataValue::Number(value as f64))
                    .collect::<Vec<_>>(),
            ),
            Column::new(
                "size",
                (0..categories.len())
                    .map(|index| DataValue::Number((index % 4) as f64))
                    .collect::<Vec<_>>(),
            ),
            Column::new(
                "category",
                categories
                    .into_iter()
                    .map(|value| DataValue::Category(Arc::from(value)))
                    .collect::<Vec<_>>(),
            ),
            Column::new(
                "series",
                (0..categories.len())
                    .map(|index| DataValue::Category(Arc::from(format!("series-{index}"))))
                    .collect::<Vec<_>>(),
            ),
            Column::new(
                "facet",
                (0..categories.len())
                    .map(|index| DataValue::Category(Arc::from(format!("facet-{}", index % 3))))
                    .collect::<Vec<_>>(),
            ),
        ],
    )
    .unwrap();
    Arc::new(ChartData::new(revision, vec![dataset], &ChartLimits::default()).unwrap())
}

fn with_series(mut spec: ChartSpec) -> ChartSpec {
    spec.layers[0].encodings.push(Encoding {
        channel: Channel::Series,
        field: field("series"),
        scale: None,
    });
    spec
}

fn fixtures() -> Vec<(&'static str, ChartSpec)> {
    let mut step = with_series(Recipes::line(
        "Multi-series step line",
        field("x"),
        field("value"),
    ));
    step.layers[0].mark = Mark::Line {
        interpolation: Interpolation::StepAfter,
        points: true,
    };

    let mut combo = Recipes::bar("Layered combo", field("x"), field("positive"));
    combo.layers.push(
        Layer::new(Mark::Line {
            interpolation: Interpolation::Straight,
            points: true,
        })
        .encode(Encoding {
            channel: Channel::X,
            field: field("x"),
            scale: Some(Arc::from("x")),
        })
        .encode(Encoding {
            channel: Channel::Y,
            field: field("value"),
            scale: Some(Arc::from("y")),
        }),
    );

    let mut shared_facet = Recipes::line("Shared facets", field("x"), field("value"));
    shared_facet.facet = Some(Facet {
        row: None,
        column: Some(field("facet")),
        scales: FacetScalePolicy::Shared,
    });
    let mut independent_facet = shared_facet.clone();
    independent_facet.title = Arc::from("Independent facets");
    independent_facet.facet.as_mut().unwrap().scales = FacetScalePolicy::Independent;

    vec![
        ("line", Recipes::line("Line", field("x"), field("value"))),
        ("line-step-multi", step),
        ("area", Recipes::area("Area", field("x"), field("value"))),
        (
            "stacked-area",
            Recipes::stacked_area(
                "Stacked area",
                field("x"),
                field("series"),
                field("value"),
                false,
            ),
        ),
        (
            "normalized-area",
            Recipes::stacked_area(
                "Normalized area",
                field("x"),
                field("series"),
                field("value"),
                true,
            ),
        ),
        (
            "bar",
            Recipes::bar("Bar", field("category"), field("value")),
        ),
        (
            "horizontal-bar",
            Recipes::horizontal_bar("Horizontal bar", field("category"), field("value")),
        ),
        (
            "grouped-bar",
            Recipes::grouped_bar(
                "Grouped bar",
                field("category"),
                field("series"),
                field("value"),
            ),
        ),
        (
            "stacked-bar",
            Recipes::stacked_bar(
                "Stacked bar",
                field("category"),
                field("series"),
                field("value"),
                false,
            ),
        ),
        (
            "normalized-horizontal-bar",
            Recipes::stacked_horizontal_bar(
                "Normalized horizontal bar",
                field("category"),
                field("series"),
                field("value"),
                true,
            ),
        ),
        (
            "scatter",
            Recipes::scatter("Scatter", field("x"), field("value")),
        ),
        (
            "bubble",
            Recipes::bubble("Bubble", field("x"), field("value"), field("size")),
        ),
        (
            "pie",
            Recipes::pie("Pie", field("category"), field("positive")),
        ),
        (
            "donut",
            Recipes::donut("Donut", field("category"), field("positive"), 0.55),
        ),
        (
            "histogram",
            Recipes::histogram(
                "Histogram",
                field("value"),
                BinOptions {
                    start: -10.0,
                    end: 1.0e9,
                    count: 8,
                    density: false,
                },
            ),
        ),
        (
            "heatmap",
            Recipes::heatmap("Heatmap", field("x"), field("size"), field("value")),
        ),
        (
            "sparkline",
            Recipes::sparkline("Sparkline", field("x"), field("value")),
        ),
        ("combo", combo),
        ("shared-facet", shared_facet),
        ("independent-facet", independent_facet),
    ]
}

#[test]
fn every_family_is_deterministic_across_theme_size_and_scale_matrix() {
    let data = fixture_data(41);
    let limits = ChartLimits::default();
    let themes = [
        ChartTheme::default(),
        ChartTheme::dark(),
        ChartTheme::high_contrast(),
    ];
    let sizes = [(112.0, 84.0), (960.0, 540.0)];
    let scales = [1.0, 1.25, 1.5, 2.0];

    for (fixture_name, spec) in fixtures() {
        for (theme_index, theme) in themes.iter().enumerate() {
            for (width, height) in sizes {
                for scale_factor in scales {
                    let input = RetainedSurfaceInput::new(
                        Rect::new(17.0, 23.0, width, height),
                        scale_factor,
                        RetainedSurfaceRevisions {
                            content: data.revision(),
                            theme: theme_index as u64,
                            ..Default::default()
                        },
                    );
                    let render = || {
                        let model = ChartModel::new(
                            Arc::new(spec.clone()),
                            Arc::clone(&data),
                            limits.clone(),
                        )
                        .unwrap()
                        .with_theme(theme.clone())
                        .unwrap();
                        let mut view = ChartView::new(model).unwrap();
                        view.update(input).unwrap();
                        (view.snapshot().output.clone(), view.hits().to_vec())
                    };
                    let first = render();
                    let second = render();
                    assert_eq!(first, second, "nondeterministic fixture {fixture_name}");
                    assert!(!first.0.paint.is_empty(), "no paint for {fixture_name}");
                    assert!(
                        first.0.semantics.iter().any(|record| {
                            record.role == SemanticRole::GraphicalCustomControl
                                && record
                                    .label
                                    .as_deref()
                                    .is_some_and(|label| !label.is_empty())
                        }),
                        "missing chart semantics for {fixture_name}"
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "pinned headless GPU raster matrix; run explicitly for renderer admission"]
fn every_family_software_and_gpu_rasters_agree_within_declared_tolerance() {
    // Thin strokes, text edges, and curved boundaries use backend-specific
    // coverage. Across the full matrix we require at least 91% of pixels to
    // remain within 32/255 per channel and a mean delta below 8.25/255.
    const MAX_LARGE_DELTA_FRACTION: f64 = 0.09;
    const MAX_MEAN_CHANNEL_DELTA: f64 = 8.25;

    let data = fixture_data(42);
    let limits = ChartLimits::default();
    let gpu = twinkle_ui::GpuOffscreenRenderer::new()
        .expect("chart raster parity requires the pinned headless GPU environment");
    let themes = [
        ChartTheme::default(),
        ChartTheme::dark(),
        ChartTheme::high_contrast(),
    ];
    let sizes = [(112.0_f32, 84.0_f32), (960.0, 540.0)];
    for (fixture_name, spec) in fixtures() {
        for (theme_index, theme) in themes.iter().enumerate() {
            for (logical_width, logical_height) in sizes {
                for scale in [1.0_f32, 1.25, 1.5, 2.0] {
                    let input = RetainedSurfaceInput::new(
                        Rect::new(0.0, 0.0, logical_width, logical_height),
                        scale,
                        RetainedSurfaceRevisions {
                            content: data.revision(),
                            theme: theme_index as u64,
                            ..Default::default()
                        },
                    );
                    let model =
                        ChartModel::new(Arc::new(spec.clone()), Arc::clone(&data), limits.clone())
                            .unwrap()
                            .with_theme(theme.clone())
                            .unwrap();
                    let mut view = ChartView::new(model).unwrap();
                    let commands = &view.update(input).unwrap().output.paint;
                    let width = (logical_width * scale).ceil() as u32;
                    let height = (logical_height * scale).ceil() as u32;
                    let mut software =
                        twinkle_ui::SoftwareRenderer::new_pixel_buffer(width, height, scale);
                    software.render(commands);
                    let gpu_pixels = gpu.render(commands, width, height, scale).unwrap();
                    assert_eq!(gpu_pixels.len(), software.pixels().len());

                    let mut large_delta = 0usize;
                    let mut total_delta = 0u64;
                    for (gpu, software) in gpu_pixels.iter().zip(software.pixels()) {
                        let deltas = [
                            gpu.r.abs_diff(software.r),
                            gpu.g.abs_diff(software.g),
                            gpu.b.abs_diff(software.b),
                            gpu.a.abs_diff(software.a),
                        ];
                        if deltas.into_iter().max().unwrap_or(0) > 32 {
                            large_delta += 1;
                        }
                        total_delta += deltas.into_iter().map(u64::from).sum::<u64>();
                    }
                    let large_fraction = large_delta as f64 / gpu_pixels.len() as f64;
                    let mean_channel_delta = total_delta as f64 / (gpu_pixels.len() * 4) as f64;
                    assert!(
                        large_fraction <= MAX_LARGE_DELTA_FRACTION,
                        "{fixture_name} theme {theme_index} scale {scale}: large-delta fraction {large_fraction:.5}"
                    );
                    assert!(
                        mean_channel_delta <= MAX_MEAN_CHANNEL_DELTA,
                        "{fixture_name} theme {theme_index} scale {scale}: mean channel delta {mean_channel_delta:.5}"
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "pinned full software raster golden matrix; run explicitly for renderer admission"]
fn every_family_matches_pinned_software_raster_goldens() {
    const EXPECTED: &[(&str, u64)] = &[
        ("line", 0x2c48ee104684b694),
        ("line-step-multi", 0xf4c0e24f250281e6),
        ("area", 0xebbe186fb835ce12),
        ("stacked-area", 0x870aec46ea0d512a),
        ("normalized-area", 0x5d9d111c767bdd55),
        ("bar", 0x7641045b1150adb3),
        ("horizontal-bar", 0x0c220513978f0485),
        ("grouped-bar", 0x42f538c397a730aa),
        ("stacked-bar", 0x6726e42c4fa88ad9),
        ("normalized-horizontal-bar", 0x697e67cea533f3f2),
        ("scatter", 0xe817830a45f05c5c),
        ("bubble", 0x79866ca1fa706522),
        ("pie", 0x24787436005e4495),
        ("donut", 0x52f6f8b0f8135f34),
        ("histogram", 0x948cb6025af4c0be),
        ("heatmap", 0x2456d6035e14665a),
        ("sparkline", 0x3f02ac88f8799847),
        ("combo", 0xeb41b2df3fdf1d22),
        ("shared-facet", 0x960556bde5ad9cac),
        ("independent-facet", 0x679d5ac2386dc25b),
    ];
    fn mix(hash: &mut u64, bytes: &[u8]) {
        for byte in bytes {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(0x100000001b3);
        }
    }

    let data = fixture_data(43);
    let limits = ChartLimits::default();
    let themes = [
        ChartTheme::default(),
        ChartTheme::dark(),
        ChartTheme::high_contrast(),
    ];
    let sizes = [(112.0_f32, 84.0_f32), (960.0, 540.0)];
    let mut observed = Vec::new();
    for (fixture_name, spec) in fixtures() {
        let mut digest = 0xcbf29ce484222325_u64;
        mix(&mut digest, fixture_name.as_bytes());
        for (theme_index, theme) in themes.iter().enumerate() {
            for (logical_width, logical_height) in sizes {
                for scale in [1.0_f32, 1.25, 1.5, 2.0] {
                    let input = RetainedSurfaceInput::new(
                        Rect::new(0.0, 0.0, logical_width, logical_height),
                        scale,
                        RetainedSurfaceRevisions {
                            content: data.revision(),
                            theme: theme_index as u64,
                            ..Default::default()
                        },
                    );
                    let model =
                        ChartModel::new(Arc::new(spec.clone()), Arc::clone(&data), limits.clone())
                            .unwrap()
                            .with_theme(theme.clone())
                            .unwrap();
                    let mut view = ChartView::new(model).unwrap();
                    let commands = &view.update(input).unwrap().output.paint;
                    let width = (logical_width * scale).ceil() as u32;
                    let height = (logical_height * scale).ceil() as u32;
                    let mut renderer =
                        twinkle_ui::SoftwareRenderer::new_pixel_buffer(width, height, scale);
                    renderer.render(commands);
                    mix(&mut digest, &(theme_index as u64).to_le_bytes());
                    mix(&mut digest, &width.to_le_bytes());
                    mix(&mut digest, &height.to_le_bytes());
                    mix(&mut digest, &scale.to_bits().to_le_bytes());
                    for pixel in renderer.pixels() {
                        mix(&mut digest, &[pixel.r, pixel.g, pixel.b, pixel.a]);
                    }
                }
            }
        }
        observed.push((fixture_name, digest));
    }
    assert_eq!(observed, EXPECTED);
}

#[test]
fn empty_data_and_exact_table_remain_bounded_and_reachable() {
    let limits = ChartLimits {
        max_visible_table_rows: 5,
        ..ChartLimits::default()
    };
    let empty = Dataset::new(
        DatasetId::from("fixture"),
        Vec::<RowKey>::new(),
        vec![
            Column::new("x", Vec::<DataValue>::new()),
            Column::new("value", Vec::<DataValue>::new()),
        ],
    )
    .unwrap();
    let empty = Arc::new(ChartData::new(42, vec![empty], &limits).unwrap());
    let mut view = ChartView::new(
        ChartModel::new(
            Arc::new(Recipes::line("Empty", field("x"), field("value"))),
            Arc::clone(&empty),
            limits.clone(),
        )
        .unwrap(),
    )
    .unwrap();
    view.update(RetainedSurfaceInput::new(
        Rect::new(0.0, 0.0, 64.0, 48.0),
        2.0,
        RetainedSurfaceRevisions {
            content: 42,
            ..Default::default()
        },
    ))
    .unwrap();
    assert!(view.hits().is_empty());
    assert!(view.snapshot().output.paint.iter().any(
        |command| matches!(command, twinkle_ui::backend::PaintCommand::Text { text, .. } if text == "No data")
    ));

    let data = fixture_data(43);
    let table = ChartTable::new(Arc::clone(&data), limits);
    let mut offset = 0;
    let mut reached = Vec::new();
    loop {
        let page = table.page("fixture", offset).unwrap();
        assert!(page.rows.len() <= 5);
        reached.extend(page.rows.iter().map(|row| row.key.clone()));
        let Some(next) = table.next_offset(&page) else {
            break;
        };
        offset = next;
    }
    assert_eq!(reached, data.dataset("fixture").unwrap().keys());
}

#[test]
fn first_failure_has_bounded_semantic_fallback_and_table_action() {
    let data = fixture_data(44);
    let spec = Recipes::pie("Invalid negative pie", field("category"), field("value"));
    let mut view =
        ChartView::new(ChartModel::new(Arc::new(spec), data, ChartLimits::default()).unwrap())
            .unwrap();
    let input = RetainedSurfaceInput::new(
        Rect::new(7.0, 11.0, 180.0, 96.0),
        1.5,
        RetainedSurfaceRevisions {
            content: 44,
            ..Default::default()
        },
    );
    assert!(view.update(input).is_err());
    assert!(view.last_error().is_some());
    let snapshot = view.snapshot();
    assert_eq!(snapshot.input, Some(input));
    assert_eq!(snapshot.output.interactions.len(), 2);
    assert_eq!(snapshot.output.interactions[0].bounds, input.bounds);
    assert!(snapshot.output.paint.len() <= 3);
    assert!(snapshot.output.semantics.iter().any(|record| {
        record.state.as_deref().is_some_and(|state| {
            state.contains("Chart unavailable") && state.contains("exact paged data table")
        })
    }));
    snapshot.try_element("error-chart", |_| ()).unwrap();
}

//! Serial release-admission measurements for specification 0025.
//!
//! Run with:
//! `cargo run --release -p twinkle-charts --example measure`

use std::{
    hint::black_box,
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

use twinkle_charts::{
    ChartData, ChartLimits, ChartModel, ChartState, ChartView, Column, DataValue, Dataset,
    DatasetId, Facet, FacetScalePolicy, FieldRef, Recipes, RowKey, Viewport,
};
use twinkle_ui::{
    Point, Rect, UiFrame,
    retained_surface::{RetainedSurfaceInput, RetainedSurfaceRevisions},
};

const COLD_SAMPLES: usize = 11;
const WARM_SAMPLES: usize = 101;
const VIEWPORT: Rect = Rect::new(0.0, 0.0, 1_200.0, 800.0);

fn field(name: &str) -> FieldRef {
    FieldRef::new("bench", name)
}

fn data(rows: usize, revision: u64, facets: usize) -> Arc<ChartData> {
    let facet_side = (facets as f64).sqrt().ceil().max(1.0) as usize;
    let keys = (0..rows)
        .map(|index| RowKey::from(index.to_string()))
        .collect::<Vec<_>>();
    let dataset = Dataset::new(
        DatasetId::from("bench"),
        keys,
        vec![
            Column::new(
                "x",
                (0..rows)
                    .map(|index| DataValue::Number(index as f64))
                    .collect::<Vec<_>>(),
            ),
            Column::new(
                "y",
                (0..rows)
                    .map(|index| {
                        let x = index as f64 * 0.007;
                        DataValue::Number(x.sin() * 100.0 + (x * 0.13).cos() * 20.0)
                    })
                    .collect::<Vec<_>>(),
            ),
            Column::new(
                "positive",
                (0..rows)
                    .map(|index| DataValue::Number((index % 97 + 1) as f64))
                    .collect::<Vec<_>>(),
            ),
            Column::new(
                "series",
                (0..rows)
                    .map(|index| DataValue::Category(Arc::from(format!("s{}", index % 8))))
                    .collect::<Vec<_>>(),
            ),
            Column::new(
                "facet_row",
                (0..rows)
                    .map(|index| DataValue::Category(Arc::from(format!("r{}", index % facet_side))))
                    .collect::<Vec<_>>(),
            ),
            Column::new(
                "facet_column",
                (0..rows)
                    .map(|index| {
                        DataValue::Category(Arc::from(format!(
                            "c{}",
                            (index / facet_side) % facet_side
                        )))
                    })
                    .collect::<Vec<_>>(),
            ),
        ],
    )
    .unwrap();
    Arc::new(ChartData::new(revision, vec![dataset], &ChartLimits::default()).unwrap())
}

fn input(revision: u64) -> RetainedSurfaceInput {
    RetainedSurfaceInput::new(
        VIEWPORT,
        1.0,
        RetainedSurfaceRevisions {
            content: revision,
            ..Default::default()
        },
    )
}

fn distribution(samples: &mut [Duration]) -> (u128, u128) {
    samples.sort_unstable();
    let at = |percentile: usize| {
        samples[((samples.len() * percentile).div_ceil(100)).saturating_sub(1)].as_nanos()
    };
    (at(50), at(95))
}

fn measure(samples: usize, mut operation: impl FnMut()) -> (u128, u128) {
    operation();
    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        operation();
        durations.push(started.elapsed());
    }
    distribution(&mut durations)
}

fn cold(
    name: &str,
    data: Arc<ChartData>,
    spec: Arc<twinkle_charts::ChartSpec>,
) -> Result<(), twinkle_charts::ChartError> {
    let revision = data.revision();
    let limits = ChartLimits::default();
    let mut displayed = 0;
    let mut bytes = 0;
    let timings = measure(COLD_SAMPLES, || {
        let model = ChartModel::new(Arc::clone(&spec), Arc::clone(&data), limits.clone()).unwrap();
        let mut view = ChartView::new(model).unwrap();
        view.update(input(revision)).unwrap();
        displayed = view.diagnostics().displayed_rows;
        bytes = view.diagnostics().steady_bytes;
        black_box(view.snapshot());
    });
    println!(
        "{name},{COLD_SAMPLES},{},{},{},{displayed},{bytes}",
        timings.0,
        timings.1,
        data.row_count()
    );
    Ok(())
}

fn command_text(program: &str, arguments: &[&str]) -> String {
    Command::new(program)
        .args(arguments)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .replace(',', " ")
        })
        .unwrap_or_else(|| "unavailable".into())
}

fn main() -> Result<(), twinkle_charts::ChartError> {
    assert!(
        !cfg!(debug_assertions),
        "run the chart admission harness with --release"
    );
    println!("# schema=twinkle-chart-admission-v1");
    println!(
        "# target={}-{}",
        std::env::consts::ARCH,
        std::env::consts::OS
    );
    println!("# rustc={}", command_text("rustc", &["-Vv"]));
    println!(
        "# cpu={}",
        command_text("sh", &["-c", "grep -m1 'model name' /proc/cpuinfo"])
    );
    println!(
        "# memory={}",
        command_text("sh", &["-c", "grep -m1 'MemTotal' /proc/meminfo"])
    );
    println!(
        "# profile=release viewport=1200x800 scale=1 samples_cold={COLD_SAMPLES} samples_warm={WARM_SAMPLES} serial=true"
    );
    println!("scenario,samples,p50_ns,p95_ns,source_rows,displayed_rows,steady_bytes");

    for rows in [1_000, 100_000, 1_000_000] {
        let source = data(rows, rows as u64, 1);
        cold(
            &format!("line_{rows}"),
            Arc::clone(&source),
            Arc::new(Recipes::line("Line", field("x"), field("y"))),
        )?;
        cold(
            &format!("scatter_{rows}"),
            source,
            Arc::new(Recipes::scatter("Scatter", field("x"), field("y"))),
        )?;
    }

    let heatmap = data(10_000, 20_001, 1);
    cold(
        "heatmap_10000",
        heatmap,
        Arc::new(Recipes::heatmap(
            "Heatmap",
            field("x"),
            field("positive"),
            field("y"),
        )),
    )?;
    let bars = data(10_000, 20_002, 1);
    cold(
        "grouped_bar_10000",
        Arc::clone(&bars),
        Arc::new(Recipes::grouped_bar(
            "Grouped",
            field("x"),
            field("series"),
            field("positive"),
        )),
    )?;
    cold(
        "stacked_bar_10000",
        bars,
        Arc::new(Recipes::stacked_bar(
            "Stacked",
            field("x"),
            field("series"),
            field("positive"),
            false,
        )),
    )?;
    let facets = data(6_400, 20_003, 64);
    let mut facet_spec = Recipes::line("64 facets", field("x"), field("y"));
    facet_spec.facet = Some(Facet {
        row: Some(field("facet_row")),
        column: Some(field("facet_column")),
        scales: FacetScalePolicy::Shared,
    });
    cold("facets_64", facets, Arc::new(facet_spec))?;

    let million = data(1_000_000, 30_001, 1);
    let mut inspection = ChartView::new(ChartModel::new(
        Arc::new(Recipes::line("Inspection", field("x"), field("y"))),
        Arc::clone(&million),
        ChartLimits::default(),
    )?)?;
    inspection.update(input(million.revision()))?;
    let inspect = measure(WARM_SAMPLES, || {
        black_box(inspection.inspect_nearest(Point { x: 611.0, y: 377.0 }));
    });
    println!(
        "inspect_line_1000000,{WARM_SAMPLES},{},{},1000000,{},{}",
        inspect.0,
        inspect.1,
        inspection.diagnostics().displayed_rows,
        inspection.diagnostics().steady_bytes
    );

    let dashboard_data = data(10_000, 40_001, 1);
    let dashboard_spec = Arc::new(Recipes::line("Dashboard", field("x"), field("y")));
    let mut dashboard = (0..16)
        .map(|_| {
            let mut view = ChartView::new(
                ChartModel::new(
                    Arc::clone(&dashboard_spec),
                    Arc::clone(&dashboard_data),
                    ChartLimits::default(),
                )
                .unwrap(),
            )
            .unwrap();
            view.update(input(dashboard_data.revision())).unwrap();
            view
        })
        .collect::<Vec<_>>();
    let unchanged = measure(WARM_SAMPLES, || {
        for view in &mut dashboard {
            black_box(view.update(input(dashboard_data.revision())).unwrap());
        }
    });
    println!(
        "dashboard_16_unchanged,{WARM_SAMPLES},{},{},160000,0,{}",
        unchanged.0,
        unchanged.1,
        dashboard
            .iter()
            .map(|view| view.diagnostics().steady_bytes)
            .sum::<usize>()
    );

    let mut revision = 50_000_u64;
    let updated = measure(COLD_SAMPLES, || {
        revision += 1;
        let state = ChartState {
            viewport: Some(Viewport {
                x: Some([2_000.0, 8_000.0]),
                y: None,
            }),
            ..ChartState::default()
        };
        black_box(dashboard[0].update_state(state, input(revision)).unwrap());
        for view in &mut dashboard[1..] {
            black_box(view.update(input(dashboard_data.revision())).unwrap());
        }
    });
    println!(
        "dashboard_15_unchanged_1_viewport,{COLD_SAMPLES},{},{},160000,{},{}",
        updated.0,
        updated.1,
        dashboard[0].diagnostics().displayed_rows,
        dashboard
            .iter()
            .map(|view| view.diagnostics().steady_bytes)
            .sum::<usize>()
    );

    let native_frame = measure(WARM_SAMPLES, || {
        let element = dashboard[0].element("bench-chart", |_| ()).unwrap();
        black_box(UiFrame::layout(element, VIEWPORT));
    });
    println!(
        "native_frame_chart_10000,{WARM_SAMPLES},{},{},10000,{},{}",
        native_frame.0,
        native_frame.1,
        dashboard[0].diagnostics().displayed_rows,
        dashboard[0].diagnostics().steady_bytes
    );
    Ok(())
}

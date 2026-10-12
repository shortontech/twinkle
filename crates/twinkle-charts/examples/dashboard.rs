use std::{collections::BTreeSet, sync::Arc};

use twinkle_charts::{
    ChartData, ChartLimits, ChartModel, ChartState, ChartTable, ChartTableFilter, ChartView, Column,
    CsvOptions, DataValue, Dataset, DatasetId, FieldRef, Legend, RasterOptions, Recipes, RowKey,
    Scale, ScaleKind, export_csv, export_raster,
};
use twinkle_ui::{
    Rect, UiFrame,
    retained_surface::{RetainedSurfaceInput, RetainedSurfaceRevisions},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let limits = ChartLimits::default();
    let data = Arc::new(dashboard_data(&limits)?);
    let specs = dashboard_specs();
    let input = RetainedSurfaceInput::new(
        Rect::new(0.0, 0.0, 640.0, 360.0),
        1.25,
        RetainedSurfaceRevisions {
            content: data.revision(),
            ..Default::default()
        },
    );

    let mut accepted = Vec::new();
    for mut spec in specs {
        if spec
            .layers
            .iter()
            .flat_map(|layer| &layer.encodings)
            .any(|encoding| encoding.channel == twinkle_charts::Channel::Series)
        {
            for encoding in spec
                .layers
                .iter_mut()
                .flat_map(|layer| &mut layer.encodings)
                .filter(|encoding| encoding.channel == twinkle_charts::Channel::Series)
            {
                encoding.scale = Some(Arc::from("series"));
            }
            spec.scales.push(Scale {
                name: Arc::from("series"),
                kind: ScaleKind::Color,
                domain: None,
                clamp: true,
                reversed: false,
                unit: None,
            });
            spec.legends.push(Legend {
                scale: Arc::from("series"),
                title: Some(Arc::from("Region")),
                toggleable: true,
            });
        }
        let state = ChartState {
            visible_series: Some(
                [Arc::from("North"), Arc::from("South")]
                    .into_iter()
                    .collect::<BTreeSet<_>>(),
            ),
            ..Default::default()
        };
        let mut view = ChartView::new(
            ChartModel::new(Arc::new(spec), data.clone(), limits.clone())?.with_state(state),
        )?;
        view.update(input)?;
        println!(
            "{}",
            view.snapshot().output.semantics[0]
                .label
                .as_deref()
                .unwrap_or("Chart")
        );
        println!("  {:?}", view.diagnostics());
        accepted.push(view.snapshot().clone());
    }

    let table = ChartTable::new(data.clone(), limits);
    let first_page = table.page_filtered(
        "sales",
        0,
        &ChartTableFilter {
            series_column: Some(Arc::from("region")),
            visible_series: Some([Arc::from("North")].into_iter().collect()),
            include_hidden_series: false,
        },
    )?;
    println!(
        "table rows {} of {}",
        first_page.rows.len(),
        first_page.total_rows
    );
    let table_frame = UiFrame::layout(
        table.element(&first_page, "sales-table", |event| event),
        Rect::new(0.0, 0.0, 640.0, 360.0),
    );
    println!(
        "table semantic nodes {}",
        table_frame.semantic_nodes().len()
    );
    println!(
        "CSV bytes: {}",
        export_csv(&data, CsvOptions::default())?.len()
    );
    println!(
        "RGBA bytes: {}",
        export_raster(
            &accepted[0],
            RasterOptions {
                logical_width: 640,
                logical_height: 360,
                scale: 1.0,
                ..Default::default()
            },
        )?
        .len()
    );
    Ok(())
}

fn dashboard_data(limits: &ChartLimits) -> Result<ChartData, twinkle_charts::ChartError> {
    let rows = 8;
    Dataset::new(
        DatasetId::from("sales"),
        (0..rows)
            .map(|index| RowKey::from(format!("row-{index}")))
            .collect::<Vec<_>>(),
        vec![
            Column::new(
                "month",
                (0..rows)
                    .map(|index| DataValue::Number((index / 2 + 1) as f64))
                    .collect::<Vec<_>>(),
            ),
            Column::new(
                "region",
                (0..rows)
                    .map(|index| {
                        DataValue::Category(Arc::from(if index % 2 == 0 {
                            "North"
                        } else {
                            "South"
                        }))
                    })
                    .collect::<Vec<_>>(),
            ),
            Column::new(
                "revenue",
                [10.0, 18.0, 14.0, 22.0, 17.0, 25.0, 20.0, 29.0]
                    .into_iter()
                    .map(DataValue::Number)
                    .collect::<Vec<_>>(),
            )
            .with_unit("USD"),
            Column::new(
                "orders",
                [3.0, 6.0, 4.0, 8.0, 5.0, 10.0, 7.0, 12.0]
                    .into_iter()
                    .map(DataValue::Number)
                    .collect::<Vec<_>>(),
            ),
        ],
    )
    .and_then(|dataset| ChartData::new(1, vec![dataset], limits))
}

fn dashboard_specs() -> Vec<twinkle_charts::ChartSpec> {
    let field = |name| FieldRef::new("sales", name);
    vec![
        Recipes::line("Revenue trend", field("month"), field("revenue")),
        Recipes::grouped_bar(
            "Revenue by month",
            field("month"),
            field("region"),
            field("revenue"),
        ),
        Recipes::stacked_area(
            "Revenue composition",
            field("month"),
            field("region"),
            field("revenue"),
            false,
        ),
        Recipes::bubble(
            "Orders and revenue",
            field("orders"),
            field("revenue"),
            field("orders"),
        ),
        Recipes::donut("Revenue share", field("region"), field("revenue"), 0.55),
        Recipes::histogram(
            "Order distribution",
            field("orders"),
            twinkle_charts::BinOptions {
                start: 0.0,
                end: 12.0,
                count: 4,
                density: false,
            },
        ),
        Recipes::heatmap(
            "Revenue heatmap",
            field("month"),
            field("orders"),
            field("revenue"),
        ),
        Recipes::sparkline("Revenue sparkline", field("month"), field("revenue")),
    ]
}

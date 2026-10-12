use twinkle_ui::{SoftwareRenderer, retained_surface::RetainedSurfaceSnapshot};

use crate::{ChartData, ChartError, ChartStage, DataValue};

/// Export support advertised without probing by failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportCapability {
    pub csv: bool,
    pub raster_rgba8: bool,
    pub svg: bool,
}
impl Default for ExportCapability {
    fn default() -> Self {
        Self {
            csv: true,
            raster_rgba8: true,
            svg: false,
        }
    }
}

/// CSV serialization policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CsvOptions {
    pub include_dataset: bool,
    pub include_units: bool,
    pub protect_spreadsheet_formulas: bool,
    pub max_bytes: usize,
}
impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            include_dataset: true,
            include_units: true,
            protect_spreadsheet_formulas: true,
            max_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Explicit bounded raster request.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RasterOptions {
    pub logical_width: u32,
    pub logical_height: u32,
    pub scale: f32,
    pub background: Option<u32>,
    pub max_pixels: usize,
    pub max_bytes: usize,
}
impl Default for RasterOptions {
    fn default() -> Self {
        Self {
            logical_width: 1200,
            logical_height: 800,
            scale: 1.0,
            background: None,
            max_pixels: 16_777_216,
            max_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Export all admitted source data in stable dataset/row/column order.
pub fn export_csv(data: &ChartData, options: CsvOptions) -> Result<Vec<u8>, ChartError> {
    let mut out = Vec::new();
    for dataset in data.datasets() {
        let mut header = Vec::new();
        if options.include_dataset {
            header.push("dataset".to_owned());
        }
        header.push("row_key".to_owned());
        header.extend(dataset.columns().iter().map(|column| {
            if options.include_units {
                column.unit.as_ref().map_or_else(
                    || column.name.to_string(),
                    |unit| format!("{} ({unit})", column.name),
                )
            } else {
                column.name.to_string()
            }
        }));
        push_record(&mut out, &header, options)?;
        for row in dataset.rows() {
            let mut record = Vec::with_capacity(header.len());
            if options.include_dataset {
                record.push(dataset.id.0.to_string());
            }
            record.push(row.key().0.to_string());
            for column in dataset.columns() {
                record.push(format_value(&column.values[row.index()]));
            }
            push_record(&mut out, &record, options)?;
        }
    }
    Ok(out)
}

/// Render one already-accepted snapshot to bounded RGBA8 pixels.
pub fn export_raster(
    snapshot: &RetainedSurfaceSnapshot,
    options: RasterOptions,
) -> Result<Vec<u8>, ChartError> {
    if snapshot.input.is_none() {
        return Err(ChartError::invalid(
            ChartStage::Export,
            "chart has no accepted revision",
        ));
    }
    if !options.scale.is_finite()
        || options.scale <= 0.0
        || options.logical_width == 0
        || options.logical_height == 0
    {
        return Err(ChartError::invalid(
            ChartStage::Export,
            "raster dimensions and scale must be positive",
        ));
    }
    let width = (options.logical_width as f64 * options.scale as f64).ceil();
    let height = (options.logical_height as f64 * options.scale as f64).ceil();
    if width > u32::MAX as f64 || height > u32::MAX as f64 {
        return Err(ChartError::invalid(
            ChartStage::Export,
            "raster dimensions overflow",
        ));
    }
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| ChartError::invalid(ChartStage::Export, "raster pixel count overflow"))?;
    if pixels > options.max_pixels {
        return Err(ChartError::Limit {
            resource: "export pixels",
            actual: pixels,
            limit: options.max_pixels,
        });
    }
    let bytes = pixels
        .checked_mul(4)
        .ok_or_else(|| ChartError::invalid(ChartStage::Export, "raster byte count overflow"))?;
    if bytes > options.max_bytes {
        return Err(ChartError::Limit {
            resource: "export bytes",
            actual: bytes,
            limit: options.max_bytes,
        });
    }
    let mut commands =
        Vec::with_capacity(snapshot.output.paint.len() + usize::from(options.background.is_some()));
    if let Some(color) = options.background {
        commands.push(twinkle_ui::backend::PaintCommand::Fill {
            rect: twinkle_ui::Rect::new(
                0.0,
                0.0,
                options.logical_width as f32,
                options.logical_height as f32,
            ),
            color,
        });
    }
    commands.extend(snapshot.output.paint.iter().cloned());
    let mut renderer =
        SoftwareRenderer::new_pixel_buffer(width as u32, height as u32, options.scale);
    renderer.render(&commands);
    let mut rgba = Vec::with_capacity(bytes);
    for pixel in renderer.pixels() {
        rgba.extend([pixel.r, pixel.g, pixel.b, pixel.a]);
    }
    Ok(rgba)
}

fn format_value(value: &DataValue) -> String {
    match value {
        DataValue::Null => String::new(),
        DataValue::Number(value) => value.to_string(),
        DataValue::Timestamp(value) => value.to_string(),
        DataValue::Category(value) => value.to_string(),
        DataValue::Boolean(value) => value.to_string(),
    }
}
fn push_record(
    out: &mut Vec<u8>,
    values: &[String],
    options: CsvOptions,
) -> Result<(), ChartError> {
    let mut line = String::new();
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            line.push(',');
        }
        let mut value = value.as_str();
        let protected;
        if options.protect_spreadsheet_formulas && value.starts_with(['=', '+', '-', '@']) {
            protected = format!("'{value}");
            value = &protected;
        }
        line.push('"');
        line.push_str(&value.replace('"', "\"\""));
        line.push('"');
    }
    line.push_str("\r\n");
    let next = out
        .len()
        .checked_add(line.len())
        .ok_or_else(|| ChartError::invalid(ChartStage::Export, "CSV size overflow"))?;
    if next > options.max_bytes {
        return Err(ChartError::Limit {
            resource: "CSV bytes",
            actual: next,
            limit: options.max_bytes,
        });
    }
    out.extend_from_slice(line.as_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChartLimits, Column, Dataset, DatasetId, RowKey};
    use std::sync::Arc;
    #[test]
    fn csv_quotes_and_protects_formulas() {
        let dataset = Dataset::new(
            DatasetId::from("d"),
            vec![RowKey::from("1")],
            vec![Column::new(
                "label",
                vec![DataValue::Category(Arc::from("=1+1,\"x\""))],
            )],
        )
        .unwrap();
        let data = ChartData::new(1, vec![dataset], &ChartLimits::default()).unwrap();
        let text = String::from_utf8(export_csv(&data, CsvOptions::default()).unwrap()).unwrap();
        assert!(text.contains("'=1+1,\"\"x\"\""));
    }
    #[test]
    fn csv_limit_is_enforced_before_growth() {
        let data = ChartData::new(1, Vec::<Dataset>::new(), &ChartLimits::default()).unwrap();
        assert!(
            export_csv(
                &data,
                CsvOptions {
                    max_bytes: 0,
                    ..Default::default()
                }
            )
            .unwrap()
            .is_empty()
        );
    }
}

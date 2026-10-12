//! Bounded, composable chart preparation and native Twinkle projection.
//!
//! Applications own [`ChartData`], [`ChartSpec`], and controlled [`ChartState`].
//! [`ChartModel`] prepares immutable accepted snapshots; recipes are ordinary
//! specs and can be inspected or extended before preparation.

mod data;
mod error;
mod export;
mod interaction;
mod model;
mod scale;
mod spec;
mod table;
mod time;
mod transform;

pub use data::{ChartData, Column, DataValue, Dataset, DatasetId, Row, RowKey};
pub use error::{ChartDiagnostic, ChartError, ChartStage, DiagnosticSeverity};
pub use export::{CsvOptions, ExportCapability, RasterOptions, export_csv, export_raster};
pub use interaction::{ChartNavigation, NavigationCommand, PanZoomPolicy};
pub use model::{ChartHit, ChartModel, ChartView};
pub use scale::{BandScale, ContinuousScale, LinearScale, LogScale, ScaleValue};
pub use spec::*;
pub use table::{ChartTable, ChartTableCell, ChartTablePage, ChartTableRow};
pub use time::{CalendarTick, TimeScale, TimeTickInterval, TimeZonePolicy};
pub use transform::{
    AggregateOp, BinOptions, MissingPolicy, StackMode, Transform, execute_transforms,
};

/// Crate version for diagnostics and export manifests.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

use std::{error::Error, fmt};

/// Preparation stage associated with a diagnostic or failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChartStage {
    Validate,
    Transform,
    Domain,
    Layout,
    Geometry,
    Interaction,
    Semantics,
    Export,
}

/// Diagnostic severity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticSeverity {
    Info,
    Warning,
}

/// Inspectable non-fatal chart diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChartDiagnostic {
    pub stage: ChartStage,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub affected_rows: usize,
}

/// Structured validation, capability, or resource failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChartError {
    Invalid {
        stage: ChartStage,
        message: String,
    },
    MissingDataset(String),
    MissingColumn {
        dataset: String,
        column: String,
    },
    TypeMismatch {
        column: String,
        expected: &'static str,
    },
    DuplicateKey(String),
    Limit {
        resource: &'static str,
        actual: usize,
        limit: usize,
    },
    Unsupported(String),
    StaleRevision {
        expected: u64,
        actual: u64,
    },
}

impl ChartError {
    pub(crate) fn invalid(stage: ChartStage, message: impl Into<String>) -> Self {
        Self::Invalid {
            stage,
            message: message.into(),
        }
    }
}

impl fmt::Display for ChartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { stage, message } => write!(f, "{stage:?}: {message}"),
            Self::MissingDataset(value) => write!(f, "missing dataset `{value}`"),
            Self::MissingColumn { dataset, column } => {
                write!(f, "missing column `{column}` in `{dataset}`")
            }
            Self::TypeMismatch { column, expected } => {
                write!(f, "column `{column}` is not {expected}")
            }
            Self::DuplicateKey(value) => write!(f, "duplicate stable key `{value}`"),
            Self::Limit {
                resource,
                actual,
                limit,
            } => write!(f, "{resource} limit exceeded ({actual} > {limit})"),
            Self::Unsupported(value) => write!(f, "unsupported chart capability: {value}"),
            Self::StaleRevision { expected, actual } => {
                write!(f, "stale revision {actual}; expected {expected}")
            }
        }
    }
}

impl Error for ChartError {}

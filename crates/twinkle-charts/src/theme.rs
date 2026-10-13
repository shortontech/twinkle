use std::sync::Arc;

use crate::{ChartError, ChartStage, Symbol};

const HIGH_CONTRAST_SURFACE: u32 = 0;
const HIGH_CONTRAST_TEXT: u32 = 0xFF_FF_FF;

/// Non-color cue used when series outnumber categorical colors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SeriesCue {
    pub color: u32,
    pub symbol: Symbol,
    pub dash: Option<[u8; 2]>,
    pub palette_exhausted: bool,
}

/// Font sizes shared by chart measurement and retained text painting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartTypography {
    /// Logical pixels, independent of device scale.
    pub title: f32,
    pub body: f32,
    pub label: f32,
    pub control: f32,
}

impl Default for ChartTypography {
    fn default() -> Self {
        Self {
            title: 12.0,
            body: 12.0,
            label: 12.0,
            control: 12.0,
        }
    }
}

/// Semantic chart tokens shared by native recipes.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartTheme {
    pub typography: ChartTypography,
    pub background: u32,
    pub plot_background: u32,
    pub foreground: u32,
    pub muted: u32,
    pub grid: u32,
    pub focus: u32,
    pub selection: u32,
    pub missing: u32,
    pub categorical: Arc<[u32]>,
    pub high_contrast: bool,
}

impl Default for ChartTheme {
    fn default() -> Self {
        Self {
            typography: ChartTypography::default(),
            background: 0xF7F9FC,
            plot_background: 0xFFFFFF,
            foreground: 0x172033,
            muted: 0x667085,
            grid: 0xCBD2DD,
            focus: 0x005FCC,
            selection: 0xA23E48,
            missing: 0xAAB2BF,
            categorical: Arc::from([
                0x0072B2, 0xE69F00, 0x009E73, 0xCC79A7, 0xF0E442, 0x56B4E9, 0xD55E00, 0x6B5B95,
            ]),
            high_contrast: false,
        }
    }
}

impl ChartTheme {
    pub fn dark() -> Self {
        Self {
            typography: ChartTypography::default(),
            background: 0x151A23,
            plot_background: 0x1D2430,
            foreground: 0xF4F6FA,
            muted: 0xB2BBC9,
            grid: 0x465064,
            focus: 0x72B7FF,
            selection: 0xFF8996,
            missing: 0x737E91,
            categorical: Arc::from([
                0x56B4E9, 0xE69F00, 0x5FD39B, 0xD890E8, 0xF0E442, 0x86C5F4, 0xF47B4E, 0xA99BE8,
            ]),
            high_contrast: false,
        }
    }
    pub fn high_contrast() -> Self {
        Self {
            typography: ChartTypography::default(),
            background: HIGH_CONTRAST_SURFACE,
            plot_background: HIGH_CONTRAST_SURFACE,
            foreground: HIGH_CONTRAST_TEXT,
            muted: HIGH_CONTRAST_TEXT,
            grid: HIGH_CONTRAST_TEXT,
            focus: 0x00FFFF,
            selection: 0xFFFF00,
            missing: 0xBFBFBF,
            categorical: Arc::from([0x00FFFF, 0xFFFF00, 0xFF7FFF, 0x7FFF7F]),
            high_contrast: true,
        }
    }
    pub fn validate(&self) -> Result<(), ChartError> {
        let fonts = self.typography;
        if [fonts.title, fonts.body, fonts.label, fonts.control]
            .into_iter()
            .any(|size| !size.is_finite() || size <= 0.0)
        {
            return Err(ChartError::invalid(
                ChartStage::Validate,
                "chart font sizes must be finite and positive logical pixels",
            ));
        }
        if self.categorical.is_empty() {
            return Err(ChartError::invalid(
                ChartStage::Validate,
                "chart theme requires at least one categorical color",
            ));
        }
        if self.categorical.len() > 256 {
            return Err(ChartError::Limit {
                resource: "categorical palette entries",
                actual: self.categorical.len(),
                limit: 256,
            });
        }
        Ok(())
    }
    pub fn series_cue(&self, index: usize) -> SeriesCue {
        let symbols = [
            Symbol::Circle,
            Symbol::Square,
            Symbol::Triangle,
            Symbol::Diamond,
            Symbol::Cross,
        ];
        let cycle = index / self.categorical.len();
        SeriesCue {
            color: self.categorical[index % self.categorical.len()],
            symbol: symbols[cycle % symbols.len()],
            dash: match cycle % 4 {
                0 => None,
                1 => Some([6, 3]),
                2 => Some([2, 3]),
                _ => Some([8, 3]),
            },
            palette_exhausted: index >= self.categorical.len(),
        }
    }
    pub fn sequential(&self, t: f64) -> u32 {
        interpolate(self.plot_background, self.categorical[0], t.clamp(0.0, 1.0))
    }
    pub fn diverging(&self, t: f64, midpoint: f64) -> u32 {
        let midpoint = midpoint.clamp(0.0, 1.0);
        if t <= midpoint {
            interpolate(
                self.categorical.get(1).copied().unwrap_or(self.muted),
                self.plot_background,
                if midpoint == 0.0 { 1.0 } else { t / midpoint },
            )
        } else {
            interpolate(
                self.plot_background,
                self.categorical[0],
                if midpoint == 1.0 {
                    1.0
                } else {
                    (t - midpoint) / (1.0 - midpoint)
                },
            )
        }
    }
}

fn interpolate(a: u32, b: u32, t: f64) -> u32 {
    let channel = |shift| {
        let a = ((a >> shift) & 0xff_u32) as f64;
        let b = ((b >> shift) & 0xff_u32) as f64;
        (a + (b - a) * t).round().clamp(0.0, 255.0) as u32
    };
    channel(16) << 16 | channel(8) << 8 | channel(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typography_rejects_nonpositive_and_nonfinite_sizes() {
        for invalid in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            for role in 0..4 {
                let mut theme = ChartTheme::default();
                match role {
                    0 => theme.typography.title = invalid,
                    1 => theme.typography.body = invalid,
                    2 => theme.typography.label = invalid,
                    _ => theme.typography.control = invalid,
                }
                assert!(theme.validate().is_err());
            }
        }
    }
    #[test]
    fn palette_exhaustion_adds_secondary_cues() {
        let theme = ChartTheme::default();
        let cue = theme.series_cue(theme.categorical.len());
        assert!(cue.palette_exhausted);
        assert_ne!(cue.symbol, theme.series_cue(0).symbol);
        assert!(cue.dash.is_some());
    }
    #[test]
    fn diverging_midpoint_is_plot_background() {
        let theme = ChartTheme::default();
        assert_eq!(theme.diverging(0.4, 0.4), theme.plot_background);
    }
}

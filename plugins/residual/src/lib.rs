//! Scalar reference operations for structure, texture, and residual fields.
//!
//! Frame transforms use caller-provided planes. [`spectrum`] is analysis and
//! returns its variable-sized frequency field as owned data.

use core::fmt;
use vsip_core::{Extent, Plane, PlaneMut};
use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-residual",
    namespace: "residual",
    summary: "Structure, texture and noise decomposition",
    filters: &[
        Filter {
            name: "Decompose",
            summary: "Separate structure, texture and residual",
            maturity: Maturity::Experimental,
            execution: Execution::Analysis,
        },
        Filter {
            name: "Temporal",
            summary: "Measure motion-aligned temporal residual",
            maturity: Maturity::Experimental,
            execution: Execution::Temporal,
        },
        Filter {
            name: "Spectrum",
            summary: "Measure residual power spectrum",
            maturity: Maturity::Experimental,
            execution: Execution::Analysis,
        },
        Filter {
            name: "Compare",
            summary: "Compare residual fields between clips",
            maturity: Maturity::Experimental,
            execution: Execution::MultiInput,
        },
    ],
};

/// Failure returned by residual operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResidualError {
    /// Inputs or outputs have different visible extents.
    ExtentMismatch,
    /// Spectrum analysis does not define a power value for non-finite samples.
    NonFiniteSample,
    /// The owned spectrum result could not reserve its output storage.
    AllocationFailed,
}

impl fmt::Display for ResidualError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ExtentMismatch => "input and output extents differ",
            Self::NonFiniteSample => "spectrum analysis requires finite samples",
            Self::AllocationFailed => "spectrum output allocation failed",
        })
    }
}

impl std::error::Error for ResidualError {}

/// Invalid construction input for [`DecomposeConfig`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecomposeConfigError {
    /// Texture smoothing was not strictly narrower than structure smoothing.
    InvalidRadiusOrder,
}

impl fmt::Display for DecomposeConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("texture radius must be strictly smaller than structure radius")
    }
}

impl std::error::Error for DecomposeConfigError {}

/// Validated scale separation settings for [`decompose`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecomposeConfig {
    texture_radius: usize,
    structure_radius: usize,
}

impl DecomposeConfig {
    /// Creates a two-scale decomposition configuration.
    ///
    /// A texture radius of zero preserves the source at the fine scale. The
    /// wider structure radius must be positive through the ordering invariant.
    pub const fn new(
        texture_radius: usize,
        structure_radius: usize,
    ) -> Result<Self, DecomposeConfigError> {
        if texture_radius >= structure_radius {
            return Err(DecomposeConfigError::InvalidRadiusOrder);
        }
        Ok(Self {
            texture_radius,
            structure_radius,
        })
    }

    /// Returns the fine-scale box radius.
    pub const fn texture_radius(self) -> usize {
        self.texture_radius
    }

    /// Returns the coarse-scale box radius.
    pub const fn structure_radius(self) -> usize {
        self.structure_radius
    }
}

/// Invalid construction input for [`TemporalResidualConfig`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TemporalResidualConfigError {
    /// Gain was negative or non-finite.
    InvalidGain,
}

impl fmt::Display for TemporalResidualConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("temporal residual gain must be finite and non-negative")
    }
}

impl std::error::Error for TemporalResidualConfigError {}

/// Validated scaling for [`temporal`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TemporalResidualConfig {
    gain: f32,
}

impl TemporalResidualConfig {
    /// Creates temporal residual scaling.
    pub fn new(gain: f32) -> Result<Self, TemporalResidualConfigError> {
        if !gain.is_finite() || gain < 0.0 {
            return Err(TemporalResidualConfigError::InvalidGain);
        }
        Ok(Self { gain })
    }

    /// Returns the output scaling factor.
    pub const fn gain(self) -> f32 {
        self.gain
    }
}

/// Normalization used by [`spectrum`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpectrumNormalization {
    /// Return unscaled squared magnitudes.
    None,
    /// Divide each squared magnitude by the number of input pixels.
    ByPixels,
}

/// Settings for [`spectrum`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpectrumConfig {
    normalization: SpectrumNormalization,
}

impl SpectrumConfig {
    /// Creates spectrum-analysis settings.
    pub const fn new(normalization: SpectrumNormalization) -> Self {
        Self { normalization }
    }

    /// Returns the configured normalization.
    pub const fn normalization(self) -> SpectrumNormalization {
        self.normalization
    }
}

/// Difference function used by [`compare`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompareMetric {
    /// Return `left - right`.
    Signed,
    /// Return `abs(left - right)`.
    Absolute,
    /// Return `(left - right)²`.
    Squared,
}

/// Settings for [`compare`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompareConfig {
    metric: CompareMetric,
}

impl CompareConfig {
    /// Creates a residual comparison configuration.
    pub const fn new(metric: CompareMetric) -> Self {
        Self { metric }
    }

    /// Returns the selected difference function.
    pub const fn metric(self) -> CompareMetric {
        self.metric
    }
}

/// Owned power-spectrum result in unshifted row-major frequency order.
#[derive(Clone, Debug, PartialEq)]
pub struct Spectrum {
    /// Frequency-grid extent, equal to the source extent.
    pub extent: Extent,
    /// Tight row-major squared magnitudes indexed by `(frequency_y, frequency_x)`.
    pub power: Vec<f32>,
}

/// Separates a source plane into coarse structure, texture, and residual.
///
/// The invariant `structure + texture + residual == source` holds up to normal
/// floating-point rounding. Structure is a wide box mean, texture is the
/// difference between fine and wide box means, and residual is source minus
/// the fine mean. This direct reference costs `O(pixels × structure window²)`
/// and allocates no memory. Non-finite values propagate to affected outputs.
pub fn decompose(
    source: Plane<'_, f32>,
    mut structure: PlaneMut<'_, f32>,
    mut texture: PlaneMut<'_, f32>,
    mut residual: PlaneMut<'_, f32>,
    config: DecomposeConfig,
) -> Result<(), ResidualError> {
    let extent = matching_extent(source.extent(), structure.extent())?;
    matching_extent(extent, texture.extent())?;
    matching_extent(extent, residual.extent())?;
    for y in 0..extent.height() {
        for x in 0..extent.width() {
            let fine = local_mean(source, x, y, config.texture_radius());
            let coarse = local_mean(source, x, y, config.structure_radius());
            structure.row_mut(y).expect("validated row")[x] = coarse;
            texture.row_mut(y).expect("validated row")[x] = fine;
            residual.row_mut(y).expect("validated row")[x] =
                source.row(y).expect("validated row")[x] - fine;
        }
    }
    for y in 0..extent.height() {
        for x in 0..extent.width() {
            let fine = texture.row_mut(y).expect("validated row")[x];
            let coarse = structure.row_mut(y).expect("validated row")[x];
            texture.row_mut(y).expect("validated row")[x] = fine - coarse;
        }
    }
    Ok(())
}

/// Measures temporal residual against the mean of aligned neighbours.
///
/// The output is `gain × abs(current - mean(previous, next))`. The finite mean
/// is evaluated in `f64` so adding two large finite `f32` neighbours does not
/// overflow spuriously. Non-finite inputs propagate to the corresponding
/// result. The scalar operation is `O(pixels)` with no allocation.
pub fn temporal(
    previous: Plane<'_, f32>,
    current: Plane<'_, f32>,
    next: Plane<'_, f32>,
    mut output: PlaneMut<'_, f32>,
    config: TemporalResidualConfig,
) -> Result<(), ResidualError> {
    let extent = matching_extent(previous.extent(), current.extent())?;
    matching_extent(extent, next.extent())?;
    matching_extent(extent, output.extent())?;
    for y in 0..extent.height() {
        let before = previous.row(y).expect("validated row");
        let sample = current.row(y).expect("validated row");
        let after = next.row(y).expect("validated row");
        let destination = output.row_mut(y).expect("validated row");
        for x in 0..extent.width() {
            let neighbour_mean = (f64::from(before[x]) + f64::from(after[x])) * 0.5;
            destination[x] =
                (f64::from(config.gain()) * (f64::from(sample[x]) - neighbour_mean).abs()) as f32;
        }
    }
    Ok(())
}

/// Computes a direct two-dimensional discrete Fourier power spectrum.
///
/// The result is unshifted and row-major. This is a deterministic scalar
/// reference requiring `O(pixels²)` arithmetic and allocating only the owned
/// returned spectrum. It rejects non-finite inputs before allocating output.
pub fn spectrum(source: Plane<'_, f32>, config: SpectrumConfig) -> Result<Spectrum, ResidualError> {
    require_finite(source)?;
    let extent = source.extent();
    let area = extent.area().ok_or(ResidualError::AllocationFailed)?;
    let mut power = Vec::new();
    power
        .try_reserve_exact(area)
        .map_err(|_| ResidualError::AllocationFailed)?;
    let scale = match config.normalization() {
        SpectrumNormalization::None => 1.0,
        SpectrumNormalization::ByPixels => 1.0 / area as f64,
    };
    for frequency_y in 0..extent.height() {
        for frequency_x in 0..extent.width() {
            let mut real = 0.0_f64;
            let mut imaginary = 0.0_f64;
            for y in 0..extent.height() {
                let row = source.row(y).expect("validated row");
                for (x, &sample) in row.iter().enumerate() {
                    let phase = core::f64::consts::TAU
                        * (frequency_x as f64 * x as f64 / extent.width() as f64
                            + frequency_y as f64 * y as f64 / extent.height() as f64);
                    let value = f64::from(sample);
                    real += value * phase.cos();
                    imaginary -= value * phase.sin();
                }
            }
            power.push(((real.mul_add(real, imaginary * imaginary)) * scale) as f32);
        }
    }
    Ok(Spectrum { extent, power })
}

/// Compares two residual fields using a caller-selected difference metric.
///
/// The transform costs `O(pixels)` and allocates no storage. Like ordinary
/// arithmetic, non-finite source values propagate to the corresponding output.
pub fn compare(
    left: Plane<'_, f32>,
    right: Plane<'_, f32>,
    mut output: PlaneMut<'_, f32>,
    config: CompareConfig,
) -> Result<(), ResidualError> {
    let extent = matching_extent(left.extent(), right.extent())?;
    matching_extent(extent, output.extent())?;
    for y in 0..extent.height() {
        let left_row = left.row(y).expect("validated row");
        let right_row = right.row(y).expect("validated row");
        let output_row = output.row_mut(y).expect("validated row");
        for x in 0..extent.width() {
            let difference = left_row[x] - right_row[x];
            output_row[x] = match config.metric() {
                CompareMetric::Signed => difference,
                CompareMetric::Absolute => difference.abs(),
                CompareMetric::Squared => difference * difference,
            };
        }
    }
    Ok(())
}

fn matching_extent(left: Extent, right: Extent) -> Result<Extent, ResidualError> {
    if left != right {
        return Err(ResidualError::ExtentMismatch);
    }
    Ok(left)
}

fn local_mean(source: Plane<'_, f32>, x: usize, y: usize, radius: usize) -> f32 {
    let extent = source.extent();
    let left = x.saturating_sub(radius);
    let right = x.saturating_add(radius).min(extent.width() - 1);
    let top = y.saturating_sub(radius);
    let bottom = y.saturating_add(radius).min(extent.height() - 1);
    let mut sum = 0.0_f64;
    let mut count = 0_usize;
    for sample_y in top..=bottom {
        for &sample in &source.row(sample_y).expect("validated row")[left..=right] {
            sum += f64::from(sample);
            count += 1;
        }
    }
    (sum / count as f64) as f32
}

fn require_finite(source: Plane<'_, f32>) -> Result<(), ResidualError> {
    if source.rows().flatten().any(|sample| !sample.is_finite()) {
        return Err(ResidualError::NonFiniteSample);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        CompareConfig, CompareMetric, DecomposeConfig, DecomposeConfigError, ResidualError,
        SpectrumConfig, SpectrumNormalization, TemporalResidualConfig, TemporalResidualConfigError,
        compare, decompose, spectrum, temporal,
    };
    use vsip_core::{Extent, Plane, PlaneMut};

    #[test]
    fn decomposition_preserves_the_source_with_padded_odd_geometry() {
        let extent = Extent::new(3, 1).expect("extent");
        let source = [1.0, 3.0, 5.0, 99.0];
        let mut structure = [-1.0; 4];
        let mut texture = [-1.0; 4];
        let mut residual = [-1.0; 4];
        decompose(
            Plane::new(&source, extent, 4).expect("plane"),
            PlaneMut::new(&mut structure, extent, 4).expect("plane"),
            PlaneMut::new(&mut texture, extent, 4).expect("plane"),
            PlaneMut::new(&mut residual, extent, 4).expect("plane"),
            DecomposeConfig::new(0, 1).expect("config"),
        )
        .expect("decompose");
        for x in 0..3 {
            assert!((structure[x] + texture[x] + residual[x] - source[x]).abs() < 1e-6);
        }
        assert_eq!(structure[3], -1.0);
    }

    #[test]
    fn temporal_residual_propagates_nan_and_applies_gain() {
        let extent = Extent::new(2, 1).expect("extent");
        let mut output = [0.0; 2];
        temporal(
            Plane::new(&[0.0, 1.0], extent, 2).expect("plane"),
            Plane::new(&[2.0, f32::NAN], extent, 2).expect("plane"),
            Plane::new(&[0.0, 1.0], extent, 2).expect("plane"),
            PlaneMut::new(&mut output, extent, 2).expect("plane"),
            TemporalResidualConfig::new(0.5).expect("config"),
        )
        .expect("temporal");
        assert_eq!(output[0], 1.0);
        assert!(output[1].is_nan());
    }

    #[test]
    fn temporal_residual_does_not_overflow_a_finite_neighbour_mean() {
        let extent = Extent::new(1, 1).expect("extent");
        let mut output = [1.0];
        temporal(
            Plane::new(&[f32::MAX], extent, 1).expect("plane"),
            Plane::new(&[f32::MAX], extent, 1).expect("plane"),
            Plane::new(&[f32::MAX], extent, 1).expect("plane"),
            PlaneMut::new(&mut output, extent, 1).expect("plane"),
            TemporalResidualConfig::new(1.0).expect("config"),
        )
        .expect("temporal");
        assert_eq!(output, [0.0]);
    }

    #[test]
    fn spectrum_reports_dc_power_and_rejects_nan() {
        let extent = Extent::new(2, 1).expect("extent");
        let result = spectrum(
            Plane::new(&[1.0, 1.0], extent, 2).expect("plane"),
            SpectrumConfig::new(SpectrumNormalization::ByPixels),
        )
        .expect("spectrum");
        assert_eq!(result.power.len(), 2);
        assert!((result.power[0] - 2.0).abs() < 1e-6);
        assert_eq!(
            spectrum(
                Plane::new(&[f32::NAN], Extent::new(1, 1).expect("extent"), 1).expect("plane"),
                SpectrumConfig::new(SpectrumNormalization::None),
            ),
            Err(ResidualError::NonFiniteSample)
        );
    }

    #[test]
    fn comparison_uses_the_selected_metric() {
        let extent = Extent::new(2, 1).expect("extent");
        let mut output = [0.0; 2];
        compare(
            Plane::new(&[1.0, -2.0], extent, 2).expect("plane"),
            Plane::new(&[-1.0, 1.0], extent, 2).expect("plane"),
            PlaneMut::new(&mut output, extent, 2).expect("plane"),
            CompareConfig::new(CompareMetric::Squared),
        )
        .expect("compare");
        assert_eq!(output, [4.0, 9.0]);
    }

    #[test]
    fn invalid_config_and_geometry_are_reported() {
        assert_eq!(
            DecomposeConfig::new(2, 2),
            Err(DecomposeConfigError::InvalidRadiusOrder)
        );
        assert_eq!(
            TemporalResidualConfig::new(f32::INFINITY),
            Err(TemporalResidualConfigError::InvalidGain)
        );
        let input_extent = Extent::new(1, 1).expect("extent");
        let output_extent = Extent::new(2, 1).expect("extent");
        let mut output = [0.0; 2];
        assert_eq!(
            compare(
                Plane::new(&[0.0], input_extent, 1).expect("plane"),
                Plane::new(&[0.0], input_extent, 1).expect("plane"),
                PlaneMut::new(&mut output, output_extent, 2).expect("plane"),
                CompareConfig::new(CompareMetric::Signed),
            ),
            Err(ResidualError::ExtentMismatch)
        );
    }
}

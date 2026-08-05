//! Deterministic scalar reference operations for grain analysis and synthesis.
//!
//! [`analyze`] costs `O(width * height * (2 * local_radius + 1)^2)` because it
//! computes a clamped local average per output sample. [`synthesize`],
//! [`match_grain`], and [`residual`] are single-pass, allocation-free transforms
//! over caller-owned output planes.

use core::fmt;

use vsip_core::{Extent, GeometryError, Plane, PlaneMut};
use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Validated statistics describing an additive grain residual.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrainProfile {
    residual_mean: f32,
    residual_variance: f32,
    temporal_variance: Option<f32>,
}

impl GrainProfile {
    /// Creates a profile from population statistics.
    ///
    /// Variances must be finite and non-negative. `temporal_variance` is `None`
    /// when no temporal reference was analysed.
    pub fn new(
        residual_mean: f32,
        residual_variance: f32,
        temporal_variance: Option<f32>,
    ) -> Result<Self, GrainError> {
        let temporal_is_valid = temporal_variance.is_none_or(is_valid_variance);
        if !residual_mean.is_finite() || !is_valid_variance(residual_variance) || !temporal_is_valid
        {
            return Err(GrainError::InvalidProfile);
        }
        Ok(Self {
            residual_mean,
            residual_variance,
            temporal_variance,
        })
    }

    /// Returns the residual population mean.
    pub const fn residual_mean(self) -> f32 {
        self.residual_mean
    }

    /// Returns the residual population variance.
    pub const fn residual_variance(self) -> f32 {
        self.residual_variance
    }

    /// Returns temporal difference variance when a previous plane was supplied.
    pub const fn temporal_variance(self) -> Option<f32> {
        self.temporal_variance
    }

    /// Returns the residual population standard deviation.
    pub fn standard_deviation(self) -> f32 {
        self.residual_variance.sqrt()
    }
}

/// Validated configuration for [`analyze`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnalysisConfig {
    local_radius: usize,
}

impl AnalysisConfig {
    /// Creates an analysis configuration with a non-zero local averaging radius.
    pub fn new(local_radius: usize) -> Result<Self, GrainError> {
        if local_radius == 0 {
            return Err(GrainError::ZeroLocalRadius);
        }
        let radius = isize::try_from(local_radius).map_err(|_| GrainError::CoordinatesTooLarge)?;
        let side = radius
            .checked_mul(2)
            .and_then(|side| side.checked_add(1))
            .ok_or(GrainError::CoordinatesTooLarge)?;
        side.checked_mul(side)
            .ok_or(GrainError::CoordinatesTooLarge)?;
        Ok(Self { local_radius })
    }

    /// Returns the local square radius used to calculate the residual.
    pub const fn local_radius(self) -> usize {
        self.local_radius
    }
}

/// Failure returned by a GrainLab reference operation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GrainError {
    /// A source input contained an infinite or NaN sample.
    NonFiniteSample,
    /// A profile mean or variance was not finite, or a variance was negative.
    InvalidProfile,
    /// Analysis requires a non-zero local averaging radius.
    ZeroLocalRadius,
    /// Planes that must correspond do not have equal visible extents.
    ExtentMismatch,
    /// Geometry cannot be represented by the scalar reference coordinates.
    CoordinatesTooLarge,
    /// Finite inputs produced a result outside the f32 range.
    NumericalOverflow,
    /// A plane failed its stride or backing-buffer validation.
    Geometry(GeometryError),
}

impl fmt::Display for GrainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFiniteSample => formatter.write_str("input contains a non-finite sample"),
            Self::InvalidProfile => formatter.write_str("grain profile is invalid"),
            Self::ZeroLocalRadius => formatter.write_str("local radius must be non-zero"),
            Self::ExtentMismatch => formatter.write_str("all planes must have the same extent"),
            Self::CoordinatesTooLarge => {
                formatter.write_str("plane coordinates exceed the scalar reference range")
            }
            Self::NumericalOverflow => {
                formatter.write_str("grain calculation exceeded the f32 range")
            }
            Self::Geometry(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for GrainError {}

impl From<GeometryError> for GrainError {
    fn from(error: GeometryError) -> Self {
        Self::Geometry(error)
    }
}

/// Measures spatial grain residual statistics and optional temporal change.
///
/// `previous`, when supplied, must have the same extent as `current`; its
/// temporal variance is the population variance of current/previous
/// differences. Spatial grain is the current sample minus a clamped square
/// local average.
pub fn analyze(
    current: Plane<'_, f32>,
    previous: Option<Plane<'_, f32>>,
    config: AnalysisConfig,
) -> Result<GrainProfile, GrainError> {
    ensure_finite(current)?;
    if let Some(previous) = previous {
        ensure_same_extent(current.extent(), previous.extent())?;
        ensure_finite(previous)?;
    }
    let radius =
        isize::try_from(config.local_radius).map_err(|_| GrainError::CoordinatesTooLarge)?;
    let extent = current.extent();
    let count = extent.area().ok_or(GrainError::CoordinatesTooLarge)?;
    let count = count as f64;
    let mut residual_sum = 0.0_f64;
    let mut residual_square_sum = 0.0_f64;
    let mut temporal_sum = 0.0_f64;
    let mut temporal_square_sum = 0.0_f64;

    for y in 0..extent.height() {
        let current_row = current.row(y).ok_or(GrainError::ExtentMismatch)?;
        let previous_row = previous.and_then(|plane| plane.row(y));
        for (x, &sample) in current_row.iter().enumerate() {
            let average = local_average(current, x, y, radius, extent)?;
            let residual = f64::from(sample) - average;
            residual_sum += residual;
            residual_square_sum += residual * residual;
            if let Some(previous_row) = previous_row {
                let temporal = f64::from(sample) - f64::from(previous_row[x]);
                temporal_sum += temporal;
                temporal_square_sum += temporal * temporal;
            }
        }
    }
    let mean = residual_sum / count;
    let variance = finite_f32((residual_square_sum / count - mean * mean).max(0.0))?;
    let temporal_variance = previous
        .map(|_| {
            let temporal_mean = temporal_sum / count;
            finite_f32((temporal_square_sum / count - temporal_mean * temporal_mean).max(0.0))
        })
        .transpose()?;
    GrainProfile::new(finite_f32(mean)?, variance, temporal_variance)
}

/// Synthesizes a deterministic, spatially uncorrelated additive grain plane.
///
/// `seed` and coordinates fully determine the output, independent of traversal
/// or thread count. The generated distribution has the requested mean and an
/// approximately matching population variance over sufficiently large planes.
pub fn synthesize(
    profile: GrainProfile,
    seed: u64,
    mut output: PlaneMut<'_, f32>,
) -> Result<(), GrainError> {
    validate_profile(profile)?;
    for (y, row) in output.rows_mut().enumerate() {
        for (x, destination) in row.iter_mut().enumerate() {
            *destination = generated_sample(profile, seed, x, y);
        }
    }
    Ok(())
}

/// Adds deterministic synthesized grain to a finite clean source plane.
///
/// The output has the same extent as `clean` and is written in a single pass.
/// A finite sum outside the f32 range returns [`GrainError::NumericalOverflow`].
pub fn match_grain(
    clean: Plane<'_, f32>,
    profile: GrainProfile,
    seed: u64,
    mut output: PlaneMut<'_, f32>,
) -> Result<(), GrainError> {
    ensure_same_extent(clean.extent(), output.extent())?;
    ensure_finite(clean)?;
    validate_profile(profile)?;
    for (y, output_row) in output.rows_mut().enumerate() {
        let clean_row = clean.row(y).ok_or(GrainError::ExtentMismatch)?;
        for (x, (destination, &clean_sample)) in output_row.iter_mut().zip(clean_row).enumerate() {
            *destination = finite_f32(
                f64::from(clean_sample) + f64::from(generated_sample(profile, seed, x, y)),
            )?;
        }
    }
    Ok(())
}

/// Extracts the candidate additive residual between `source` and `reference`.
///
/// This operation is `source - reference`; both inputs must be finite and all
/// visible extents must agree. It does not filter or estimate a reference.
pub fn residual(
    source: Plane<'_, f32>,
    reference: Plane<'_, f32>,
    mut output: PlaneMut<'_, f32>,
) -> Result<(), GrainError> {
    ensure_same_extent(source.extent(), reference.extent())?;
    ensure_same_extent(source.extent(), output.extent())?;
    ensure_finite(source)?;
    ensure_finite(reference)?;
    for (y, output_row) in output.rows_mut().enumerate() {
        let source_row = source.row(y).ok_or(GrainError::ExtentMismatch)?;
        let reference_row = reference.row(y).ok_or(GrainError::ExtentMismatch)?;
        for ((destination, &source_sample), &reference_sample) in
            output_row.iter_mut().zip(source_row).zip(reference_row)
        {
            *destination = finite_f32(f64::from(source_sample) - f64::from(reference_sample))?;
        }
    }
    Ok(())
}

fn is_valid_variance(value: f32) -> bool {
    value.is_finite() && value >= 0.0
}

fn validate_profile(profile: GrainProfile) -> Result<(), GrainError> {
    GrainProfile::new(
        profile.residual_mean,
        profile.residual_variance,
        profile.temporal_variance,
    )
    .map(|_| ())
}

fn ensure_finite(plane: Plane<'_, f32>) -> Result<(), GrainError> {
    if plane.rows().flatten().all(|sample| sample.is_finite()) {
        Ok(())
    } else {
        Err(GrainError::NonFiniteSample)
    }
}

fn ensure_same_extent(left: Extent, right: Extent) -> Result<(), GrainError> {
    if left == right {
        Ok(())
    } else {
        Err(GrainError::ExtentMismatch)
    }
}

fn local_average(
    input: Plane<'_, f32>,
    x: usize,
    y: usize,
    radius: isize,
    extent: Extent,
) -> Result<f64, GrainError> {
    let mut sum = 0.0_f64;
    let side = radius
        .checked_mul(2)
        .and_then(|value| value.checked_add(1))
        .ok_or(GrainError::CoordinatesTooLarge)?;
    let sample_count = side
        .checked_mul(side)
        .ok_or(GrainError::CoordinatesTooLarge)? as f64;
    for offset_y in -radius..=radius {
        for offset_x in -radius..=radius {
            let sample_x = clamped_offset(x, offset_x, extent.width())?;
            let sample_y = clamped_offset(y, offset_y, extent.height())?;
            sum += f64::from(sample_visible(input, sample_x, sample_y));
        }
    }
    Ok(sum / sample_count)
}

fn clamped_offset(base: usize, offset: isize, limit: usize) -> Result<usize, GrainError> {
    let base = isize::try_from(base).map_err(|_| GrainError::CoordinatesTooLarge)?;
    let limit = isize::try_from(limit).map_err(|_| GrainError::CoordinatesTooLarge)?;
    Ok(base.saturating_add(offset).clamp(0, limit - 1) as usize)
}

fn sample_visible(plane: Plane<'_, f32>, x: usize, y: usize) -> f32 {
    plane.row(y).map_or(0.0, |row| row[x])
}

fn generated_sample(profile: GrainProfile, seed: u64, x: usize, y: usize) -> f32 {
    let hash = coordinate_hash(seed, x as u64, y as u64);
    let unit = (hash as f64 / u64::MAX as f64) as f32;
    let centered = unit.mul_add(2.0, -1.0);
    profile.residual_mean + centered * profile.standard_deviation() * 3.0_f32.sqrt()
}

fn coordinate_hash(seed: u64, x: u64, y: u64) -> u64 {
    let mut value = seed ^ x.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    value ^= y.wrapping_mul(0xD1B5_4A32_D192_ED03);
    value ^= value >> 30;
    value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn finite_f32(value: f64) -> Result<f32, GrainError> {
    let value = value as f32;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(GrainError::NumericalOverflow)
    }
}

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-grainlab",
    namespace: "grainlab",
    summary: "Grain measurement, matching and synthesis",
    filters: &[
        Filter {
            name: "Analyze",
            summary: "Measure spatial and temporal grain statistics",
            maturity: Maturity::Experimental,
            execution: Execution::Analysis,
        },
        Filter {
            name: "Synthesize",
            summary: "Generate grain from a measured profile",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
        Filter {
            name: "Match",
            summary: "Transfer a grain profile to a clean clip",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
        Filter {
            name: "Residual",
            summary: "Extract the candidate grain residual",
            maturity: Maturity::Experimental,
            execution: Execution::MultiInput,
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::{
        AnalysisConfig, GrainError, GrainProfile, Plane, PlaneMut, analyze, match_grain, residual,
        synthesize,
    };
    use vsip_core::Extent;

    #[test]
    fn analysis_uses_odd_padded_rows_and_temporal_input() {
        let extent = Extent::new(3, 1).expect("valid extent");
        let current = [2.0, 2.0, 2.0, 99.0];
        let previous = [1.0, 1.0, 1.0, 98.0];
        let profile = analyze(
            Plane::new(&current, extent, 4).expect("current"),
            Some(Plane::new(&previous, extent, 4).expect("previous")),
            AnalysisConfig::new(1).expect("config"),
        )
        .expect("analysis succeeds");
        assert_eq!(profile.residual_mean(), 0.0);
        assert_eq!(profile.residual_variance(), 0.0);
        assert_eq!(profile.temporal_variance(), Some(0.0));
    }

    #[test]
    fn synthesis_is_seeded_and_match_and_residual_are_exact() {
        let extent = Extent::new(3, 1).expect("valid extent");
        let profile = GrainProfile::new(0.0, 0.25, None).expect("profile");
        let mut first = [0.0; 3];
        let mut second = [0.0; 3];
        synthesize(
            profile,
            7,
            PlaneMut::new(&mut first, extent, 3).expect("first"),
        )
        .expect("synthesize");
        synthesize(
            profile,
            7,
            PlaneMut::new(&mut second, extent, 3).expect("second"),
        )
        .expect("synthesize");
        assert_eq!(first, second);

        let clean = [10.0, 10.0, 10.0];
        let mut matched = [0.0; 3];
        match_grain(
            Plane::new(&clean, extent, 3).expect("clean"),
            profile,
            7,
            PlaneMut::new(&mut matched, extent, 3).expect("matched"),
        )
        .expect("match");
        let mut extracted = [0.0; 3];
        residual(
            Plane::new(&matched, extent, 3).expect("matched"),
            Plane::new(&clean, extent, 3).expect("clean"),
            PlaneMut::new(&mut extracted, extent, 3).expect("residual"),
        )
        .expect("residual");
        for (&actual, expected) in extracted.iter().zip(first) {
            assert!((actual - expected).abs() < 1.0e-6);
        }
    }

    #[test]
    fn temporal_statistic_is_population_variance_not_mean_square() {
        let extent = Extent::new(2, 1).expect("valid extent");
        let profile = analyze(
            Plane::new(&[2.0, 3.0], extent, 2).expect("current"),
            Some(Plane::new(&[1.0, 1.0], extent, 2).expect("previous")),
            AnalysisConfig::new(1).expect("config"),
        )
        .expect("analysis");
        assert_eq!(profile.temporal_variance(), Some(0.25));
    }

    #[test]
    fn rejects_invalid_configuration_geometry_and_nan() {
        assert!(matches!(
            AnalysisConfig::new(0),
            Err(GrainError::ZeroLocalRadius)
        ));
        assert!(matches!(
            GrainProfile::new(0.0, -1.0, None),
            Err(GrainError::InvalidProfile)
        ));
        let extent = Extent::new(1, 1).expect("valid extent");
        assert!(matches!(
            analyze(
                Plane::new(&[f32::NAN], extent, 1).expect("plane"),
                None,
                AnalysisConfig::new(1).expect("config"),
            ),
            Err(GrainError::NonFiniteSample)
        ));
        let source_extent = Extent::new(2, 1).expect("source extent");
        let mut output = [0.0; 1];
        assert!(matches!(
            residual(
                Plane::new(&[1.0, 2.0], source_extent, 2).expect("source"),
                Plane::new(&[1.0, 2.0], source_extent, 2).expect("reference"),
                PlaneMut::new(&mut output, extent, 1).expect("output"),
            ),
            Err(GrainError::ExtentMismatch)
        ));

        let mut overflow = [0.0];
        assert!(matches!(
            residual(
                Plane::new(&[f32::MAX], extent, 1).expect("source"),
                Plane::new(&[-f32::MAX], extent, 1).expect("reference"),
                PlaneMut::new(&mut overflow, extent, 1).expect("output"),
            ),
            Err(GrainError::NumericalOverflow)
        ));
    }
}

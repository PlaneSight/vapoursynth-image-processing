//! Scalar reference filters for edge-preserving smoothing.
//!
//! Every transform writes into a caller-owned plane.  Filters that need a
//! full-frame intermediate explicitly receive caller-owned scratch planes; no
//! frame-sized allocation occurs on the processing path.

use core::fmt;

use vsip_core::{Extent, Plane, PlaneMut};
use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// A validation failure reported by an EdgeAware filter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EdgeAwareError {
    /// Two planes that participate in one operation do not have the same extent.
    ExtentMismatch,
    /// A radius, iteration count, or smoothing parameter is not usable.
    InvalidConfiguration,
}

impl fmt::Display for EdgeAwareError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ExtentMismatch => "all planes must have the same extent",
            Self::InvalidConfiguration => "edge-aware configuration is invalid",
        })
    }
}

impl std::error::Error for EdgeAwareError {}

/// Configuration for guided filtering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuidedConfig {
    /// Positive half-width of the square local window.
    pub radius: usize,
    /// Positive regularizer for the local linear model.
    pub epsilon: f32,
}

impl GuidedConfig {
    /// Validates the configuration before entering the reference kernel.
    pub fn validate(self) -> Result<(), EdgeAwareError> {
        if self.radius == 0 || !self.epsilon.is_finite() || self.epsilon <= 0.0 {
            return Err(EdgeAwareError::InvalidConfiguration);
        }
        Ok(())
    }
}

/// Configuration for recursive domain-transform smoothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DomainTransformConfig {
    /// Spatial standard deviation in pixels.
    pub sigma_spatial: f32,
    /// Guide-value standard deviation.
    pub sigma_range: f32,
    /// Number of horizontal/vertical smoothing passes.
    pub iterations: u8,
}

impl DomainTransformConfig {
    /// Validates the configuration before entering the reference kernel.
    pub fn validate(self) -> Result<(), EdgeAwareError> {
        if !self.sigma_spatial.is_finite()
            || !self.sigma_range.is_finite()
            || self.sigma_spatial <= 0.0
            || self.sigma_range <= 0.0
            || self.iterations == 0
        {
            return Err(EdgeAwareError::InvalidConfiguration);
        }
        Ok(())
    }
}

/// Configuration for rolling-guidance smoothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RollingGuidanceConfig {
    /// Positive half-width of the square local window.
    pub radius: usize,
    /// Differences larger than this value receive exponentially smaller weight.
    pub range_sigma: f32,
    /// Number of guide refreshes.
    pub iterations: u8,
}

impl RollingGuidanceConfig {
    /// Validates the configuration before entering the reference kernel.
    pub fn validate(self) -> Result<(), EdgeAwareError> {
        if self.radius == 0
            || self.iterations == 0
            || !self.range_sigma.is_finite()
            || self.range_sigma <= 0.0
        {
            return Err(EdgeAwareError::InvalidConfiguration);
        }
        Ok(())
    }
}

/// Configuration for global edge-aware diffusion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlobalSmoothConfig {
    /// Number of four-neighbour diffusion steps.
    pub iterations: u8,
    /// Conductance scale; larger values cross stronger edges.
    pub edge_sigma: f32,
    /// Diffusion amount per step, bounded to preserve the scalar scheme.
    pub step: f32,
}

impl GlobalSmoothConfig {
    /// Validates the configuration before entering the reference kernel.
    pub fn validate(self) -> Result<(), EdgeAwareError> {
        if self.iterations == 0
            || !self.edge_sigma.is_finite()
            || self.edge_sigma <= 0.0
            || !self.step.is_finite()
            || !(0.0..=0.25).contains(&self.step)
        {
            return Err(EdgeAwareError::InvalidConfiguration);
        }
        Ok(())
    }
}

/// Applies a self-guided filter.
///
/// `mean_a`, `mean_b`, and `coefficients` are same-sized scratch planes.  The
/// reference implementation uses square box windows, so its cost is
/// `O(width * height * radius^2)` and its additional storage is the three
/// caller-owned planes.
pub fn guided(
    input: Plane<'_, f32>,
    output: PlaneMut<'_, f32>,
    mean_a: PlaneMut<'_, f32>,
    mean_b: PlaneMut<'_, f32>,
    coefficients: PlaneMut<'_, f32>,
    config: GuidedConfig,
) -> Result<(), EdgeAwareError> {
    guided_with_guide(input, input, output, mean_a, mean_b, coefficients, config)
}

/// Applies a guided filter using a separate guide plane.
///
/// Non-finite input samples are preserved. Local statistics include only
/// positions where both input and guide are finite, so every moment uses the
/// same sample population. If a finite sample's local model is not
/// representable as `f32`, that input sample is preserved.
pub fn guided_with_guide(
    input: Plane<'_, f32>,
    guide: Plane<'_, f32>,
    mut output: PlaneMut<'_, f32>,
    mut mean_a: PlaneMut<'_, f32>,
    mut mean_b: PlaneMut<'_, f32>,
    mut coefficients: PlaneMut<'_, f32>,
    config: GuidedConfig,
) -> Result<(), EdgeAwareError> {
    config.validate()?;
    let extent = input.extent();
    require_extent(extent, guide.extent())?;
    require_extent(extent, output.extent())?;
    require_extent(extent, mean_a.extent())?;
    require_extent(extent, mean_b.extent())?;
    require_extent(extent, coefficients.extent())?;

    box_mean_pair(input, guide, &mut mean_a, config.radius, |_, guide| {
        f64::from(guide)
    });
    box_mean_pair(input, guide, &mut mean_b, config.radius, |source, _| {
        f64::from(source)
    });
    box_mean_pair(
        input,
        guide,
        &mut coefficients,
        config.radius,
        |_, guide| f64::from(guide) * f64::from(guide),
    );
    box_mean_pair(input, guide, &mut output, config.radius, |source, guide| {
        f64::from(source) * f64::from(guide)
    });

    for y in 0..extent.height() {
        let mean_guide_row = mean_a.row_mut(y).expect("validated scratch row");
        let mean_source_row = mean_b.row_mut(y).expect("validated scratch row");
        let correlation_row = output.row_mut(y).expect("validated output row");
        let coefficient_row = coefficients.row_mut(y).expect("validated scratch row");

        for x in 0..extent.width() {
            let mean_guide = f64::from(mean_guide_row[x]);
            let mean_source = f64::from(mean_source_row[x]);
            let guide_moment = f64::from(coefficient_row[x]);
            let correlation = f64::from(correlation_row[x]);
            let variance = (guide_moment - mean_guide * mean_guide).max(0.0);
            let covariance = correlation - mean_guide * mean_source;
            let a = covariance / (variance + f64::from(config.epsilon));
            let b = mean_source - a * mean_guide;
            if a.is_finite() && b.is_finite() {
                mean_source_row[x] = a as f32;
                coefficient_row[x] = b as f32;
            } else {
                mean_source_row[x] = f32::NAN;
                coefficient_row[x] = f32::NAN;
            }
        }
    }

    box_mean_mut(&mut mean_b, &mut mean_a, config.radius);
    box_mean_mut(&mut coefficients, &mut mean_b, config.radius);

    for y in 0..extent.height() {
        let source_row = input.row(y).expect("validated input row");
        let guide_row = guide.row(y).expect("validated guide row");
        let mean_coefficient_row = mean_a.row_mut(y).expect("validated scratch row");
        let mean_offset_row = mean_b.row_mut(y).expect("validated scratch row");
        let destination_row = output.row_mut(y).expect("validated output row");
        for x in 0..extent.width() {
            let filtered = f64::from(mean_coefficient_row[x])
                .mul_add(f64::from(guide_row[x]), f64::from(mean_offset_row[x]));
            destination_row[x] = if source_row[x].is_finite()
                && guide_row[x].is_finite()
                && filtered.is_finite()
                && filtered.abs() <= f64::from(f32::MAX)
            {
                filtered as f32
            } else {
                source_row[x]
            };
        }
    }
    Ok(())
}

/// Applies recursive domain-transform smoothing with one same-sized scratch plane.
///
/// The kernel has `O(iterations * width * height)` work and `O(width * height)`
/// caller-owned scratch storage.
pub fn domain_transform(
    input: Plane<'_, f32>,
    mut output: PlaneMut<'_, f32>,
    mut scratch: PlaneMut<'_, f32>,
    config: DomainTransformConfig,
) -> Result<(), EdgeAwareError> {
    config.validate()?;
    let extent = input.extent();
    require_extent(extent, output.extent())?;
    require_extent(extent, scratch.extent())?;
    copy_plane(input, &mut output);

    for pass in 0..config.iterations {
        let ratio = f64::from(config.sigma_spatial) / f64::from(config.sigma_range);
        let pass_scale = 2.0_f64.powi(i32::from(config.iterations - pass) - 1);
        let normalization = (2.0_f64.powi(2 * i32::from(config.iterations)) - 1.0).sqrt();
        let sigma = f64::from(config.sigma_spatial) * 3.0_f64.sqrt() * pass_scale / normalization;
        recursive_horizontal(input, &mut output, &mut scratch, ratio, sigma);
        recursive_vertical(input, &mut scratch, &mut output, ratio, sigma);
    }
    Ok(())
}

/// Repeatedly refreshes a guide from the previous smooth result.
///
/// This scalar rolling-guidance reference costs
/// `O(iterations * width * height * radius^2)` and uses one caller-owned
/// same-sized scratch plane.
pub fn rolling_guidance(
    input: Plane<'_, f32>,
    mut output: PlaneMut<'_, f32>,
    mut scratch: PlaneMut<'_, f32>,
    config: RollingGuidanceConfig,
) -> Result<(), EdgeAwareError> {
    config.validate()?;
    let extent = input.extent();
    require_extent(extent, output.extent())?;
    require_extent(extent, scratch.extent())?;
    box_mean_pair(input, input, &mut output, config.radius, |source, _| {
        f64::from(source)
    });

    for _ in 0..config.iterations {
        rolling_guidance_step(
            input,
            &mut output,
            &mut scratch,
            config.radius,
            config.range_sigma,
        );
        copy_mut_plane(&mut scratch, &mut output);
    }
    Ok(())
}

/// Applies global edge-aware smoothing through explicit four-neighbour diffusion.
///
/// This is a deterministic scalar reference for the global smoothness energy;
/// it costs `O(iterations * width * height)` and uses one caller-owned scratch
/// plane.
pub fn global_smooth(
    input: Plane<'_, f32>,
    mut output: PlaneMut<'_, f32>,
    mut scratch: PlaneMut<'_, f32>,
    config: GlobalSmoothConfig,
) -> Result<(), EdgeAwareError> {
    config.validate()?;
    let extent = input.extent();
    require_extent(extent, output.extent())?;
    require_extent(extent, scratch.extent())?;
    copy_plane(input, &mut output);

    for _ in 0..config.iterations {
        diffuse_step(&mut output, &mut scratch, config.edge_sigma, config.step);
        copy_mut_plane(&mut scratch, &mut output);
    }
    Ok(())
}

fn require_extent(expected: Extent, actual: Extent) -> Result<(), EdgeAwareError> {
    if expected != actual {
        return Err(EdgeAwareError::ExtentMismatch);
    }
    Ok(())
}

fn copy_plane(source: Plane<'_, f32>, destination: &mut PlaneMut<'_, f32>) {
    for (source_row, destination_row) in source.rows().zip(destination.rows_mut()) {
        destination_row.copy_from_slice(source_row);
    }
}

fn copy_mut_plane(source: &mut PlaneMut<'_, f32>, destination: &mut PlaneMut<'_, f32>) {
    let height = source.extent().height();
    for y in 0..height {
        let source_row = source.row_mut(y).expect("validated source row");
        let destination_row = destination.row_mut(y).expect("validated destination row");
        destination_row.copy_from_slice(source_row);
    }
}

fn box_mean_pair(
    input: Plane<'_, f32>,
    guide: Plane<'_, f32>,
    output: &mut PlaneMut<'_, f32>,
    radius: usize,
    map: impl Fn(f32, f32) -> f64,
) {
    let extent = input.extent();
    for y in 0..extent.height() {
        let destination_row = output.row_mut(y).expect("validated output row");
        for (x, destination) in destination_row.iter_mut().enumerate() {
            let (sum, count) = local_sum_pair(input, guide, x, y, radius, &map);
            *destination = if count == 0 {
                f32::NAN
            } else {
                (sum / count as f64) as f32
            };
        }
    }
}

fn box_mean_mut(input: &mut PlaneMut<'_, f32>, output: &mut PlaneMut<'_, f32>, radius: usize) {
    let extent = input.extent();
    for y in 0..extent.height() {
        let destination_row = output.row_mut(y).expect("validated output row");
        for (x, destination) in destination_row.iter_mut().enumerate() {
            let (sum, count) = local_sum_mut(input, x, y, radius);
            *destination = if count == 0 {
                f32::NAN
            } else {
                (sum / count as f64) as f32
            };
        }
    }
}

fn local_sum_pair(
    input: Plane<'_, f32>,
    guide: Plane<'_, f32>,
    x: usize,
    y: usize,
    radius: usize,
    map: &impl Fn(f32, f32) -> f64,
) -> (f64, usize) {
    let extent = input.extent();
    let x0 = x.saturating_sub(radius);
    let x1 = x.saturating_add(radius).min(extent.width() - 1);
    let y0 = y.saturating_sub(radius);
    let y1 = y.saturating_add(radius).min(extent.height() - 1);
    let mut sum = 0.0_f64;
    let mut count = 0_usize;
    for yy in y0..=y1 {
        let input_row = input.row(yy).expect("validated input row");
        let guide_row = guide.row(yy).expect("validated guide row");
        for xx in x0..=x1 {
            let source = input_row[xx];
            let guide_value = guide_row[xx];
            if source.is_finite() && guide_value.is_finite() {
                sum += map(source, guide_value);
                count += 1;
            }
        }
    }
    (sum, count)
}

fn local_sum_mut(input: &mut PlaneMut<'_, f32>, x: usize, y: usize, radius: usize) -> (f64, usize) {
    let extent = input.extent();
    let x0 = x.saturating_sub(radius);
    let x1 = x.saturating_add(radius).min(extent.width() - 1);
    let y0 = y.saturating_sub(radius);
    let y1 = y.saturating_add(radius).min(extent.height() - 1);
    let mut sum = 0.0_f64;
    let mut count = 0_usize;
    for yy in y0..=y1 {
        let row = input.row_mut(yy).expect("validated input row");
        for &sample in &row[x0..=x1] {
            if sample.is_finite() {
                sum += f64::from(sample);
                count += 1;
            }
        }
    }
    (sum, count)
}

fn recursive_horizontal(
    guide: Plane<'_, f32>,
    source: &mut PlaneMut<'_, f32>,
    destination: &mut PlaneMut<'_, f32>,
    range_ratio: f64,
    sigma: f64,
) {
    let extent = guide.extent();
    for y in 0..extent.height() {
        let guide_row = guide.row(y).expect("validated guide row");
        let source_row = source.row_mut(y).expect("validated source row");
        let destination_row = destination.row_mut(y).expect("validated destination row");
        destination_row[0] = source_row[0];
        for x in 1..extent.width() {
            let distance = domain_distance(guide_row[x - 1], guide_row[x], range_ratio);
            let alpha = (-core::f64::consts::SQRT_2 * distance / sigma).exp();
            destination_row[x] = if source_row[x].is_finite() && destination_row[x - 1].is_finite()
            {
                blend(source_row[x], destination_row[x - 1], alpha)
            } else {
                source_row[x]
            };
        }
        for x in (0..extent.width() - 1).rev() {
            let distance = domain_distance(guide_row[x], guide_row[x + 1], range_ratio);
            let alpha = (-core::f64::consts::SQRT_2 * distance / sigma).exp();
            destination_row[x] =
                if destination_row[x].is_finite() && destination_row[x + 1].is_finite() {
                    blend(destination_row[x], destination_row[x + 1], alpha)
                } else {
                    destination_row[x]
                };
        }
    }
}

fn recursive_vertical(
    guide: Plane<'_, f32>,
    source: &mut PlaneMut<'_, f32>,
    destination: &mut PlaneMut<'_, f32>,
    range_ratio: f64,
    sigma: f64,
) {
    let extent = guide.extent();
    for x in 0..extent.width() {
        let first = source.row_mut(0).expect("validated source row")[x];
        destination.row_mut(0).expect("validated destination row")[x] = first;
        for y in 1..extent.height() {
            let guide_previous = guide.row(y - 1).expect("validated guide row")[x];
            let guide_current = guide.row(y).expect("validated guide row")[x];
            let previous = destination
                .row_mut(y - 1)
                .expect("validated destination row")[x];
            let current = source.row_mut(y).expect("validated source row")[x];
            let alpha = (-core::f64::consts::SQRT_2
                * domain_distance(guide_previous, guide_current, range_ratio)
                / sigma)
                .exp();
            destination.row_mut(y).expect("validated destination row")[x] =
                if current.is_finite() && previous.is_finite() {
                    blend(current, previous, alpha)
                } else {
                    current
                };
        }
        for y in (0..extent.height() - 1).rev() {
            let current = destination.row_mut(y).expect("validated destination row")[x];
            let next = destination
                .row_mut(y + 1)
                .expect("validated destination row")[x];
            let guide_current = guide.row(y).expect("validated guide row")[x];
            let guide_next = guide.row(y + 1).expect("validated guide row")[x];
            let alpha = (-core::f64::consts::SQRT_2
                * domain_distance(guide_current, guide_next, range_ratio)
                / sigma)
                .exp();
            destination.row_mut(y).expect("validated destination row")[x] =
                if current.is_finite() && next.is_finite() {
                    blend(current, next, alpha)
                } else {
                    current
                };
        }
    }
}

fn blend(current: f32, previous: f32, alpha: f64) -> f32 {
    (f64::from(current) * (1.0 - alpha) + f64::from(previous) * alpha) as f32
}

fn domain_distance(a: f32, b: f32, range_ratio: f64) -> f64 {
    if a.is_finite() && b.is_finite() {
        1.0 + range_ratio * (f64::from(a) - f64::from(b)).abs()
    } else {
        f64::INFINITY
    }
}

fn rolling_guidance_step(
    input: Plane<'_, f32>,
    guide: &mut PlaneMut<'_, f32>,
    destination: &mut PlaneMut<'_, f32>,
    radius: usize,
    range_sigma: f32,
) {
    let extent = input.extent();
    let range_sigma = f64::from(range_sigma);
    let inverse_two_sigma_squared = -0.5 / (range_sigma * range_sigma);
    for y in 0..extent.height() {
        let destination_row = destination.row_mut(y).expect("validated destination row");
        for (x, destination_sample) in destination_row.iter_mut().enumerate() {
            let center_source = input.row(y).expect("validated input row")[x];
            let center_guide = guide.row_mut(y).expect("validated guide row")[x];
            if !center_source.is_finite() || !center_guide.is_finite() {
                *destination_sample = center_source;
                continue;
            }
            let x0 = x.saturating_sub(radius);
            let x1 = x.saturating_add(radius).min(extent.width() - 1);
            let y0 = y.saturating_sub(radius);
            let y1 = y.saturating_add(radius).min(extent.height() - 1);
            let mut weighted_sum = 0.0_f64;
            let mut weight_sum = 0.0_f64;
            for yy in y0..=y1 {
                let source_row = input.row(yy).expect("validated input row");
                let guide_row = guide.row_mut(yy).expect("validated guide row");
                for xx in x0..=x1 {
                    let sample = source_row[xx];
                    let guide_sample = guide_row[xx];
                    if sample.is_finite() && guide_sample.is_finite() {
                        let difference = f64::from(guide_sample) - f64::from(center_guide);
                        let weight = (difference * difference * inverse_two_sigma_squared).exp();
                        weighted_sum += f64::from(sample) * weight;
                        weight_sum += weight;
                    }
                }
            }
            *destination_sample = if weight_sum == 0.0 {
                center_source
            } else {
                (weighted_sum / weight_sum) as f32
            };
        }
    }
}

fn diffuse_step(
    source: &mut PlaneMut<'_, f32>,
    destination: &mut PlaneMut<'_, f32>,
    edge_sigma: f32,
    step: f32,
) {
    let extent = source.extent();
    let edge_sigma = f64::from(edge_sigma);
    let inverse_two_sigma_squared = -0.5 / (edge_sigma * edge_sigma);
    for y in 0..extent.height() {
        let destination_row = destination.row_mut(y).expect("validated destination row");
        for (x, destination_sample) in destination_row.iter_mut().enumerate() {
            let center = source.row_mut(y).expect("validated source row")[x];
            if !center.is_finite() {
                *destination_sample = center;
                continue;
            }
            let mut delta = 0.0_f64;
            for (nx, ny) in neighbours(x, y, extent) {
                let neighbour = source.row_mut(ny).expect("validated source row")[nx];
                if neighbour.is_finite() {
                    let difference = f64::from(neighbour) - f64::from(center);
                    delta +=
                        difference * (difference * difference * inverse_two_sigma_squared).exp();
                }
            }
            *destination_sample = (f64::from(center) + f64::from(step) * delta) as f32;
        }
    }
}

fn neighbours(x: usize, y: usize, extent: Extent) -> [(usize, usize); 4] {
    [
        (x.saturating_sub(1), y),
        (x.saturating_add(1).min(extent.width() - 1), y),
        (x, y.saturating_sub(1)),
        (x, y.saturating_add(1).min(extent.height() - 1)),
    ]
}

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-edgeaware",
    namespace: "edgeaware",
    summary: "Guided and edge-preserving smoothing",
    filters: &[
        Filter {
            name: "Guided",
            summary: "Apply self-guided smoothing",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
        Filter {
            name: "JointGuided",
            summary: "Filter using another guide clip",
            maturity: Maturity::Experimental,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "DomainTransform",
            summary: "Apply domain-transform filtering",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
        Filter {
            name: "RollingGuidance",
            summary: "Remove small structures iteratively",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
        Filter {
            name: "GlobalSmooth",
            summary: "Apply edge-aware global smoothing",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::{
        DomainTransformConfig, EdgeAwareError, GlobalSmoothConfig, GuidedConfig,
        RollingGuidanceConfig, domain_transform, global_smooth, guided, guided_with_guide,
        rolling_guidance,
    };
    use vsip_core::{Extent, Plane, PlaneMut};

    fn extent() -> Extent {
        Extent::new(3, 3).expect("valid extent")
    }

    #[test]
    fn guided_handles_odd_padded_planes_and_preserves_constant_image() {
        let source = [2.0, 2.0, 2.0, 9.0, 2.0, 2.0, 2.0, 9.0, 2.0, 2.0, 2.0];
        let mut output = [-1.0; 11];
        let mut a = [0.0; 11];
        let mut b = [0.0; 11];
        let mut c = [0.0; 11];
        guided(
            Plane::new(&source, extent(), 4).expect("valid input"),
            PlaneMut::new(&mut output, extent(), 4).expect("valid output"),
            PlaneMut::new(&mut a, extent(), 4).expect("valid scratch"),
            PlaneMut::new(&mut b, extent(), 4).expect("valid scratch"),
            PlaneMut::new(&mut c, extent(), 4).expect("valid scratch"),
            GuidedConfig {
                radius: 1,
                epsilon: 0.01,
            },
        )
        .expect("guided filter succeeds");
        for index in [0, 1, 2, 4, 5, 6, 8, 9, 10] {
            assert!((output[index] - 2.0).abs() < 1e-4);
        }
        assert_eq!(output[3], -1.0);
        assert_eq!(output[7], -1.0);
    }

    #[test]
    fn joint_guided_rejects_mismatched_geometry() {
        let e = extent();
        let small = Extent::new(2, 3).expect("valid extent");
        let input = [0.0; 9];
        let guide = [0.0; 6];
        let mut output = [0.0; 9];
        let mut a = [0.0; 9];
        let mut b = [0.0; 9];
        let mut c = [0.0; 9];
        let result = guided_with_guide(
            Plane::new(&input, e, 3).expect("valid input"),
            Plane::new(&guide, small, 2).expect("valid guide"),
            PlaneMut::new(&mut output, e, 3).expect("valid output"),
            PlaneMut::new(&mut a, e, 3).expect("valid scratch"),
            PlaneMut::new(&mut b, e, 3).expect("valid scratch"),
            PlaneMut::new(&mut c, e, 3).expect("valid scratch"),
            GuidedConfig {
                radius: 1,
                epsilon: 0.01,
            },
        );
        assert_eq!(result, Err(EdgeAwareError::ExtentMismatch));
    }

    #[test]
    fn recursive_filters_accept_odd_padded_planes_and_preserve_nan() {
        let e = extent();
        let source = [1.0, 2.0, 3.0, 77.0, f32::NAN, 4.0, 5.0, 77.0, 6.0, 7.0, 8.0];
        let mut output = [0.0; 11];
        let mut scratch = [0.0; 11];
        domain_transform(
            Plane::new(&source, e, 4).expect("valid input"),
            PlaneMut::new(&mut output, e, 4).expect("valid output"),
            PlaneMut::new(&mut scratch, e, 4).expect("valid scratch"),
            DomainTransformConfig {
                sigma_spatial: 2.0,
                sigma_range: 1.0,
                iterations: 2,
            },
        )
        .expect("domain transform succeeds");
        assert!(output[4].is_nan());
        assert_eq!(output[3], 0.0);

        rolling_guidance(
            Plane::new(&source, e, 4).expect("valid input"),
            PlaneMut::new(&mut output, e, 4).expect("valid output"),
            PlaneMut::new(&mut scratch, e, 4).expect("valid scratch"),
            RollingGuidanceConfig {
                radius: 1,
                range_sigma: 1.0,
                iterations: 1,
            },
        )
        .expect("rolling guidance succeeds");
        assert!(output[4].is_nan());
        global_smooth(
            Plane::new(&source, e, 4).expect("valid input"),
            PlaneMut::new(&mut output, e, 4).expect("valid output"),
            PlaneMut::new(&mut scratch, e, 4).expect("valid scratch"),
            GlobalSmoothConfig {
                iterations: 1,
                edge_sigma: 1.0,
                step: 0.2,
            },
        )
        .expect("global smoothing succeeds");
        assert!(output[4].is_nan());
    }

    #[test]
    fn invalid_configuration_is_rejected_before_processing() {
        let e = extent();
        let source = [0.0; 9];
        let mut output = [0.0; 9];
        let mut scratch = [0.0; 9];
        assert_eq!(
            domain_transform(
                Plane::new(&source, e, 3).expect("valid input"),
                PlaneMut::new(&mut output, e, 3).expect("valid output"),
                PlaneMut::new(&mut scratch, e, 3).expect("valid scratch"),
                DomainTransformConfig {
                    sigma_spatial: f32::NAN,
                    sigma_range: 1.0,
                    iterations: 1
                },
            ),
            Err(EdgeAwareError::InvalidConfiguration)
        );
    }

    #[test]
    fn guided_uses_one_paired_population_for_all_moments() {
        let e = Extent::new(3, 1).expect("valid extent");
        let source = [1.0, f32::NAN, 3.0];
        let guide = [1.0, 2.0, f32::NAN];
        let mut output = [0.0; 3];
        let mut first = [0.0; 3];
        let mut second = [0.0; 3];
        let mut third = [0.0; 3];
        guided_with_guide(
            Plane::new(&source, e, 3).expect("valid input"),
            Plane::new(&guide, e, 3).expect("valid guide"),
            PlaneMut::new(&mut output, e, 3).expect("valid output"),
            PlaneMut::new(&mut first, e, 3).expect("valid scratch"),
            PlaneMut::new(&mut second, e, 3).expect("valid scratch"),
            PlaneMut::new(&mut third, e, 3).expect("valid scratch"),
            GuidedConfig {
                radius: 1,
                epsilon: 0.01,
            },
        )
        .expect("paired guided filter succeeds");
        assert_eq!(output[0], 1.0);
        assert!(output[1].is_nan());
        assert_eq!(output[2], 3.0);
    }

    #[test]
    fn recursive_filters_keep_finite_constants_at_extreme_scales() {
        let e = Extent::new(2, 1).expect("valid extent");
        let source = [1.0, 1.0];
        let mut output = [0.0; 2];
        let mut scratch = [0.0; 2];
        domain_transform(
            Plane::new(&source, e, 2).expect("valid input"),
            PlaneMut::new(&mut output, e, 2).expect("valid output"),
            PlaneMut::new(&mut scratch, e, 2).expect("valid scratch"),
            DomainTransformConfig {
                sigma_spatial: f32::MAX,
                sigma_range: f32::MIN_POSITIVE,
                iterations: 2,
            },
        )
        .expect("finite extreme domain parameters succeed");
        assert_eq!(output, source);

        rolling_guidance(
            Plane::new(&source, e, 2).expect("valid input"),
            PlaneMut::new(&mut output, e, 2).expect("valid output"),
            PlaneMut::new(&mut scratch, e, 2).expect("valid scratch"),
            RollingGuidanceConfig {
                radius: 1,
                range_sigma: f32::MIN_POSITIVE,
                iterations: 1,
            },
        )
        .expect("finite extreme range succeeds");
        assert_eq!(output, source);

        global_smooth(
            Plane::new(&source, e, 2).expect("valid input"),
            PlaneMut::new(&mut output, e, 2).expect("valid output"),
            PlaneMut::new(&mut scratch, e, 2).expect("valid scratch"),
            GlobalSmoothConfig {
                iterations: 1,
                edge_sigma: f32::MIN_POSITIVE,
                step: 0.25,
            },
        )
        .expect("finite extreme edge scale succeeds");
        assert_eq!(output, source);
    }
}

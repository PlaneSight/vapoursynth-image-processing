//! Deterministic scalar reference operations for translation registration.

use core::fmt;

use vsip_core::{Extent, Plane, PlaneMut};
use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Errors returned by registration operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisterError {
    /// Input or output planes have different visible extents.
    ExtentMismatch,
    /// An operation requiring an input set received none.
    EmptyInput,
    /// A public configuration violates its documented invariant.
    InvalidConfiguration,
    /// Caller-provided scratch cannot hold the requested values.
    ScratchTooSmall,
    /// No candidate met the configured finite-pair coverage requirement.
    InsufficientOverlap,
    /// A mathematically valid operation produced a translation outside `f32`.
    ArithmeticOverflow,
}
impl fmt::Display for RegisterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ExtentMismatch => "all planes must have the same extent",
            Self::EmptyInput => "at least one input is required",
            Self::InvalidConfiguration => "registration configuration is invalid",
            Self::ScratchTooSmall => "caller scratch is too small",
            Self::InsufficientOverlap => "no candidate met the finite-overlap requirement",
            Self::ArithmeticOverflow => "registration arithmetic exceeded the output range",
        })
    }
}
impl std::error::Error for RegisterError {}

/// A translation maps a destination position to a source position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Translation {
    /// Horizontal source-coordinate offset in pixels.
    pub x: f32,
    /// Vertical source-coordinate offset in pixels.
    pub y: f32,
}
impl Translation {
    /// Rejects non-finite coordinate offsets.
    pub fn validate(self) -> Result<(), RegisterError> {
        if self.x.is_finite() && self.y.is_finite() {
            Ok(())
        } else {
            Err(RegisterError::InvalidConfiguration)
        }
    }
}

/// Integer-search settings for translation estimation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EstimateConfig {
    /// Inclusive integer search radius in pixels.
    pub max_shift: i32,
    /// Positive raster sampling increment used during estimation.
    pub sample_step: usize,
    /// Required fraction of sampled positions containing finite overlapping pairs.
    pub minimum_overlap: f32,
}
impl EstimateConfig {
    /// Validates the bounded integer-search configuration.
    pub fn validate(self) -> Result<(), RegisterError> {
        if self.max_shift < 0
            || self.sample_step == 0
            || !self.minimum_overlap.is_finite()
            || self.minimum_overlap <= 0.0
            || self.minimum_overlap > 1.0
        {
            Err(RegisterError::InvalidConfiguration)
        } else {
            Ok(())
        }
    }

    /// Validates that the search includes only translations with a possible overlap.
    pub fn validate_for_extent(self, extent: Extent) -> Result<(), RegisterError> {
        self.validate()?;
        let maximum_useful_shift = extent.width().max(extent.height()) - 1;
        if self.max_shift as usize > maximum_useful_shift {
            return Err(RegisterError::InvalidConfiguration);
        }
        Ok(())
    }
}

/// Resampling mode used by [`warp`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Interpolation {
    /// Select the closest source sample.
    Nearest,
    /// Blend the four surrounding source samples.
    Bilinear,
}
/// Behavior for source coordinates outside a plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Border {
    /// Clamp outside coordinates to the nearest source edge.
    Clamp,
    /// Return this finite value for outside coordinates.
    Constant(f32),
}
impl Border {
    fn validate(self) -> Result<(), RegisterError> {
        match self {
            Self::Clamp => Ok(()),
            Self::Constant(value) if value.is_finite() => Ok(()),
            Self::Constant(_) => Err(RegisterError::InvalidConfiguration),
        }
    }
}
/// Configuration for [`warp`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WarpConfig {
    /// Sampling rule used for non-integer source coordinates.
    pub interpolation: Interpolation,
    /// Behavior outside the source extent.
    pub border: Border,
}
impl WarpConfig {
    /// Validates the border value.
    pub fn validate(self) -> Result<(), RegisterError> {
        self.border.validate()
    }
}
/// Combination method used by [`stack`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StackMethod {
    /// Average all finite input samples.
    Mean,
    /// Select the median finite input sample.
    Median,
}
/// Configuration for [`stack`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StackConfig {
    /// Combination rule applied at each pixel.
    pub method: StackMethod,
}
/// Configuration for camera-path stabilization.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StabilizeConfig {
    /// Positive half-width of the temporal smoothing window.
    pub radius: usize,
    /// Blend from no compensation at zero to full compensation at one.
    pub strength: f32,
}
impl StabilizeConfig {
    /// Validates the smoothing window and blend factor.
    pub fn validate(self) -> Result<(), RegisterError> {
        if self.radius == 0 || !self.strength.is_finite() || !(0.0..=1.0).contains(&self.strength) {
            Err(RegisterError::InvalidConfiguration)
        } else {
            Ok(())
        }
    }
}

/// Estimates an integer translation by minimizing finite-sample squared error.
///
/// Work is `O((2 * max_shift + 1)^2 * width * height / sample_step^2)` and no
/// allocation or scratch storage is required. The returned translation samples
/// `moving` at `reference + translation`. Searches larger than the maximum
/// dimension are rejected because those candidates cannot overlap. Candidates
/// below `minimum_overlap` finite-pair coverage are ignored. Equal scores
/// prefer more finite pairs and then the translation nearest identity.
pub fn estimate(
    reference: Plane<'_, f32>,
    moving: Plane<'_, f32>,
    config: EstimateConfig,
) -> Result<Translation, RegisterError> {
    require_extent(reference.extent(), moving.extent())?;
    let e = reference.extent();
    config.validate_for_extent(e)?;
    let sample_positions = e
        .width()
        .div_ceil(config.sample_step)
        .checked_mul(e.height().div_ceil(config.sample_step))
        .ok_or(RegisterError::ArithmeticOverflow)?;
    let minimum_pairs =
        ((sample_positions as f64 * f64::from(config.minimum_overlap)).ceil() as usize).max(1);
    let mut best: Option<(f64, usize, i64, Translation)> = None;
    for dy in -config.max_shift..=config.max_shift {
        for dx in -config.max_shift..=config.max_shift {
            let mut sum = 0.0_f64;
            let mut count = 0_usize;
            for y in (0..e.height()).step_by(config.sample_step) {
                let my = y as i64 + i64::from(dy);
                if my < 0 || my >= e.height() as i64 {
                    continue;
                }
                let a = reference.row(y).expect("validated reference row");
                let b = moving.row(my as usize).expect("validated moving row");
                for x in (0..e.width()).step_by(config.sample_step) {
                    let mx = x as i64 + i64::from(dx);
                    if mx < 0 || mx >= e.width() as i64 {
                        continue;
                    }
                    let av = a[x];
                    let bv = b[mx as usize];
                    if av.is_finite() && bv.is_finite() {
                        let d = f64::from(av) - f64::from(bv);
                        sum += d * d;
                        count += 1;
                    }
                }
            }
            if count >= minimum_pairs {
                let score = sum / count as f64;
                let candidate = Translation {
                    x: dx as f32,
                    y: dy as f32,
                };
                let distance = i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy);
                let is_better = best.is_none_or(|(best_score, best_count, best_distance, _)| {
                    score < best_score
                        || (score == best_score
                            && (count > best_count
                                || (count == best_count && distance < best_distance)))
                });
                if is_better {
                    best = Some((score, count, distance, candidate));
                }
            }
        }
    }
    best.map(|(_, _, _, transform)| transform)
        .ok_or(RegisterError::InsufficientOverlap)
}

/// Resamples a source into a caller-owned destination plane.
///
/// Work is `O(width * height)` and uses no allocation. NaN source values
/// propagate through nearest sampling; bilinear sampling returns NaN if any
/// contributing in-bounds sample is non-finite.
pub fn warp(
    input: Plane<'_, f32>,
    mut output: PlaneMut<'_, f32>,
    translation: Translation,
    config: WarpConfig,
) -> Result<(), RegisterError> {
    translation.validate()?;
    config.validate()?;
    require_extent(input.extent(), output.extent())?;
    let e = input.extent();
    for y in 0..e.height() {
        let row = output.row_mut(y).expect("validated output row");
        for (x, dst) in row.iter_mut().enumerate() {
            *dst = sample(
                input,
                x as f64 + f64::from(translation.x),
                y as f64 + f64::from(translation.y),
                config,
            );
        }
    }
    Ok(())
}

/// Combines matching planes, using caller-owned `scratch` for each pixel's
/// finite samples. Mean is `O(inputs * pixels)`; median adds an in-place sort
/// of at most `inputs` values per pixel. No allocation occurs.
pub fn stack(
    inputs: &[Plane<'_, f32>],
    mut output: PlaneMut<'_, f32>,
    scratch: &mut [f32],
    config: StackConfig,
) -> Result<(), RegisterError> {
    let Some(first) = inputs.first().copied() else {
        return Err(RegisterError::EmptyInput);
    };
    let e = first.extent();
    require_extent(e, output.extent())?;
    if scratch.len() < inputs.len() {
        return Err(RegisterError::ScratchTooSmall);
    }
    for &plane in inputs {
        require_extent(e, plane.extent())?;
    }
    for y in 0..e.height() {
        let row = output.row_mut(y).expect("validated output row");
        for (x, dst) in row.iter_mut().enumerate() {
            let mut n = 0;
            for plane in inputs {
                let value = plane.row(y).expect("validated input row")[x];
                if value.is_finite() {
                    scratch[n] = value;
                    n += 1;
                }
            }
            *dst = match (config.method, n) {
                (_, 0) => f32::NAN,
                (StackMethod::Mean, _) => {
                    (scratch[..n].iter().map(|&v| f64::from(v)).sum::<f64>() / n as f64) as f32
                }
                (StackMethod::Median, _) => {
                    scratch[..n].sort_unstable_by(f32::total_cmp);
                    let upper = n / 2;
                    if n % 2 == 0 {
                        ((f64::from(scratch[upper - 1]) + f64::from(scratch[upper])) * 0.5) as f32
                    } else {
                        scratch[upper]
                    }
                }
            };
        }
    }
    Ok(())
}

/// Smooths a finite translation path and writes compensation translations.
/// `output` and `scratch` must match the input length. The algorithm is
/// `O(frames * radius)` and performs no allocation.
pub fn stabilize(
    input: &[Translation],
    output: &mut [Translation],
    scratch: &mut [Translation],
    config: StabilizeConfig,
) -> Result<(), RegisterError> {
    config.validate()?;
    if input.is_empty() {
        return Err(RegisterError::EmptyInput);
    }
    if input.len() != output.len() || input.len() != scratch.len() {
        return Err(RegisterError::ScratchTooSmall);
    }
    for &value in input {
        value.validate()?;
    }
    for (index, smoothed) in scratch.iter_mut().enumerate() {
        let start = index.saturating_sub(config.radius);
        let end = index.saturating_add(config.radius).min(input.len() - 1);
        let count = (end - start + 1) as f64;
        let mut x = 0.0_f64;
        let mut y = 0.0_f64;
        for &value in &input[start..=end] {
            x += f64::from(value.x);
            y += f64::from(value.y);
        }
        *smoothed = Translation {
            x: (x / count) as f32,
            y: (y / count) as f32,
        };
    }
    for ((&original, &smooth), destination) in
        input.iter().zip(scratch.iter()).zip(output.iter_mut())
    {
        let compensation = Translation {
            x: ((f64::from(smooth.x) - f64::from(original.x)) * f64::from(config.strength)) as f32,
            y: ((f64::from(smooth.y) - f64::from(original.y)) * f64::from(config.strength)) as f32,
        };
        compensation
            .validate()
            .map_err(|_| RegisterError::ArithmeticOverflow)?;
        *destination = compensation;
    }
    Ok(())
}

fn require_extent(expected: Extent, actual: Extent) -> Result<(), RegisterError> {
    if expected == actual {
        Ok(())
    } else {
        Err(RegisterError::ExtentMismatch)
    }
}
fn sample(input: Plane<'_, f32>, mut x: f64, mut y: f64, config: WarpConfig) -> f32 {
    if x.is_nan() || y.is_nan() {
        return f32::NAN;
    }
    if x.is_infinite() || y.is_infinite() {
        match config.border {
            Border::Constant(value) => return value,
            Border::Clamp => {
                let extent = input.extent();
                if x.is_infinite() {
                    x = if x.is_sign_negative() {
                        0.0
                    } else {
                        (extent.width() - 1) as f64
                    };
                }
                if y.is_infinite() {
                    y = if y.is_sign_negative() {
                        0.0
                    } else {
                        (extent.height() - 1) as f64
                    };
                }
            }
        }
    }
    match config.interpolation {
        Interpolation::Nearest => {
            sample_one(input, x.round() as i64, y.round() as i64, config.border)
        }
        Interpolation::Bilinear => {
            let x0 = x.floor();
            let y0 = y.floor();
            let tx = x - x0;
            let ty = y - y0;
            let a = sample_one(input, x0 as i64, y0 as i64, config.border);
            if tx == 0.0 && ty == 0.0 {
                return a;
            }
            let b = sample_one(input, x0 as i64 + 1, y0 as i64, config.border);
            if ty == 0.0 {
                return lerp(a, b, tx);
            }
            let c = sample_one(input, x0 as i64, y0 as i64 + 1, config.border);
            if tx == 0.0 {
                return lerp(a, c, ty);
            }
            let d = sample_one(input, x0 as i64 + 1, y0 as i64 + 1, config.border);
            if [a, b, c, d].iter().any(|v| !v.is_finite()) {
                f32::NAN
            } else {
                let top = f64::from(a) * (1.0 - tx) + f64::from(b) * tx;
                let bottom = f64::from(c) * (1.0 - tx) + f64::from(d) * tx;
                (top * (1.0 - ty) + bottom * ty) as f32
            }
        }
    }
}

fn lerp(a: f32, b: f32, amount: f64) -> f32 {
    if !a.is_finite() || !b.is_finite() {
        return f32::NAN;
    }
    (f64::from(a) * (1.0 - amount) + f64::from(b) * amount) as f32
}
fn sample_one(input: Plane<'_, f32>, x: i64, y: i64, border: Border) -> f32 {
    let e = input.extent();
    match border {
        Border::Constant(value)
            if x < 0 || y < 0 || x >= e.width() as i64 || y >= e.height() as i64 =>
        {
            value
        }
        Border::Clamp => {
            let xx = x.clamp(0, e.width() as i64 - 1) as usize;
            let yy = y.clamp(0, e.height() as i64 - 1) as usize;
            input.row(yy).expect("validated input row")[xx]
        }
        Border::Constant(_) => input.row(y as usize).expect("validated input row")[x as usize],
    }
}

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-register",
    namespace: "register",
    summary: "Image registration, alignment and robust stacking",
    filters: &[
        Filter {
            name: "Estimate",
            summary: "Estimate a geometric transform",
            maturity: Maturity::Experimental,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "Warp",
            summary: "Apply a registration transform",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
        Filter {
            name: "Stack",
            summary: "Robustly combine aligned clips",
            maturity: Maturity::Experimental,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "Stabilize",
            summary: "Smooth and compensate camera motion",
            maturity: Maturity::Experimental,
            execution: Execution::Temporal,
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::*;
    use vsip_core::{Extent, Plane, PlaneMut};
    #[test]
    fn estimate_and_warp_work_on_odd_padded_planes() {
        let e = Extent::new(3, 3).unwrap();
        let reference = [0., 1., 2., 99., 3., 4., 5., 99., 6., 7., 8.];
        let moving = [1., 2., 0., 99., 4., 5., 3., 99., 7., 8., 6.];
        let t = estimate(
            Plane::new(&reference, e, 4).unwrap(),
            Plane::new(&moving, e, 4).unwrap(),
            EstimateConfig {
                max_shift: 1,
                sample_step: 1,
                minimum_overlap: 0.5,
            },
        )
        .unwrap();
        assert_eq!(t, Translation { x: -1., y: 0. });
        let mut out = [-1.; 11];
        warp(
            Plane::new(&moving, e, 4).unwrap(),
            PlaneMut::new(&mut out, e, 4).unwrap(),
            Translation { x: -1., y: 0. },
            WarpConfig {
                interpolation: Interpolation::Nearest,
                border: Border::Clamp,
            },
        )
        .unwrap();
        assert_eq!(&out[..3], &[1., 1., 2.]);
        assert_eq!(out[3], -1.);
    }
    #[test]
    fn stack_handles_nan_and_scratch_errors() {
        let e = Extent::new(1, 1).unwrap();
        let a = [1.];
        let b = [f32::NAN];
        let c = [3.];
        let mut out = [0.];
        let mut scratch = [0.; 3];
        stack(
            &[
                Plane::new(&a, e, 1).unwrap(),
                Plane::new(&b, e, 1).unwrap(),
                Plane::new(&c, e, 1).unwrap(),
            ],
            PlaneMut::new(&mut out, e, 1).unwrap(),
            &mut scratch,
            StackConfig {
                method: StackMethod::Median,
            },
        )
        .unwrap();
        assert_eq!(out, [2.]);
        assert_eq!(
            stack(
                &[],
                PlaneMut::new(&mut out, e, 1).unwrap(),
                &mut scratch,
                StackConfig {
                    method: StackMethod::Mean
                }
            ),
            Err(RegisterError::EmptyInput)
        );
    }
    #[test]
    fn stabilization_rejects_invalid_paths() {
        let path = [
            Translation { x: 0., y: 0. },
            Translation { x: f32::NAN, y: 0. },
        ];
        let mut out = path;
        let mut scratch = path;
        assert_eq!(
            stabilize(
                &path,
                &mut out,
                &mut scratch,
                StabilizeConfig {
                    radius: 1,
                    strength: 1.
                }
            ),
            Err(RegisterError::InvalidConfiguration)
        );
    }

    #[test]
    fn estimate_prefers_identity_for_equal_scores_and_bounds_search() {
        let e = Extent::new(3, 2).unwrap();
        let constant = [4.0; 6];
        assert_eq!(
            estimate(
                Plane::new(&constant, e, 3).unwrap(),
                Plane::new(&constant, e, 3).unwrap(),
                EstimateConfig {
                    max_shift: 2,
                    sample_step: 1,
                    minimum_overlap: 0.5,
                },
            )
            .unwrap(),
            Translation { x: 0.0, y: 0.0 }
        );
        assert_eq!(
            estimate(
                Plane::new(&constant, e, 3).unwrap(),
                Plane::new(&constant, e, 3).unwrap(),
                EstimateConfig {
                    max_shift: 3,
                    sample_step: 1,
                    minimum_overlap: 0.5,
                },
            ),
            Err(RegisterError::InvalidConfiguration)
        );
    }

    #[test]
    fn estimate_enforces_finite_overlap_coverage() {
        let e = Extent::new(2, 1).unwrap();
        let sparse = [1.0, f32::NAN];
        assert_eq!(
            estimate(
                Plane::new(&sparse, e, 2).unwrap(),
                Plane::new(&sparse, e, 2).unwrap(),
                EstimateConfig {
                    max_shift: 0,
                    sample_step: 1,
                    minimum_overlap: 1.0,
                },
            ),
            Err(RegisterError::InsufficientOverlap)
        );
        assert_eq!(
            EstimateConfig {
                max_shift: 0,
                sample_step: 1,
                minimum_overlap: 0.0,
            }
            .validate(),
            Err(RegisterError::InvalidConfiguration)
        );
    }

    #[test]
    fn bilinear_identity_ignores_zero_weight_nan_neighbours() {
        let e = Extent::new(2, 1).unwrap();
        let input = [7.0, f32::NAN];
        let mut output = [0.0; 2];
        warp(
            Plane::new(&input, e, 2).unwrap(),
            PlaneMut::new(&mut output, e, 2).unwrap(),
            Translation { x: 0.0, y: 0.0 },
            WarpConfig {
                interpolation: Interpolation::Bilinear,
                border: Border::Clamp,
            },
        )
        .unwrap();
        assert_eq!(output[0], 7.0);
        assert!(output[1].is_nan());
    }

    #[test]
    fn median_and_stabilization_handle_numeric_extremes_explicitly() {
        let e = Extent::new(1, 1).unwrap();
        let first = [f32::MAX];
        let second = [f32::MAX];
        let mut output_sample = [0.0];
        let mut values = [0.0; 2];
        stack(
            &[
                Plane::new(&first, e, 1).unwrap(),
                Plane::new(&second, e, 1).unwrap(),
            ],
            PlaneMut::new(&mut output_sample, e, 1).unwrap(),
            &mut values,
            StackConfig {
                method: StackMethod::Median,
            },
        )
        .unwrap();
        assert_eq!(output_sample, [f32::MAX]);

        let path = [
            Translation {
                x: f32::MAX,
                y: 0.0,
            },
            Translation {
                x: -f32::MAX,
                y: 0.0,
            },
            Translation {
                x: f32::MAX,
                y: 0.0,
            },
        ];
        let mut output = path;
        let mut scratch = path;
        assert_eq!(
            stabilize(
                &path,
                &mut output,
                &mut scratch,
                StabilizeConfig {
                    radius: 1,
                    strength: 1.0,
                },
            ),
            Err(RegisterError::ArithmeticOverflow)
        );
    }
}

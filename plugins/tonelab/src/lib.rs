//! Scalar reference filters for local tone and exposure adjustment.

use core::fmt;
use vsip_core::{Extent, Plane, PlaneMut};
use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Errors returned by tone operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToneLabError {
    /// Input or output planes have different visible extents.
    ExtentMismatch,
    /// An operation requiring an input set received none.
    EmptyInput,
    /// A public configuration violates its documented invariant.
    InvalidConfiguration,
    /// Caller-provided storage is shorter than the required scratch length.
    ScratchTooSmall,
    /// Tile and bin count arithmetic exceeded `usize`.
    ArithmeticOverflow,
}
impl fmt::Display for ToneLabError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ExtentMismatch => "all planes must have the same extent",
            Self::EmptyInput => "at least one input is required",
            Self::InvalidConfiguration => "tone configuration is invalid",
            Self::ScratchTooSmall => "caller scratch is too small",
            Self::ArithmeticOverflow => "configuration arithmetic overflowed",
        })
    }
}
impl std::error::Error for ToneLabError {}
/// Finite input range used by histogram-based operations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampleRange {
    /// Inclusive finite lower sample value.
    pub minimum: f32,
    /// Inclusive finite upper sample value, greater than `minimum`.
    pub maximum: f32,
}
impl SampleRange {
    /// Validates finite ascending range endpoints.
    pub fn validate(self) -> Result<(), ToneLabError> {
        if self.minimum.is_finite() && self.maximum.is_finite() && self.minimum < self.maximum {
            Ok(())
        } else {
            Err(ToneLabError::InvalidConfiguration)
        }
    }
}
/// CLAHE tiling, histogram, and clipping parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClaheConfig {
    /// Positive horizontal tile count, no greater than image width.
    pub tiles_x: usize,
    /// Positive vertical tile count, no greater than image height.
    pub tiles_y: usize,
    /// Number of histogram bins, at least two.
    pub bins: usize,
    /// Positive maximum count retained in each histogram bin.
    pub clip_limit: u32,
    /// Range used to quantize and reconstruct histogram samples.
    pub range: SampleRange,
}
impl ClaheConfig {
    /// Validates extent-independent parameters and returns required histogram cells.
    pub fn scratch_len(self) -> Result<usize, ToneLabError> {
        self.range.validate()?;
        if self.tiles_x == 0 || self.tiles_y == 0 || self.bins < 2 || self.clip_limit == 0 {
            return Err(ToneLabError::InvalidConfiguration);
        }
        self.tiles_x
            .checked_mul(self.tiles_y)
            .and_then(|n| n.checked_mul(self.bins))
            .ok_or(ToneLabError::ArithmeticOverflow)
    }

    /// Validates the configuration for an image and returns required histogram cells.
    pub fn validate(self, e: Extent) -> Result<usize, ToneLabError> {
        let required = self.scratch_len()?;
        if self.tiles_x > e.width() || self.tiles_y > e.height() {
            return Err(ToneLabError::InvalidConfiguration);
        }
        Ok(required)
    }
}
/// Single-scale local-Laplacian parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalLaplacianConfig {
    /// Positive local-window half-width.
    pub radius: usize,
    /// Finite signed local-detail adjustment amount.
    pub detail: f32,
    /// Positive finite falloff threshold for local detail.
    pub edge_threshold: f32,
}
impl LocalLaplacianConfig {
    /// Validates local detail settings.
    pub fn validate(self) -> Result<(), ToneLabError> {
        if self.radius == 0
            || !self.detail.is_finite()
            || !self.edge_threshold.is_finite()
            || self.edge_threshold <= 0.
        {
            Err(ToneLabError::InvalidConfiguration)
        } else {
            Ok(())
        }
    }
}
/// Well-exposedness settings for multi-input fusion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExposureFusionConfig {
    /// Finite intensity receiving the largest fusion weight.
    pub midpoint: f32,
    /// Positive finite well-exposedness width.
    pub sigma: f32,
}
impl ExposureFusionConfig {
    /// Validates fusion weighting settings.
    pub fn validate(self) -> Result<(), ToneLabError> {
        if self.midpoint.is_finite() && self.sigma.is_finite() && self.sigma > 0. {
            Ok(())
        } else {
            Err(ToneLabError::InvalidConfiguration)
        }
    }
}
/// Illumination estimation and normalization settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalizeIlluminationConfig {
    /// Positive local illumination-window half-width.
    pub radius: usize,
    /// Positive finite output illumination target.
    pub target: f32,
    /// Positive finite denominator floor.
    pub floor: f32,
}
impl NormalizeIlluminationConfig {
    /// Validates illumination normalization settings.
    pub fn validate(self) -> Result<(), ToneLabError> {
        if self.radius > 0
            && self.target.is_finite()
            && self.target > 0.
            && self.floor.is_finite()
            && self.floor > 0.
        {
            Ok(())
        } else {
            Err(ToneLabError::InvalidConfiguration)
        }
    }
}

/// Applies contrast-limited adaptive histogram equalization.
/// Histograms and their cumulative distributions live in caller-owned
/// `scratch`. Adjacent tile mappings are bilinearly interpolated; work is
/// `O(pixels + tiles*bins)`.
pub fn clahe(
    input: Plane<'_, f32>,
    mut output: PlaneMut<'_, f32>,
    scratch: &mut [u32],
    config: ClaheConfig,
) -> Result<(), ToneLabError> {
    let e = input.extent();
    let required = config.validate(e)?;
    same(e, output.extent())?;
    if scratch.len() < required {
        return Err(ToneLabError::ScratchTooSmall);
    }
    let histograms = &mut scratch[..required];
    histograms.fill(0);
    for y in 0..e.height() {
        let row = input.row(y).unwrap();
        for (x, &value) in row.iter().enumerate() {
            if value.is_finite() {
                let tile = tile_index(x, y, e, config.tiles_x, config.tiles_y);
                let bin = bin_of(value, config.range, config.bins);
                let cell = &mut histograms[tile * config.bins + bin];
                *cell = cell
                    .checked_add(1)
                    .ok_or(ToneLabError::ArithmeticOverflow)?;
            }
        }
    }
    for histogram in histograms.chunks_exact_mut(config.bins) {
        let mut excess = 0_u64;
        for count in histogram.iter_mut() {
            if *count > config.clip_limit {
                excess = excess
                    .checked_add(u64::from(*count - config.clip_limit))
                    .ok_or(ToneLabError::ArithmeticOverflow)?;
                *count = config.clip_limit;
            }
        }
        let shared = u32::try_from(excess / config.bins as u64)
            .map_err(|_| ToneLabError::ArithmeticOverflow)?;
        let remainder = (excess % config.bins as u64) as usize;
        for (index, count) in histogram.iter_mut().enumerate() {
            let redistributed = shared
                .checked_add(u32::from(index < remainder))
                .ok_or(ToneLabError::ArithmeticOverflow)?;
            *count = count
                .checked_add(redistributed)
                .ok_or(ToneLabError::ArithmeticOverflow)?;
        }
        let mut cumulative = 0_u32;
        for count in histogram {
            cumulative = cumulative
                .checked_add(*count)
                .ok_or(ToneLabError::ArithmeticOverflow)?;
            *count = cumulative;
        }
    }
    for y in 0..e.height() {
        let src = input.row(y).unwrap();
        let dst = output.row_mut(y).unwrap();
        for (x, output_sample) in dst.iter_mut().enumerate() {
            let value = src[x];
            if !value.is_finite() {
                *output_sample = value;
                continue;
            }
            let bin = bin_of(value, config.range, config.bins);
            let (left, right, horizontal) = interpolation_tiles(x, e.width(), config.tiles_x);
            let (top, bottom, vertical) = interpolation_tiles(y, e.height(), config.tiles_y);
            let top_left = mapped_sample(
                histograms,
                top * config.tiles_x + left,
                config.bins,
                bin,
                value,
                config.range,
            );
            let top_right = mapped_sample(
                histograms,
                top * config.tiles_x + right,
                config.bins,
                bin,
                value,
                config.range,
            );
            let bottom_left = mapped_sample(
                histograms,
                bottom * config.tiles_x + left,
                config.bins,
                bin,
                value,
                config.range,
            );
            let bottom_right = mapped_sample(
                histograms,
                bottom * config.tiles_x + right,
                config.bins,
                bin,
                value,
                config.range,
            );
            let top_value = top_left * (1.0 - horizontal) + top_right * horizontal;
            let bottom_value = bottom_left * (1.0 - horizontal) + bottom_right * horizontal;
            *output_sample = (top_value * (1.0 - vertical) + bottom_value * vertical) as f32;
        }
    }
    Ok(())
}
/// Performs a single-scale local-Laplacian detail adjustment with caller-owned image scratch.
/// Cost is `O(width * height * radius^2)`; this deliberately scalar reference allocates nothing.
pub fn local_laplacian(
    input: Plane<'_, f32>,
    mut output: PlaneMut<'_, f32>,
    mut scratch: PlaneMut<'_, f32>,
    config: LocalLaplacianConfig,
) -> Result<(), ToneLabError> {
    config.validate()?;
    let e = input.extent();
    same(e, output.extent())?;
    same(e, scratch.extent())?;
    box_mean(input, &mut scratch, config.radius);
    for y in 0..e.height() {
        let src = input.row(y).unwrap();
        let base = scratch.row_mut(y).unwrap();
        let dst = output.row_mut(y).unwrap();
        for (x, output_sample) in dst.iter_mut().enumerate() {
            let detail = f64::from(src[x]) - f64::from(base[x]);
            *output_sample = if src[x].is_finite() {
                (f64::from(src[x])
                    + f64::from(config.detail)
                        * detail
                        * (-detail.abs() / f64::from(config.edge_threshold)).exp())
                    as f32
            } else {
                src[x]
            };
        }
    }
    Ok(())
}
/// Fuses finite samples from matching exposure planes by well-exposedness weights.
/// The caller supplies `inputs.len()` f32s of scratch, making storage independent of image size.
pub fn exposure_fusion(
    inputs: &[Plane<'_, f32>],
    mut output: PlaneMut<'_, f32>,
    scratch: &mut [f32],
    config: ExposureFusionConfig,
) -> Result<(), ToneLabError> {
    config.validate()?;
    let Some(first) = inputs.first().copied() else {
        return Err(ToneLabError::EmptyInput);
    };
    let e = first.extent();
    same(e, output.extent())?;
    if scratch.len() < inputs.len() {
        return Err(ToneLabError::ScratchTooSmall);
    }
    for &p in inputs {
        same(e, p.extent())?
    }
    let sigma = f64::from(config.sigma);
    let inverse = -0.5 / (sigma * sigma);
    for y in 0..e.height() {
        let dst = output.row_mut(y).unwrap();
        for (x, output_sample) in dst.iter_mut().enumerate() {
            let mut n = 0;
            for plane in inputs {
                let v = plane.row(y).unwrap()[x];
                if v.is_finite() {
                    scratch[n] = v;
                    n += 1;
                }
            }
            let maximum_exponent = scratch[..n]
                .iter()
                .map(|&value| {
                    let difference = f64::from(value) - f64::from(config.midpoint);
                    difference * difference * inverse
                })
                .fold(f64::NEG_INFINITY, f64::max);
            let (mut sum, mut weights) = (0_f64, 0_f64);
            for &value in &scratch[..n] {
                let difference = f64::from(value) - f64::from(config.midpoint);
                let exponent = difference * difference * inverse;
                let weight = (exponent - maximum_exponent).exp();
                sum += f64::from(value) * weight;
                weights += weight;
            }
            *output_sample = if weights == 0. {
                f32::NAN
            } else {
                (sum / weights) as f32
            };
        }
    }
    Ok(())
}
/// Estimates illumination with a local mean and normalizes it to a target level.
/// Cost is `O(width * height * radius^2)` and one caller-owned plane of scratch.
pub fn normalize_illumination(
    input: Plane<'_, f32>,
    mut output: PlaneMut<'_, f32>,
    mut scratch: PlaneMut<'_, f32>,
    config: NormalizeIlluminationConfig,
) -> Result<(), ToneLabError> {
    config.validate()?;
    let e = input.extent();
    same(e, output.extent())?;
    same(e, scratch.extent())?;
    box_mean(input, &mut scratch, config.radius);
    for y in 0..e.height() {
        let src = input.row(y).unwrap();
        let illumination = scratch.row_mut(y).unwrap();
        let dst = output.row_mut(y).unwrap();
        for x in 0..e.width() {
            dst[x] = if src[x].is_finite() {
                (f64::from(src[x]) * f64::from(config.target)
                    / f64::from(illumination[x].max(config.floor))) as f32
            } else {
                src[x]
            };
        }
    }
    Ok(())
}
fn same(a: Extent, b: Extent) -> Result<(), ToneLabError> {
    if a == b {
        Ok(())
    } else {
        Err(ToneLabError::ExtentMismatch)
    }
}
fn tile_index(x: usize, y: usize, e: Extent, tx: usize, ty: usize) -> usize {
    let ix = (x as u128 * tx as u128 / e.width() as u128) as usize;
    let iy = (y as u128 * ty as u128 / e.height() as u128) as usize;
    iy * tx + ix
}
fn bin_of(value: f32, range: SampleRange, bins: usize) -> usize {
    let minimum = f64::from(range.minimum);
    let normalized =
        ((f64::from(value) - minimum) / (f64::from(range.maximum) - minimum)).clamp(0.0, 1.0);
    (normalized * (bins - 1) as f64) as usize
}

fn interpolation_tiles(position: usize, length: usize, tiles: usize) -> (usize, usize, f64) {
    let current = (position as u128 * tiles as u128 / length as u128) as usize;
    let position_twice = 2 * position as u128;
    let current_center = tile_center_twice(current, length, tiles);
    if position_twice < current_center && current > 0 {
        let previous = current - 1;
        let previous_center = tile_center_twice(previous, length, tiles);
        let amount =
            (position_twice - previous_center) as f64 / (current_center - previous_center) as f64;
        (previous, current, amount)
    } else if position_twice > current_center && current + 1 < tiles {
        let next = current + 1;
        let next_center = tile_center_twice(next, length, tiles);
        let amount =
            (position_twice - current_center) as f64 / (next_center - current_center) as f64;
        (current, next, amount)
    } else {
        (current, current, 0.0)
    }
}

fn tile_center_twice(tile: usize, length: usize, tiles: usize) -> u128 {
    let start = (tile as u128 * length as u128).div_ceil(tiles as u128);
    let end = ((tile + 1) as u128 * length as u128).div_ceil(tiles as u128);
    start + end - 1
}

fn mapped_sample(
    histograms: &[u32],
    tile: usize,
    bins: usize,
    bin: usize,
    original: f32,
    range: SampleRange,
) -> f64 {
    let histogram = &histograms[tile * bins..(tile + 1) * bins];
    let total = histogram[bins - 1];
    let minimum_cdf = histogram
        .iter()
        .copied()
        .find(|&count| count != 0)
        .unwrap_or(0);
    if total == minimum_cdf {
        return f64::from(original.clamp(range.minimum, range.maximum));
    }
    let fraction =
        f64::from(histogram[bin].saturating_sub(minimum_cdf)) / f64::from(total - minimum_cdf);
    f64::from(range.minimum) + fraction * (f64::from(range.maximum) - f64::from(range.minimum))
}
fn box_mean(input: Plane<'_, f32>, output: &mut PlaneMut<'_, f32>, radius: usize) {
    let e = input.extent();
    for y in 0..e.height() {
        let dst = output.row_mut(y).unwrap();
        for (x, out) in dst.iter_mut().enumerate() {
            let x0 = x.saturating_sub(radius);
            let x1 = x.saturating_add(radius).min(e.width() - 1);
            let y0 = y.saturating_sub(radius);
            let y1 = y.saturating_add(radius).min(e.height() - 1);
            let (mut sum, mut count) = (0_f64, 0_usize);
            for yy in y0..=y1 {
                for &v in &input.row(yy).unwrap()[x0..=x1] {
                    if v.is_finite() {
                        sum += f64::from(v);
                        count += 1;
                    }
                }
            }
            *out = if count == 0 {
                f32::NAN
            } else {
                (sum / count as f64) as f32
            };
        }
    }
}

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-tonelab",
    namespace: "tonelab",
    summary: "Local tone, contrast and exposure operations",
    filters: &[
        Filter {
            name: "Clahe",
            summary: "Apply contrast-limited adaptive equalization",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
        Filter {
            name: "LocalLaplacian",
            summary: "Perform edge-aware local tone adjustment",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
        Filter {
            name: "ExposureFusion",
            summary: "Fuse differently exposed inputs",
            maturity: Maturity::Experimental,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "NormalizeIllumination",
            summary: "Separate and normalize illumination",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::*;
    use vsip_core::{Extent, Plane, PlaneMut};

    #[test]
    fn clahe_handles_odd_padding_and_rejects_short_scratch() {
        let e = Extent::new(3, 3).unwrap();
        let input = [0.0, 0.2, 0.4, 99.0, 0.6, 0.8, 1.0, 99.0, f32::NAN, 0.1, 0.3];
        let mut out = [-1.0; 11];
        let mut hist = [0; 8];
        let config = ClaheConfig {
            tiles_x: 2,
            tiles_y: 1,
            bins: 4,
            clip_limit: 3,
            range: SampleRange {
                minimum: 0.0,
                maximum: 1.0,
            },
        };
        clahe(
            Plane::new(&input, e, 4).unwrap(),
            PlaneMut::new(&mut out, e, 4).unwrap(),
            &mut hist,
            config,
        )
        .unwrap();
        assert_eq!(out[3], -1.0);
        assert!(out[8].is_nan());
        assert_eq!(
            clahe(
                Plane::new(&input, e, 4).unwrap(),
                PlaneMut::new(&mut out, e, 4).unwrap(),
                &mut [],
                config
            ),
            Err(ToneLabError::ScratchTooSmall)
        );
    }

    #[test]
    fn local_and_fusion_validate_boundaries() {
        let e = Extent::new(1, 1).unwrap();
        let input = [0.5];
        let mut out = [0.0];
        let mut scratch = [0.0];
        normalize_illumination(
            Plane::new(&input, e, 1).unwrap(),
            PlaneMut::new(&mut out, e, 1).unwrap(),
            PlaneMut::new(&mut scratch, e, 1).unwrap(),
            NormalizeIlluminationConfig {
                radius: 1,
                target: 1.0,
                floor: 0.1,
            },
        )
        .unwrap();
        assert_eq!(out, [1.0]);
        assert_eq!(
            local_laplacian(
                Plane::new(&input, e, 1).unwrap(),
                PlaneMut::new(&mut out, e, 1).unwrap(),
                PlaneMut::new(&mut scratch, e, 1).unwrap(),
                LocalLaplacianConfig {
                    radius: 0,
                    detail: 1.0,
                    edge_threshold: 1.0
                }
            ),
            Err(ToneLabError::InvalidConfiguration)
        );
        let nan = [f32::NAN];
        let mut weights = [0.0; 2];
        exposure_fusion(
            &[
                Plane::new(&input, e, 1).unwrap(),
                Plane::new(&nan, e, 1).unwrap(),
            ],
            PlaneMut::new(&mut out, e, 1).unwrap(),
            &mut weights,
            ExposureFusionConfig {
                midpoint: 0.5,
                sigma: 0.1,
            },
        )
        .unwrap();
        assert_eq!(out, [0.5]);
    }

    #[test]
    fn clahe_handles_the_full_f32_range_without_non_finite_math() {
        let e = Extent::new(3, 1).unwrap();
        let input = [-f32::MAX, 0.0, f32::MAX];
        let mut output = [0.0; 3];
        let mut histogram = [0; 2];
        clahe(
            Plane::new(&input, e, 3).unwrap(),
            PlaneMut::new(&mut output, e, 3).unwrap(),
            &mut histogram,
            ClaheConfig {
                tiles_x: 1,
                tiles_y: 1,
                bins: 2,
                clip_limit: 3,
                range: SampleRange {
                    minimum: -f32::MAX,
                    maximum: f32::MAX,
                },
            },
        )
        .unwrap();
        assert!(output.iter().all(|value| value.is_finite()));
        assert_eq!(output[0], -f32::MAX);
        assert_eq!(output[2], f32::MAX);
    }

    #[test]
    fn exposure_fusion_normalizes_extreme_log_weights() {
        let e = Extent::new(1, 1).unwrap();
        let near = [0.0];
        let far = [1.0];
        let mut output = [f32::NAN];
        let mut scratch = [0.0; 2];
        exposure_fusion(
            &[
                Plane::new(&near, e, 1).unwrap(),
                Plane::new(&far, e, 1).unwrap(),
            ],
            PlaneMut::new(&mut output, e, 1).unwrap(),
            &mut scratch,
            ExposureFusionConfig {
                midpoint: 0.0,
                sigma: f32::MIN_POSITIVE,
            },
        )
        .unwrap();
        assert_eq!(output, [0.0]);
    }

    #[test]
    fn clahe_rejects_invalid_parameters_before_an_extent_is_available() {
        assert_eq!(
            ClaheConfig {
                tiles_x: 0,
                tiles_y: 1,
                bins: 2,
                clip_limit: 1,
                range: SampleRange {
                    minimum: 0.0,
                    maximum: 1.0,
                },
            }
            .scratch_len(),
            Err(ToneLabError::InvalidConfiguration)
        );
        assert_eq!(
            ClaheConfig {
                tiles_x: usize::MAX,
                tiles_y: 2,
                bins: 2,
                clip_limit: 1,
                range: SampleRange {
                    minimum: 0.0,
                    maximum: 1.0,
                },
            }
            .scratch_len(),
            Err(ToneLabError::ArithmeticOverflow)
        );
    }
}

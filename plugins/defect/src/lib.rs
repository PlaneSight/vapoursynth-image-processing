//! Scalar reference filters for defect detection and conservative repair.
//!
//! Frame transforms borrow validated planes and write caller-owned outputs.
//! Detection reports own their variable-length observations.

use core::fmt;
use vsip_core::{Extent, Plane, PlaneMut};
use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-defect",
    namespace: "defect",
    summary: "Dust, scratches, dropouts and temporal restoration",
    filters: &[
        Filter {
            name: "TemporalOutliers",
            summary: "Detect isolated temporal defects",
            maturity: Maturity::Experimental,
            execution: Execution::Temporal,
        },
        Filter {
            name: "ScratchDetect",
            summary: "Detect persistent line-shaped defects",
            maturity: Maturity::Experimental,
            execution: Execution::Analysis,
        },
        Filter {
            name: "DropoutRepair",
            summary: "Repair explicitly masked dropouts from a reference clip",
            maturity: Maturity::Experimental,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "SpatialOutliers",
            summary: "Render single-frame local outlier candidates",
            maturity: Maturity::Experimental,
            execution: Execution::Analysis,
        },
        Filter {
            name: "InpaintTemporal",
            summary: "Repair an explicit mask from aligned temporal neighbours",
            maturity: Maturity::Experimental,
            execution: Execution::MultiInput,
        },
    ],
};

/// Failure returned by Defect reference operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DefectError {
    /// Inputs or output have different visible extents.
    ExtentMismatch,
    /// An analysis input contained a non-finite sample.
    NonFiniteSample,
    /// An owned detection report could not reserve storage.
    AllocationFailed,
}

impl fmt::Display for DefectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ExtentMismatch => "input and output extents differ",
            Self::NonFiniteSample => "analysis requires finite samples",
            Self::AllocationFailed => "detection report allocation failed",
        })
    }
}

impl std::error::Error for DefectError {}

/// Invalid configuration input for a defect filter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DefectConfigError {
    /// A threshold was negative or non-finite.
    InvalidThreshold,
    /// A blend weight was outside the closed range `[0, 1]` or non-finite.
    InvalidWeight,
    /// Scratch detection was configured with zero minimum length.
    ZeroMinimumLength,
}

impl fmt::Display for DefectConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidThreshold => "threshold must be finite and non-negative",
            Self::InvalidWeight => "repair weight must be finite and within zero through one",
            Self::ZeroMinimumLength => "minimum scratch length must be at least one",
        })
    }
}

impl std::error::Error for DefectConfigError {}

/// Validated thresholds for [`temporal_outliers`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TemporalOutlierConfig {
    threshold: f32,
    neighbour_tolerance: f32,
}

impl TemporalOutlierConfig {
    /// Creates a detector configuration.
    pub fn new(threshold: f32, neighbour_tolerance: f32) -> Result<Self, DefectConfigError> {
        if !threshold.is_finite()
            || !neighbour_tolerance.is_finite()
            || threshold < 0.0
            || neighbour_tolerance < 0.0
        {
            return Err(DefectConfigError::InvalidThreshold);
        }
        Ok(Self {
            threshold,
            neighbour_tolerance,
        })
    }

    /// Returns the required deviation from the neighbouring-frame estimate.
    pub const fn threshold(self) -> f32 {
        self.threshold
    }

    /// Returns the largest permitted difference between neighbouring frames.
    pub const fn neighbour_tolerance(self) -> f32 {
        self.neighbour_tolerance
    }
}

/// Validated settings for [`scratch_detect`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScratchDetectConfig {
    contrast_threshold: f32,
    minimum_length: usize,
}

impl ScratchDetectConfig {
    /// Creates a vertical-run scratch detector configuration.
    pub fn new(contrast_threshold: f32, minimum_length: usize) -> Result<Self, DefectConfigError> {
        if !contrast_threshold.is_finite() || contrast_threshold < 0.0 {
            return Err(DefectConfigError::InvalidThreshold);
        }
        if minimum_length == 0 {
            return Err(DefectConfigError::ZeroMinimumLength);
        }
        Ok(Self {
            contrast_threshold,
            minimum_length,
        })
    }

    /// Returns the minimum absolute horizontal contrast.
    pub const fn contrast_threshold(self) -> f32 {
        self.contrast_threshold
    }

    /// Returns the shortest emitted vertical run.
    pub const fn minimum_length(self) -> usize {
        self.minimum_length
    }
}

/// Validated settings for [`dropout_repair`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropoutRepairConfig {
    repair_weight: f32,
}

impl DropoutRepairConfig {
    /// Creates a repair blend configuration. Zero retains the current frame;
    /// one selects the supplied reference exactly.
    pub fn new(repair_weight: f32) -> Result<Self, DefectConfigError> {
        if !repair_weight.is_finite() || !(0.0..=1.0).contains(&repair_weight) {
            return Err(DefectConfigError::InvalidWeight);
        }
        Ok(Self { repair_weight })
    }

    /// Returns the reference-frame contribution for marked pixels.
    pub const fn repair_weight(self) -> f32 {
        self.repair_weight
    }
}

/// Validated settings for [`dead_pixels`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeadPixelConfig {
    deviation_threshold: f32,
}

impl DeadPixelConfig {
    /// Creates a local-deviation detector configuration.
    pub fn new(deviation_threshold: f32) -> Result<Self, DefectConfigError> {
        if !deviation_threshold.is_finite() || deviation_threshold < 0.0 {
            return Err(DefectConfigError::InvalidThreshold);
        }
        Ok(Self {
            deviation_threshold,
        })
    }

    /// Returns the minimum deviation from the four-neighbour mean.
    pub const fn deviation_threshold(self) -> f32 {
        self.deviation_threshold
    }
}

/// Estimate used by [`inpaint_temporal`] for marked samples.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TemporalEstimate {
    /// Use the arithmetic mean of finite aligned neighbours.
    Average,
    /// Use the finite aligned neighbour closest to the current sample.
    NearestToCurrent,
}

/// Settings for [`inpaint_temporal`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TemporalInpaintConfig {
    estimate: TemporalEstimate,
}

impl TemporalInpaintConfig {
    /// Creates a temporal inpainting configuration.
    pub const fn new(estimate: TemporalEstimate) -> Self {
        Self { estimate }
    }

    /// Returns the chosen neighbour estimate.
    pub const fn estimate(self) -> TemporalEstimate {
        self.estimate
    }
}

/// A position in a visible plane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pixel {
    /// Horizontal position.
    pub x: usize,
    /// Vertical position.
    pub y: usize,
}

/// Contrast direction of a detected scratch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScratchPolarity {
    /// The scratch is brighter than its horizontal neighbours.
    Bright,
    /// The scratch is darker than its horizontal neighbours.
    Dark,
}

/// One contiguous vertical scratch candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Scratch {
    /// Column containing the candidate.
    pub x: usize,
    /// First row in the candidate.
    pub start_y: usize,
    /// Number of contiguous rows in the candidate.
    pub length: usize,
    /// Candidate contrast direction.
    pub polarity: ScratchPolarity,
}

/// Owned variable-length output from [`scratch_detect`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScratchReport {
    /// Visible extent analysed by the detector.
    pub extent: Extent,
    /// Candidates in deterministic column, then row order.
    pub scratches: Vec<Scratch>,
}

/// Detects samples isolated from two temporally aligned neighbouring frames.
///
/// A sample is marked with 255 only when its current-frame deviation exceeds
/// `threshold` and the two neighbours agree within `neighbour_tolerance`.
/// Any non-finite triplet is deliberately treated as unclassified and marked 0.
/// The algorithm costs `O(pixels)` and allocates no memory.
pub fn temporal_outliers(
    previous: Plane<'_, f32>,
    current: Plane<'_, f32>,
    next: Plane<'_, f32>,
    mut output: PlaneMut<'_, u8>,
    config: TemporalOutlierConfig,
) -> Result<(), DefectError> {
    let extent = same_extent(previous.extent(), current.extent())?;
    same_extent(extent, next.extent())?;
    same_extent(extent, output.extent())?;
    for y in 0..extent.height() {
        let previous_row = previous.row(y).expect("validated row");
        let current_row = current.row(y).expect("validated row");
        let next_row = next.row(y).expect("validated row");
        let output_row = output.row_mut(y).expect("validated row");
        for x in 0..extent.width() {
            let before = previous_row[x];
            let sample = current_row[x];
            let after = next_row[x];
            let is_outlier = before.is_finite()
                && sample.is_finite()
                && after.is_finite()
                && (f64::from(sample) - temporal_mean(before, after)).abs()
                    > f64::from(config.threshold())
                && (f64::from(before) - f64::from(after)).abs()
                    <= f64::from(config.neighbour_tolerance());
            output_row[x] = if is_outlier { u8::MAX } else { 0 };
        }
    }
    Ok(())
}

/// Finds vertical line candidates using horizontal-neighbour contrast.
///
/// This reference scans each interior column in `O(pixels)` time. It returns
/// no candidate for the first or last column because a two-sided local baseline
/// does not exist there. Analysis rejects non-finite input instead of silently
/// inventing a contrast value.
pub fn scratch_detect(
    image: Plane<'_, f32>,
    config: ScratchDetectConfig,
) -> Result<ScratchReport, DefectError> {
    require_finite(image)?;
    let extent = image.extent();
    let mut scratches = Vec::new();
    if extent.width() < 3 {
        return Ok(ScratchReport { extent, scratches });
    }
    for x in 1..extent.width() - 1 {
        let mut run_start = None;
        let mut run_polarity = ScratchPolarity::Bright;
        for y in 0..extent.height() {
            let row = image.row(y).expect("validated row");
            let delta = f64::from(row[x]) - temporal_mean(row[x - 1], row[x + 1]);
            let candidate = if delta.abs() >= f64::from(config.contrast_threshold()) {
                Some(if delta >= 0.0 {
                    ScratchPolarity::Bright
                } else {
                    ScratchPolarity::Dark
                })
            } else {
                None
            };
            if candidate == Some(run_polarity) {
                if run_start.is_none() {
                    run_start = Some(y);
                }
                continue;
            }
            finish_scratch_run(
                &mut scratches,
                x,
                &mut run_start,
                run_polarity,
                y,
                config.minimum_length(),
            )?;
            if let Some(polarity) = candidate {
                run_start = Some(y);
                run_polarity = polarity;
            }
        }
        finish_scratch_run(
            &mut scratches,
            x,
            &mut run_start,
            run_polarity,
            extent.height(),
            config.minimum_length(),
        )?;
    }
    Ok(ScratchReport { extent, scratches })
}

/// Repairs marked samples by mixing the current and reference planes.
///
/// The caller supplies all stable-size storage. A weight of one bypasses the
/// current sample so a marked `NaN` is replaced by a finite reference exactly.
pub fn dropout_repair(
    current: Plane<'_, f32>,
    reference: Plane<'_, f32>,
    dropout_mask: Plane<'_, u8>,
    mut output: PlaneMut<'_, f32>,
    config: DropoutRepairConfig,
) -> Result<(), DefectError> {
    let extent = same_extent(current.extent(), reference.extent())?;
    same_extent(extent, dropout_mask.extent())?;
    same_extent(extent, output.extent())?;
    for y in 0..extent.height() {
        let current_row = current.row(y).expect("validated row");
        let reference_row = reference.row(y).expect("validated row");
        let mask_row = dropout_mask.row(y).expect("validated row");
        let output_row = output.row_mut(y).expect("validated row");
        for x in 0..extent.width() {
            output_row[x] = if mask_row[x] == 0 || config.repair_weight() == 0.0 {
                current_row[x]
            } else if config.repair_weight() == 1.0 {
                reference_row[x]
            } else {
                let weight = f64::from(config.repair_weight());
                (f64::from(current_row[x]) * (1.0 - weight) + f64::from(reference_row[x]) * weight)
                    as f32
            };
        }
    }
    Ok(())
}

/// Finds local spatial outliers as candidate dead pixels.
///
/// A reported point differs from the mean of its available four-connected
/// neighbours by at least the configured threshold. Establishing that a point
/// is *fixed* remains a multi-frame policy for the caller. The returned vector
/// is the only allocation and is ordered in raster order.
pub fn dead_pixels(
    image: Plane<'_, f32>,
    config: DeadPixelConfig,
) -> Result<Vec<Pixel>, DefectError> {
    require_finite(image)?;
    let extent = image.extent();
    let mut candidates = Vec::new();
    for y in 0..extent.height() {
        for x in 0..extent.width() {
            let mut sum = 0.0_f64;
            let mut count = 0_u32;
            for_neighbours(extent, x, y, |nx, ny| {
                sum += f64::from(image.row(ny).expect("validated row")[nx]);
                count += 1;
            });
            if count == 0 {
                continue;
            }
            let mean = (sum / f64::from(count)) as f32;
            if (image.row(y).expect("validated row")[x] - mean).abs()
                >= config.deviation_threshold()
            {
                candidates
                    .try_reserve(1)
                    .map_err(|_| DefectError::AllocationFailed)?;
                candidates.push(Pixel { x, y });
            }
        }
    }
    Ok(candidates)
}

/// Inpaints marked pixels from temporally aligned neighbours.
///
/// When only one neighbour is finite it is selected. When neither is finite,
/// the original sample is preserved. This transform allocates no memory.
pub fn inpaint_temporal(
    previous: Plane<'_, f32>,
    current: Plane<'_, f32>,
    next: Plane<'_, f32>,
    repair_mask: Plane<'_, u8>,
    mut output: PlaneMut<'_, f32>,
    config: TemporalInpaintConfig,
) -> Result<(), DefectError> {
    let extent = same_extent(previous.extent(), current.extent())?;
    same_extent(extent, next.extent())?;
    same_extent(extent, repair_mask.extent())?;
    same_extent(extent, output.extent())?;
    for y in 0..extent.height() {
        let before_row = previous.row(y).expect("validated row");
        let current_row = current.row(y).expect("validated row");
        let after_row = next.row(y).expect("validated row");
        let mask_row = repair_mask.row(y).expect("validated row");
        let output_row = output.row_mut(y).expect("validated row");
        for x in 0..extent.width() {
            output_row[x] = if mask_row[x] == 0 {
                current_row[x]
            } else {
                temporal_estimate(
                    before_row[x],
                    current_row[x],
                    after_row[x],
                    config.estimate(),
                )
            };
        }
    }
    Ok(())
}

fn same_extent(left: Extent, right: Extent) -> Result<Extent, DefectError> {
    if left != right {
        return Err(DefectError::ExtentMismatch);
    }
    Ok(left)
}

fn require_finite(image: Plane<'_, f32>) -> Result<(), DefectError> {
    if image.rows().flatten().any(|sample| !sample.is_finite()) {
        return Err(DefectError::NonFiniteSample);
    }
    Ok(())
}

fn finish_scratch_run(
    scratches: &mut Vec<Scratch>,
    x: usize,
    start: &mut Option<usize>,
    polarity: ScratchPolarity,
    end: usize,
    minimum_length: usize,
) -> Result<(), DefectError> {
    let Some(start_y) = start.take() else {
        return Ok(());
    };
    let length = end - start_y;
    if length >= minimum_length {
        scratches
            .try_reserve(1)
            .map_err(|_| DefectError::AllocationFailed)?;
        scratches.push(Scratch {
            x,
            start_y,
            length,
            polarity,
        });
    }
    Ok(())
}

fn for_neighbours(extent: Extent, x: usize, y: usize, mut visit: impl FnMut(usize, usize)) {
    if x > 0 {
        visit(x - 1, y);
    }
    if x + 1 < extent.width() {
        visit(x + 1, y);
    }
    if y > 0 {
        visit(x, y - 1);
    }
    if y + 1 < extent.height() {
        visit(x, y + 1);
    }
}

fn temporal_estimate(previous: f32, current: f32, next: f32, estimate: TemporalEstimate) -> f32 {
    match (previous.is_finite(), next.is_finite()) {
        (true, true) => match estimate {
            TemporalEstimate::Average => temporal_mean(previous, next) as f32,
            TemporalEstimate::NearestToCurrent if current.is_finite() => {
                if (f64::from(previous) - f64::from(current)).abs()
                    <= (f64::from(next) - f64::from(current)).abs()
                {
                    previous
                } else {
                    next
                }
            }
            TemporalEstimate::NearestToCurrent => previous,
        },
        (true, false) => previous,
        (false, true) => next,
        (false, false) => current,
    }
}

fn temporal_mean(left: f32, right: f32) -> f64 {
    (f64::from(left) + f64::from(right)) * 0.5
}

#[cfg(test)]
mod tests {
    use super::{
        DeadPixelConfig, DefectConfigError, DefectError, DropoutRepairConfig, Pixel, Scratch,
        ScratchDetectConfig, ScratchPolarity, TemporalEstimate, TemporalInpaintConfig,
        TemporalOutlierConfig, dead_pixels, dropout_repair, inpaint_temporal, scratch_detect,
        temporal_outliers,
    };
    use vsip_core::{Extent, Plane, PlaneMut};

    #[test]
    fn temporal_outliers_respect_padded_odd_rows_and_ignore_nan() {
        let extent = Extent::new(3, 1).expect("extent");
        let previous = [1.0, 1.0, 1.0, 99.0];
        let current = [1.0, 5.0, f32::NAN, 88.0];
        let next = [1.0, 1.0, 1.0, 77.0];
        let mut output = [7; 4];
        temporal_outliers(
            Plane::new(&previous, extent, 4).expect("plane"),
            Plane::new(&current, extent, 4).expect("plane"),
            Plane::new(&next, extent, 4).expect("plane"),
            PlaneMut::new(&mut output, extent, 4).expect("plane"),
            TemporalOutlierConfig::new(2.0, 0.0).expect("config"),
        )
        .expect("detect");
        assert_eq!(output, [0, u8::MAX, 0, 7]);
    }

    #[test]
    fn finite_extremes_do_not_overflow_detector_baselines() {
        let extent = Extent::new(3, 1).expect("extent");
        let maximums = [f32::MAX; 3];
        let mut temporal_mask = [1; 3];
        temporal_outliers(
            Plane::new(&maximums, extent, 3).expect("plane"),
            Plane::new(&maximums, extent, 3).expect("plane"),
            Plane::new(&maximums, extent, 3).expect("plane"),
            PlaneMut::new(&mut temporal_mask, extent, 3).expect("plane"),
            TemporalOutlierConfig::new(0.0, 0.0).expect("config"),
        )
        .expect("detect");
        assert_eq!(temporal_mask, [0; 3]);

        let report = scratch_detect(
            Plane::new(&maximums, extent, 3).expect("plane"),
            ScratchDetectConfig::new(1.0, 1).expect("config"),
        )
        .expect("detect");
        assert!(report.scratches.is_empty());
    }

    #[test]
    fn scratch_detection_returns_deterministic_runs() {
        let extent = Extent::new(3, 3).expect("extent");
        let image = [0.0, 4.0, 0.0, 0.0, 4.0, 0.0, 0.0, 4.0, 0.0];
        let report = scratch_detect(
            Plane::new(&image, extent, 3).expect("plane"),
            ScratchDetectConfig::new(3.0, 2).expect("config"),
        )
        .expect("detect");
        assert_eq!(
            report.scratches,
            vec![Scratch {
                x: 1,
                start_y: 0,
                length: 3,
                polarity: ScratchPolarity::Bright,
            }]
        );
    }

    #[test]
    fn dropout_repair_can_replace_a_marked_nan_without_touching_padding() {
        let extent = Extent::new(2, 1).expect("extent");
        let current = [f32::NAN, 2.0, 30.0];
        let reference = [4.0, 8.0, 40.0];
        let mask = [1, 0, 9];
        let mut output = [-1.0; 3];
        dropout_repair(
            Plane::new(&current, extent, 3).expect("plane"),
            Plane::new(&reference, extent, 3).expect("plane"),
            Plane::new(&mask, extent, 3).expect("plane"),
            PlaneMut::new(&mut output, extent, 3).expect("plane"),
            DropoutRepairConfig::new(1.0).expect("config"),
        )
        .expect("repair");
        assert_eq!(output, [4.0, 2.0, -1.0]);
    }

    #[test]
    fn dead_pixel_analysis_rejects_nan_and_finds_local_outlier() {
        let extent = Extent::new(3, 3).expect("extent");
        let image = [0.0, 0.0, 0.0, 0.0, 9.0, 0.0, 0.0, 0.0, 0.0];
        assert_eq!(
            dead_pixels(
                Plane::new(&image, extent, 3).expect("plane"),
                DeadPixelConfig::new(4.0).expect("config"),
            )
            .expect("detect"),
            vec![Pixel { x: 1, y: 1 }]
        );
        assert_eq!(
            dead_pixels(
                Plane::new(&[f32::NAN], Extent::new(1, 1).expect("extent"), 1).expect("plane"),
                DeadPixelConfig::new(1.0).expect("config"),
            ),
            Err(DefectError::NonFiniteSample)
        );
    }

    #[test]
    fn temporal_inpaint_selects_a_finite_aligned_neighbour() {
        let extent = Extent::new(1, 1).expect("extent");
        let mut output = [0.0];
        inpaint_temporal(
            Plane::new(&[f32::NAN], extent, 1).expect("plane"),
            Plane::new(&[10.0], extent, 1).expect("plane"),
            Plane::new(&[7.0], extent, 1).expect("plane"),
            Plane::new(&[1], extent, 1).expect("plane"),
            PlaneMut::new(&mut output, extent, 1).expect("plane"),
            TemporalInpaintConfig::new(TemporalEstimate::NearestToCurrent),
        )
        .expect("inpaint");
        assert_eq!(output, [7.0]);
    }

    #[test]
    fn repair_averages_remain_finite_at_f32_extremes() {
        let extent = Extent::new(1, 1).expect("extent");
        let mut dropout = [0.0];
        dropout_repair(
            Plane::new(&[f32::MAX], extent, 1).expect("plane"),
            Plane::new(&[f32::MAX], extent, 1).expect("plane"),
            Plane::new(&[1], extent, 1).expect("plane"),
            PlaneMut::new(&mut dropout, extent, 1).expect("plane"),
            DropoutRepairConfig::new(0.5).expect("config"),
        )
        .expect("repair");
        assert_eq!(dropout, [f32::MAX]);

        let mut temporal = [0.0];
        inpaint_temporal(
            Plane::new(&[f32::MAX], extent, 1).expect("plane"),
            Plane::new(&[0.0], extent, 1).expect("plane"),
            Plane::new(&[f32::MAX], extent, 1).expect("plane"),
            Plane::new(&[1], extent, 1).expect("plane"),
            PlaneMut::new(&mut temporal, extent, 1).expect("plane"),
            TemporalInpaintConfig::new(TemporalEstimate::Average),
        )
        .expect("inpaint");
        assert_eq!(temporal, [f32::MAX]);
    }

    #[test]
    fn isolated_pixel_has_no_spatial_baseline() {
        let extent = Extent::new(1, 1).expect("extent");
        let candidates = dead_pixels(
            Plane::new(&[7.0], extent, 1).expect("plane"),
            DeadPixelConfig::new(0.0).expect("config"),
        )
        .expect("detect");
        assert!(candidates.is_empty());
    }

    #[test]
    fn configuration_and_geometry_failures_are_values() {
        assert_eq!(
            TemporalOutlierConfig::new(-1.0, 0.0),
            Err(DefectConfigError::InvalidThreshold)
        );
        assert_eq!(
            ScratchDetectConfig::new(1.0, 0),
            Err(DefectConfigError::ZeroMinimumLength)
        );
        let left = Extent::new(1, 1).expect("extent");
        let right = Extent::new(2, 1).expect("extent");
        let mut output = [0; 2];
        assert_eq!(
            temporal_outliers(
                Plane::new(&[0.0], left, 1).expect("plane"),
                Plane::new(&[0.0], left, 1).expect("plane"),
                Plane::new(&[0.0], left, 1).expect("plane"),
                PlaneMut::new(&mut output, right, 2).expect("plane"),
                TemporalOutlierConfig::new(1.0, 1.0).expect("config"),
            ),
            Err(DefectError::ExtentMismatch)
        );
    }
}

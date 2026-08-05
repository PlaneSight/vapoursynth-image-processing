//! Deterministic scalar reference operations for dense motion fields.
//!
//! [`estimate`] performs exhaustive local block matching. Its work is
//! `O(width * height * (2 * search_radius + 1)^2 * (2 * patch_radius + 1)^2)`.
//! [`warp`], [`confidence`], [`compose`], and [`visualize`] are linear in the
//! visible pixel count. All fixed-size operations write caller-owned planes.

use core::fmt;

use vsip_core::{Extent, GeometryError, Plane, PlaneMut};
use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// One motion displacement in pixel units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MotionVector {
    /// Horizontal displacement; positive points right.
    pub x: f32,
    /// Vertical displacement; positive points down.
    pub y: f32,
}

/// An RGB value in the normalized `0.0..=1.0` range.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rgb {
    /// Red channel.
    pub red: f32,
    /// Green channel.
    pub green: f32,
    /// Blue channel.
    pub blue: f32,
}

/// Direction to estimate from a pair of frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    /// Store displacements from the first plane toward the second plane.
    Forward,
    /// Store displacements from the second plane toward the first plane.
    Backward,
}

/// Validated configuration for exhaustive dense motion estimation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EstimateConfig {
    search_radius: usize,
    patch_radius: usize,
    direction: Direction,
}

impl EstimateConfig {
    /// Creates an estimator configuration.
    ///
    /// `search_radius` must be non-zero. A zero `patch_radius` is valid and
    /// compares one sample per candidate.
    pub fn new(
        search_radius: usize,
        patch_radius: usize,
        direction: Direction,
    ) -> Result<Self, FlowError> {
        if search_radius == 0 {
            return Err(FlowError::ZeroSearchRadius);
        }
        signed_radius(search_radius)?;
        signed_radius(patch_radius)?;
        Ok(Self {
            search_radius,
            patch_radius,
            direction,
        })
    }

    /// Returns the maximum displacement searched in each axis.
    pub const fn search_radius(self) -> usize {
        self.search_radius
    }

    /// Returns the square patch radius used for matching.
    pub const fn patch_radius(self) -> usize {
        self.patch_radius
    }

    /// Returns the requested temporal direction.
    pub const fn direction(self) -> Direction {
        self.direction
    }
}

/// Border handling for resampling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BorderMode {
    /// Reuse the closest visible source sample.
    Clamp,
    /// Use a finite constant for samples outside the source plane.
    Constant(f32),
}

/// Interpolation used by [`warp`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Interpolation {
    /// Select the closest source sample.
    Nearest,
    /// Blend the four neighbouring source samples.
    Bilinear,
}

/// Validated configuration for [`warp`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WarpConfig {
    border: BorderMode,
    interpolation: Interpolation,
}

impl WarpConfig {
    /// Creates a warp configuration and validates its constant border value.
    pub fn new(border: BorderMode, interpolation: Interpolation) -> Result<Self, FlowError> {
        if let BorderMode::Constant(value) = border {
            if !value.is_finite() {
                return Err(FlowError::NonFiniteConfiguration);
            }
        }
        Ok(Self {
            border,
            interpolation,
        })
    }

    /// Returns the selected border policy.
    pub const fn border(self) -> BorderMode {
        self.border
    }

    /// Returns the selected interpolation policy.
    pub const fn interpolation(self) -> Interpolation {
        self.interpolation
    }
}

/// Validated configuration for [`visualize`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VisualizationConfig {
    maximum_magnitude: f32,
}

impl VisualizationConfig {
    /// Creates a visualization configuration.
    ///
    /// Vectors at or above `maximum_magnitude` render at full saturation.
    pub fn new(maximum_magnitude: f32) -> Result<Self, FlowError> {
        if !maximum_magnitude.is_finite() || maximum_magnitude <= 0.0 {
            return Err(FlowError::NonFiniteConfiguration);
        }
        Ok(Self { maximum_magnitude })
    }

    /// Returns the full-saturation motion magnitude.
    pub const fn maximum_magnitude(self) -> f32 {
        self.maximum_magnitude
    }
}

/// Failure returned by a FlowField reference operation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FlowError {
    /// A source or motion sample was not finite.
    NonFiniteSample,
    /// A configuration scalar was zero, infinite, or NaN.
    NonFiniteConfiguration,
    /// Estimation needs a non-zero search radius.
    ZeroSearchRadius,
    /// The patch radius exceeds a visible plane dimension.
    PatchRadiusTooLarge,
    /// Inputs or output have unequal visible extents.
    ExtentMismatch,
    /// The geometry cannot be represented in signed sampling coordinates.
    CoordinatesTooLarge,
    /// Finite motion inputs produced a displacement outside the f32 range.
    NumericalOverflow,
    /// A plane failed its stride or backing-buffer validation.
    Geometry(GeometryError),
}

impl fmt::Display for FlowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFiniteSample => formatter.write_str("input contains a non-finite sample"),
            Self::NonFiniteConfiguration => {
                formatter.write_str("configuration contains an invalid scalar")
            }
            Self::ZeroSearchRadius => formatter.write_str("search radius must be non-zero"),
            Self::PatchRadiusTooLarge => {
                formatter.write_str("patch radius must be smaller than both plane dimensions")
            }
            Self::ExtentMismatch => formatter.write_str("all planes must have the same extent"),
            Self::CoordinatesTooLarge => {
                formatter.write_str("plane coordinates exceed the scalar reference range")
            }
            Self::NumericalOverflow => {
                formatter.write_str("motion calculation exceeded the f32 range")
            }
            Self::Geometry(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for FlowError {}

impl From<GeometryError> for FlowError {
    fn from(error: GeometryError) -> Self {
        Self::Geometry(error)
    }
}

/// Estimates a dense integer-pixel field with deterministic exhaustive matching.
///
/// The returned vector at a source coordinate points to the matching coordinate
/// in the selected target frame. Edge patches use clamped source and target
/// samples. Inputs must be finite and every visible plane must have the same
/// extent. The output buffer is wholly caller-owned.
pub fn estimate(
    first: Plane<'_, f32>,
    second: Plane<'_, f32>,
    mut output: PlaneMut<'_, MotionVector>,
    config: EstimateConfig,
) -> Result<(), FlowError> {
    ensure_same_extent(first.extent(), second.extent())?;
    ensure_same_extent(first.extent(), output.extent())?;
    ensure_finite(first)?;
    ensure_finite(second)?;
    ensure_coordinate_range(first.extent())?;
    let smallest_dimension = first.extent().width().min(first.extent().height());
    if config.patch_radius >= smallest_dimension {
        return Err(FlowError::PatchRadiusTooLarge);
    }
    let radius = signed_radius(config.search_radius)?;
    let patch = signed_radius(config.patch_radius)?;
    let (source, target) = match config.direction {
        Direction::Forward => (first, second),
        Direction::Backward => (second, first),
    };
    let matching = MatchContext {
        source,
        target,
        patch_radius: patch,
        extent: source.extent(),
    };

    for (y, output_row) in output.rows_mut().enumerate() {
        for (x, destination) in output_row.iter_mut().enumerate() {
            let mut best_cost = f64::INFINITY;
            let mut best_x = 0_isize;
            let mut best_y = 0_isize;
            let x = isize::try_from(x).map_err(|_| FlowError::CoordinatesTooLarge)?;
            let y = isize::try_from(y).map_err(|_| FlowError::CoordinatesTooLarge)?;
            let maximum_x = isize::try_from(matching.extent.width() - 1)
                .map_err(|_| FlowError::CoordinatesTooLarge)?;
            let maximum_y = isize::try_from(matching.extent.height() - 1)
                .map_err(|_| FlowError::CoordinatesTooLarge)?;
            let minimum_dx = (-radius).max(-x);
            let maximum_dx = radius.min(maximum_x - x);
            let minimum_dy = (-radius).max(-y);
            let maximum_dy = radius.min(maximum_y - y);
            for dy in minimum_dy..=maximum_dy {
                for dx in minimum_dx..=maximum_dx {
                    let cost = matching.squared_error(x, y, dx, dy)?;
                    if is_better_match(cost, dx, dy, best_cost, best_x, best_y) {
                        best_cost = cost;
                        best_x = dx;
                        best_y = dy;
                    }
                }
            }
            *destination = MotionVector {
                x: best_x as f32,
                y: best_y as f32,
            };
        }
    }
    Ok(())
}

/// Resamples `source` into `output` with a displacement field.
///
/// A vector is interpreted as a displacement from an output coordinate to a
/// source coordinate: `output(x, y) = source(x + vector.x, y + vector.y)`.
/// This convention makes an inverse field directly usable for warping. Motion
/// values must be finite; source values may contain NaNs, which propagate
/// through interpolation.
pub fn warp(
    source: Plane<'_, f32>,
    field: Plane<'_, MotionVector>,
    mut output: PlaneMut<'_, f32>,
    config: WarpConfig,
) -> Result<(), FlowError> {
    ensure_same_extent(source.extent(), field.extent())?;
    ensure_same_extent(source.extent(), output.extent())?;
    ensure_finite_field(field)?;
    let extent = source.extent();
    ensure_coordinate_range(extent)?;
    let width = extent.width();
    let height = extent.height();

    for (y, output_row) in output.rows_mut().enumerate() {
        let field_row = field.row(y).ok_or(FlowError::ExtentMismatch)?;
        for (x, (destination, vector)) in output_row.iter_mut().zip(field_row).enumerate() {
            let source_x = x as f64 + f64::from(vector.x);
            let source_y = y as f64 + f64::from(vector.y);
            *destination = match config.interpolation {
                Interpolation::Nearest => {
                    sample_nearest(source, source_x, source_y, width, height, config.border)
                }
                Interpolation::Bilinear => {
                    sample_bilinear(source, source_x, source_y, width, height, config.border)
                }
            };
        }
    }
    Ok(())
}

/// Computes a forward/backward consistency confidence map.
///
/// Each output is `1 / (1 + |forward + backward_at_endpoint|)` and therefore
/// lies in `0.0..=1.0`. Backward vectors use nearest-neighbour clamped lookup.
pub fn confidence(
    forward: Plane<'_, MotionVector>,
    backward: Plane<'_, MotionVector>,
    mut output: PlaneMut<'_, f32>,
) -> Result<(), FlowError> {
    ensure_same_extent(forward.extent(), backward.extent())?;
    ensure_same_extent(forward.extent(), output.extent())?;
    ensure_finite_field(forward)?;
    ensure_finite_field(backward)?;
    let extent = forward.extent();

    for (y, output_row) in output.rows_mut().enumerate() {
        let forward_row = forward.row(y).ok_or(FlowError::ExtentMismatch)?;
        for (x, (destination, vector)) in output_row.iter_mut().zip(forward_row).enumerate() {
            let opposite = field_nearest(
                backward,
                x as f64 + f64::from(vector.x),
                y as f64 + f64::from(vector.y),
                extent,
            );
            let error_x = f64::from(vector.x) + f64::from(opposite.x);
            let error_y = f64::from(vector.y) + f64::from(opposite.y);
            *destination = (1.0 / (1.0 + error_x.hypot(error_y))) as f32;
        }
    }
    Ok(())
}

/// Composes `first` followed by `second` into `output`.
///
/// The result is `first(x, y) + second(x + first.x, y + first.y)` using a
/// nearest-neighbour clamped lookup for `second`. All fields must be finite.
pub fn compose(
    first: Plane<'_, MotionVector>,
    second: Plane<'_, MotionVector>,
    mut output: PlaneMut<'_, MotionVector>,
) -> Result<(), FlowError> {
    ensure_same_extent(first.extent(), second.extent())?;
    ensure_same_extent(first.extent(), output.extent())?;
    ensure_finite_field(first)?;
    ensure_finite_field(second)?;
    let extent = first.extent();

    for (y, output_row) in output.rows_mut().enumerate() {
        let first_row = first.row(y).ok_or(FlowError::ExtentMismatch)?;
        for (x, (destination, vector)) in output_row.iter_mut().zip(first_row).enumerate() {
            let next = field_nearest(
                second,
                x as f64 + f64::from(vector.x),
                y as f64 + f64::from(vector.y),
                extent,
            );
            *destination = MotionVector {
                x: finite_f32(f64::from(vector.x) + f64::from(next.x))?,
                y: finite_f32(f64::from(vector.y) + f64::from(next.y))?,
            };
        }
    }
    Ok(())
}

/// Renders field direction as hue and magnitude as saturation into RGB output.
///
/// A zero vector renders neutral grey. Magnitudes are clamped to the finite
/// maximum supplied in `config`; all motion samples must be finite.
pub fn visualize(
    field: Plane<'_, MotionVector>,
    mut output: PlaneMut<'_, Rgb>,
    config: VisualizationConfig,
) -> Result<(), FlowError> {
    ensure_same_extent(field.extent(), output.extent())?;
    ensure_finite_field(field)?;

    for (y, output_row) in output.rows_mut().enumerate() {
        let field_row = field.row(y).ok_or(FlowError::ExtentMismatch)?;
        for (destination, vector) in output_row.iter_mut().zip(field_row) {
            let magnitude = vector.x.hypot(vector.y);
            let saturation = (magnitude / config.maximum_magnitude).min(1.0);
            let hue = (vector.y.atan2(vector.x) / core::f32::consts::TAU).rem_euclid(1.0);
            *destination = hsv_to_rgb(hue, saturation, 0.5 + 0.5 * saturation);
        }
    }
    Ok(())
}

fn ensure_same_extent(left: Extent, right: Extent) -> Result<(), FlowError> {
    if left == right {
        Ok(())
    } else {
        Err(FlowError::ExtentMismatch)
    }
}

fn ensure_finite(plane: Plane<'_, f32>) -> Result<(), FlowError> {
    if plane.rows().flatten().all(|sample| sample.is_finite()) {
        Ok(())
    } else {
        Err(FlowError::NonFiniteSample)
    }
}

fn ensure_finite_field(plane: Plane<'_, MotionVector>) -> Result<(), FlowError> {
    if plane
        .rows()
        .flatten()
        .all(|vector| vector.x.is_finite() && vector.y.is_finite())
    {
        Ok(())
    } else {
        Err(FlowError::NonFiniteSample)
    }
}

fn signed_radius(radius: usize) -> Result<isize, FlowError> {
    isize::try_from(radius).map_err(|_| FlowError::CoordinatesTooLarge)
}

#[derive(Clone, Copy)]
struct MatchContext<'a> {
    source: Plane<'a, f32>,
    target: Plane<'a, f32>,
    patch_radius: isize,
    extent: Extent,
}

impl MatchContext<'_> {
    fn squared_error(self, x: isize, y: isize, dx: isize, dy: isize) -> Result<f64, FlowError> {
        let mut cost = 0.0_f64;
        for patch_y in -self.patch_radius..=self.patch_radius {
            for patch_x in -self.patch_radius..=self.patch_radius {
                let source_x = clamped_signed_offset(x, patch_x, self.extent.width())?;
                let source_y = clamped_signed_offset(y, patch_y, self.extent.height())?;
                let target_x =
                    clamped_signed_offset(x, patch_x.saturating_add(dx), self.extent.width())?;
                let target_y =
                    clamped_signed_offset(y, patch_y.saturating_add(dy), self.extent.height())?;
                let source_value = sample_visible(self.source, source_x, source_y);
                let target_value = sample_visible(self.target, target_x, target_y);
                let delta = f64::from(source_value) - f64::from(target_value);
                cost += delta * delta;
            }
        }
        Ok(cost)
    }
}

fn is_better_match(
    cost: f64,
    x: isize,
    y: isize,
    best_cost: f64,
    best_x: isize,
    best_y: isize,
) -> bool {
    if cost < best_cost {
        return true;
    }
    if cost != best_cost {
        return false;
    }
    let distance = x.saturating_mul(x).saturating_add(y.saturating_mul(y));
    let best_distance = best_x
        .saturating_mul(best_x)
        .saturating_add(best_y.saturating_mul(best_y));
    distance < best_distance
}

fn clamped_signed_offset(base: isize, offset: isize, limit: usize) -> Result<usize, FlowError> {
    let limit = isize::try_from(limit).map_err(|_| FlowError::CoordinatesTooLarge)?;
    Ok(base.saturating_add(offset).clamp(0, limit - 1) as usize)
}

fn sample_visible(plane: Plane<'_, f32>, x: usize, y: usize) -> f32 {
    plane.row(y).map_or(0.0, |row| row[x])
}

fn field_nearest(field: Plane<'_, MotionVector>, x: f64, y: f64, extent: Extent) -> MotionVector {
    let x = nearest_coordinate(x, extent.width());
    let y = nearest_coordinate(y, extent.height());
    field
        .row(y)
        .and_then(|row| row.get(x).copied())
        .unwrap_or_default()
}

fn nearest_coordinate(value: f64, limit: usize) -> usize {
    let last = (limit - 1) as f64;
    if value <= 0.0 {
        0
    } else if value >= last {
        limit - 1
    } else {
        value.round() as usize
    }
}

fn sample_nearest(
    source: Plane<'_, f32>,
    x: f64,
    y: f64,
    width: usize,
    height: usize,
    border: BorderMode,
) -> f32 {
    let rounded_x = x.round();
    let rounded_y = y.round();
    if rounded_x < 0.0 || rounded_y < 0.0 || rounded_x >= width as f64 || rounded_y >= height as f64
    {
        return match border {
            BorderMode::Constant(value) => value,
            BorderMode::Clamp => sample_visible(
                source,
                rounded_x.clamp(0.0, (width - 1) as f64) as usize,
                rounded_y.clamp(0.0, (height - 1) as f64) as usize,
            ),
        };
    }
    sample_visible(source, rounded_x as usize, rounded_y as usize)
}

fn sample_bilinear(
    source: Plane<'_, f32>,
    mut x: f64,
    mut y: f64,
    width: usize,
    height: usize,
    border: BorderMode,
) -> f32 {
    match border {
        BorderMode::Constant(value)
            if x <= -1.0 || y <= -1.0 || x >= width as f64 || y >= height as f64 =>
        {
            return value;
        }
        BorderMode::Clamp => {
            x = x.clamp(0.0, (width - 1) as f64);
            y = y.clamp(0.0, (height - 1) as f64);
        }
        BorderMode::Constant(_) => {}
    }
    let x0 = x.floor() as isize;
    let y0 = y.floor() as isize;
    let x_weight = x - x0 as f64;
    let y_weight = y - y0 as f64;
    let top = lerp(
        sample_border(source, x0, y0, width, height, border),
        sample_border(source, x0.saturating_add(1), y0, width, height, border),
        x_weight,
    );
    let bottom = lerp(
        sample_border(source, x0, y0.saturating_add(1), width, height, border),
        sample_border(
            source,
            x0.saturating_add(1),
            y0.saturating_add(1),
            width,
            height,
            border,
        ),
        x_weight,
    );
    lerp(top, bottom, y_weight)
}

fn sample_border(
    source: Plane<'_, f32>,
    x: isize,
    y: isize,
    width: usize,
    height: usize,
    border: BorderMode,
) -> f32 {
    if x >= 0 && y >= 0 && x < width as isize && y < height as isize {
        return sample_visible(source, x as usize, y as usize);
    }
    match border {
        BorderMode::Constant(value) => value,
        BorderMode::Clamp => sample_visible(
            source,
            x.clamp(0, width as isize - 1) as usize,
            y.clamp(0, height as isize - 1) as usize,
        ),
    }
}

fn lerp(left: f32, right: f32, amount: f64) -> f32 {
    (f64::from(left) + (f64::from(right) - f64::from(left)) * amount) as f32
}

fn ensure_coordinate_range(extent: Extent) -> Result<(), FlowError> {
    isize::try_from(extent.width()).map_err(|_| FlowError::CoordinatesTooLarge)?;
    isize::try_from(extent.height()).map_err(|_| FlowError::CoordinatesTooLarge)?;
    Ok(())
}

fn finite_f32(value: f64) -> Result<f32, FlowError> {
    let value = value as f32;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(FlowError::NumericalOverflow)
    }
}

fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> Rgb {
    let sector = hue * 6.0;
    let chroma = value * saturation;
    let secondary = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let (red, green, blue) = match sector as u32 {
        0 => (chroma, secondary, 0.0),
        1 => (secondary, chroma, 0.0),
        2 => (0.0, chroma, secondary),
        3 => (0.0, secondary, chroma),
        4 => (secondary, 0.0, chroma),
        _ => (chroma, 0.0, secondary),
    };
    let adjustment = value - chroma;
    Rgb {
        red: red + adjustment,
        green: green + adjustment,
        blue: blue + adjustment,
    }
}

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-flowfield",
    namespace: "flowfield",
    summary: "Dense motion fields, confidence and warping",
    filters: &[
        Filter {
            name: "Estimate",
            summary: "Estimate forward or backward dense motion",
            maturity: Maturity::Experimental,
            execution: Execution::Temporal,
        },
        Filter {
            name: "Warp",
            summary: "Warp a clip with a motion field",
            maturity: Maturity::Experimental,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "Confidence",
            summary: "Compute forward-backward confidence",
            maturity: Maturity::Experimental,
            execution: Execution::Analysis,
        },
        Filter {
            name: "Compose",
            summary: "Compose two motion fields",
            maturity: Maturity::Experimental,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "Visualize",
            summary: "Render direction and magnitude",
            maturity: Maturity::Experimental,
            execution: Execution::Analysis,
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::{
        BorderMode, Direction, EstimateConfig, FlowError, Interpolation, MotionVector, Plane,
        PlaneMut, Rgb, VisualizationConfig, WarpConfig, compose, confidence, estimate, visualize,
        warp,
    };
    use vsip_core::Extent;

    #[test]
    fn warp_handles_odd_padded_geometry_and_constant_borders() {
        let extent = Extent::new(3, 3).expect("valid extent");
        let source = [1.0, 2.0, 3.0, 91.0, 4.0, 5.0, 6.0, 92.0, 7.0, 8.0, 9.0];
        let field = [MotionVector { x: -1.0, y: 0.0 }; 9];
        let mut destination = [-1.0; 12];
        warp(
            Plane::new(&source, extent, 4).expect("source"),
            Plane::new(&field, extent, 3).expect("field"),
            PlaneMut::new(&mut destination, extent, 4).expect("destination"),
            WarpConfig::new(BorderMode::Constant(0.25), Interpolation::Nearest).expect("config"),
        )
        .expect("warp succeeds");
        assert_eq!(&destination[0..3], &[0.25, 1.0, 2.0]);
        assert_eq!(destination[3], -1.0);
    }

    #[test]
    fn compose_and_confidence_use_visible_rows_only() {
        let extent = Extent::new(3, 1).expect("valid extent");
        let forward = [
            MotionVector { x: 1.0, y: 0.0 },
            MotionVector { x: 1.0, y: 0.0 },
            MotionVector { x: 1.0, y: 0.0 },
            MotionVector { x: 99.0, y: 99.0 },
        ];
        let backward = [
            MotionVector { x: -1.0, y: 0.0 },
            MotionVector { x: -1.0, y: 0.0 },
            MotionVector { x: -1.0, y: 0.0 },
        ];
        let mut confidence_output = [-1.0; 3];
        confidence(
            Plane::new(&forward, extent, 4).expect("forward"),
            Plane::new(&backward, extent, 3).expect("backward"),
            PlaneMut::new(&mut confidence_output, extent, 3).expect("output"),
        )
        .expect("confidence succeeds");
        assert_eq!(confidence_output, [1.0; 3]);

        let mut composed = [MotionVector::default(); 3];
        compose(
            Plane::new(&forward, extent, 4).expect("forward"),
            Plane::new(&forward, extent, 4).expect("second"),
            PlaneMut::new(&mut composed, extent, 3).expect("output"),
        )
        .expect("compose succeeds");
        assert_eq!(composed[1], MotionVector { x: 2.0, y: 0.0 });
    }

    #[test]
    fn invalid_configuration_and_nan_motion_are_rejected() {
        assert!(matches!(
            EstimateConfig::new(0, 0, Direction::Forward),
            Err(FlowError::ZeroSearchRadius)
        ));
        assert!(matches!(
            VisualizationConfig::new(f32::NAN),
            Err(FlowError::NonFiniteConfiguration)
        ));
        let extent = Extent::new(1, 1).expect("valid extent");
        let field = [MotionVector {
            x: f32::NAN,
            y: 0.0,
        }];
        let mut output = [Rgb::default()];
        assert!(matches!(
            visualize(
                Plane::new(&field, extent, 1).expect("field"),
                PlaneMut::new(&mut output, extent, 1).expect("output"),
                VisualizationConfig::new(1.0).expect("config"),
            ),
            Err(FlowError::NonFiniteSample)
        ));
        let mut estimated = [MotionVector::default()];
        assert!(matches!(
            estimate(
                Plane::new(&[0.0], extent, 1).expect("first"),
                Plane::new(&[0.0], extent, 1).expect("second"),
                PlaneMut::new(&mut estimated, extent, 1).expect("output"),
                EstimateConfig::new(1, 1, Direction::Forward).expect("config"),
            ),
            Err(FlowError::PatchRadiusTooLarge)
        ));
    }

    #[test]
    fn estimate_is_deterministic_at_the_search_boundary() {
        let extent = Extent::new(3, 1).expect("valid extent");
        let first = [0.0, 0.0, 1.0];
        let second = [0.0, 1.0, 1.0];
        let mut one = [MotionVector::default(); 3];
        let mut two = [MotionVector::default(); 3];
        let config = EstimateConfig::new(1, 0, Direction::Forward).expect("config");
        estimate(
            Plane::new(&first, extent, 3).expect("first"),
            Plane::new(&second, extent, 3).expect("second"),
            PlaneMut::new(&mut one, extent, 3).expect("output"),
            config,
        )
        .expect("first estimate");
        estimate(
            Plane::new(&first, extent, 3).expect("first"),
            Plane::new(&second, extent, 3).expect("second"),
            PlaneMut::new(&mut two, extent, 3).expect("output"),
            config,
        )
        .expect("second estimate");
        assert_eq!(one, two);
    }

    #[test]
    fn estimate_prefers_zero_motion_on_equal_costs_and_stays_in_bounds() {
        let extent = Extent::new(2, 1).expect("valid extent");
        let source = [1.0, 1.0];
        let mut field = [MotionVector::default(); 2];
        estimate(
            Plane::new(&source, extent, 2).expect("source"),
            Plane::new(&source, extent, 2).expect("target"),
            PlaneMut::new(&mut field, extent, 2).expect("field"),
            EstimateConfig::new(8, 0, Direction::Forward).expect("config"),
        )
        .expect("estimate");
        assert_eq!(field, [MotionVector::default(); 2]);
    }

    #[test]
    fn bilinear_warp_handles_extreme_finite_coordinates_without_panicking() {
        let extent = Extent::new(1, 1).expect("valid extent");
        let source = [3.0];
        let field = [MotionVector {
            x: f32::MAX,
            y: -f32::MAX,
        }];
        let mut output = [0.0];
        warp(
            Plane::new(&source, extent, 1).expect("source"),
            Plane::new(&field, extent, 1).expect("field"),
            PlaneMut::new(&mut output, extent, 1).expect("output"),
            WarpConfig::new(BorderMode::Constant(0.25), Interpolation::Bilinear).expect("config"),
        )
        .expect("warp");
        assert_eq!(output, [0.25]);
    }

    #[test]
    fn compose_rejects_displacement_overflow() {
        let extent = Extent::new(1, 1).expect("valid extent");
        let field = [MotionVector {
            x: f32::MAX,
            y: 0.0,
        }];
        let mut output = [MotionVector::default()];
        assert!(matches!(
            compose(
                Plane::new(&field, extent, 1).expect("first"),
                Plane::new(&field, extent, 1).expect("second"),
                PlaneMut::new(&mut output, extent, 1).expect("output"),
            ),
            Err(FlowError::NumericalOverflow)
        ));
    }
}

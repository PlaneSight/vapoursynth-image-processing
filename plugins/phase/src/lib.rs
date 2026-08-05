//! Deterministic scalar reference operations for temporal translation analysis.
//!
//! [`correlate`] searches integer translations and refines an interior minimum
//! with a three-point quadratic fit. Its cost is
//! `O(width * height * (2 * max_displacement + 1)^2)`. [`local_motion`] applies
//! the same method per tile and returns its necessarily variable-size analysis
//! vector. [`magnify`] and [`event_energy`] are linear-time three-frame passes.

use core::fmt;

use vsip_core::{Extent, GeometryError, Plane, PlaneMut};
use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// A translation in pixel units from a reference plane toward a compared plane.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Translation {
    /// Horizontal translation; positive points right.
    pub x: f32,
    /// Vertical translation; positive points down.
    pub y: f32,
}

/// Validated configuration for global translation correlation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CorrelationConfig {
    max_displacement: usize,
}

impl CorrelationConfig {
    /// Creates a configuration with a non-zero exhaustive integer search radius.
    pub fn new(max_displacement: usize) -> Result<Self, PhaseError> {
        if max_displacement == 0 {
            return Err(PhaseError::ZeroSearchRadius);
        }
        signed_radius(max_displacement)?;
        Ok(Self { max_displacement })
    }

    /// Returns the maximum searched translation in each axis.
    pub const fn max_displacement(self) -> usize {
        self.max_displacement
    }
}

/// Validated configuration for tiled local motion analysis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalMotionConfig {
    tile_width: usize,
    tile_height: usize,
    max_displacement: usize,
}

impl LocalMotionConfig {
    /// Creates a tiled motion configuration with non-zero tile dimensions.
    pub fn new(
        tile_width: usize,
        tile_height: usize,
        max_displacement: usize,
    ) -> Result<Self, PhaseError> {
        if tile_width == 0 || tile_height == 0 {
            return Err(PhaseError::ZeroTileExtent);
        }
        if max_displacement == 0 {
            return Err(PhaseError::ZeroSearchRadius);
        }
        signed_radius(max_displacement)?;
        Ok(Self {
            tile_width,
            tile_height,
            max_displacement,
        })
    }

    /// Returns the nominal tile width.
    pub const fn tile_width(self) -> usize {
        self.tile_width
    }

    /// Returns the nominal tile height.
    pub const fn tile_height(self) -> usize {
        self.tile_height
    }

    /// Returns the per-tile maximum search displacement.
    pub const fn max_displacement(self) -> usize {
        self.max_displacement
    }
}

/// One local motion result, including the visible edge tile dimensions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileMotion {
    /// Left coordinate of this tile.
    pub x: usize,
    /// Top coordinate of this tile.
    pub y: usize,
    /// Visible tile width.
    pub width: usize,
    /// Visible tile height.
    pub height: usize,
    /// Translation estimated from the tile region.
    pub translation: Translation,
}

/// Validated configuration for three-frame motion magnification.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MagnifyConfig {
    gain: f32,
}

impl MagnifyConfig {
    /// Creates a magnification configuration with a finite gain.
    ///
    /// A gain of zero is valid and copies the center frame.
    pub fn new(gain: f32) -> Result<Self, PhaseError> {
        if !gain.is_finite() {
            return Err(PhaseError::NonFiniteConfiguration);
        }
        Ok(Self { gain })
    }

    /// Returns the temporal high-pass gain.
    pub const fn gain(self) -> f32 {
        self.gain
    }
}

/// Validated configuration for transient event energy scoring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EventEnergyConfig {
    energy_threshold: f32,
}

impl EventEnergyConfig {
    /// Creates a scorer configuration.
    ///
    /// A pixel is active when its centered temporal energy is at least this
    /// finite, non-negative threshold.
    pub fn new(energy_threshold: f32) -> Result<Self, PhaseError> {
        if !energy_threshold.is_finite() || energy_threshold < 0.0 {
            return Err(PhaseError::NonFiniteConfiguration);
        }
        Ok(Self { energy_threshold })
    }

    /// Returns the active-event energy threshold.
    pub const fn energy_threshold(self) -> f32 {
        self.energy_threshold
    }
}

/// Aggregate transient event measurements for one center frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EventEnergy {
    mean: f32,
    peak: f32,
    active_fraction: f32,
}

impl EventEnergy {
    /// Returns mean centered temporal energy across the plane.
    pub const fn mean(self) -> f32 {
        self.mean
    }

    /// Returns the greatest per-pixel centered temporal energy.
    pub const fn peak(self) -> f32 {
        self.peak
    }

    /// Returns the fraction of pixels at or above the energy threshold.
    pub const fn active_fraction(self) -> f32 {
        self.active_fraction
    }
}

/// Failure returned by a Phase reference operation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PhaseError {
    /// An input plane contains NaN or infinity.
    NonFiniteSample,
    /// A finite scalar configuration requirement was violated.
    NonFiniteConfiguration,
    /// Correlation requires a non-zero search radius.
    ZeroSearchRadius,
    /// Tile width and height must both be non-zero.
    ZeroTileExtent,
    /// Corresponding input and output planes do not have equal extents.
    ExtentMismatch,
    /// Geometry cannot be represented by scalar reference coordinates.
    CoordinatesTooLarge,
    /// Finite inputs produced a result outside the f32 range.
    NumericalOverflow,
    /// The variable-size local-motion result allocation failed before analysis.
    AllocationFailure,
    /// A plane failed its stride or backing-buffer validation.
    Geometry(GeometryError),
}

impl fmt::Display for PhaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFiniteSample => formatter.write_str("input contains a non-finite sample"),
            Self::NonFiniteConfiguration => {
                formatter.write_str("configuration contains an invalid scalar")
            }
            Self::ZeroSearchRadius => formatter.write_str("search radius must be non-zero"),
            Self::ZeroTileExtent => formatter.write_str("tile dimensions must be non-zero"),
            Self::ExtentMismatch => formatter.write_str("all planes must have the same extent"),
            Self::CoordinatesTooLarge => {
                formatter.write_str("plane coordinates exceed the scalar reference range")
            }
            Self::NumericalOverflow => {
                formatter.write_str("phase calculation exceeded the f32 range")
            }
            Self::AllocationFailure => formatter.write_str("local-motion result allocation failed"),
            Self::Geometry(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for PhaseError {}

impl From<GeometryError> for PhaseError {
    fn from(error: GeometryError) -> Self {
        Self::Geometry(error)
    }
}

/// Measures the global translation from `reference` toward `compared`.
///
/// This scalar reference minimizes mean squared error over all overlapping
/// samples. Equal costs choose smaller squared displacement, then the stable
/// search order, which gives reproducible results for textureless frames.
pub fn correlate(
    reference: Plane<'_, f32>,
    compared: Plane<'_, f32>,
    config: CorrelationConfig,
) -> Result<Translation, PhaseError> {
    ensure_same_extent(reference.extent(), compared.extent())?;
    ensure_finite(reference)?;
    ensure_finite(compared)?;
    let radius = signed_radius(config.max_displacement)?;
    correlate_region(reference, compared, 0, 0, reference.extent(), radius)
}

/// Measures one translation for each fixed tile, including partial edge tiles.
///
/// The returned vector is the only allocation in this API: result count varies
/// with plane geometry. Its capacity is reserved exactly from the tile grid.
pub fn local_motion(
    reference: Plane<'_, f32>,
    compared: Plane<'_, f32>,
    config: LocalMotionConfig,
) -> Result<Vec<TileMotion>, PhaseError> {
    validate_pair(reference, compared)?;
    let extent = reference.extent();
    let capacity = tile_count(extent, config)?;
    let mut results = Vec::new();
    results
        .try_reserve_exact(capacity)
        .map_err(|_| PhaseError::AllocationFailure)?;
    visit_validated_local_motion(reference, compared, config, |tile| results.push(tile))?;
    Ok(results)
}

/// Visits one translation for each fixed tile without allocating a result vector.
///
/// Tiles are emitted in row-major order, including partial right and bottom
/// edge tiles. The visitor runs synchronously and must not retain borrowed
/// plane data beyond the call.
pub fn visit_local_motion(
    reference: Plane<'_, f32>,
    compared: Plane<'_, f32>,
    config: LocalMotionConfig,
    visitor: impl FnMut(TileMotion),
) -> Result<(), PhaseError> {
    validate_pair(reference, compared)?;
    visit_validated_local_motion(reference, compared, config, visitor)
}

fn visit_validated_local_motion(
    reference: Plane<'_, f32>,
    compared: Plane<'_, f32>,
    config: LocalMotionConfig,
    mut visitor: impl FnMut(TileMotion),
) -> Result<(), PhaseError> {
    let extent = reference.extent();
    let radius = signed_radius(config.max_displacement)?;
    for y in (0..extent.height()).step_by(config.tile_height) {
        for x in (0..extent.width()).step_by(config.tile_width) {
            let tile_width = (extent.width() - x).min(config.tile_width);
            let tile_height = (extent.height() - y).min(config.tile_height);
            let tile_extent = Extent::new(tile_width, tile_height)?;
            visitor(TileMotion {
                x,
                y,
                width: tile_width,
                height: tile_height,
                translation: correlate_region(reference, compared, x, y, tile_extent, radius)?,
            });
        }
    }
    Ok(())
}

fn validate_pair(reference: Plane<'_, f32>, compared: Plane<'_, f32>) -> Result<(), PhaseError> {
    ensure_same_extent(reference.extent(), compared.extent())?;
    ensure_finite(reference)?;
    ensure_finite(compared)
}

fn tile_count(extent: Extent, config: LocalMotionConfig) -> Result<usize, PhaseError> {
    extent
        .width()
        .div_ceil(config.tile_width)
        .checked_mul(extent.height().div_ceil(config.tile_height))
        .ok_or(PhaseError::CoordinatesTooLarge)
}

/// Amplifies the three-sample temporal high-pass component around `current`.
///
/// The fixed reference band is represented by
/// `current - (previous + next) / 2`, so this API does not implement arbitrary
/// temporal-frequency selection. It is a deterministic, allocation-free
/// reference useful for short-window vibration inspection.
pub fn magnify(
    previous: Plane<'_, f32>,
    current: Plane<'_, f32>,
    next: Plane<'_, f32>,
    mut output: PlaneMut<'_, f32>,
    config: MagnifyConfig,
) -> Result<(), PhaseError> {
    ensure_same_extent(previous.extent(), current.extent())?;
    ensure_same_extent(previous.extent(), next.extent())?;
    ensure_same_extent(previous.extent(), output.extent())?;
    ensure_finite(previous)?;
    ensure_finite(current)?;
    ensure_finite(next)?;
    for (y, output_row) in output.rows_mut().enumerate() {
        let previous_row = previous.row(y).ok_or(PhaseError::ExtentMismatch)?;
        let current_row = current.row(y).ok_or(PhaseError::ExtentMismatch)?;
        let next_row = next.row(y).ok_or(PhaseError::ExtentMismatch)?;
        for (((destination, &before), &center), &after) in output_row
            .iter_mut()
            .zip(previous_row)
            .zip(current_row)
            .zip(next_row)
        {
            let high_pass = f64::from(center) - (f64::from(before) + f64::from(after)) * 0.5;
            *destination = finite_f32(f64::from(center) + f64::from(config.gain) * high_pass)?;
        }
    }
    Ok(())
}

/// Scores centered temporal energy and the fraction of active event pixels.
///
/// Per-pixel energy is `(current - (previous + next) / 2)^2`. The result is a
/// constant-size analysis value and consequently allocates no per-frame memory.
pub fn event_energy(
    previous: Plane<'_, f32>,
    current: Plane<'_, f32>,
    next: Plane<'_, f32>,
    config: EventEnergyConfig,
) -> Result<EventEnergy, PhaseError> {
    ensure_same_extent(previous.extent(), current.extent())?;
    ensure_same_extent(previous.extent(), next.extent())?;
    ensure_finite(previous)?;
    ensure_finite(current)?;
    ensure_finite(next)?;
    let mut sum = 0.0_f64;
    let mut peak = 0.0_f64;
    let mut active = 0_usize;
    let count = previous
        .extent()
        .area()
        .ok_or(PhaseError::CoordinatesTooLarge)?;
    for y in 0..previous.extent().height() {
        let previous_row = previous.row(y).ok_or(PhaseError::ExtentMismatch)?;
        let current_row = current.row(y).ok_or(PhaseError::ExtentMismatch)?;
        let next_row = next.row(y).ok_or(PhaseError::ExtentMismatch)?;
        for ((&before, &center), &after) in previous_row.iter().zip(current_row).zip(next_row) {
            let change = f64::from(center) - (f64::from(before) + f64::from(after)) * 0.5;
            let energy = change * change;
            sum += energy;
            peak = peak.max(energy);
            active += usize::from(energy >= f64::from(config.energy_threshold));
        }
    }
    Ok(EventEnergy {
        mean: finite_f32(sum / count as f64)?,
        peak: finite_f32(peak)?,
        active_fraction: (active as f64 / count as f64) as f32,
    })
}

#[derive(Clone, Copy)]
struct CorrelationRegion<'a> {
    reference: Plane<'a, f32>,
    compared: Plane<'a, f32>,
    origin_x: usize,
    origin_y: usize,
    extent: Extent,
    radius: isize,
}

fn correlate_region(
    reference: Plane<'_, f32>,
    compared: Plane<'_, f32>,
    origin_x: usize,
    origin_y: usize,
    region: Extent,
    radius: isize,
) -> Result<Translation, PhaseError> {
    CorrelationRegion {
        reference,
        compared,
        origin_x,
        origin_y,
        extent: region,
        radius,
    }
    .estimate()
}

impl CorrelationRegion<'_> {
    fn estimate(self) -> Result<Translation, PhaseError> {
        ensure_coordinate_range(self.reference.extent())?;
        let horizontal_radius = self.horizontal_radius()?;
        let vertical_radius = self.vertical_radius()?;
        let mut best_cost = f64::INFINITY;
        let mut best_x = 0_isize;
        let mut best_y = 0_isize;
        for offset_y in -vertical_radius..=vertical_radius {
            for offset_x in -horizontal_radius..=horizontal_radius {
                let Some(cost) = translation_cost(self, offset_x, offset_y)? else {
                    continue;
                };
                if is_better_candidate(cost, offset_x, offset_y, best_cost, best_x, best_y) {
                    best_cost = cost;
                    best_x = offset_x;
                    best_y = offset_y;
                }
            }
        }
        let x = refine_x(self, best_x, best_y, best_cost)?;
        let y = refine_y(self, best_x, best_y, best_cost)?;
        Ok(Translation {
            x: best_x as f32 + x,
            y: best_y as f32 + y,
        })
    }

    fn horizontal_radius(self) -> Result<isize, PhaseError> {
        Ok(self.radius.min(
            isize::try_from(self.reference.extent().width() - 1)
                .map_err(|_| PhaseError::CoordinatesTooLarge)?,
        ))
    }

    fn vertical_radius(self) -> Result<isize, PhaseError> {
        Ok(self.radius.min(
            isize::try_from(self.reference.extent().height() - 1)
                .map_err(|_| PhaseError::CoordinatesTooLarge)?,
        ))
    }
}

fn is_better_candidate(
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

fn refine_x(
    region: CorrelationRegion<'_>,
    best_x: isize,
    best_y: isize,
    center: f64,
) -> Result<f32, PhaseError> {
    if best_x.abs() >= region.horizontal_radius()? {
        return Ok(0.0);
    }
    let left = translation_cost(region, best_x - 1, best_y)?;
    let right = translation_cost(region, best_x + 1, best_y)?;
    Ok(refine_quadratic(left, center, right))
}

fn refine_y(
    region: CorrelationRegion<'_>,
    best_x: isize,
    best_y: isize,
    center: f64,
) -> Result<f32, PhaseError> {
    if best_y.abs() >= region.vertical_radius()? {
        return Ok(0.0);
    }
    let up = translation_cost(region, best_x, best_y - 1)?;
    let down = translation_cost(region, best_x, best_y + 1)?;
    Ok(refine_quadratic(up, center, down))
}

fn refine_quadratic(left: Option<f64>, center: f64, right: Option<f64>) -> f32 {
    let (Some(left), Some(right)) = (left, right) else {
        return 0.0;
    };
    let denominator = left - 2.0 * center + right;
    if denominator <= 0.0 || !denominator.is_finite() {
        return 0.0;
    }
    (0.5 * (left - right) / denominator).clamp(-0.5, 0.5) as f32
}

fn translation_cost(
    region: CorrelationRegion<'_>,
    offset_x: isize,
    offset_y: isize,
) -> Result<Option<f64>, PhaseError> {
    let extent = region.reference.extent();
    let mut square_sum = 0.0_f64;
    let mut count = 0_usize;
    for region_y in 0..region.extent.height() {
        let y = region.origin_y + region_y;
        let Some(compared_y) = offset_coordinate(y, offset_y, extent.height())? else {
            continue;
        };
        let reference_row = region.reference.row(y).ok_or(PhaseError::ExtentMismatch)?;
        let compared_row = region
            .compared
            .row(compared_y)
            .ok_or(PhaseError::ExtentMismatch)?;
        for region_x in 0..region.extent.width() {
            let x = region.origin_x + region_x;
            let Some(compared_x) = offset_coordinate(x, offset_x, extent.width())? else {
                continue;
            };
            let difference = f64::from(reference_row[x]) - f64::from(compared_row[compared_x]);
            square_sum += difference * difference;
            count += 1;
        }
    }
    if count == 0 {
        Ok(None)
    } else {
        Ok(Some(square_sum / count as f64))
    }
}

fn ensure_same_extent(left: Extent, right: Extent) -> Result<(), PhaseError> {
    if left == right {
        Ok(())
    } else {
        Err(PhaseError::ExtentMismatch)
    }
}

fn ensure_finite(plane: Plane<'_, f32>) -> Result<(), PhaseError> {
    if plane.rows().flatten().all(|sample| sample.is_finite()) {
        Ok(())
    } else {
        Err(PhaseError::NonFiniteSample)
    }
}

fn ensure_coordinate_range(extent: Extent) -> Result<(), PhaseError> {
    isize::try_from(extent.width()).map_err(|_| PhaseError::CoordinatesTooLarge)?;
    isize::try_from(extent.height()).map_err(|_| PhaseError::CoordinatesTooLarge)?;
    Ok(())
}

fn signed_radius(radius: usize) -> Result<isize, PhaseError> {
    isize::try_from(radius).map_err(|_| PhaseError::CoordinatesTooLarge)
}

fn offset_coordinate(
    base: usize,
    offset: isize,
    limit: usize,
) -> Result<Option<usize>, PhaseError> {
    let base = isize::try_from(base).map_err(|_| PhaseError::CoordinatesTooLarge)?;
    let limit = isize::try_from(limit).map_err(|_| PhaseError::CoordinatesTooLarge)?;
    let coordinate = base.saturating_add(offset);
    if coordinate < 0 || coordinate >= limit {
        Ok(None)
    } else {
        Ok(Some(coordinate as usize))
    }
}

fn finite_f32(value: f64) -> Result<f32, PhaseError> {
    let value = value as f32;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(PhaseError::NumericalOverflow)
    }
}

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-phase",
    namespace: "phase",
    summary: "Translation, vibration and transient event analysis",
    filters: &[
        Filter {
            name: "Correlate",
            summary: "Measure global translation by squared error",
            maturity: Maturity::Experimental,
            execution: Execution::Temporal,
        },
        Filter {
            name: "LocalMotion",
            summary: "Measure tiled translation by squared error",
            maturity: Maturity::Experimental,
            execution: Execution::Temporal,
        },
        Filter {
            name: "Magnify",
            summary: "Amplify a three-frame temporal high-pass signal",
            maturity: Maturity::Experimental,
            execution: Execution::Temporal,
        },
        Filter {
            name: "EventEnergy",
            summary: "Score thresholded transient energy",
            maturity: Maturity::Experimental,
            execution: Execution::Analysis,
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::{
        CorrelationConfig, EventEnergyConfig, LocalMotionConfig, MagnifyConfig, PhaseError, Plane,
        PlaneMut, correlate, event_energy, local_motion, magnify, visit_local_motion,
    };
    use vsip_core::Extent;

    #[test]
    fn correlation_and_local_motion_are_deterministic_for_padded_odd_planes() {
        let extent = Extent::new(3, 3).expect("valid extent");
        let reference = [0.0, 1.0, 0.0, 99.0, 1.0, 0.0, 1.0, 98.0, 0.0, 1.0, 0.0];
        let compared = [1.0, 0.0, 1.0, 87.0, 0.0, 1.0, 0.0, 86.0, 1.0, 0.0, 1.0];
        let config = CorrelationConfig::new(1).expect("config");
        let first = correlate(
            Plane::new(&reference, extent, 4).expect("reference"),
            Plane::new(&compared, extent, 4).expect("compared"),
            config,
        )
        .expect("correlation");
        let second = correlate(
            Plane::new(&reference, extent, 4).expect("reference"),
            Plane::new(&compared, extent, 4).expect("compared"),
            config,
        )
        .expect("correlation");
        assert_eq!(first, second);
        let tiles = local_motion(
            Plane::new(&reference, extent, 4).expect("reference"),
            Plane::new(&compared, extent, 4).expect("compared"),
            LocalMotionConfig::new(2, 2, 1).expect("tile config"),
        )
        .expect("local motion");
        assert_eq!(tiles.len(), 4);
        assert_eq!((tiles[3].width, tiles[3].height), (1, 1));
        let mut visited = 0;
        visit_local_motion(
            Plane::new(&reference, extent, 4).expect("reference"),
            Plane::new(&compared, extent, 4).expect("compared"),
            LocalMotionConfig::new(2, 2, 1).expect("tile config"),
            |_| visited += 1,
        )
        .expect("visit local motion");
        assert_eq!(visited, tiles.len());
    }

    #[test]
    fn magnify_and_energy_cover_temporal_boundaries() {
        let extent = Extent::new(3, 1).expect("valid extent");
        let previous = [0.0, 2.0, 0.0, 99.0];
        let current = [1.0, 2.0, 1.0, 98.0];
        let next = [0.0, 2.0, 0.0, 97.0];
        let mut output = [-1.0; 4];
        magnify(
            Plane::new(&previous, extent, 4).expect("previous"),
            Plane::new(&current, extent, 4).expect("current"),
            Plane::new(&next, extent, 4).expect("next"),
            PlaneMut::new(&mut output, extent, 4).expect("output"),
            MagnifyConfig::new(2.0).expect("config"),
        )
        .expect("magnify");
        assert_eq!(&output[..3], &[3.0, 2.0, 3.0]);
        assert_eq!(output[3], -1.0);
        let energy = event_energy(
            Plane::new(&previous, extent, 4).expect("previous"),
            Plane::new(&current, extent, 4).expect("current"),
            Plane::new(&next, extent, 4).expect("next"),
            EventEnergyConfig::new(1.0).expect("config"),
        )
        .expect("energy");
        assert_eq!(energy.active_fraction(), 2.0 / 3.0);
        assert_eq!(energy.peak(), 1.0);
    }

    #[test]
    fn rejects_invalid_configuration_geometry_and_nan() {
        assert!(matches!(
            CorrelationConfig::new(0),
            Err(PhaseError::ZeroSearchRadius)
        ));
        assert!(matches!(
            LocalMotionConfig::new(0, 1, 1),
            Err(PhaseError::ZeroTileExtent)
        ));
        assert!(matches!(
            EventEnergyConfig::new(f32::NAN),
            Err(PhaseError::NonFiniteConfiguration)
        ));
        assert!(matches!(
            CorrelationConfig::new(usize::MAX),
            Err(PhaseError::CoordinatesTooLarge)
        ));
        let extent = Extent::new(1, 1).expect("valid extent");
        assert!(matches!(
            correlate(
                Plane::new(&[f32::NAN], extent, 1).expect("nan"),
                Plane::new(&[0.0], extent, 1).expect("valid"),
                CorrelationConfig::new(1).expect("config"),
            ),
            Err(PhaseError::NonFiniteSample)
        ));

        assert!(matches!(
            event_energy(
                Plane::new(&[-f32::MAX], extent, 1).expect("previous"),
                Plane::new(&[f32::MAX], extent, 1).expect("current"),
                Plane::new(&[-f32::MAX], extent, 1).expect("next"),
                EventEnergyConfig::new(0.0).expect("config"),
            ),
            Err(PhaseError::NumericalOverflow)
        ));
    }
}

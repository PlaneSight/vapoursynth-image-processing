//! Scalar reference operations for binary masks and labelled regions.
//!
//! Frame-sized transforms borrow validated input planes and mutate only
//! caller-provided output planes. [`components`] is analysis and therefore
//! returns owned labels and component statistics.

use core::fmt;
use vsip_core::{Extent, Plane, PlaneMut};
pub use vsip_kernels::distance::{DistanceError, l1_to_zero};
use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-masklab",
    namespace: "masklab",
    summary: "Morphology, distance fields and connected-region analysis",
    filters: &[
        Filter {
            name: "DistanceL1",
            summary: "Manhattan distance to the nearest zero-valued mask pixel",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
        Filter {
            name: "DistanceEuclidean",
            summary: "Exact Euclidean distance transform",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
        Filter {
            name: "ComponentLabels",
            summary: "Render deterministic connected-component labels",
            maturity: Maturity::Experimental,
            execution: Execution::Analysis,
        },
        Filter {
            name: "Reconstruct",
            summary: "Geodesic morphological reconstruction",
            maturity: Maturity::Experimental,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "Thin",
            summary: "Topology-preserving binary thinning",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
        Filter {
            name: "Feather",
            summary: "Distance-based inner and outer mask feathering",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
    ],
};

/// Neighbourhood used by binary-region operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Connectivity {
    /// Pixels share an edge.
    Four,
    /// Pixels share an edge or a corner.
    Eight,
}

/// A rectangle enclosing a connected component.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bounds {
    /// Inclusive left coordinate.
    pub left: usize,
    /// Inclusive top coordinate.
    pub top: usize,
    /// Inclusive right coordinate.
    pub right: usize,
    /// Inclusive bottom coordinate.
    pub bottom: usize,
}

/// Statistics for one non-zero connected component.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Component {
    /// Deterministic label, equal to the component's first raster index plus one.
    pub label: u32,
    /// Number of pixels in the component.
    pub area: usize,
    /// Inclusive bounding rectangle.
    pub bounds: Bounds,
}

/// Owned output of [`components`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentAnalysis {
    /// Extent represented by [`Self::labels`].
    pub extent: Extent,
    /// Tight, row-major labels. Zero represents mask background.
    pub labels: Vec<u32>,
    /// Components ordered by their deterministic labels.
    pub components: Vec<Component>,
}

/// Failure shared by MaskLab reference operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaskLabError {
    /// Input and output extents differ.
    ExtentMismatch,
    /// A tight label image cannot use a distinct non-zero `u32` label per pixel.
    LabelOverflow,
    /// A reconstruction marker is greater than its corresponding mask value.
    MarkerExceedsMask,
    /// A caller-owned thinning scratch plane has the wrong extent.
    ScratchExtentMismatch,
    /// Thinning reached its configured iteration limit before convergence.
    IterationLimitReached,
    /// An owned component-analysis output could not reserve storage.
    AllocationFailed,
    /// Internal label propagation did not establish its required root label.
    ComponentInvariant,
}

impl fmt::Display for MaskLabError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ExtentMismatch => "input and output extents differ",
            Self::LabelOverflow => "component labels do not fit in u32",
            Self::MarkerExceedsMask => "reconstruction marker exceeds mask",
            Self::ScratchExtentMismatch => "scratch extent differs from the input extent",
            Self::IterationLimitReached => "thinning did not converge within its iteration limit",
            Self::AllocationFailed => "component-analysis output allocation failed",
            Self::ComponentInvariant => "component labels lost their raster-order root",
        })
    }
}

impl std::error::Error for MaskLabError {}

/// Invalid [`FeatherConfig`] construction input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeatherConfigError {
    /// A radius was negative or non-finite.
    InvalidRadius,
}

impl fmt::Display for FeatherConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("feather radii must be finite and non-negative")
    }
}

impl std::error::Error for FeatherConfigError {}

/// Validated inner and outer feather radii, expressed in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeatherConfig {
    inner_radius: f32,
    outer_radius: f32,
}

impl FeatherConfig {
    /// Validates radii. A zero radius produces a hard edge on that side.
    pub fn new(inner_radius: f32, outer_radius: f32) -> Result<Self, FeatherConfigError> {
        if !inner_radius.is_finite()
            || !outer_radius.is_finite()
            || inner_radius < 0.0
            || outer_radius < 0.0
        {
            return Err(FeatherConfigError::InvalidRadius);
        }
        Ok(Self {
            inner_radius,
            outer_radius,
        })
    }

    /// Returns the inner feather radius.
    pub const fn inner_radius(self) -> f32 {
        self.inner_radius
    }

    /// Returns the outer feather radius.
    pub const fn outer_radius(self) -> f32 {
        self.outer_radius
    }
}

/// Invalid [`ThinConfig`] construction input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThinConfigError {
    /// The maximum number of thinning cycles was zero.
    ZeroIterationLimit,
}

impl fmt::Display for ThinConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("thinning requires at least one iteration")
    }
}

impl std::error::Error for ThinConfigError {}

/// Validated convergence limit for [`thin`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThinConfig {
    max_iterations: usize,
}

impl ThinConfig {
    /// Creates a finite thinning configuration.
    pub const fn new(max_iterations: usize) -> Result<Self, ThinConfigError> {
        if max_iterations == 0 {
            return Err(ThinConfigError::ZeroIterationLimit);
        }
        Ok(Self { max_iterations })
    }

    /// Returns the largest number of Zhang-Suen cycles to run.
    pub const fn max_iterations(self) -> usize {
        self.max_iterations
    }
}

/// Computes exact Euclidean distance to the nearest zero-valued mask pixel.
///
/// This scalar reference uses exhaustive seed search, takes `O(pixels²)` work,
/// and allocates no memory. If the input contains no zero, output is infinity.
pub fn euclidean_to_zero(
    mask: Plane<'_, u8>,
    mut output: PlaneMut<'_, f32>,
) -> Result<(), MaskLabError> {
    let extent = matching_extent(mask.extent(), output.extent())?;
    for y in 0..extent.height() {
        for x in 0..extent.width() {
            let mut nearest_squared = f64::INFINITY;
            for seed_y in 0..extent.height() {
                let seed_row = mask.row(seed_y).expect("validated row");
                for (seed_x, &value) in seed_row.iter().enumerate() {
                    if value != 0 {
                        continue;
                    }
                    let dx = x.abs_diff(seed_x) as f64;
                    let dy = y.abs_diff(seed_y) as f64;
                    nearest_squared = nearest_squared.min(dx.mul_add(dx, dy * dy));
                }
            }
            output.row_mut(y).expect("validated row")[x] = nearest_squared.sqrt() as f32;
        }
    }
    Ok(())
}

/// Labels non-zero connected components and reports their basic statistics.
///
/// The output owns a tight label plane and a variable-length component list.
/// It deliberately performs no temporary allocation; its repeated label
/// propagation is a scalar reference with `O(pixels × convergence passes)` work.
pub fn components(
    mask: Plane<'_, u8>,
    connectivity: Connectivity,
) -> Result<ComponentAnalysis, MaskLabError> {
    let extent = mask.extent();
    let area = extent.area().ok_or(MaskLabError::LabelOverflow)?;
    u32::try_from(area).map_err(|_| MaskLabError::LabelOverflow)?;
    let mut labels = Vec::new();
    labels
        .try_reserve_exact(area)
        .map_err(|_| MaskLabError::AllocationFailed)?;
    labels.resize(area, 0_u32);

    for (y, row) in mask.rows().enumerate() {
        for (x, &value) in row.iter().enumerate() {
            if value != 0 {
                let index = y * extent.width() + x;
                labels[index] =
                    u32::try_from(index + 1).map_err(|_| MaskLabError::LabelOverflow)?;
            }
        }
    }

    while propagate_component_labels(&mut labels, extent, connectivity) {}

    let mut components = Vec::new();
    for (index, &label) in labels.iter().enumerate() {
        if label == 0 {
            continue;
        }
        let x = index % extent.width();
        let y = index / extent.width();
        if label == u32::try_from(index + 1).map_err(|_| MaskLabError::LabelOverflow)? {
            components
                .try_reserve(1)
                .map_err(|_| MaskLabError::AllocationFailed)?;
            components.push(Component {
                label,
                area: 0,
                bounds: Bounds {
                    left: x,
                    top: y,
                    right: x,
                    bottom: y,
                },
            });
        }
        let component_index = components
            .binary_search_by_key(&label, |component| component.label)
            .map_err(|_| MaskLabError::ComponentInvariant)?;
        let component = &mut components[component_index];
        component.area += 1;
        component.bounds.left = component.bounds.left.min(x);
        component.bounds.top = component.bounds.top.min(y);
        component.bounds.right = component.bounds.right.max(x);
        component.bounds.bottom = component.bounds.bottom.max(y);
    }

    Ok(ComponentAnalysis {
        extent,
        labels,
        components,
    })
}

/// Reconstructs `marker` under `mask` by geodesic dilation.
///
/// Inputs are `u8` grey-scale masks. The marker must be pointwise no greater
/// than the mask. The routine converges in finite steps, allocates no memory,
/// and writes only the caller-owned output plane.
pub fn reconstruct(
    marker: Plane<'_, u8>,
    mask: Plane<'_, u8>,
    mut output: PlaneMut<'_, u8>,
    connectivity: Connectivity,
) -> Result<(), MaskLabError> {
    let extent = matching_extent(marker.extent(), mask.extent())?;
    matching_extent(extent, output.extent())?;
    for (marker_row, mask_row) in marker.rows().zip(mask.rows()) {
        for (&marker_value, &mask_value) in marker_row.iter().zip(mask_row) {
            if marker_value > mask_value {
                return Err(MaskLabError::MarkerExceedsMask);
            }
        }
    }
    for (marker_row, output_row) in marker.rows().zip(output.rows_mut()) {
        for (&marker_value, destination) in marker_row.iter().zip(output_row) {
            *destination = marker_value;
        }
    }

    loop {
        let changed = reconstruction_pass(&mut output, mask, connectivity, true)
            | reconstruction_pass(&mut output, mask, connectivity, false);
        if !changed {
            return Ok(());
        }
    }
}

/// Applies Zhang-Suen binary thinning using caller-owned output and scratch planes.
///
/// Non-zero source values are foreground and the output is normalized to zero
/// or 255. Scratch is only used as a deletion map and is left unspecified on
/// return. Each cycle costs `O(pixels)` work and no allocation.
pub fn thin(
    mask: Plane<'_, u8>,
    mut output: PlaneMut<'_, u8>,
    mut scratch: PlaneMut<'_, u8>,
    config: ThinConfig,
) -> Result<(), MaskLabError> {
    let extent = matching_extent(mask.extent(), output.extent())?;
    if scratch.extent() != extent {
        return Err(MaskLabError::ScratchExtentMismatch);
    }
    for (source, destination) in mask.rows().zip(output.rows_mut()) {
        for (&value, result) in source.iter().zip(destination) {
            *result = if value == 0 { 0 } else { u8::MAX };
        }
    }

    for _ in 0..config.max_iterations() {
        let first = thinning_phase(&mut output, &mut scratch, true);
        let second = thinning_phase(&mut output, &mut scratch, false);
        if !first && !second {
            return Ok(());
        }
    }
    Err(MaskLabError::IterationLimitReached)
}

/// Produces a floating mask with distance-based inner and outer feathering.
///
/// The exhaustive opposite-class search is allocation-free and `O(pixels²)`.
/// Non-zero input values are foreground; output is clamped to the closed range
/// `[0, 1]` for finite radii.
pub fn feather(
    mask: Plane<'_, u8>,
    mut output: PlaneMut<'_, f32>,
    config: FeatherConfig,
) -> Result<(), MaskLabError> {
    let extent = matching_extent(mask.extent(), output.extent())?;
    for y in 0..extent.height() {
        let source_row = mask.row(y).expect("validated row");
        for (x, &source) in source_row.iter().enumerate() {
            let is_foreground = source != 0;
            let radius = if is_foreground {
                config.inner_radius()
            } else {
                config.outer_radius()
            };
            let value = if radius == 0.0 {
                f32::from(is_foreground)
            } else {
                let distance = nearest_opposite_distance(mask, x, y, is_foreground);
                if is_foreground {
                    (distance / radius).min(1.0)
                } else {
                    (1.0 - distance / radius).max(0.0)
                }
            };
            output.row_mut(y).expect("validated row")[x] = value;
        }
    }
    Ok(())
}

fn matching_extent(left: Extent, right: Extent) -> Result<Extent, MaskLabError> {
    if left != right {
        return Err(MaskLabError::ExtentMismatch);
    }
    Ok(left)
}

fn propagate_component_labels(
    labels: &mut [u32],
    extent: Extent,
    connectivity: Connectivity,
) -> bool {
    let mut changed = false;
    for y in 0..extent.height() {
        for x in 0..extent.width() {
            let index = y * extent.width() + x;
            let label = labels[index];
            if label == 0 {
                continue;
            }
            let minimum = neighbouring_minimum(labels, extent, x, y, connectivity, label);
            if minimum < label {
                labels[index] = minimum;
                changed = true;
            }
        }
    }
    changed
}

fn neighbouring_minimum(
    labels: &[u32],
    extent: Extent,
    x: usize,
    y: usize,
    connectivity: Connectivity,
    initial: u32,
) -> u32 {
    let width = extent.width();
    let mut minimum = initial;
    let mut consider = |nx: usize, ny: usize| {
        let candidate = labels[ny * width + nx];
        if candidate != 0 {
            minimum = minimum.min(candidate);
        }
    };
    if x > 0 {
        consider(x - 1, y);
    }
    if x + 1 < width {
        consider(x + 1, y);
    }
    if y > 0 {
        consider(x, y - 1);
    }
    if y + 1 < extent.height() {
        consider(x, y + 1);
    }
    if connectivity == Connectivity::Eight {
        if x > 0 && y > 0 {
            consider(x - 1, y - 1);
        }
        if x + 1 < width && y > 0 {
            consider(x + 1, y - 1);
        }
        if x > 0 && y + 1 < extent.height() {
            consider(x - 1, y + 1);
        }
        if x + 1 < width && y + 1 < extent.height() {
            consider(x + 1, y + 1);
        }
    }
    minimum
}

fn reconstruction_pass(
    output: &mut PlaneMut<'_, u8>,
    mask: Plane<'_, u8>,
    connectivity: Connectivity,
    forward: bool,
) -> bool {
    let extent = output.extent();
    let mut changed = false;
    for offset_y in 0..extent.height() {
        let y = if forward {
            offset_y
        } else {
            extent.height() - 1 - offset_y
        };
        for offset_x in 0..extent.width() {
            let x = if forward {
                offset_x
            } else {
                extent.width() - 1 - offset_x
            };
            let current = output_value(output, x, y);
            let neighbour_maximum =
                reconstruction_neighbour_maximum(output, extent, x, y, connectivity, forward);
            let next = current
                .max(neighbour_maximum)
                .min(mask.row(y).expect("validated row")[x]);
            if next != current {
                output.row_mut(y).expect("validated row")[x] = next;
                changed = true;
            }
        }
    }
    changed
}

fn reconstruction_neighbour_maximum(
    output: &mut PlaneMut<'_, u8>,
    extent: Extent,
    x: usize,
    y: usize,
    connectivity: Connectivity,
    forward: bool,
) -> u8 {
    let mut maximum = 0;
    let mut consider = |nx: usize, ny: usize| maximum = maximum.max(output_value(output, nx, ny));
    if forward {
        if x > 0 {
            consider(x - 1, y);
        }
        if y > 0 {
            consider(x, y - 1);
        }
        if connectivity == Connectivity::Eight && x > 0 && y > 0 {
            consider(x - 1, y - 1);
        }
        if connectivity == Connectivity::Eight && x + 1 < extent.width() && y > 0 {
            consider(x + 1, y - 1);
        }
    } else {
        if x + 1 < extent.width() {
            consider(x + 1, y);
        }
        if y + 1 < extent.height() {
            consider(x, y + 1);
        }
        if connectivity == Connectivity::Eight && x > 0 && y + 1 < extent.height() {
            consider(x - 1, y + 1);
        }
        if connectivity == Connectivity::Eight && x + 1 < extent.width() && y + 1 < extent.height()
        {
            consider(x + 1, y + 1);
        }
    }
    maximum
}

fn output_value(output: &mut PlaneMut<'_, u8>, x: usize, y: usize) -> u8 {
    output.row_mut(y).expect("validated row")[x]
}

fn thinning_phase(
    output: &mut PlaneMut<'_, u8>,
    scratch: &mut PlaneMut<'_, u8>,
    first: bool,
) -> bool {
    let extent = output.extent();
    for y in 0..extent.height() {
        for x in 0..extent.width() {
            let delete = should_delete(output, extent, x, y, first);
            scratch.row_mut(y).expect("validated row")[x] = u8::from(delete);
        }
    }
    let mut changed = false;
    for y in 0..extent.height() {
        for x in 0..extent.width() {
            if scratch.row_mut(y).expect("validated row")[x] != 0 {
                output.row_mut(y).expect("validated row")[x] = 0;
                changed = true;
            }
        }
    }
    changed
}

fn should_delete(
    output: &mut PlaneMut<'_, u8>,
    extent: Extent,
    x: usize,
    y: usize,
    first: bool,
) -> bool {
    if output_value(output, x, y) == 0 {
        return false;
    }
    let neighbours = [
        foreground_at(output, extent, x as isize, y as isize - 1),
        foreground_at(output, extent, x as isize + 1, y as isize - 1),
        foreground_at(output, extent, x as isize + 1, y as isize),
        foreground_at(output, extent, x as isize + 1, y as isize + 1),
        foreground_at(output, extent, x as isize, y as isize + 1),
        foreground_at(output, extent, x as isize - 1, y as isize + 1),
        foreground_at(output, extent, x as isize - 1, y as isize),
        foreground_at(output, extent, x as isize - 1, y as isize - 1),
    ];
    let neighbour_count = neighbours.iter().filter(|&&value| value).count();
    if !(2..=6).contains(&neighbour_count) || transitions(&neighbours) != 1 {
        return false;
    }
    let north = neighbours[0];
    let east = neighbours[2];
    let south = neighbours[4];
    let west = neighbours[6];
    if first {
        !((north && east && south) || (east && south && west))
    } else {
        !((north && east && west) || (north && south && west))
    }
}

fn foreground_at(output: &mut PlaneMut<'_, u8>, extent: Extent, x: isize, y: isize) -> bool {
    let (Ok(x), Ok(y)) = (usize::try_from(x), usize::try_from(y)) else {
        return false;
    };
    if x >= extent.width() || y >= extent.height() {
        return false;
    }
    output_value(output, x, y) != 0
}

fn transitions(neighbours: &[bool; 8]) -> usize {
    neighbours
        .iter()
        .zip(neighbours.iter().cycle().skip(1))
        .take(8)
        .filter(|(before, after)| !**before && **after)
        .count()
}

fn nearest_opposite_distance(mask: Plane<'_, u8>, x: usize, y: usize, foreground: bool) -> f32 {
    let extent = mask.extent();
    let mut nearest_squared = f64::INFINITY;
    for sample_y in 0..extent.height() {
        for (sample_x, &value) in mask
            .row(sample_y)
            .expect("validated row")
            .iter()
            .enumerate()
        {
            if (value != 0) == foreground {
                continue;
            }
            let dx = x.abs_diff(sample_x) as f64;
            let dy = y.abs_diff(sample_y) as f64;
            nearest_squared = nearest_squared.min(dx.mul_add(dx, dy * dy));
        }
    }
    nearest_squared.sqrt() as f32
}

#[cfg(test)]
mod tests {
    use super::{
        Component, Connectivity, FeatherConfig, FeatherConfigError, MaskLabError, ThinConfig,
        ThinConfigError, components, euclidean_to_zero, feather, reconstruct, thin,
    };
    use vsip_core::{Extent, Plane, PlaneMut};

    #[test]
    fn euclidean_distance_respects_odd_padded_geometry() {
        let extent = Extent::new(3, 1).expect("valid extent");
        let mask = [0, 1, 1, 77];
        let mut output = [-1.0; 4];
        euclidean_to_zero(
            Plane::new(&mask, extent, 4).expect("valid input"),
            PlaneMut::new(&mut output, extent, 4).expect("valid output"),
        )
        .expect("distance succeeds");
        assert_eq!(&output[..3], &[0.0, 1.0, 2.0]);
        assert_eq!(output[3], -1.0);
    }

    #[test]
    fn components_distinguish_four_and_eight_connectivity() {
        let extent = Extent::new(2, 2).expect("valid extent");
        let mask = [1, 0, 0, 1];
        let four = components(
            Plane::new(&mask, extent, 2).expect("plane"),
            Connectivity::Four,
        )
        .expect("components");
        let eight = components(
            Plane::new(&mask, extent, 2).expect("plane"),
            Connectivity::Eight,
        )
        .expect("components");
        assert_eq!(four.components.len(), 2);
        assert_eq!(
            eight.components,
            vec![Component {
                label: 1,
                area: 2,
                bounds: super::Bounds {
                    left: 0,
                    top: 0,
                    right: 1,
                    bottom: 1
                },
            }]
        );
    }

    #[test]
    fn reconstruct_rejects_an_invalid_marker_before_writing() {
        let extent = Extent::new(2, 1).expect("valid extent");
        let marker = [1, 2];
        let mask = [1, 1];
        let mut output = [99, 99];
        assert_eq!(
            reconstruct(
                Plane::new(&marker, extent, 2).expect("plane"),
                Plane::new(&mask, extent, 2).expect("plane"),
                PlaneMut::new(&mut output, extent, 2).expect("plane"),
                Connectivity::Four,
            ),
            Err(MaskLabError::MarkerExceedsMask)
        );
        assert_eq!(output, [99, 99]);
    }

    #[test]
    fn reconstruction_fills_a_mask_connected_to_the_marker() {
        let extent = Extent::new(3, 1).expect("valid extent");
        let marker = [4, 0, 0];
        let mask = [4, 3, 2];
        let mut output = [0; 3];
        reconstruct(
            Plane::new(&marker, extent, 3).expect("plane"),
            Plane::new(&mask, extent, 3).expect("plane"),
            PlaneMut::new(&mut output, extent, 3).expect("plane"),
            Connectivity::Four,
        )
        .expect("reconstruction");
        assert_eq!(output, [4, 3, 2]);
    }

    #[test]
    fn thinning_uses_caller_owned_padded_scratch() {
        let extent = Extent::new(3, 3).expect("valid extent");
        let mask = [0, 0, 0, 9, 0, 9, 9, 9, 9, 9, 0, 0];
        let mut output = [17; 12];
        let mut scratch = [23; 12];
        thin(
            Plane::new(&mask, extent, 4).expect("plane"),
            PlaneMut::new(&mut output, extent, 4).expect("plane"),
            PlaneMut::new(&mut scratch, extent, 4).expect("plane"),
            ThinConfig::new(8).expect("config"),
        )
        .expect("thin");
        assert_eq!(output[3], 17);
        assert_eq!(output[7], 17);
        assert_eq!(output[11], 17);
        assert_eq!(output[5], u8::MAX);
    }

    #[test]
    fn thinning_treats_pixels_beyond_the_visible_extent_as_background() {
        let extent = Extent::new(3, 3).expect("valid extent");
        let mask = [u8::MAX; 9];
        let mut output = [0; 9];
        let mut scratch = [0; 9];
        thin(
            Plane::new(&mask, extent, 3).expect("plane"),
            PlaneMut::new(&mut output, extent, 3).expect("plane"),
            PlaneMut::new(&mut scratch, extent, 3).expect("plane"),
            ThinConfig::new(8).expect("config"),
        )
        .expect("thin");
        assert!(output.iter().filter(|&&value| value != 0).count() < mask.len());
        assert_eq!(output[4], u8::MAX);
    }

    #[test]
    fn feather_has_hard_edge_and_rejects_invalid_radius() {
        let extent = Extent::new(3, 1).expect("valid extent");
        let mask = [0, 1, 0];
        let mut output = [0.0; 3];
        feather(
            Plane::new(&mask, extent, 3).expect("plane"),
            PlaneMut::new(&mut output, extent, 3).expect("plane"),
            FeatherConfig::new(0.0, 0.0).expect("config"),
        )
        .expect("feather");
        assert_eq!(output, [0.0, 1.0, 0.0]);
        assert_eq!(
            FeatherConfig::new(f32::NAN, 1.0),
            Err(FeatherConfigError::InvalidRadius)
        );
        assert_eq!(ThinConfig::new(0), Err(ThinConfigError::ZeroIterationLimit));
    }

    #[test]
    fn reports_extent_mismatch() {
        let source_extent = Extent::new(1, 1).expect("valid extent");
        let output_extent = Extent::new(2, 1).expect("valid extent");
        let mut output = [0.0; 2];
        assert_eq!(
            euclidean_to_zero(
                Plane::new(&[0], source_extent, 1).expect("plane"),
                PlaneMut::new(&mut output, output_extent, 2).expect("plane"),
            ),
            Err(MaskLabError::ExtentMismatch)
        );
    }
}

//! Scalar reference filters for labelled image segmentation.
//!
//! Frame operations use validated caller-owned planes. [`region_graph`] is the
//! only analysis operation and owns the nodes and edges that it returns.

use core::{cmp::Ordering, fmt};
use vsip_core::{Extent, Plane, PlaneMut};
use vsip_plugin_api::{Execution, Filter, Maturity, Plugin};

/// Public plugin catalogue.
pub const PLUGIN: Plugin = Plugin {
    package: "vs-segment",
    namespace: "segment",
    summary: "Watershed, superpixels and region graphs",
    filters: &[
        Filter {
            name: "Watershed",
            summary: "Segment from image gradients and markers",
            maturity: Maturity::Experimental,
            execution: Execution::MultiInput,
        },
        Filter {
            name: "Superpixels",
            summary: "Generate compact perceptual regions",
            maturity: Maturity::Experimental,
            execution: Execution::Spatial,
        },
        Filter {
            name: "RegionDegreeMap",
            summary: "Render each labelled region's graph degree",
            maturity: Maturity::Experimental,
            execution: Execution::Analysis,
        },
        Filter {
            name: "Merge",
            summary: "Merge regions under a typed criterion",
            maturity: Maturity::Experimental,
            execution: Execution::MultiInput,
        },
    ],
};

/// Neighbourhood used by segmentation operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Connectivity {
    /// Pixels share an edge.
    Four,
    /// Pixels share an edge or a corner.
    Eight,
}

/// Treatment for pixels reached by two watershed basins.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatershedBoundary {
    /// Emit zero for watershed boundaries.
    Line,
    /// Assign the smallest adjacent marker label at a boundary.
    LowestAdjacentLabel,
}

/// Settings for [`watershed`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WatershedConfig {
    connectivity: Connectivity,
    boundary: WatershedBoundary,
}

impl WatershedConfig {
    /// Creates deterministic marker-watershed settings.
    pub const fn new(connectivity: Connectivity, boundary: WatershedBoundary) -> Self {
        Self {
            connectivity,
            boundary,
        }
    }

    /// Returns the flood neighbourhood.
    pub const fn connectivity(self) -> Connectivity {
        self.connectivity
    }

    /// Returns the boundary policy.
    pub const fn boundary(self) -> WatershedBoundary {
        self.boundary
    }
}

/// Invalid construction input for [`SuperpixelConfig`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SuperpixelConfigError {
    /// A grid-cell dimension was zero.
    ZeroCellDimension,
    /// Compactness was negative or non-finite.
    InvalidCompactness,
}

impl fmt::Display for SuperpixelConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ZeroCellDimension => "superpixel cell dimensions must be non-zero",
            Self::InvalidCompactness => "superpixel compactness must be finite and non-negative",
        })
    }
}

impl std::error::Error for SuperpixelConfigError {}

/// Fixed-seed settings for [`superpixels`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SuperpixelConfig {
    cell_width: usize,
    cell_height: usize,
    compactness: f32,
}

impl SuperpixelConfig {
    /// Creates regular-grid seed settings.
    pub fn new(
        cell_width: usize,
        cell_height: usize,
        compactness: f32,
    ) -> Result<Self, SuperpixelConfigError> {
        if cell_width == 0 || cell_height == 0 {
            return Err(SuperpixelConfigError::ZeroCellDimension);
        }
        if !compactness.is_finite() || compactness < 0.0 {
            return Err(SuperpixelConfigError::InvalidCompactness);
        }
        Ok(Self {
            cell_width,
            cell_height,
            compactness,
        })
    }

    /// Returns the horizontal seed spacing.
    pub const fn cell_width(self) -> usize {
        self.cell_width
    }

    /// Returns the vertical seed spacing.
    pub const fn cell_height(self) -> usize {
        self.cell_height
    }

    /// Returns the relative spatial penalty.
    pub const fn compactness(self) -> f32 {
        self.compactness
    }
}

/// Comparison used by [`merge`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergeCriterion {
    /// Join neighbouring labels where the signal jump is within a threshold.
    BoundaryDifference,
}

/// Invalid construction input for [`MergeConfig`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergeConfigError {
    /// The boundary threshold was negative or non-finite.
    InvalidThreshold,
}

impl fmt::Display for MergeConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("merge threshold must be finite and non-negative")
    }
}

impl std::error::Error for MergeConfigError {}

/// Settings for [`merge`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MergeConfig {
    criterion: MergeCriterion,
    threshold: f32,
    connectivity: Connectivity,
}

impl MergeConfig {
    /// Creates validated boundary-difference merge settings.
    pub fn new(
        criterion: MergeCriterion,
        threshold: f32,
        connectivity: Connectivity,
    ) -> Result<Self, MergeConfigError> {
        if !threshold.is_finite() || threshold < 0.0 {
            return Err(MergeConfigError::InvalidThreshold);
        }
        Ok(Self {
            criterion,
            threshold,
            connectivity,
        })
    }

    /// Returns the selected merge criterion.
    pub const fn criterion(self) -> MergeCriterion {
        self.criterion
    }

    /// Returns the maximum accepted boundary difference.
    pub const fn threshold(self) -> f32 {
        self.threshold
    }

    /// Returns the adjacency used to identify region boundaries.
    pub const fn connectivity(self) -> Connectivity {
        self.connectivity
    }
}

/// Failure returned by Segment reference operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SegmentError {
    /// Inputs, output, or caller workspace have different visible extents.
    ExtentMismatch,
    /// Watershed requires at least one non-zero marker.
    NoMarkers,
    /// Marker input used the internally reserved label `u32::MAX`.
    ReservedMarkerLabel,
    /// A gradient or signal value was non-finite.
    NonFiniteSample,
    /// The grid would require labels that do not fit in `u32`.
    LabelOverflow,
    /// Watershed lines isolated unlabelled pixels from every basin.
    UnreachablePixels,
    /// An owned analysis report could not reserve its required storage.
    AllocationFailed,
    /// A graph boundary count overflowed `usize`.
    CountOverflow,
}

impl fmt::Display for SegmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ExtentMismatch => "input, output, and workspace extents must match",
            Self::NoMarkers => "watershed requires at least one non-zero marker",
            Self::ReservedMarkerLabel => "u32::MAX is reserved for internal watershed lines",
            Self::NonFiniteSample => "segmentation requires finite gradient and signal samples",
            Self::LabelOverflow => "segmentation labels do not fit in u32",
            Self::UnreachablePixels => "watershed boundaries isolated unlabelled pixels",
            Self::AllocationFailed => "region graph output allocation failed",
            Self::CountOverflow => "region graph boundary count overflowed",
        })
    }
}

impl std::error::Error for SegmentError {}

/// Inclusive bounding rectangle for a labelled region.
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

/// One labelled region in a [`RegionGraph`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegionNode {
    /// Non-zero label identifying the region.
    pub label: u32,
    /// Visible pixel count.
    pub area: usize,
    /// Inclusive bounding rectangle.
    pub bounds: Bounds,
}

/// An undirected adjacency between two non-zero labels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegionEdge {
    /// Lower endpoint label.
    pub left: u32,
    /// Higher endpoint label.
    pub right: u32,
    /// Number of adjacent pixel pairs on the shared boundary.
    pub boundary: usize,
}

/// Owned adjacency analysis for a labelled plane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegionGraph {
    /// Extent analysed by the graph builder.
    pub extent: Extent,
    /// Nodes ordered by label.
    pub nodes: Vec<RegionNode>,
    /// Deduplicated edges ordered by `(left, right)`.
    pub edges: Vec<RegionEdge>,
}

/// Segments a gradient image by deterministic marker-controlled flooding.
///
/// `workspace` is caller-owned and may be reused. Each iteration chooses the
/// globally lowest-gradient currently reachable pixel, with raster order as a
/// tie-breaker. This direct priority-queue-free reference requires `O(pixels²)`
/// work, allocates no memory, and leaves `output` untouched when validation
/// fails. The workspace contents are unspecified after an error.
pub fn watershed(
    gradient: Plane<'_, f32>,
    markers: Plane<'_, u32>,
    mut output: PlaneMut<'_, u32>,
    mut workspace: PlaneMut<'_, u32>,
    config: WatershedConfig,
) -> Result<(), SegmentError> {
    let extent = same_extent(gradient.extent(), markers.extent())?;
    same_extent(extent, output.extent())?;
    same_extent(extent, workspace.extent())?;
    require_finite(gradient)?;
    let mut has_marker = false;
    for row in markers.rows() {
        for &marker in row {
            if marker == u32::MAX {
                return Err(SegmentError::ReservedMarkerLabel);
            }
            has_marker |= marker != 0;
        }
    }
    if !has_marker {
        return Err(SegmentError::NoMarkers);
    }
    for y in 0..extent.height() {
        workspace
            .row_mut(y)
            .expect("validated row")
            .copy_from_slice(markers.row(y).expect("validated row"));
    }
    let area = extent.area().ok_or(SegmentError::LabelOverflow)?;
    for _ in 0..area {
        let Some(candidate) =
            next_watershed_candidate(&mut workspace, gradient, extent, config.connectivity())
        else {
            if has_unassigned(&mut workspace, extent) {
                return Err(SegmentError::UnreachablePixels);
            }
            break;
        };
        let label = if candidate.conflict && config.boundary() == WatershedBoundary::Line {
            u32::MAX
        } else {
            candidate.label
        };
        workspace.row_mut(candidate.y).expect("validated row")[candidate.x] = label;
    }
    if has_unassigned(&mut workspace, extent) {
        return Err(SegmentError::UnreachablePixels);
    }
    for y in 0..extent.height() {
        let destination = output.row_mut(y).expect("validated row");
        let source = workspace.row_mut(y).expect("validated row");
        for x in 0..extent.width() {
            destination[x] = if source[x] == u32::MAX { 0 } else { source[x] };
        }
    }
    Ok(())
}

/// Generates compact fixed-seed perceptual regions from a luminance plane.
///
/// Each pixel evaluates seed cells in a 3×3 neighbourhood. Its score combines
/// squared luminance difference with a squared, normalized spatial distance.
/// There are no centroid updates, so this is a deterministic seed-assignment
/// reference rather than a full iterative SLIC implementation. It performs at
/// most nine seed comparisons per pixel and allocates no memory.
pub fn superpixels(
    luminance: Plane<'_, f32>,
    mut output: PlaneMut<'_, u32>,
    config: SuperpixelConfig,
) -> Result<(), SegmentError> {
    let extent = same_extent(luminance.extent(), output.extent())?;
    require_finite(luminance)?;
    let cells_x = cell_count(extent.width(), config.cell_width());
    let cells_y = cell_count(extent.height(), config.cell_height());
    let label_count = cells_x
        .checked_mul(cells_y)
        .ok_or(SegmentError::LabelOverflow)?;
    u32::try_from(label_count).map_err(|_| SegmentError::LabelOverflow)?;
    for y in 0..extent.height() {
        for x in 0..extent.width() {
            let cell_x = x / config.cell_width();
            let cell_y = y / config.cell_height();
            let mut best_score = f64::INFINITY;
            let mut best_label = 0_u32;
            let first_x = cell_x.saturating_sub(1);
            let last_x = cell_x.saturating_add(1).min(cells_x - 1);
            let first_y = cell_y.saturating_sub(1);
            let last_y = cell_y.saturating_add(1).min(cells_y - 1);
            for seed_cell_y in first_y..=last_y {
                for seed_cell_x in first_x..=last_x {
                    let seed_x = seed_coordinate(seed_cell_x, config.cell_width(), extent.width());
                    let seed_y =
                        seed_coordinate(seed_cell_y, config.cell_height(), extent.height());
                    let intensity = f64::from(luminance.row(y).expect("validated row")[x]);
                    let seed_intensity =
                        f64::from(luminance.row(seed_y).expect("validated row")[seed_x]);
                    let dx = x.abs_diff(seed_x) as f64 / config.cell_width() as f64;
                    let dy = y.abs_diff(seed_y) as f64 / config.cell_height() as f64;
                    let spatial = f64::from(config.compactness()) * f64::from(config.compactness());
                    let score = (intensity - seed_intensity).mul_add(
                        intensity - seed_intensity,
                        spatial * (dx.mul_add(dx, dy * dy)),
                    );
                    let label = u32::try_from(seed_cell_y * cells_x + seed_cell_x + 1)
                        .map_err(|_| SegmentError::LabelOverflow)?;
                    if score.total_cmp(&best_score) == Ordering::Less
                        || (score.total_cmp(&best_score) == Ordering::Equal && label < best_label)
                    {
                        best_score = score;
                        best_label = label;
                    }
                }
            }
            output.row_mut(y).expect("validated row")[x] = best_label;
        }
    }
    Ok(())
}

/// Builds an adjacency graph from a labelled plane.
///
/// Label zero is background and is not represented in nodes or edges. The
/// returned graph is deterministic. This direct reference keeps nodes sorted
/// with vector insertion, then sorts boundary pairs; its two output vectors are
/// the only allocations.
pub fn region_graph(
    labels: Plane<'_, u32>,
    connectivity: Connectivity,
) -> Result<RegionGraph, SegmentError> {
    let extent = labels.extent();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for y in 0..extent.height() {
        for x in 0..extent.width() {
            let label = labels.row(y).expect("validated row")[x];
            if label != 0 {
                update_node(&mut nodes, label, x, y)?;
            }
            append_graph_edges(&mut edges, labels, extent, x, y, connectivity)?;
        }
    }
    edges.sort_unstable_by_key(|edge| (edge.left, edge.right));
    let mut write = 0;
    for read in 0..edges.len() {
        if write > 0
            && edges[write - 1].left == edges[read].left
            && edges[write - 1].right == edges[read].right
        {
            edges[write - 1].boundary = edges[write - 1]
                .boundary
                .checked_add(edges[read].boundary)
                .ok_or(SegmentError::CountOverflow)?;
            continue;
        }
        if write != read {
            edges[write] = edges[read];
        }
        write += 1;
    }
    edges.truncate(write);
    Ok(RegionGraph {
        extent,
        nodes,
        edges,
    })
}

/// Merges adjacent labels whose local boundary difference meets a typed criterion.
///
/// Equal input labels always remain connected. Eligible component labels are
/// propagated to their smallest original label by repeated scans, so output
/// labels need not be compact. The direct reference takes `O(pixels × passes)`
/// work, allocates no memory, and preserves a zero background.
pub fn merge(
    labels: Plane<'_, u32>,
    signal: Plane<'_, f32>,
    mut output: PlaneMut<'_, u32>,
    config: MergeConfig,
) -> Result<(), SegmentError> {
    let extent = same_extent(labels.extent(), signal.extent())?;
    same_extent(extent, output.extent())?;
    require_finite(signal)?;
    for y in 0..extent.height() {
        output
            .row_mut(y)
            .expect("validated row")
            .copy_from_slice(labels.row(y).expect("validated row"));
    }
    while merge_pass(labels, signal, &mut output, extent, config) {}
    Ok(())
}

fn same_extent(left: Extent, right: Extent) -> Result<Extent, SegmentError> {
    if left != right {
        return Err(SegmentError::ExtentMismatch);
    }
    Ok(left)
}

fn require_finite(image: Plane<'_, f32>) -> Result<(), SegmentError> {
    if image.rows().flatten().any(|value| !value.is_finite()) {
        return Err(SegmentError::NonFiniteSample);
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct WatershedCandidate {
    x: usize,
    y: usize,
    gradient: f32,
    label: u32,
    conflict: bool,
}

fn next_watershed_candidate(
    workspace: &mut PlaneMut<'_, u32>,
    gradient: Plane<'_, f32>,
    extent: Extent,
    connectivity: Connectivity,
) -> Option<WatershedCandidate> {
    let mut best = None;
    for y in 0..extent.height() {
        for x in 0..extent.width() {
            if plane_value(workspace, x, y) != 0 {
                continue;
            }
            let Some((label, conflict)) = neighbouring_basin(workspace, extent, x, y, connectivity)
            else {
                continue;
            };
            let candidate = WatershedCandidate {
                x,
                y,
                gradient: gradient.row(y).expect("validated row")[x],
                label,
                conflict,
            };
            if best.is_none_or(|current: WatershedCandidate| watershed_before(candidate, current)) {
                best = Some(candidate);
            }
        }
    }
    best
}

fn watershed_before(left: WatershedCandidate, right: WatershedCandidate) -> bool {
    match left.gradient.total_cmp(&right.gradient) {
        Ordering::Less => true,
        Ordering::Greater => false,
        Ordering::Equal => (left.y, left.x) < (right.y, right.x),
    }
}

fn neighbouring_basin(
    workspace: &mut PlaneMut<'_, u32>,
    extent: Extent,
    x: usize,
    y: usize,
    connectivity: Connectivity,
) -> Option<(u32, bool)> {
    let mut first = None;
    let mut conflict = false;
    visit_neighbours(extent, x, y, connectivity, |nx, ny| {
        let value = plane_value(workspace, nx, ny);
        if value == 0 || value == u32::MAX {
            return;
        }
        match first {
            None => first = Some(value),
            Some(existing) if existing != value => {
                conflict = true;
                first = Some(existing.min(value));
            }
            Some(_) => {}
        }
    });
    first.map(|label| (label, conflict))
}

fn has_unassigned(workspace: &mut PlaneMut<'_, u32>, extent: Extent) -> bool {
    for y in 0..extent.height() {
        if workspace.row_mut(y).expect("validated row").contains(&0) {
            return true;
        }
    }
    false
}

fn plane_value(plane: &mut PlaneMut<'_, u32>, x: usize, y: usize) -> u32 {
    plane.row_mut(y).expect("validated row")[x]
}

fn cell_count(length: usize, cell_length: usize) -> usize {
    (length - 1) / cell_length + 1
}

fn seed_coordinate(cell: usize, cell_length: usize, length: usize) -> usize {
    cell.saturating_mul(cell_length)
        .saturating_add(cell_length / 2)
        .min(length - 1)
}

fn update_node(
    nodes: &mut Vec<RegionNode>,
    label: u32,
    x: usize,
    y: usize,
) -> Result<(), SegmentError> {
    let index = match nodes.binary_search_by_key(&label, |node| node.label) {
        Ok(index) => index,
        Err(index) => {
            nodes
                .try_reserve(1)
                .map_err(|_| SegmentError::AllocationFailed)?;
            nodes.insert(
                index,
                RegionNode {
                    label,
                    area: 0,
                    bounds: Bounds {
                        left: x,
                        top: y,
                        right: x,
                        bottom: y,
                    },
                },
            );
            index
        }
    };
    let node = &mut nodes[index];
    node.area += 1;
    node.bounds.left = node.bounds.left.min(x);
    node.bounds.top = node.bounds.top.min(y);
    node.bounds.right = node.bounds.right.max(x);
    node.bounds.bottom = node.bounds.bottom.max(y);
    Ok(())
}

fn append_graph_edges(
    edges: &mut Vec<RegionEdge>,
    labels: Plane<'_, u32>,
    extent: Extent,
    x: usize,
    y: usize,
    connectivity: Connectivity,
) -> Result<(), SegmentError> {
    let source = labels.row(y).expect("validated row")[x];
    if x + 1 < extent.width() {
        append_edge(edges, source, labels.row(y).expect("validated row")[x + 1])?;
    }
    if y + 1 < extent.height() {
        append_edge(edges, source, labels.row(y + 1).expect("validated row")[x])?;
    }
    if connectivity == Connectivity::Eight && y + 1 < extent.height() {
        if x > 0 {
            append_edge(
                edges,
                source,
                labels.row(y + 1).expect("validated row")[x - 1],
            )?;
        }
        if x + 1 < extent.width() {
            append_edge(
                edges,
                source,
                labels.row(y + 1).expect("validated row")[x + 1],
            )?;
        }
    }
    Ok(())
}

fn append_edge(edges: &mut Vec<RegionEdge>, first: u32, second: u32) -> Result<(), SegmentError> {
    if first == 0 || second == 0 || first == second {
        return Ok(());
    }
    edges
        .try_reserve(1)
        .map_err(|_| SegmentError::AllocationFailed)?;
    edges.push(RegionEdge {
        left: first.min(second),
        right: first.max(second),
        boundary: 1,
    });
    Ok(())
}

fn merge_pass(
    labels: Plane<'_, u32>,
    signal: Plane<'_, f32>,
    output: &mut PlaneMut<'_, u32>,
    extent: Extent,
    config: MergeConfig,
) -> bool {
    let mut changed = false;
    for y in 0..extent.height() {
        for x in 0..extent.width() {
            let original = labels.row(y).expect("validated row")[x];
            if original == 0 {
                continue;
            }
            let mut lowest = plane_value(output, x, y);
            visit_neighbours(extent, x, y, config.connectivity(), |nx, ny| {
                let neighbour_original = labels.row(ny).expect("validated row")[nx];
                if neighbour_original == 0 {
                    return;
                }
                let eligible = match config.criterion() {
                    MergeCriterion::BoundaryDifference => {
                        original == neighbour_original
                            || (signal.row(y).expect("validated row")[x]
                                - signal.row(ny).expect("validated row")[nx])
                                .abs()
                                <= config.threshold()
                    }
                };
                if eligible {
                    lowest = lowest.min(plane_value(output, nx, ny));
                }
            });
            if lowest < plane_value(output, x, y) {
                output.row_mut(y).expect("validated row")[x] = lowest;
                changed = true;
            }
        }
    }
    changed
}

fn visit_neighbours(
    extent: Extent,
    x: usize,
    y: usize,
    connectivity: Connectivity,
    mut visit: impl FnMut(usize, usize),
) {
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
    if connectivity == Connectivity::Eight {
        if x > 0 && y > 0 {
            visit(x - 1, y - 1);
        }
        if x + 1 < extent.width() && y > 0 {
            visit(x + 1, y - 1);
        }
        if x > 0 && y + 1 < extent.height() {
            visit(x - 1, y + 1);
        }
        if x + 1 < extent.width() && y + 1 < extent.height() {
            visit(x + 1, y + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Bounds, Connectivity, MergeConfig, MergeConfigError, MergeCriterion, RegionEdge,
        SegmentError, SuperpixelConfig, SuperpixelConfigError, WatershedBoundary, WatershedConfig,
        merge, region_graph, superpixels, watershed,
    };
    use vsip_core::{Extent, Plane, PlaneMut};

    #[test]
    fn watershed_emits_a_line_between_equal_gradient_markers() {
        let extent = Extent::new(3, 1).expect("extent");
        let gradient = [0.0, 0.0, 0.0];
        let markers = [1, 0, 2];
        let mut output = [9; 3];
        let mut workspace = [9; 3];
        watershed(
            Plane::new(&gradient, extent, 3).expect("plane"),
            Plane::new(&markers, extent, 3).expect("plane"),
            PlaneMut::new(&mut output, extent, 3).expect("plane"),
            PlaneMut::new(&mut workspace, extent, 3).expect("plane"),
            WatershedConfig::new(Connectivity::Four, WatershedBoundary::Line),
        )
        .expect("watershed");
        assert_eq!(output, [1, 0, 2]);
    }

    #[test]
    fn watershed_rejects_invalid_inputs_before_output_write() {
        let extent = Extent::new(1, 1).expect("extent");
        let mut output = [7];
        let mut workspace = [0];
        assert_eq!(
            watershed(
                Plane::new(&[f32::NAN], extent, 1).expect("plane"),
                Plane::new(&[1], extent, 1).expect("plane"),
                PlaneMut::new(&mut output, extent, 1).expect("plane"),
                PlaneMut::new(&mut workspace, extent, 1).expect("plane"),
                WatershedConfig::new(Connectivity::Four, WatershedBoundary::Line),
            ),
            Err(SegmentError::NonFiniteSample)
        );
        assert_eq!(output, [7]);
    }

    #[test]
    fn superpixels_use_luminance_and_preserve_padded_output() {
        let extent = Extent::new(3, 1).expect("extent");
        let source = [0.0, 5.0, 0.0, 99.0];
        let mut output = [0; 4];
        superpixels(
            Plane::new(&source, extent, 4).expect("plane"),
            PlaneMut::new(&mut output, extent, 4).expect("plane"),
            SuperpixelConfig::new(2, 1, 1.0).expect("config"),
        )
        .expect("superpixels");
        assert_eq!(&output[..3], &[2, 1, 2]);
        assert_eq!(output[3], 0);
    }

    #[test]
    fn graph_deduplicates_adjacency_and_reports_bounds() {
        let extent = Extent::new(2, 2).expect("extent");
        let labels = [1, 2, 1, 3];
        let graph = region_graph(
            Plane::new(&labels, extent, 2).expect("plane"),
            Connectivity::Four,
        )
        .expect("graph");
        assert_eq!(
            graph.nodes[0].bounds,
            Bounds {
                left: 0,
                top: 0,
                right: 0,
                bottom: 1
            }
        );
        assert_eq!(
            graph.edges,
            vec![
                RegionEdge {
                    left: 1,
                    right: 2,
                    boundary: 1
                },
                RegionEdge {
                    left: 1,
                    right: 3,
                    boundary: 1
                },
                RegionEdge {
                    left: 2,
                    right: 3,
                    boundary: 1
                },
            ]
        );
    }

    #[test]
    fn merge_propagates_the_lowest_eligible_label() {
        let extent = Extent::new(3, 1).expect("extent");
        let labels = [3, 7, 9, 99];
        let signal = [1.0, 1.1, 3.0, 99.0];
        let mut output = [0; 4];
        merge(
            Plane::new(&labels, extent, 4).expect("plane"),
            Plane::new(&signal, extent, 4).expect("plane"),
            PlaneMut::new(&mut output, extent, 4).expect("plane"),
            MergeConfig::new(MergeCriterion::BoundaryDifference, 0.2, Connectivity::Four)
                .expect("config"),
        )
        .expect("merge");
        assert_eq!(output, [3, 3, 9, 0]);
    }

    #[test]
    fn configuration_and_geometry_errors_are_values() {
        assert_eq!(
            SuperpixelConfig::new(0, 1, 1.0),
            Err(SuperpixelConfigError::ZeroCellDimension)
        );
        assert_eq!(
            MergeConfig::new(MergeCriterion::BoundaryDifference, -1.0, Connectivity::Four),
            Err(MergeConfigError::InvalidThreshold)
        );
        let short = Extent::new(1, 1).expect("extent");
        let long = Extent::new(2, 1).expect("extent");
        let mut output = [0; 2];
        assert_eq!(
            superpixels(
                Plane::new(&[0.0], short, 1).expect("plane"),
                PlaneMut::new(&mut output, long, 2).expect("plane"),
                SuperpixelConfig::new(1, 1, 1.0).expect("config"),
            ),
            Err(SegmentError::ExtentMismatch)
        );
    }
}

# Filter catalogue

This page is the public VapourSynth surface of the twelve plugin families: 53
filters in total. Every entry is a working **scalar reference implementation**
with **Experimental** maturity. Experimental means that signatures and numeric
behaviour may still change before a stable release; it is not a performance
claim.

The signatures below use `VideoNode`, `int`, and `float` in their Python sense.
All shown arguments are required and may be passed by name, for example:

```python
distance = core.masklab.DistanceL1(clip=source)
```

## Runtime conventions

- **Spatial** filters process one frame at a time, **Temporal** filters request
  neighbouring frames, **Multi-input** filters combine clips, and **Analysis**
  filters render measurements as a composable video clip.
- Unless an entry says otherwise, adapters accept planar u8, u16, and
  single-precision f32 and preserve the first input clip's format, dimensions,
  frame count, and frame rate. Paired clips must have compatible geometry and
  timing; the shared binary adapters require identical formats.
- Integer clips retain their native sample scale. Results are rounded and
  clamped to the declared bit depth. Use f32 when signed or fractional values
  must survive.
- `RGBS` below means planar RGB f32 with three equal-sized planes. Vector fields
  encode **R = horizontal displacement**, **G = vertical displacement**, and
  **B = magnitude**. Statistical clips document their own three-plane encoding.
- Binary masks use zero for false and the format peak for true (`1.0` for f32).
  A mask input treats every finite non-zero sample as true.
- Integer-valued label rasters are never silently clipped or rounded. A filter
  fails if a label cannot be represented exactly by its u8, u16, or f32 output.

See [VapourSynth runtime](vapoursynth.md) for build, loading, ABI, and general
numeric details.

## Parameter reference

The catalogue signatures are intentionally compact. This section defines every
public parameter so callers do not have to infer units, ranges, or clip roles
from a name alone.

### Clip and mask inputs

- **`clip`** — Primary source clip. Unless a filter says otherwise, its format,
  dimensions, frame count, and frame rate define the output.
- **`other`** — Secondary source clip combined with `clip`. It must match the
  geometry and timing requirements stated by the filter.
- **`reference`** — Aligned comparison or clean-reference clip. It is used for
  estimation or repair and is not modified.
- **`moving`** — Clip whose translation is estimated relative to `clip`.
- **`guide`** — Guide image used to control edge-aware smoothing of `clip`.
- **`marker`** — Seed image for morphological reconstruction. Marker values are
  propagated while constrained by `mask`.
- **`mask`** — Explicit binary or weighting mask. Finite non-zero samples are
  treated as selected unless the filter documents a continuous mask.
- **`labels`** — Integer-valued region-label raster. Values must be finite,
  non-negative, exactly integral, and representable by the documented type.
- **`signal`** — Image whose boundary differences guide region merging.
- **`gradient`** — Gradient-magnitude image used by watershed segmentation.
- **`markers`** — Labelled watershed seed image.
- **`psf`** — Full-frame point-spread-function clip. It must have compatible
  plane extents; a smaller kernel image is not accepted.
- **`field`**, **`forward`**, **`backward`**, **`first`**, **`second`** — `RGBS`
  vector-field clips. R stores horizontal displacement, G vertical
  displacement, and B magnitude.
- **`previous`** — Previous or reference grain sample aligned with `clip`.

### Iteration, radius, and geometry parameters

- **`iterations`** — Number of algorithm iterations. It must be a positive
  integer.
- **`max_iterations`** — Upper bound on thinning iterations. Processing may stop
  earlier when the result converges.
- **`radius`** — Spatial neighbourhood radius in samples of each processed
  plane. It must be non-negative.
- **`inner_radius`** — Distance over which the fully selected interior is
  retained when feathering. It must be finite and non-negative.
- **`outer_radius`** — Distance over which the feather falls to zero outside the
  selected region. It must be finite, non-negative, and not smaller than the
  effective inner transition.
- **`local_radius`** — Radius of the local window used to estimate grain
  statistics.
- **`texture_radius`** — Radius used to estimate the fine texture component.
- **`structure_radius`** — Radius used to estimate the broad structure
  component.
- **`search_radius`** — Maximum displacement searched in each direction during
  motion estimation.
- **`patch_radius`** — Half-size of the square comparison patch used by motion
  estimation.
- **`max_displacement`** — Maximum translation magnitude searched in full-frame
  pixels.
- **`max_shift`** — Maximum permitted x/y translation in full-resolution
  first-plane pixels. It must be non-negative.
- **`minimum_length`** — Minimum accepted scratch length in pixels.
- **`tile_width`**, **`tile_height`** — Full-resolution tile dimensions used for
  local motion estimation. Both must be positive.
- **`cell_width`**, **`cell_height`** — Target superpixel cell dimensions in
  full-resolution pixels. Both must be positive.
- **`tiles_x`**, **`tiles_y`** — Number of CLAHE tiles across the full-resolution
  first plane. Both must be positive.

### Thresholds, strengths, and numeric controls

- **`threshold`** — Filter-specific decision threshold in the native numeric
  scale of the relevant input.
- **`contrast_threshold`** — Minimum local contrast required for a scratch
  candidate.
- **`deviation_threshold`** — Minimum deviation from the local neighbourhood for
  a spatial outlier candidate.
- **`neighbour_tolerance`** — Allowed disagreement between neighbouring frames
  when classifying a temporal outlier.
- **`energy_threshold`** — Minimum temporal energy counted as active by
  `EventEnergy`.
- **`edge_threshold`** — Edge sensitivity used to separate detail adjustment
  from edge preservation.
- **`epsilon`** — Positive numerical stabilizer that prevents division by values
  near zero.
- **`noise_to_signal`** — Estimated noise-power to signal-power ratio used by
  Wiener restoration. It must be finite and non-negative.
- **`data_step`** — Positive update step applied to the data-fidelity term in
  regularized restoration.
- **`regularization`** — Non-negative strength of the edge-aware regularization
  term.
- **`sigma_spatial`** — Spatial standard deviation, in plane samples, for domain
  transform smoothing.
- **`sigma_range`** — Range standard deviation in the clip's native sample scale.
- **`range_sigma`** — Range-domain smoothing strength in the clip's native
  sample scale.
- **`edge_sigma`** — Edge sensitivity for global smoothing in the clip's native
  sample scale.
- **`step`** — Positive optimization step used by global smoothing.
- **`compactness`** — Relative penalty on spatial distance versus signal
  similarity during superpixel generation.
- **`strength`** — Filter-specific effect strength. `Stabilize` uses it as the
  amount of smoothed camera-path correction; `Vignette` uses it as radial
  falloff strength.
- **`gain`** — Multiplier applied to the extracted temporal signal or residual.
- **`repair_weight`** — Blend weight in `[0, 1]`: `0` keeps the source and `1`
  uses the reference estimate completely.
- **`detail`** — Signed local-detail adjustment strength for local Laplacian
  filtering.
- **`midpoint`** — Exposure value considered ideally exposed during fusion.
- **`sigma`** — Width of the exposure preference around `midpoint`; it must be
  positive.
- **`target`** — Desired normalized illumination level in the clip's native
  scale.
- **`floor`** — Positive lower bound for estimated illumination, preventing
  unstable division in dark regions.
- **`minimum`**, **`maximum`** — Lower and upper sample bounds of the CLAHE
  operating range; `maximum` must be greater than `minimum`.
- **`clip_limit`** — Positive histogram-bin count limit used to constrain local
  contrast amplification in CLAHE.
- **`bins`** — Number of histogram bins used by CLAHE. It must be positive.
- **`mean`** — Requested additive grain mean in the destination sample scale.
- **`variance`** — Requested non-negative grain variance.
- **`maximum_magnitude`** — Vector magnitude mapped to maximum display intensity
  by `Visualize`; it must be positive.
- **`minimum_overlap`** — Minimum fraction of finite overlapping sample pairs
  accepted during registration. It must be in `(0, 1]`.

### Coordinates, optics, and resampling

- **`x`**, **`y`** — Horizontal and vertical translation in full-resolution
  first-plane pixels. Positive x moves right; positive y moves down.
- **`top_x`**, **`top_y`** — Translation at the top scanline for rolling-shutter
  simulation.
- **`bottom_x`**, **`bottom_y`** — Translation at the bottom scanline; offsets
  are linearly interpolated between top and bottom.
- **`red_x`**, **`red_y`** — Red-plane displacement relative to green, in
  full-resolution pixels.
- **`blue_x`**, **`blue_y`** — Blue-plane displacement relative to green, in
  full-resolution pixels.
- **`focal_x`**, **`focal_y`** — Horizontal and vertical focal lengths in pixels
  for radial distortion correction. Both must be positive.
- **`k1`** — First radial-distortion coefficient. Negative and positive values
  correct opposite distortion directions.
- **`bilinear`** — Resampling selector: `0` for nearest-neighbour or `1` for
  bilinear interpolation.
- **`interpolation_mode`** — Translation resampling selector: `0` for nearest-
  neighbour or `1` for bilinear interpolation.

### Enumerations and reproducibility

- **`connectivity_value`** — Pixel connectivity: `4` for orthogonal neighbours
  or `8` to include diagonals.
- **`estimate_value`** — Temporal inpaint estimator: `0` averages adjacent
  estimates; `1` chooses the estimate nearest to the current sample.
- **`method`** — Stack combiner: `0` for mean or `1` for median.
- **`direction`** — Vignette operation: `0` applies falloff; `1` corrects it.
- **`output_component_value`** — Residual component: `0` structure, `1` texture,
  or `2` signed residual.
- **`normalization_value`** — Spectrum normalization: `0` for none or `1` to
  divide by the number of pixels.
- **`metric_value`** — Comparison field: `0` signed difference, `1` absolute
  difference, or `2` squared difference.
- **`boundary_value`** — Watershed boundary policy: `0` emits a watershed line;
  `1` assigns the lowest adjacent label.
- **`random_seed`** — Non-negative seed controlling deterministic grain output.
  Reusing the same inputs and seed produces the same samples.

## `masklab` — 6 filters

| Public call | Shape | Output and contract |
| --- | --- | --- |
| `DistanceL1(clip: VideoNode)` | Spatial | Manhattan distance to the nearest zero mask sample, per plane. Output values are exact non-negative integers and must fit the destination format. |
| `DistanceEuclidean(clip: VideoNode)` | Spatial | Exact Euclidean distance to the nearest zero mask sample, per plane. Requires f32 so fractional distances are preserved. |
| `ComponentLabels(clip: VideoNode, connectivity_value: int)` | Analysis | Deterministic connected-component label raster; connectivity is `4` or `8`. Labels must be exactly representable. |
| `Reconstruct(marker: VideoNode, mask: VideoNode, connectivity_value: int)` | Multi-input | Geodesic reconstruction; connectivity is `4` or `8`. Both inputs must be compatible u8 clips. |
| `Thin(clip: VideoNode, max_iterations: int)` | Spatial | Topology-preserving thinning of a non-zero binary mask; renders a binary mask. |
| `Feather(clip: VideoNode, inner_radius: float, outer_radius: float)` | Spatial | Distance-based inner/outer feathering; radii are finite and non-negative. Renders `0..peak`. |

`ComponentLabels` is intentionally the raster-facing name for the pure
connected-components report.

## `defect` — 5 filters

| Public call | Shape | Output and contract |
| --- | --- | --- |
| `TemporalOutliers(clip: VideoNode, threshold: float, neighbour_tolerance: float)` | Temporal | Binary mask of samples that disagree with a two-sided temporal neighbourhood. First and last frames are all-zero masks. |
| `ScratchDetect(clip: VideoNode, contrast_threshold: float, minimum_length: int)` | Analysis | Binary raster of line-shaped scratch detections. This rasterizes the pure scratch report. |
| `DropoutRepair(clip: VideoNode, reference: VideoNode, mask: VideoNode, repair_weight: float)` | Multi-input | Blends explicitly masked samples with an aligned reference; `repair_weight` is in `[0, 1]`. `clip` and `reference` have identical formats; `mask` has compatible timing and plane layout. |
| `SpatialOutliers(clip: VideoNode, deviation_threshold: float)` | Analysis | Binary raster of single-frame local outlier candidates. This is the raster-facing name for the pure dead-pixel point list. |
| `InpaintTemporal(clip: VideoNode, mask: VideoNode, estimate_value: int)` | Multi-input | Repairs an explicit mask from adjacent aligned frames. `estimate_value` is `0` (average) or `1` (nearest to current); endpoints pass the source frame through unchanged. |

Repair never infers defects from a zero-valued image sample: both repair
operations require an explicit mask clip.

## `deconvolve` — 4 filters

| Public call | Shape | Output and contract |
| --- | --- | --- |
| `Wiener(clip: VideoNode, psf: VideoNode, noise_to_signal: float)` | Multi-input | Wiener restoration using a full-frame PSF clip. |
| `RichardsonLucy(clip: VideoNode, psf: VideoNode, iterations: int, epsilon: float)` | Multi-input | Iterative Richardson–Lucy restoration; `iterations` is positive. |
| `Regularized(clip: VideoNode, psf: VideoNode, iterations: int, data_step: float, regularization: float)` | Multi-input | Iterative edge-aware regularized restoration; `iterations` is positive. |
| `EstimatePsf(clip: VideoNode, reference: VideoNode, max_shift: int)` | Analysis | Estimates and renders a constrained **full-frame** PSF for each plane; `max_shift` is non-negative. |

The observed/reference/PSF clips use compatible full-frame plane extents; the
adapter does not accept a smaller kernel clip.

## `flowfield` — 5 filters

All five calls require and return `RGBS`. Field consumers read R and G as the
signed vector; B is a derived magnitude channel.

| Public call | Shape | Output and contract |
| --- | --- | --- |
| `Estimate(clip: VideoNode, search_radius: int, patch_radius: int)` | Temporal | Dense forward field from each frame to the next, using RGB luminance. The final frame compares with itself. |
| `Warp(clip: VideoNode, field: VideoNode, bilinear: int)` | Multi-input | Warps the RGBS `clip` by an RGBS field; `bilinear` is `0` (nearest) or `1` (bilinear). |
| `Confidence(forward: VideoNode, backward: VideoNode)` | Analysis | Forward/backward confidence as one scalar raster replicated into R, G, and B. |
| `Compose(first: VideoNode, second: VideoNode)` | Multi-input | Composed field with the standard X/Y/magnitude encoding. |
| `Visualize(field: VideoNode, maximum_magnitude: float)` | Analysis | RGB direction-and-magnitude colour rendering in RGBS. |

## `grainlab` — 4 filters

| Public call | Shape | Output and contract |
| --- | --- | --- |
| `Analyze(clip: VideoNode, previous: VideoNode, local_radius: int)` | Analysis | Requires compatible RGBS inputs. Returns frame-constant statistics: **R = residual mean**, **G = residual variance**, **B = temporal-difference population variance**. |
| `Synthesize(clip: VideoNode, mean: float, variance: float, random_seed: int)` | Spatial | Uses `clip` as the output-format template and generates deterministic per-plane grain; seed is non-negative. |
| `Match(clip: VideoNode, mean: float, variance: float, random_seed: int)` | Spatial | Applies the requested grain profile to the clip, preserving its format; seed is non-negative. |
| `Residual(clip: VideoNode, reference: VideoNode)` | Multi-input | Per-sample candidate grain residual. Use f32 for a lossless signed residual; integer destinations follow the normal clamp policy. |

## `phase` — 4 filters

| Public call | Shape | Output and contract |
| --- | --- | --- |
| `Correlate(clip: VideoNode, max_displacement: int)` | Temporal | Requires RGBS. Produces a frame-constant X/Y/magnitude translation from the current frame to the next; the final frame compares with itself. |
| `LocalMotion(clip: VideoNode, tile_width: int, tile_height: int, max_displacement: int)` | Temporal | Requires RGBS. Produces a piecewise-constant X/Y/magnitude field, one estimate per tile. |
| `Magnify(clip: VideoNode, gain: float)` | Temporal | Amplifies the three-frame temporal high-pass signal and preserves the source format. Endpoint windows reuse the current frame for a missing neighbour. |
| `EventEnergy(clip: VideoNode, energy_threshold: float)` | Analysis | Requires RGBS. Returns frame-constant statistics: **R = mean energy**, **G = peak energy**, **B = active-energy fraction**. |

## `edgeaware` — 5 filters

Spatial radii are measured in samples of each processed plane; range parameters
use the clip's native numeric scale.

| Public call | Shape | Output and contract |
| --- | --- | --- |
| `Guided(clip: VideoNode, radius: int, epsilon: float)` | Spatial | Self-guided smoothing. |
| `JointGuided(clip: VideoNode, guide: VideoNode, radius: int, epsilon: float)` | Multi-input | Filters `clip` using a format-compatible guide clip. |
| `DomainTransform(clip: VideoNode, sigma_spatial: float, sigma_range: float, iterations: int)` | Spatial | Iterated domain-transform filtering. |
| `RollingGuidance(clip: VideoNode, radius: int, range_sigma: float, iterations: int)` | Spatial | Iterative small-structure removal. |
| `GlobalSmooth(clip: VideoNode, iterations: int, edge_sigma: float, step: float)` | Spatial | Edge-aware global smoothing. |

## `register` — 4 filters

Translations use full-resolution first-plane pixel units. Warp operations scale
them for subsampled planes and clamp at the border.

| Public call | Shape | Output and contract |
| --- | --- | --- |
| `Estimate(clip: VideoNode, moving: VideoNode, max_shift: int, minimum_overlap: float)` | Multi-input | Copies the `clip` frame unchanged and attaches the estimated first-plane translation as float frame properties **`VSIPRegisterX`** and **`VSIPRegisterY`**. |
| `Warp(clip: VideoNode, x: float, y: float, interpolation_mode: int)` | Spatial | Applies a translation; interpolation is `0` (nearest) or `1` (bilinear). |
| `Stack(clip: VideoNode, other: VideoNode, method: int)` | Multi-input | Combines two aligned clips; method is `0` (mean) or `1` (median). |
| `Stabilize(clip: VideoNode, strength: float, max_shift: int, minimum_overlap: float)` | Temporal | Estimates neighbouring first-plane translations, smooths the three-frame path, and warps every plane. Endpoint windows are shortened, not duplicated. |

`minimum_overlap` is a positive finite-pair fraction. `Estimate` reads the first
plane for estimation; its two frame properties are the authoritative result.

## `lens` — 4 filters

Geometric distances and shifts use full-resolution first-plane pixel units and
are scaled for subsampled planes. Resampling is bilinear with a clamped border.

| Public call | Shape | Output and contract |
| --- | --- | --- |
| `Undistort(clip: VideoNode, focal_x: float, focal_y: float, k1: float)` | Spatial | Corrects one radial distortion coefficient around the frame centre. |
| `Chromatic(clip: VideoNode, red_x: float, red_y: float, blue_x: float, blue_y: float)` | Spatial | Requires planar RGB with exactly three equal-sized planes; shifts red and blue relative to green. |
| `Vignette(clip: VideoNode, strength: float, direction: int)` | Spatial | Applies (`0`) or corrects (`1`) radial light falloff. |
| `RollingShutter(clip: VideoNode, top_x: float, top_y: float, bottom_x: float, bottom_y: float)` | Spatial | Applies a row-interpolated translation between the top and bottom offsets. |

## `tonelab` — 4 filters

Intensity parameters use the clip's native numeric scale.

| Public call | Shape | Output and contract |
| --- | --- | --- |
| `Clahe(clip: VideoNode, tiles_x: int, tiles_y: int, bins: int, clip_limit: int, minimum: float, maximum: float)` | Spatial | Contrast-limited adaptive histogram equalization over the declared range. Tile counts describe the full-resolution first plane and scale for subsampled planes. |
| `LocalLaplacian(clip: VideoNode, radius: int, detail: float, edge_threshold: float)` | Spatial | Edge-aware local tone adjustment. |
| `ExposureFusion(clip: VideoNode, other: VideoNode, midpoint: float, sigma: float)` | Multi-input | Fuses two format-compatible exposures. |
| `NormalizeIllumination(clip: VideoNode, radius: int, target: float, floor: float)` | Spatial | Separates local illumination and normalizes it to `target` with the declared floor. |

## `residual` — 4 filters

Every Residual adapter requires f32 input and returns f32. This is a hard
contract so signed and fractional residuals are never quantized away.

| Public call | Shape | Output and contract |
| --- | --- | --- |
| `Decompose(clip: VideoNode, texture_radius: int, structure_radius: int, output_component_value: int)` | Analysis | Renders one selected field per plane: `0` structure, `1` texture, or `2` signed residual. |
| `Temporal(clip: VideoNode, gain: float)` | Temporal | Three-frame temporal residual. First and last frames are defined as all-zero residuals. |
| `Spectrum(clip: VideoNode, normalization_value: int)` | Analysis | Full-frame residual power raster; normalization is `0` (none) or `1` (by pixels). |
| `Compare(clip: VideoNode, other: VideoNode, metric_value: int)` | Multi-input | Per-sample field comparison: `0` signed, `1` absolute, or `2` squared. Inputs also require equal frame rates. |

## `segment` — 4 filters

Label inputs must contain finite, non-negative, exactly integral values within
`u32`; label and degree outputs must be exactly representable in the first
input's format.

| Public call | Shape | Output and contract |
| --- | --- | --- |
| `Watershed(gradient: VideoNode, markers: VideoNode, connectivity_value: int, boundary_value: int)` | Multi-input | Label raster from gradients and marker labels. Connectivity is `4` or `8`; boundary is `0` (watershed line) or `1` (lowest adjacent label). |
| `Superpixels(clip: VideoNode, cell_width: int, cell_height: int, compactness: float)` | Spatial | Compact-region label raster. |
| `RegionDegreeMap(clip: VideoNode, connectivity_value: int)` | Analysis | Rasterizes each labelled region's adjacency-graph degree at every pixel; background label zero remains zero. |
| `Merge(labels: VideoNode, signal: VideoNode, threshold: float, connectivity_value: int)` | Multi-input | Merges adjacent regions by boundary difference and returns the resulting label raster. |

`RegionDegreeMap` is intentionally the raster-facing public name for the pure
region-graph report.

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

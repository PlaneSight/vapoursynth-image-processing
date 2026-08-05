# Roadmap

## Phase 0 — Workspace contract

- Enforced dependency direction and plugin manifest inventory.
- Pinned compiler, MSRV job, platform CI and artifact policy.
- Pure-Rust kernels isolated from the future VapourSynth ABI adapter.

## Phase 1 — MaskLab foundation

- L1 and exact squared-Euclidean distance transforms.
- Binary and greyscale erosion, dilation, opening and closing.
- Connected-component labeling and component statistics.
- Reconstruction, hole filling, thinning and distance-based feathering.
- Scalar/reference tests followed by measured SIMD specialization.

## Phase 2 — Restoration primitives

- Defect masks and temporal repair.
- Explicit-PSF Wiener and Richardson-Lucy deconvolution.
- Residual decomposition and grain profile analysis.
- Guided and domain-transform edge-aware filtering.

## Phase 3 — Motion platform

- Dense flow representation, warp, composition and inversion.
- Forward/backward confidence and occlusion masks.
- Registration and robust temporal stacking.
- Phase correlation, local motion energy and transient event scoring.

## Phase 4 — Imaging systems

- Lens, chromatic aberration and rolling-shutter models.
- Local tone and exposure fusion.
- Watershed, superpixels and region adjacency.
- Stable VapourSynth packages for Windows, Linux and macOS.

## Acceptance rule

A planned filter becomes experimental only when it has a real implementation,
oracle-backed correctness tests and documented format behavior. It becomes
stable only after public API review, representative benchmarks and packaged
VapourSynth integration tests.

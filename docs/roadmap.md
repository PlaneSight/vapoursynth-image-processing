# Roadmap

## Completed foundation

- Enforced workspace dependency direction and all twelve plugin namespaces.
- Pinned Rust 1.97.1 development toolchain and Rust 1.85 pure-code MSRV.
- Added Windows, Linux, and macOS formatting, Clippy, debug, release, docs,
  no-default-feature, and split-MSRV CI gates.
- Isolated validated strided-plane types, reference kernels, plugin catalogues,
  reusable fixtures, and the VapourSynth runtime boundary.
- Pinned `rust-av/vapoursynth-rs` and confined generated ABI unsafe code to
  loadable adapter crates.

## Experimental scalar coverage

All 53 catalogue entries have deterministic scalar implementations and tests:

- Mask and segmentation: L1/exact Euclidean distance, components,
  reconstruction, thinning, feathering, watershed, superpixels, region graphs,
  and merging.
- Restoration: temporal defects, scratch/dead-pixel analysis, repair,
  deconvolution, residual decomposition, grain measurement, and synthesis.
- Motion: dense reference flow, warp/composition/confidence, registration,
  stacking, stabilization, correlation, local motion, magnification, and event
  energy.
- Imaging: guided/domain/global smoothing, lens/chromatic/vignette/rolling
  shutter correction, CLAHE, local tone, exposure fusion, and illumination
  normalization.

The scalar algorithms favor explicit invariants and oracle value over speed.
Several exhaustive references intentionally have quadratic or higher costs;
their Rust documentation states the cost model and scratch ownership.

## Experimental runtime coverage

- One independently loadable `cdylib` exists for every namespace.
- Every catalogue filter is registered through the pinned VapourSynth R4 API.
- Same-format image results support planar u8, u16, and f32 through one shared
  conversion policy and bounded reusable scratch pools.
- Typed analysis results have documented scalar-map renderings for graph
  composition while their richer native Rust APIs remain available.

## Before stable releases

1. Review public parameter names, defaults, output conventions, and semver
   boundaries with real VapourSynth scripts.
2. Add packaged runtime integration suites on installed Windows, Linux, and
   macOS VapourSynth hosts, including temporal and multi-input filters.
3. Establish representative restoration and motion corpora with independent
   numerical oracles and quantitative quality metrics.
4. Benchmark release builds on controlled hardware; record compiler, CPU,
   target features, allocation, working set, and end-to-end throughput.
5. Profile first, then add auto-vectorization-friendly or target-specific paths
   only where measurements show material value. Differentially test every
   optimized path against its scalar reference.
6. Add fuzzing, Miri/sanitizer jobs, package-content checks, and ABI/load tests
   where their failure models provide leverage.

## Acceptance rule

`Experimental` means a callable implementation with typed failure behavior,
correctness tests, and documented format behavior. `Stable` additionally
requires public API review, representative measured performance, and packaged
VapourSynth integration tests. No filter currently claims `Stable` maturity.

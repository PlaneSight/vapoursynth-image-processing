# VapourSynth Image Processing

A Rust workspace for native, composable VapourSynth image and video processing
plugins. It contains pure, runtime-independent scalar algorithms, typed plugin
catalogues, and loadable VapourSynth adapters built on
[`rust-av/vapoursynth-rs`](https://github.com/rust-av/vapoursynth-rs).

## Plugin families

All 53 catalogued filters have working scalar reference implementations and
correctness tests. They remain `Experimental`: a filter becomes `Stable` only
after public API review, representative release benchmarks, and packaged
VapourSynth integration tests.

| Crate | Namespace | Implemented filters |
| --- | --- | --- |
| `vs-masklab` | `masklab` | DistanceL1, DistanceEuclidean, ComponentLabels, Reconstruct, Thin, Feather |
| `vs-defect` | `defect` | TemporalOutliers, ScratchDetect, DropoutRepair, SpatialOutliers, InpaintTemporal |
| `vs-deconvolve` | `deconvolve` | Wiener, RichardsonLucy, Regularized, EstimatePsf |
| `vs-flowfield` | `flowfield` | Estimate, Warp, Confidence, Compose, Visualize |
| `vs-grainlab` | `grainlab` | Analyze, Synthesize, Match, Residual |
| `vs-phase` | `phase` | Correlate, LocalMotion, Magnify, EventEnergy |
| `vs-edgeaware` | `edgeaware` | Guided, JointGuided, DomainTransform, RollingGuidance, GlobalSmooth |
| `vs-register` | `register` | Estimate, Warp, Stack, Stabilize |
| `vs-lens` | `lens` | Undistort, Chromatic, Vignette, RollingShutter |
| `vs-tonelab` | `tonelab` | Clahe, LocalLaplacian, ExposureFusion, NormalizeIllumination |
| `vs-residual` | `residual` | Decompose, Temporal, Spectrum, Compare |
| `vs-segment` | `segment` | Watershed, Superpixels, RegionDegreeMap, Merge |

## Workspace structure

- `crates/vsip-core`: validated extents and borrowed strided planes.
- `crates/vsip-kernels`: runtime-independent shared reference kernels.
- `crates/vsip-plugin-api`: typed catalogues, maturity, and execution shape.
- `crates/vsip-test-support`: deterministic test fixtures.
- `crates/vsip-vapoursynth`: safe scheduling, frame conversion, and bounded
  reusable scratch storage.
- `plugins/*`: independently releasable pure plugin-family APIs.
- `adapters/*-vapoursynth`: one loadable `cdylib` per namespace.

Pure kernels never import VapourSynth. Runtime adapters translate validated
frame rows into tightly packed caller-owned scratch, invoke pure APIs, and copy
the result back. No adapter contains handwritten unsafe code; the pinned
upstream export macro owns the generated C ABI entry point and panic boundary.

## Build and test

```text
cargo xtask check-tree
cargo fmt --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace --release
cargo test --workspace --no-default-features
cargo doc --workspace --no-deps
```

The pinned development compiler is Rust 1.97.1. Pure algorithms and plugin
families retain Rust 1.85 as their MSRV. Runtime adapter crates require Rust
1.88 because the pinned upstream `vapoursynth-rs` revision depends on
`libloading` 0.9.

Build one loadable plugin with, for example:

```text
cargo build --release -p vs-masklab-vapoursynth
```

The resulting platform library is under `target/release`. Adapters support u8,
u16, and single-precision f32 planar samples unless a filter documents a
narrower contract. Integer samples retain their native numeric scale during
processing; output is rounded and clamped to the declared bit depth, and
non-finite integer output is rejected.

Motion-vector and aggregate analysis clips use planar RGB f32 so signed and
fractional components remain representable. Residual-family adapters also
require f32 clips. Defect repair functions take explicit mask clips; sample
value zero is never treated as an implicit defect marker.

## Design and performance policy

- Plugins may depend on shared crates; shared crates never depend on plugins.
- Configuration parsing, frame requests, scratch acquisition, and dispatch
  occur outside pixel loops.
- Stable-size transforms write to caller-owned output and scratch storage.
- Scalar implementations are authoritative and explicitly document their cost.
- No SIMD or speed claim is accepted without release benchmarks and
  generated-code inspection against the scalar reference.
- Every optimized path must be differentially tested across odd sizes, padded
  strides, tails, and supported dispatch targets.

See [Architecture](docs/architecture.md), [Roadmap](docs/roadmap.md),
[VapourSynth runtime](docs/vapoursynth.md), and
[Artifact policy](docs/artifacts.md).

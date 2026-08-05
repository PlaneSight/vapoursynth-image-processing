# VapourSynth Image Processing

A Rust monorepo for native, composable VapourSynth image and video processing
plugins. The workspace concentrates shared frame handling, typed configuration,
reference kernels, optimized dispatch, test fixtures, packaging policy, and
documentation while keeping every plugin independently releasable.

## Plugin families

| Crate         | Namespace  | Scope                                             | Phase      |
| ------------- | ---------- | ------------------------------------------------- | ---------- |
| vs-masklab    | masklab    | Morphology, distance fields and connected regions | Foundation |
| vs-defect     | defect     | Dust, scratches, dropouts and temporal repair     | Planned    |
| vs-deconvolve | deconvolve | PSF-based and regularized deconvolution           | Planned    |
| vs-flowfield  | flowfield  | Dense motion fields, confidence and warping       | Planned    |
| vs-grainlab   | grainlab   | Grain measurement, matching and synthesis         | Planned    |
| vs-phase      | phase      | Phase motion, vibration and event energy          | Planned    |
| vs-edgeaware  | edgeaware  | Guided and edge-preserving smoothing              | Planned    |
| vs-register   | register   | Alignment, stacking and registration              | Planned    |
| vs-lens       | lens       | Lens and rolling-shutter correction               | Planned    |
| vs-tonelab    | tonelab    | Local tone and exposure operations                | Planned    |
| vs-residual   | residual   | Structure, texture and noise decomposition        | Planned    |
| vs-segment    | segment    | Watershed, superpixels and region graphs          | Planned    |

vs-masklab contains the first reference implementation: a Manhattan distance
transform over caller-owned buffers. Other plugin crates start as typed,
discoverable contracts rather than pretending unfinished algorithms are usable.

## Commands

    cargo xtask check-tree
    cargo fmt --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    cargo test --workspace --release
    cargo doc --workspace --no-deps

The pinned development compiler is Rust 1.97.1. The workspace MSRV is Rust
1.85, the first stable release supporting Rust 2024.

## Design rules

- Plugins may depend on shared crates; shared crates never depend on plugins.
- Algorithm crates do not know about VapourSynth or its FFI.
- Reference scalar kernels precede target-specific optimization.
- Caller-owned output and scratch storage are preferred on frame hot paths.
- Every optimized kernel must be differentially tested against its reference.
- VapourSynth ABI code will live behind one narrow, panic-contained adapter.
- Build products and generated documentation never enter authored source roots.

See [Architecture](docs/architecture.md), [Roadmap](docs/roadmap.md), and
[Artifact policy](docs/artifacts.md).

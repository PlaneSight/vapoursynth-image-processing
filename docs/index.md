---
hide:
  - navigation
  - toc
---

# VapourSynth image processing in Rust

VSIP is a workspace of native VapourSynth filters built around small, typed,
runtime-independent Rust algorithms. It keeps image-processing code separate
from the plugin ABI, makes data ownership explicit, and treats scalar
correctness as the reference for every future optimization.

The workspace currently provides **53 experimental filters** across twelve
independently releasable plugin families. Experimental means implemented and
tested, but not yet covered by the release benchmarks and packaged runtime
tests required for a stable API.

## Start here

The project uses [uv](https://docs.astral.sh/uv/) to provision Python 3.14 and
to expose one command for development, builds, documentation, and runtime
checks:

```console
uv sync --python 3.14 --locked
uv run vsip --help
uv run vsip info
uv run vsip check
```

[Set up the project](getting-started.md) for the complete first-run workflow,
or open the [CLI reference](cli.md) when you already have a checkout.

## What is included

| Area | Plugin families |
| --- | --- |
| Masks and structure | MaskLab, Segment, Defect |
| Restoration and residuals | Deconvolve, Residual, GrainLab |
| Motion and registration | FlowField, Phase, Register |
| Edge, lens, and tone | EdgeAware, Lens, ToneLab |

Each family has a pure Rust crate under `plugins/` and a loadable VapourSynth
adapter under `adapters/`. Shared crates own validated plane geometry, reusable
kernels, catalogue types, test fixtures, and the safe scheduling bridge to
[`vapoursynth-rs`](https://github.com/rust-av/vapoursynth-rs).

Browse the [filter catalogue](filter-catalog.md) for the complete namespace and
function list.

## Project guarantees

- Pure kernels do not import VapourSynth or retain runtime-owned frame data.
- Visible image rows are validated before a kernel receives them.
- Temporary buffers have exclusive ownership for the duration of each call.
- There is no handwritten unsafe code in VSIP adapters or algorithms.
- Scalar implementations define correctness; performance claims require
  release benchmarks and generated-code inspection.
- Pure crates support Rust 1.85. The pinned runtime bridge requires Rust 1.88,
  while day-to-day development uses the workspace's pinned toolchain.

The [architecture](architecture.md) explains these boundaries in detail. For
VapourSynth formats, library names, and loading behavior, see the
[runtime guide](vapoursynth.md).

## Contributing

Start with the [development guide](development.md), use
`uv run vsip check --quick` while iterating, then run the complete static gate
through `uv run vsip check`. Bugs and design proposals are welcome in the
[GitHub issue tracker](https://github.com/PlaneSight/vapoursynth-image-processing/issues).

VSIP is dual-licensed under Apache-2.0 or MIT, at your option.

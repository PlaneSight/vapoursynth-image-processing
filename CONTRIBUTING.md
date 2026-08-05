# Contributing

Install Python 3.14 and the locked tooling with `uv sync --locked`. Use
`uv run vsip check --quick` while editing. A change is ready for review when
`uv run --locked vsip ci` passes; that command covers formatting, Clippy, every
test profile, both documentation systems, packaging, MSRV checks, and the tree
contract.

Pure crates must continue to compile and run tests on Rust 1.85. Runtime
adapters must compile on Rust 1.88, the effective MSRV of the pinned
`vapoursynth-rs` graph. Run the full quality gate with the repository's pinned
Rust 1.97.1 toolchain.

Algorithm changes require:

1. A scalar reference implementation or a cited reference oracle.
2. Boundary tests for empty, one-pixel, odd-width and stride-sensitive inputs.
3. Differential tests for every optimized dispatch path.
4. Benchmarks before performance claims.
5. A local safety contract for every unavoidable unsafe operation.

New plugin crates must own one cohesive namespace, declare their public filter
catalogue through vsip-plugin-api, document dependencies on shared capabilities,
and be added to docs/roadmap.md.

Runtime changes must use the shared `FrameBuffers` numeric policy and scheduling
adapters. Do not duplicate frame-format conversion in a plugin. Validate modes
and configuration before constructing a filter, keep handwritten unsafe code
out of adapters, and add an installed-host smoke test for new ABI behavior.

Repository automation belongs in the typed `vsip_tools` Python package and
must remain cross-platform: invoke tools with argument arrays, resolve paths
explicitly, and never depend on shell-specific environment syntax. Change
Python dependencies through uv, commit `uv.lock`, and keep the CLI reference in
sync with every public command.

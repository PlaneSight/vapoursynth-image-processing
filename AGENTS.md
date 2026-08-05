# Repository contract for coding agents

## Scope

This workspace contains shared image-processing infrastructure and twelve
independently releasable VapourSynth plugin families. Preserve plugin
boundaries; do not collapse algorithms into a generic utilities crate.

## Dependency policy

- plugins may depend on vsip-plugin-api, vsip-kernels and vsip-core;
- vsip-kernels may depend on vsip-core;
- shared crates must never depend on plugins;
- pure kernels must never import VapourSynth bindings;
- test-support code must not leak into release dependencies.

## Implementation policy

- Planned manifest entries are contracts, not implementations.
- Promote Planned to Experimental only with working code and correctness tests.
- Keep a clear scalar reference for optimized kernels.
- Parse configuration, allocate scratch and dispatch outside pixel loops.
- Prefer validated borrowed planes and caller-owned output buffers.
- Introduce unsafe code only in a narrow crate with a written safety contract.
- Never claim SIMD or performance improvement without release benchmarks and
  generated-code inspection.

## Validation

Run the commands documented in README.md. Test odd dimensions, padded strides,
empty or invalid configuration, numeric boundaries, NaNs where formats permit
them, and deterministic behavior across thread counts.

Do not commit target, dist, site, .tmp, local caches, downloaded models, or
generated benchmark output. Update docs/artifacts.md before introducing a new
artifact class.

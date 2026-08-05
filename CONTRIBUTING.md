# Contributing

Start by running cargo xtask check-tree. A change is ready for review when
formatting, Clippy, debug tests, release tests, documentation and the tree
contract pass.

Algorithm changes require:

1. A scalar reference implementation or a cited reference oracle.
2. Boundary tests for empty, one-pixel, odd-width and stride-sensitive inputs.
3. Differential tests for every optimized dispatch path.
4. Benchmarks before performance claims.
5. A local safety contract for every unavoidable unsafe operation.

New plugin crates must own one cohesive namespace, declare their public filter
catalogue through vsip-plugin-api, document dependencies on shared capabilities,
and be added to docs/roadmap.md.

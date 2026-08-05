# Architecture

## Dependency direction

    plugins/* -> vsip-plugin-api
    plugins/* -> vsip-kernels -> vsip-core
    tests     -> vsip-test-support -> vsip-core
    VapourSynth adapter (future) -> plugin public APIs

No edge may point from a shared crate into a plugin. vsip-core owns small
domain types and validated plane geometry. vsip-kernels owns algorithms that
operate on borrowed slices. vsip-plugin-api describes public namespaces and
implementation maturity without importing runtime or FFI concerns.

## Runtime boundary

The future VapourSynth adapter will:

- translate maps and frames into validated Rust views;
- request temporal dependencies before kernel execution;
- obtain destination frames and per-frame scratch storage;
- catch panics before they reach C;
- translate typed errors into VapourSynth errors;
- register namespaces from plugin manifests.

Raw bindings and safe adapters must remain separate. No kernel may retain a
borrow into a VapourSynth frame beyond the callback that owns it.

## Performance model

Frame kernels are bandwidth-sensitive unless an algorithm states otherwise.
Configuration parsing, dispatch, allocation and CPU-feature detection occur
outside pixel loops. Scalar code is authoritative. Architecture-specific
implementations are selected once per plane and must preserve the scalar
result contract.

No SIMD backend is present in the initial scaffold. SIMD will be added only
after benchmarks and generated-code inspection identify a material hot path.

## Release model

Shared crates are internal until their APIs stabilize. Plugin crates are
independent release units and may mature at different rates. Atomic workspace
changes are allowed, but a release must state exactly which plugin artifacts
were built and tested.

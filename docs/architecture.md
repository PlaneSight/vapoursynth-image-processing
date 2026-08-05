# Architecture

## Dependency direction

    plugins/* -> vsip-plugin-api
    plugins/* -> vsip-kernels -> vsip-core
    tests     -> vsip-test-support -> vsip-core
    adapters/* -> vsip-vapoursynth -> vapoursynth-rs
    adapters/* -> plugin public APIs

No edge may point from a shared crate into a plugin. vsip-core owns small
domain types and validated plane geometry. vsip-kernels owns algorithms that
operate on borrowed slices. vsip-plugin-api describes public namespaces and
implementation maturity without importing runtime or FFI concerns.

`vsip-vapoursynth` is the fifth shared crate because the runtime boundary has
different dependencies and a different MSRV from the pure algorithm graph.
The dependency is pinned to upstream `rust-av/vapoursynth-rs` commit
`ff6b542d3603c3075784303ffa1e44e283baa85c`.

## Runtime boundary

The VapourSynth adapter:

- translate maps and frames into validated Rust views;
- request temporal dependencies before kernel execution;
- obtain copy-on-write destination frames and bounded reusable scratch storage;
- catch panics before they reach C;
- translate typed errors into VapourSynth errors;
- register namespaces from plugin manifests.

Raw bindings remain in `vapoursynth-rs`; VSIP adapter code contains no
handwritten unsafe blocks. The upstream export macro generates the one C ABI
entry point in each `cdylib` adapter and contains unwinding there. No kernel may
retain a borrow into a VapourSynth frame beyond the callback that owns it.

The upstream safe API exposes padded planes as rows. Adapters therefore copy
visible rows into tightly packed scratch leases before invoking pure kernels,
then copy results back. Leases own their buffers during a callback and return
them to a bounded mutex-protected pool afterwards; kernel work never holds the
pool lock and parallel callbacks never alias storage.

## Performance model

Frame kernels are bandwidth-sensitive unless an algorithm states otherwise.
Configuration parsing, dispatch, scratch acquisition and CPU-feature detection
occur outside pixel loops. Scalar code is authoritative. Architecture-specific
implementations are selected once per plane and must preserve the scalar
result contract.

No SIMD backend is present in the initial scaffold. SIMD will be added only
after benchmarks and generated-code inspection identify a material hot path.

## Release model

Pure crates and all twelve plugin families retain Rust 1.85 as their MSRV. The
runtime adapter requires Rust 1.88 because the pinned upstream dependency uses
`libloading` 0.9. The complete workspace is built with Rust 1.97.1.

Shared crates are internal until their APIs stabilize. Plugin crates are
independent release units and may mature at different rates. Atomic workspace
changes are allowed, but a release must state exactly which plugin artifacts
were built and tested.

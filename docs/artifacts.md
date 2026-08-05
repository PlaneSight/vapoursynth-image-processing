# Artifact lifecycle

| Path    | Producer              | Class         | Tracked | Cleanup                | CI policy                 |
| ------- | --------------------- | ------------- | ------- | ---------------------- | ------------------------- |
| target/ | Cargo                 | Build/cache   | No      | cargo clean            | Never archive wholesale   |
| dist/   | Release tooling       | Distributable | No      | cargo xtask clean-dist | Upload selected artifacts |
| site/   | Documentation tooling | Rendered docs | No      | cargo xtask clean-site | Deploy, never commit      |
| .tmp/   | Tests and tools       | Ephemeral     | No      | cargo xtask clean-tmp  | Must be disposable        |
| .cache/ | Explicit local tools  | Cache         | No      | tool-specific          | Must not be required      |
| Cargo.lock | Cargo resolver     | Resolution    | Yes     | Cargo update           | Pin reviewed workspace graph |

Authored source is never removed by cleanup commands. Generated source, if
introduced, must identify its generator, inputs and deterministic drift check.
Build scripts may write only beneath Cargo's OUT_DIR and may not access the
network.

The workspace lockfile is tracked because the repository builds loadable
plugin artifacts and pins a Git dependency. Library packages still declare
semver-compatible dependency requirements for downstream resolution.

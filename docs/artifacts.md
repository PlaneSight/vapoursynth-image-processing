# Artifact lifecycle

| Path | Producer | Class | Tracked | Cleanup | CI policy |
| --- | --- | --- | --- | --- | --- |
| target/ | Cargo | Build/cache | No | `cargo clean` | Never archive wholesale |
| dist/ | uv | Python distributions | No | `uv run vsip clean dist` | Validate before selected release upload |
| site/ | Zensical | Rendered docs | No | `uv run vsip clean site` | Build strictly; deploy, never commit |
| .venv/ | uv | Python environment | No | Remove directory | Recreate from `uv.lock` |
| .ruff_cache/ | Ruff | Cache | No | Remove directory | Never required |
| .tmp/ | Tests and tools | Ephemeral | No | `uv run vsip clean tmp` | Must be disposable |
| .cache/ | Explicit local tools | Cache | No | Tool-specific | Must not be required |
| Cargo.lock | Cargo resolver | Resolution | Yes | `cargo update` | Pin reviewed Rust graph |
| uv.lock | uv resolver | Resolution | Yes | `uv lock` | Pin reviewed Python 3.14 graph |

Authored source is never removed by cleanup commands. Generated source, if
introduced, must identify its generator, inputs and deterministic drift check.
Build scripts may write only beneath Cargo's OUT_DIR and may not access the
network.

The workspace lockfile is tracked because the repository builds loadable
plugin artifacts and pins a Git dependency. Library packages still declare
semver-compatible dependency requirements for downstream resolution.

The uv lockfile is likewise tracked because the installable Python tooling and
Zensical site are project infrastructure. Change Python requirements through
uv, validate with `uv lock --check`, and never edit `uv.lock` by hand.

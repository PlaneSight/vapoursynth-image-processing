# Getting started

This guide creates a reproducible development environment, verifies the
workspace, and builds a native plugin. Python tools run under uv-managed Python
3.14; Rust remains managed by `rustup` and the checked-in toolchain file.

## Prerequisites

Install these once:

- [Git](https://git-scm.com/)
- [uv](https://docs.astral.sh/uv/getting-started/installation/)
- [rustup](https://rustup.rs/)

You do not need to install Python separately. uv downloads and selects Python
3.14 for this project. A VapourSynth R4 host is optional unless you intend to
run the native plugin smoke tests.

Check that the host tools are available:

```console
git --version
uv --version
rustup --version
```

## Create the environment

Clone the repository and enter it:

```console
git clone https://github.com/PlaneSight/vapoursynth-image-processing.git
cd vapoursynth-image-processing
```

Ask uv to install Python 3.14, then reproduce the locked project environment:

```console
uv python install 3.14
uv sync --python 3.14 --locked
```

`uv sync` creates `.venv` when needed and installs the project, its CLI, and
the locked documentation and development dependencies. There is no need to
activate the environment: prefix project commands with `uv run`.

Confirm the selected interpreter and discover the CLI:

```console
uv run python --version
uv run vsip info
uv run vsip --help
```

The Python version should begin with `3.14`. `vsip info` reports the project
root, Python and uv versions, pinned Rust toolchain, and optional host tools,
which makes it the quickest environment diagnostic.

## Verify the checkout

Run the project's static gate through the unified CLI:

```console
uv run vsip check
```

The command verifies the Python lock, formatting, linting and CLI tests, then
runs repository conformance, Rust formatting, compilation, strict Clippy,
debug tests, Rust documentation, and a strict Zensical build. It does not
require a system VapourSynth installation.

For a quicker edit loop, skip Rust tests and both documentation builds while
retaining Python tests, formatting, compilation, and strict Clippy:

```console
uv run vsip check --quick
uv run vsip check --help
```

Use `uv run vsip check --msrv` to exercise only the minimum supported Rust
versions. The `--quick` and `--msrv` modes are deliberately mutually exclusive;
run plain `uv run vsip check` before sending a change.

Before opening a pull request, reproduce the complete non-runtime CI gate:

```console
uv run --locked vsip ci
```

This adds every Rust test profile, split-MSRV checks, and a production
documentation build. VapourSynth host execution remains an explicit smoke step
because it requires `vspipe`.

## Build the native plugins

Use the CLI instead of remembering Cargo package names or platform library
suffixes:

```console
uv run vsip build --release
```

Release libraries are written under `target/release`. VapourSynth can load a
library from there directly, or it can be copied into a configured plugin
directory.

See the [CLI reference](cli.md) for selecting individual families and the
[VapourSynth runtime guide](vapoursynth.md) for format contracts and manual
loading examples.

## Preview the documentation

The same interface runs the pinned Zensical installation:

```console
uv run vsip docs serve
```

Open <http://127.0.0.1:8000/>. The preview rebuilds as Markdown files change.
To create the static site without starting a server:

```console
uv run vsip docs build
```

The generated site is written to `site/`.

## Optional runtime smoke test

An installed VapourSynth host provides `vspipe`. After building release
adapters, run both smoke scripts:

```console
uv run vsip smoke all
```

By default the command builds the release workspace, finds libraries in
`target/release`, and derives the platform-specific MaskLab filename. Pass
`--no-build` when those artifacts are already current, or see
`uv run vsip smoke --help` for explicit path overrides.

Runtime testing is kept separate from the ordinary Rust gate so contributors
can work on pure kernels without installing the host application. The smoke
tests load the built libraries, confirm every catalogue function is visible,
and execute a deterministic MaskLab frame.

## Next steps

- Browse all namespaces in the [filter catalogue](filter-catalog.md).
- Learn the complete command surface in the [CLI reference](cli.md).
- Read the [architecture](architecture.md) before changing crate boundaries.
- Follow the [development guide](development.md) before opening a pull request.

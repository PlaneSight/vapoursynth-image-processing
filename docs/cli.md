# Command-line interface

The `vsip` command is the supported front door for repository maintenance. It
keeps the same command spelling on Windows, Linux, and macOS and delegates to
Cargo, Zensical, and `vspipe` without requiring shell-specific scripts or
environment-variable syntax.

Run it from the repository root through the project uv environment:

```text
uv run vsip --help
uv run vsip <command> --help
```

For a non-interactive job that must reject lockfile drift, add uv's `--locked`
flag before the command name:

```text
uv run --locked vsip ci
```

## Command summary

| Command | Purpose |
| --- | --- |
| `info` | Report project, Python, uv, Rust, Cargo, and optional VapourSynth tooling. |
| `fmt` | Format Rust and Python sources, or verify formatting with `--check`. |
| `check` | Run repository conformance and static checks. The default is the full check set. |
| `test` | Run Rust tests under one or every supported Cargo test profile. |
| `build` | Build the workspace or selected Cargo packages. |
| `package` | Build the Python source distribution and wheel with uv. |
| `inventory` | List the twelve native plugin namespaces. |
| `clean dist\|site\|tmp` | Remove one declared generated-artifact directory. |
| `docs build` | Build the Zensical documentation site into `site/`. |
| `docs serve` | Serve the documentation locally with live rebuilding. |
| `smoke catalog` | Load all adapter libraries and verify their registered functions. |
| `smoke masklab` | Execute the deterministic MaskLab frame-processing smoke test. |
| `smoke all` | Run both VapourSynth smoke tests. |
| `ci` | Run the reproducible local CI gate. |

Every command exits nonzero as soon as an underlying tool fails. The failing
command is printed, so the same operation can be investigated directly when
needed.

## Inspect the environment

```text
uv run vsip info
```

`info` is read-only. Use it first when diagnosing a machine: it shows the
Python selected by uv, the repository's pinned Rust toolchain, availability of
the two MSRV toolchains, and whether `vspipe` can be found. VapourSynth is
optional for ordinary builds and tests.

## Format source

Format Rust and Python sources with one command:

```text
uv run vsip fmt
```

Use check mode when a job must detect drift without changing the checkout:

```text
uv run vsip fmt --check
```

The command coordinates rustfmt and Ruff. Documentation Markdown remains
authored text and is validated as part of the Zensical build.

## Check source and compatibility

The default invocation runs the complete static check set:

```text
uv run vsip check
```

This covers lockfile drift, Python formatting and linting, Python CLI tests,
repository-tree conformance, Rust formatting, a full-workspace Cargo check,
strict Clippy, debug Rust tests, Rust documentation, and a strict Zensical
production build on the pinned development toolchain.

Use the quick mode while editing:

```text
uv run vsip check --quick
```

Quick mode retains lockfile, Python test/lint, structural, formatting,
compilation, and strict Clippy feedback while omitting Rust tests and both
documentation builds. It is not a release gate.

Use the MSRV mode to exercise the compatibility split explicitly:

```text
uv run vsip check --msrv
```

The MSRV check compiles and tests the runtime-independent crates and plugin
families with Rust 1.85, then checks the complete adapter graph with Rust 1.88.
`--quick` and `--msrv` select different check modes and cannot be combined.

## Run tests

Choose one test profile with `--profile`:

```text
uv run vsip test --profile debug
uv run vsip test --profile release
uv run vsip test --profile no-default
```

The default profile is `all`, so omitting the option exercises every profile.
`no-default` tests the workspace with Cargo default features disabled:

```text
uv run vsip test
uv run vsip test --profile all
```

These are Rust unit and integration tests. They do not require an installed
VapourSynth host; host-level execution is kept in the explicit `smoke`
commands.

## Build adapters or packages

With no package selection, `build` builds the workspace in Cargo's development
profile:

```text
uv run vsip build
```

Use `--release` for loadable adapter libraries. Repeat `--package` to build a
specific set of Cargo packages without relying on shell quoting or lists:

```text
uv run vsip build --release --package vs-masklab-vapoursynth
uv run vsip build --release --package vs-masklab-vapoursynth --package vs-flowfield-vapoursynth
```

Package values are Cargo package names, not directory names. Build output stays
under `target/`; the command does not install or copy plugins into a
VapourSynth installation.

## Build the Python package

```text
uv run vsip package
```

`package` asks uv to build both the Python source distribution and wheel into
`dist/`. Those archives contain the repository's Python development tooling and
its `vsip` console entry point; they are distinct from native VapourSynth
adapter libraries produced by `build --release`. Generated `dist/` contents are
untracked and should be published only through an explicit release process.

## Inspect namespaces and clean generated outputs

List the plugin namespaces from the repository's authoritative inventory:

```text
uv run vsip inventory
```

Cleanup is deliberately limited to the three generated directories owned by
repository tooling:

```text
uv run vsip clean dist
uv run vsip clean site
uv run vsip clean tmp
```

These map to `dist/`, `site/`, and `.tmp/`. The command cannot target source,
the workspace root, Cargo's `target/`, or an arbitrary path.

## Build and serve documentation

Documentation is rendered by Zensical through the same uv environment:

```text
uv run vsip docs build
uv run vsip docs serve
```

`docs build` is the deterministic CI operation and writes the generated site to
`site/`. `docs serve` starts Zensical's local preview server and remains active
until interrupted. Edit the Markdown sources rather than generated files in
`site/`.

## Run VapourSynth smoke tests

Ensure an R4-compatible `vspipe` is available. By default, each smoke command
first builds the release workspace, then looks for adapters under
`target/release`. The shortest catalogue invocation is therefore:

```text
uv run vsip smoke catalog
```

Override the release directory when validating staged libraries:

```text
uv run vsip smoke catalog --plugin-dir staging/plugins
```

The MaskLab execution test derives the platform library name from the plugin
directory. It can also take an exact library path:

```text
uv run vsip smoke masklab
uv run vsip smoke masklab --masklab-plugin staging/vs_masklab_vapoursynth.dll
```

Use the native library filename on the current platform:

| Platform | MaskLab release library |
| --- | --- |
| Windows | `target/release/vs_masklab_vapoursynth.dll` |
| Linux | `target/release/libvs_masklab_vapoursynth.so` |
| macOS | `target/release/libvs_masklab_vapoursynth.dylib` |

`--plugin-dir` and `--masklab-plugin` are ordinary path arguments. The CLI
resolves them to absolute paths and supplies the smoke scripts' environment, so
PowerShell, POSIX shell, and command-prompt syntax never leaks into the user
interface.

Run both smoke tests together. An explicit MaskLab path is useful for a staged
or renamed artifact; otherwise the CLI derives its platform filename from the
directory:

```text
uv run vsip smoke all
uv run vsip smoke all --plugin-dir target/release
uv run vsip smoke all --plugin-dir target/release --masklab-plugin staging/vs_masklab_vapoursynth.dll
```

When the release artifacts are already available, skip the automatic Cargo
build explicitly:

```text
uv run vsip smoke all --no-build --plugin-dir target/release
```

The command validates required paths before invoking `vspipe`. `--no-build`
never accepts missing inputs or falls back to an implicit build.

## Reproduce CI locally

```text
uv run --locked vsip ci
```

`ci` composes the full static checks, every Rust test profile, MSRV checks, a
Zensical production build, and the Python source-distribution and wheel build.
It intentionally excludes VapourSynth runtime smoke tests because those require
a separately installed host. Run `smoke all` when producing or validating
native plugin artifacts.

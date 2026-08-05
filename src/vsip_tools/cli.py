"""Unified, cross-platform command line interface for the VSIP workspace."""

from __future__ import annotations

import argparse
import os
import shlex
import shutil
import subprocess
import sys
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path

from vsip_tools import __version__

type Command = tuple[str, ...]

PURE_PACKAGES = (
    "vsip-core",
    "vsip-kernels",
    "vsip-plugin-api",
    "vsip-test-support",
    "vs-masklab",
    "vs-defect",
    "vs-deconvolve",
    "vs-flowfield",
    "vs-grainlab",
    "vs-phase",
    "vs-edgeaware",
    "vs-register",
    "vs-lens",
    "vs-tonelab",
    "vs-residual",
    "vs-segment",
    "xtask",
)


class _CommandFailed(RuntimeError):
    def __init__(self, command: Command, return_code: int) -> None:
        self.command = command
        self.return_code = return_code
        super().__init__(f"command failed with exit code {return_code}: {_display(command)}")


@dataclass(frozen=True)
class _Runner:
    root: Path

    def run(self, command: Command, *, env: Mapping[str, str] | None = None) -> None:
        print(f"+ {_display(command)}", flush=True)
        completed = subprocess.run(command, cwd=self.root, env=env, check=False)
        if completed.returncode != 0:
            raise _CommandFailed(command, completed.returncode)

    def run_all(self, commands: Sequence[Command]) -> None:
        for command in commands:
            self.run(command)


def _display(command: Command) -> str:
    if os.name == "nt":
        return subprocess.list2cmdline(command)
    return shlex.join(command)


def _discover_root(start: Path) -> Path:
    override = os.environ.get("VSIP_ROOT")
    if override:
        candidates = (Path(override).expanduser(),)
    else:
        resolved = start.resolve()
        candidates = (resolved, *resolved.parents)

    for candidate in candidates:
        if (candidate / "Cargo.toml").is_file() and (candidate / "pyproject.toml").is_file():
            return candidate.resolve()
    raise FileNotFoundError(
        "could not find the VSIP workspace; run inside the repository or set VSIP_ROOT"
    )


def _python_check_commands() -> tuple[Command, ...]:
    return (
        ("uv", "lock", "--check"),
        ("ruff", "format", "--check", "."),
        ("ruff", "check", "."),
        (sys.executable, "-m", "unittest", "discover", "-s", "tests/python", "-v"),
    )


def _quick_check_commands() -> tuple[Command, ...]:
    return (
        *_python_check_commands(),
        ("cargo", "xtask", "check-tree"),
        ("cargo", "fmt", "--all", "--check"),
        ("cargo", "check", "--workspace", "--all-targets"),
        ("cargo", "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"),
    )


def _full_check_commands() -> tuple[Command, ...]:
    return (
        *_quick_check_commands(),
        ("cargo", "test", "--workspace"),
        ("cargo", "doc", "--workspace", "--no-deps"),
        ("zensical", "build", "--strict"),
    )


def _package_arguments(packages: Sequence[str]) -> tuple[str, ...]:
    return tuple(part for package in packages for part in ("-p", package))


def _msrv_commands() -> tuple[Command, ...]:
    packages = _package_arguments(PURE_PACKAGES)
    return (
        ("cargo", "+1.85.0", "check", "--all-targets", *packages),
        ("cargo", "+1.85.0", "test", *packages),
        ("cargo", "+1.88.0", "check", "--workspace", "--all-targets"),
    )


def _test_commands(profile: str) -> tuple[Command, ...]:
    commands: dict[str, tuple[Command, ...]] = {
        "debug": (("cargo", "test", "--workspace"),),
        "release": (("cargo", "test", "--workspace", "--release"),),
        "no-default": (("cargo", "test", "--workspace", "--no-default-features"),),
    }
    if profile == "all":
        return commands["debug"] + commands["release"] + commands["no-default"]
    return commands[profile]


def _build_command(*, release: bool, packages: Sequence[str]) -> Command:
    command = ["cargo", "build"]
    if packages:
        command.extend(_package_arguments(packages))
    else:
        command.append("--workspace")
    if release:
        command.append("--release")
    return tuple(command)


def _library_filename(namespace: str, platform: str | None = None) -> str:
    platform = platform or sys.platform
    stem = f"vs_{namespace}_vapoursynth"
    if platform == "win32":
        return f"{stem}.dll"
    if platform == "darwin":
        return f"lib{stem}.dylib"
    return f"lib{stem}.so"


def _resolve_workspace_path(root: Path, value: Path) -> Path:
    expanded = value.expanduser()
    if not expanded.is_absolute():
        expanded = root / expanded
    return expanded.resolve()


def _require_path(path: Path, description: str) -> None:
    if not path.exists():
        raise FileNotFoundError(f"{description} does not exist: {path}")


def _run_smoke(runner: _Runner, args: argparse.Namespace) -> None:
    if shutil.which("vspipe") is None:
        raise FileNotFoundError("vspipe is required for VapourSynth smoke tests")

    if not args.no_build:
        packages = ("vs-masklab-vapoursynth",) if args.mode == "masklab" else ()
        runner.run(_build_command(release=True, packages=packages))

    plugin_dir = _resolve_workspace_path(runner.root, args.plugin_dir)
    masklab = args.masklab_plugin or plugin_dir / _library_filename("masklab")
    masklab = _resolve_workspace_path(runner.root, masklab)
    environment = os.environ.copy()

    if args.mode in {"catalog", "all"}:
        _require_path(plugin_dir, "plugin directory")
        environment["VSIP_PLUGIN_DIR"] = str(plugin_dir)
        runner.run(
            ("vspipe", "--info", "tests/vapoursynth/catalog_smoke.vpy", "-"),
            env=environment,
        )

    if args.mode in {"masklab", "all"}:
        _require_path(masklab, "MaskLab plugin")
        environment["VSIP_MASKLAB_PLUGIN"] = str(masklab)
        runner.run(
            ("vspipe", "--info", "tests/vapoursynth/masklab_smoke.vpy", "-"),
            env=environment,
        )


def _version_output(executable: str, *arguments: str) -> str:
    if shutil.which(executable) is None:
        return "not found"
    completed = subprocess.run(
        (executable, *arguments),
        check=False,
        capture_output=True,
        text=True,
    )
    output = completed.stdout or completed.stderr
    lines = [line.strip() for line in output.splitlines() if line.strip()]
    return lines[0] if lines else f"exit code {completed.returncode}"


def _rust_toolchain_status(version: str) -> str:
    if shutil.which("rustup") is None:
        return "rustup not found"
    completed = subprocess.run(
        ("rustup", "toolchain", "list"),
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        return f"unavailable (rustup exit code {completed.returncode})"
    installed = any(
        line == version or line.startswith((f"{version}-", f"{version} "))
        for line in completed.stdout.splitlines()
    )
    return "installed" if installed else "not installed"


def _show_info(root: Path) -> None:
    print(f"vsip tools {__version__}")
    print(f"workspace: {root}")
    print(f"python: {sys.version.split()[0]} ({sys.executable})")
    print(f"uv: {_version_output('uv', '--version')}")
    print(f"rustc (workspace): {_version_output('rustc', '--version')}")
    print(f"Rust 1.85.0 pure MSRV: {_rust_toolchain_status('1.85.0')}")
    print(f"Rust 1.88.0 adapter MSRV: {_rust_toolchain_status('1.88.0')}")
    print(f"cargo: {_version_output('cargo', '--version')}")
    print(f"vspipe: {_version_output('vspipe', '--version')}")


def _add_smoke_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("mode", choices=("catalog", "masklab", "all"))
    parser.add_argument(
        "--plugin-dir",
        type=Path,
        default=Path("target/release"),
        help="directory containing all adapter libraries (default: target/release)",
    )
    parser.add_argument(
        "--masklab-plugin",
        type=Path,
        help="MaskLab library path; defaults to the platform library in --plugin-dir",
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="use existing release libraries instead of building first",
    )


def build_parser() -> argparse.ArgumentParser:
    """Build the public command-line parser."""
    parser = argparse.ArgumentParser(
        prog="vsip",
        description="Unified development interface for VapourSynth Image Processing",
    )
    parser.add_argument("--version", action="version", version=f"%(prog)s {__version__}")
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser("info", help="show workspace and toolchain versions")

    format_parser = subparsers.add_parser("fmt", help="format Rust and Python sources")
    format_parser.add_argument("--check", action="store_true", help="verify without writing")

    check_parser = subparsers.add_parser("check", help="run repository quality gates")
    check_mode = check_parser.add_mutually_exclusive_group()
    check_mode.add_argument("--quick", action="store_true", help="skip Rust tests and docs")
    check_mode.add_argument("--msrv", action="store_true", help="run only the MSRV gates")

    test_parser = subparsers.add_parser("test", help="run Rust workspace tests")
    test_parser.add_argument(
        "--profile",
        choices=("debug", "release", "no-default", "all"),
        default="all",
        help="test configuration (default: all)",
    )

    build = subparsers.add_parser("build", help="build native Rust crates and adapters")
    build.add_argument("--release", action="store_true", help="build optimized artifacts")
    build.add_argument(
        "--package",
        action="append",
        default=[],
        metavar="NAME",
        help="build one Cargo package; repeat to select multiple packages",
    )

    subparsers.add_parser("package", help="build the Python sdist and wheel with uv")

    subparsers.add_parser("inventory", help="list the twelve plugin namespaces")

    clean = subparsers.add_parser("clean", help="remove one generated artifact directory")
    clean.add_argument("artifact", choices=("dist", "site", "tmp"))

    docs = subparsers.add_parser("docs", help="build or serve the Zensical site")
    docs.add_argument("action", choices=("build", "serve"))

    smoke = subparsers.add_parser("smoke", help="run live VapourSynth host tests")
    _add_smoke_arguments(smoke)

    subparsers.add_parser("ci", help="run every local continuous-integration gate")
    return parser


def _dispatch(runner: _Runner, args: argparse.Namespace) -> None:
    match args.command:
        case "info":
            _show_info(runner.root)
        case "fmt":
            suffix = ("--check",) if args.check else ()
            runner.run(("cargo", "fmt", "--all", *suffix))
            runner.run(("ruff", "format", *suffix, "."))
        case "check":
            if args.msrv:
                runner.run_all(_msrv_commands())
            elif args.quick:
                runner.run_all(_quick_check_commands())
            else:
                runner.run_all(_full_check_commands())
        case "test":
            runner.run_all(_test_commands(args.profile))
        case "build":
            runner.run(_build_command(release=args.release, packages=args.package))
        case "package":
            runner.run(("uv", "build"))
        case "inventory":
            runner.run(("cargo", "xtask", "inventory"))
        case "clean":
            runner.run(("cargo", "xtask", f"clean-{args.artifact}"))
        case "docs":
            suffix = ("--strict",) if args.action == "build" else ()
            runner.run(("zensical", args.action, *suffix))
        case "smoke":
            _run_smoke(runner, args)
        case "ci":
            runner.run_all(_full_check_commands())
            runner.run_all(_test_commands("release") + _test_commands("no-default"))
            runner.run_all(_msrv_commands())
            runner.run(("uv", "build"))
        case _:
            raise AssertionError(f"unhandled command: {args.command}")


def main(argv: Sequence[str] | None = None) -> int:
    """Run the VSIP command-line interface and return a process exit code."""
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        root = _discover_root(Path.cwd())
        _dispatch(_Runner(root), args)
    except _CommandFailed as error:
        return error.return_code
    except (FileNotFoundError, KeyError, ValueError) as error:
        parser.exit(2, f"vsip: error: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

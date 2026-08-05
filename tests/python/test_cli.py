from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path
from subprocess import CompletedProcess
from unittest.mock import patch

from vsip_tools import cli


class ParserTests(unittest.TestCase):
    def test_test_defaults_to_all_profiles(self) -> None:
        arguments = cli.build_parser().parse_args(["test"])
        self.assertEqual(arguments.profile, "all")

    def test_quick_and_msrv_are_mutually_exclusive(self) -> None:
        with self.assertRaises(SystemExit):
            cli.build_parser().parse_args(["check", "--quick", "--msrv"])

    def test_smoke_paths_have_workspace_defaults(self) -> None:
        arguments = cli.build_parser().parse_args(["smoke", "all"])
        self.assertEqual(arguments.plugin_dir, Path("target/release"))
        self.assertIsNone(arguments.masklab_plugin)
        self.assertFalse(arguments.no_build)

    def test_cleanup_accepts_only_declared_generated_artifacts(self) -> None:
        arguments = cli.build_parser().parse_args(["clean", "site"])
        self.assertEqual(arguments.artifact, "site")
        with self.assertRaises(SystemExit):
            cli.build_parser().parse_args(["clean", "target"])


class CommandConstructionTests(unittest.TestCase):
    def test_full_check_extends_quick_check(self) -> None:
        quick = cli._quick_check_commands()
        full = cli._full_check_commands()
        self.assertEqual(full[: len(quick)], quick)
        self.assertIn(("zensical", "build", "--strict"), full)

    def test_all_profiles_have_three_distinct_cargo_invocations(self) -> None:
        commands = cli._test_commands("all")
        self.assertEqual(len(commands), 3)
        self.assertEqual(commands[0], ("cargo", "test", "--workspace"))
        self.assertIn("--release", commands[1])
        self.assertIn("--no-default-features", commands[2])

    def test_selected_build_packages_do_not_build_workspace(self) -> None:
        command = cli._build_command(release=True, packages=("first", "second"))
        self.assertEqual(
            command,
            ("cargo", "build", "-p", "first", "-p", "second", "--release"),
        )

    def test_default_build_targets_workspace(self) -> None:
        self.assertEqual(
            cli._build_command(release=False, packages=()),
            ("cargo", "build", "--workspace"),
        )

    def test_msrv_contracts_use_both_declared_toolchains(self) -> None:
        commands = cli._msrv_commands()
        self.assertEqual(commands[0][:3], ("cargo", "+1.85.0", "check"))
        self.assertEqual(commands[-1][:3], ("cargo", "+1.88.0", "check"))
        for package in cli.PURE_PACKAGES:
            self.assertIn(package, commands[0])


class PlatformTests(unittest.TestCase):
    def test_library_names_are_platform_native(self) -> None:
        self.assertEqual(cli._library_filename("masklab", "win32"), "vs_masklab_vapoursynth.dll")
        self.assertEqual(
            cli._library_filename("masklab", "darwin"),
            "libvs_masklab_vapoursynth.dylib",
        )
        self.assertEqual(cli._library_filename("masklab", "linux"), "libvs_masklab_vapoursynth.so")

    def test_toolchain_status_is_read_from_rustup_without_installing(self) -> None:
        completed = CompletedProcess(
            args=("rustup", "toolchain", "list"),
            returncode=0,
            stdout="1.85.0-x86_64-pc-windows-msvc\nstable-x86_64-pc-windows-msvc\n",
            stderr="",
        )
        with (
            patch("vsip_tools.cli.shutil.which", return_value="rustup"),
            patch("vsip_tools.cli.subprocess.run", return_value=completed),
        ):
            self.assertEqual(cli._rust_toolchain_status("1.85.0"), "installed")
            self.assertEqual(cli._rust_toolchain_status("1.88.0"), "not installed")

    def test_relative_workspace_path_is_resolved_from_root(self) -> None:
        root = Path.cwd().resolve()
        self.assertEqual(
            cli._resolve_workspace_path(root, Path("target/release")),
            (root / "target/release").resolve(),
        )


class RootDiscoveryTests(unittest.TestCase):
    def test_discovers_workspace_from_a_descendant(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").touch()
            (root / "pyproject.toml").touch()
            descendant = root / "nested" / "directory"
            descendant.mkdir(parents=True)
            self.assertEqual(cli._discover_root(descendant), root.resolve())

    def test_environment_override_is_authoritative(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").touch()
            (root / "pyproject.toml").touch()
            with patch.dict(os.environ, {"VSIP_ROOT": str(root)}):
                self.assertEqual(cli._discover_root(Path.cwd()), root.resolve())

    def test_invalid_environment_override_fails_clearly(self) -> None:
        with (
            tempfile.TemporaryDirectory() as directory,
            patch.dict(os.environ, {"VSIP_ROOT": directory}),
            self.assertRaises(FileNotFoundError),
        ):
            cli._discover_root(Path.cwd())


if __name__ == "__main__":
    unittest.main()

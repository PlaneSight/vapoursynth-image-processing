from __future__ import annotations

import tomllib
import unittest
from pathlib import Path

from vsip_tools import __version__


class PackagingMetadataTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        with Path("pyproject.toml").open("rb") as project_file:
            cls.metadata = tomllib.load(project_file)

    def test_package_and_module_versions_match(self) -> None:
        self.assertEqual(self.metadata["project"]["version"], __version__)

    def test_python_series_is_exactly_314(self) -> None:
        self.assertEqual(self.metadata["project"]["requires-python"], ">=3.14,<3.15")
        self.assertEqual(Path(".python-version").read_text(encoding="utf-8").strip(), "3.14")

    def test_console_entry_point_is_declared(self) -> None:
        self.assertEqual(self.metadata["project"]["scripts"]["vsip"], "vsip_tools.cli:main")

    def test_dual_license_files_are_packaged(self) -> None:
        project = self.metadata["project"]
        self.assertEqual(project["license"], "MIT OR Apache-2.0")
        self.assertEqual(set(project["license-files"]), {"LICENSE-MIT", "LICENSE-APACHE"})


if __name__ == "__main__":
    unittest.main()

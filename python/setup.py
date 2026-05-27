from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

from setuptools import setup
from setuptools.command.build_py import build_py
from setuptools.command.install import install
from wheel.bdist_wheel import bdist_wheel


ROOT = Path(__file__).resolve().parents[1]


def _library_filename() -> str:
    if sys.platform == "win32":
        return "callbook.dll"
    if sys.platform == "darwin":
        return "libcallbook.dylib"
    return "libcallbook.so"


def _cargo_target_dir(env: dict[str, str]) -> Path:
    if "CARGO_TARGET_DIR" in env:
        return Path(env["CARGO_TARGET_DIR"])
    metadata = subprocess.run(
        [
            "cargo",
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
            str(ROOT / "Cargo.toml"),
        ],
        check=True,
        cwd=ROOT,
        env=env,
        text=True,
        capture_output=True,
    )
    return Path(json.loads(metadata.stdout)["target_directory"])


class BuildPy(build_py):
    def run(self) -> None:
        super().run()
        self._build_and_copy_native_library()

    def _build_and_copy_native_library(self) -> None:
        env = os.environ.copy()
        manifest = ROOT / "Cargo.toml"
        subprocess.run(
            [
                "cargo",
                "build",
                "--release",
                "--manifest-path",
                str(manifest),
                "-p",
                "callbook-rs",
            ],
            check=True,
            cwd=ROOT,
            env=env,
        )

        target_dir = _cargo_target_dir(env)
        source = target_dir / "release" / _library_filename()
        if not source.exists():
            raise RuntimeError(f"Rust shared library was not built: {source}")

        package_lib = Path(self.build_lib) / "callbook_rs" / "lib"
        package_lib.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, package_lib / source.name)


class BdistWheel(bdist_wheel):
    def finalize_options(self) -> None:
        super().finalize_options()
        self.root_is_pure = False


class Install(install):
    def finalize_options(self) -> None:
        super().finalize_options()
        self.install_lib = self.install_platlib


setup(cmdclass={"build_py": BuildPy, "bdist_wheel": BdistWheel, "install": Install})

# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import sys
from collections.abc import Sequence
from pathlib import Path

_FUCHSIA_DIR = Path(__file__).parent.parent.parent.parent
sys.path.insert(0, str(_FUCHSIA_DIR / "build/bazel/scripts"))
import build_utils

_PREBUILT_GN = _FUCHSIA_DIR / "prebuilt/third_party/gn/linux-x64/gn"


class GnRunner(object):
    """Wrapper class to invoke GN."""

    def __init__(
        self,
        build_dir: Path | None = None,
        gn: Path = _PREBUILT_GN,
        command_runner: build_utils.CommandRunner | None = None,
    ):
        """Create instance.

        Args:
            build_dir: Path to GN build directory.
            gn: Path to GN binary. Default to Fuchsia's prebuilt GN.
            command_runner: Optional CommandRunner instance. If None, a default instance will be created.
        """
        self._gn = gn
        if build_dir is None:
            build_dir = build_utils.find_fx_build_dir(_FUCHSIA_DIR)
        if build_dir is None:
            raise RuntimeError("Could not find GN build directory")
        self._build_dir = build_dir
        self._cmd_runner = (
            command_runner if command_runner else build_utils.CommandRunner()
        )

    @property
    def gn(self) -> Path:
        return self._gn

    @property
    def build_dir(self) -> Path:
        return self._build_dir

    def run_and_extract_output(self, cmd: list[str]) -> str:
        """Run a given GN command and return its output.

        Args:
            cmd: list of GN options.
                 This method inserts <build_dir> as the second argument (after the subcommand).
                 Example: if cmd is ["desc", "//:default"], the actual command will be
                 ["gn", "desc", "path/to/build_dir", "//:default"].

        Returns:
           The command's stdout in case of success. stderr is captured but never returned
           unless the command fails (in which case it will be available from the corresponding
           exception object).

        Raises:
           RuntimeError if the command failed.
        """
        if not cmd:
            raise ValueError("Command list cannot be empty")

        ret = self._cmd_runner.run_command(
            [str(self.gn), cmd[0], str(self.build_dir)] + cmd[1:],
            **self._cmd_runner.CAPTURE_KWARGS,
        )
        if ret.returncode != 0:
            print("==== GN command failed ===")
            print("Command: " + build_utils.cmd_args_to_string(ret.args))
            print("stdout: " + ret.stdout)
            print("stderr: " + ret.stderr)
            raise RuntimeError()
        return ret.stdout


class MockGnRunner(GnRunner):
    """A mock GnRunner instance that can be used in tests."""

    def __init__(self, build_dir: Path, mock_output: str):
        self._mock_runner = build_utils.MockCommandRunner()
        super().__init__(build_dir, Path("gn"), self._mock_runner)
        self._mock_runner.push_result(0, mock_output, "")

    def last_gn_args(self) -> Sequence[str | Path]:
        """Return the original cmd passed to run_and_extract_output."""
        last_args = self._mock_runner.results[-1].args
        assert last_args[0] == str(self._gn)
        assert last_args[2] == str(self.build_dir)
        # args were [gn, cmd[0], build_dir, cmd[1], ...], so we want [cmd[0], cmd[1], ...]
        return [last_args[1]] + last_args[3:]

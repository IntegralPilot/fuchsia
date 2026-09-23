# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Hermetic base test case providing tempdir, stream capture, and test utilities."""

from __future__ import annotations

import io
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import Any
from unittest import mock


class BaseTestCase(unittest.TestCase):
    """Base test class providing isolated temporary directory and stream capture."""

    def setUp(self) -> None:
        super().setUp()
        self.temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp_dir.cleanup)
        self.test_dir = Path(self.temp_dir.name)

        self._stdout_patch = mock.patch("sys.stdout", new_callable=io.StringIO)
        self.mock_stdout = self._stdout_patch.start()
        self.addCleanup(self._stdout_patch.stop)

        self._stderr_patch = mock.patch("sys.stderr", new_callable=io.StringIO)
        self.mock_stderr = self._stderr_patch.start()
        self.addCleanup(self._stderr_patch.stop)

        # Ensure tests run in a hermetic human baseline by stripping ambient agent env vars.
        self.enterContext(mock.patch.dict(os.environ))
        for var in (
            "ANTIGRAVITY_AGENT",
            "GEMINI_CLI",
            "ANTIGRAVITY_EDITOR_APP_ROOT",
        ):
            os.environ.pop(var, None)

    @property
    def stdout(self) -> str:
        """Returns captured standard output as a string."""
        return self.mock_stdout.getvalue()

    @property
    def stderr(self) -> str:
        """Returns captured standard error as a string."""
        return self.mock_stderr.getvalue()

    def write_file(self, rel_path: str | Path, content: str = "") -> Path:
        """Helper to create files relative to test_dir."""
        path = self.test_dir / rel_path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        return path

    def patch_object(
        self, target: Any, attribute: str, *args: Any, **kwargs: Any
    ) -> Any:
        """Helper to mock an object attribute and register automatic cleanup."""
        p = mock.patch.object(target, attribute, *args, **kwargs)
        mocked = p.start()
        self.addCleanup(p.stop)
        return mocked

    def patch_environ(self, clear: bool = False, **kwargs: str) -> None:
        """Helper to safely patch os.environ keys and register automatic cleanup."""
        patcher = mock.patch.dict(os.environ, kwargs, clear=clear)
        patcher.start()
        self.addCleanup(patcher.stop)

    def make_completed_process(
        self,
        returncode: int = 0,
        stdout: str = "",
        stderr: str = "",
        args: list[str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        """Factory helper to construct a subprocess.CompletedProcess."""
        return subprocess.CompletedProcess(
            args=args or [],
            returncode=returncode,
            stdout=stdout,
            stderr=stderr,
        )

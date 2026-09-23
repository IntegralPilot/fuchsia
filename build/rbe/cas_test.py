#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest
from pathlib import Path
from unittest import mock

import cas
import cl_utils


class FileDownloadTests(unittest.TestCase):
    def test_success(self) -> None:
        file = cas.File(
            instance="projects/p/instance/foo",
            digest="123feedface9812300/99",
            filename=Path("interesting.log"),
        )
        output = Path("/path/to/my-copy-of-interesting.log")
        with mock.patch.object(
            cl_utils,
            "subprocess_call",
            return_value=cl_utils.SubprocessResult(0),
        ) as mock_call:
            with mock.patch.object(Path, "rename") as mock_rename:
                result = file.download(output)

        self.assertEqual(result.returncode, 0)
        mock_call.assert_called_once()
        mock_rename.assert_called_once()

    def test_failure(self) -> None:
        file = cas.File(
            instance="projects/q/instance/goo",
            digest="019231l123feedface/77",
            filename=Path("foo.log"),
        )
        output = Path("/path/to/other.log")
        with mock.patch.object(
            cl_utils,
            "subprocess_call",
            return_value=cl_utils.SubprocessResult(1),
        ) as mock_call:
            with mock.patch.object(Path, "rename") as mock_rename:
                result = file.download(output)

        self.assertEqual(result.returncode, 1)
        mock_call.assert_called_once()
        mock_rename.assert_not_called()


if __name__ == "__main__":
    unittest.main()

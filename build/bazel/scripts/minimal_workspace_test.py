#!/usr/bin/env python3
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).parent))
import minimal_workspace
import remote_services_utils

_FAKE_RBE_SUBSTITUTIONS = {
    "remote_download_outputs": "",
    "remote_instance_name": "johnny/cache/instance/default",
    "rbe_project": "cache-is-king",
    "container_image": "docker://gcr.io/cloud-marketplace/google/debian11@sha256:69e2789c9f3d28c6a0f13b25062c240ee7772be1f5e6d41bb4680b63eae6b304",
}


class MainTests(unittest.TestCase):
    def test_generate(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            # Args such as --fuchsia-dir, --bazel-bin can point to
            # arbitrary/default locations because this test will not
            # actually try to access those paths.
            with mock.patch.object(
                Path, "read_text", return_value="template text\n"
            ) as mock_read:
                with mock.patch.object(
                    remote_services_utils,
                    "generate_rbe_template_substitutions",
                    return_value=_FAKE_RBE_SUBSTITUTIONS,
                ) as mock_build_config:
                    status = minimal_workspace.main([f"--topdir={td}"])

            self.assertEqual(status, 0)
            mock_build_config.assert_called_once()
            mock_read.assert_called()  # twice, once per template file


if __name__ == "__main__":
    unittest.main()

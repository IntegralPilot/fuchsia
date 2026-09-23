# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.affordances.sl4f.screenshot.py."""

import unittest
from pathlib import Path
from unittest import mock

import png
from honeydew.affordances.ui.screenshot import screenshot_using_ffx
from honeydew.transports.ffx import ffx as ffx_transport


class ScreenshotUsingFfxTests(unittest.TestCase):
    """Unit tests for honeydew.affordances.ffx.ui.screenshot.py."""

    def setUp(self) -> None:
        super().setUp()
        self.mock_ffx = mock.MagicMock(spec=ffx_transport.FFX)
        self.screenshot_obj = screenshot_using_ffx.ScreenshotUsingFfx(
            ffx=self.mock_ffx
        )

    def test_verify_supported(self) -> None:
        """Test if verify_supported works."""

    def test_take_screenshot(self) -> None:
        # An image with a single pixel:
        expected_img_bytes = [100, 50, 255, 255]

        def mock_run(cmd: list[str]) -> str:
            expected_cmd = ["target", "screenshot", "--format", "png", "-d"]
            self.assertEqual(expected_cmd, cmd[0:-1])
            output_dir = Path(cmd[-1])
            fake_output_file = output_dir / "screenshot.png"
            png.from_array([expected_img_bytes], mode="RGBA").save(
                fake_output_file
            )
            return f"output: {fake_output_file}"

        self.mock_ffx.run = mock.MagicMock(side_effect=mock_run)

        img = self.screenshot_obj.take()

        self.mock_ffx.run.assert_called_once()
        self.assertEqual(img.size.width, 1.0)
        self.assertEqual(img.size.height, 1.0)
        self.assertEqual(img.data, bytes(expected_img_bytes))

#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest

from antlion.libs.ota.ota_tools import ota_tool


class OtaToolTests(unittest.TestCase):
    """Tests the OtaTool class."""

    def test_init(self):
        expected_value = "commmand string"
        self.assertEqual(
            ota_tool.OtaTool(expected_value).command, expected_value
        )

    def test_start_throws_error_on_unimplemented(self):
        obj = "some object"
        with self.assertRaises(NotImplementedError):
            ota_tool.OtaTool("").update(obj)

    def test_end_is_not_abstract(self):
        obj = "some object"
        try:
            ota_tool.OtaTool("").cleanup(obj)
        except:
            self.fail("End is not required and should be a virtual function.")


if __name__ == "__main__":
    unittest.main()

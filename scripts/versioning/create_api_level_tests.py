#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import os
import shutil
import tempfile
import unittest
from unittest import mock

import create_api_level

FAKE_VERSION_HISTORY_FILE_CONTENT = """{
    "data": {
        "name": "Platform version map",
        "type": "version_history",
        "api_levels": {
            "1" : {
                "abi_revision": "0x0000000000000001",
                "phase": "supported"
            }
        }
    },
    "schema_id": "https://fuchsia.dev/schema/version_history.json"
}
"""

OLD_API_LEVEL = 1
OLD_SUPPORTED_API_LEVELS = [1]

NEW_API_LEVEL = 2
# This script doesn't update the set of supported API levels, this only happen
# when freezing an API level.
NEW_SUPPORTED_API_LEVELS = OLD_SUPPORTED_API_LEVELS


class TestUpdatePlatformVersionMethods(unittest.TestCase):
    def setUp(self) -> None:
        self.test_dir = tempfile.mkdtemp()

        self.fake_version_history_file = os.path.join(
            self.test_dir, "version_history.json"
        )
        with open(self.fake_version_history_file, "w") as f:
            f.write(FAKE_VERSION_HISTORY_FILE_CONTENT)

    def tearDown(self) -> None:
        shutil.rmtree(self.test_dir)

    @mock.patch("secrets.randbelow")
    def test_update_version_history(self, mock_secrets: mock.MagicMock) -> None:
        mock_secrets.return_value = 0x1234ABC

        self.assertTrue(
            create_api_level.update_version_history(
                NEW_API_LEVEL, self.fake_version_history_file
            )
        )

        with open(self.fake_version_history_file, "r") as f:
            updated = json.load(f)
            self.assertEqual(
                updated,
                {
                    "data": {
                        "name": "Platform version map",
                        "type": "version_history",
                        "api_levels": {
                            "1": {
                                "abi_revision": "0x0000000000000001",
                                "phase": "supported",
                            },
                            "2": {
                                "abi_revision": "0x0000000001234ABC",
                                "phase": "supported",
                            },
                        },
                    },
                    "schema_id": "https://fuchsia.dev/schema/version_history.json",
                },
            )

        self.assertFalse(
            create_api_level.update_version_history(
                NEW_API_LEVEL, self.fake_version_history_file
            )
        )

    @mock.patch("secrets.randbelow")
    def test_abi_collision(self, mock_secrets: mock.MagicMock) -> None:
        # Return 0x1 the first time, and 0x4321 the second time. We expect the
        # second one to be used.
        mock_secrets.side_effect = [0x1, 0x4321CBA]

        self.assertTrue(
            create_api_level.update_version_history(
                NEW_API_LEVEL, self.fake_version_history_file
            )
        )

        with open(self.fake_version_history_file, "r") as f:
            updated = json.load(f)
            self.assertEqual(
                updated,
                {
                    "data": {
                        "name": "Platform version map",
                        "type": "version_history",
                        "api_levels": {
                            "1": {
                                "abi_revision": "0x0000000000000001",
                                "phase": "supported",
                            },
                            "2": {
                                "abi_revision": "0x0000000004321CBA",
                                "phase": "supported",
                            },
                        },
                    },
                    "schema_id": "https://fuchsia.dev/schema/version_history.json",
                },
            )


if __name__ == "__main__":
    unittest.main()

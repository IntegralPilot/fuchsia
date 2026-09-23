# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import copy
import datetime
import unittest

import generate_version_history

FAKE_VERSION_HISTORY = {
    "data": {
        "name": "Platform version map",
        "type": "version_history",
        "api_levels": {
            "9": {
                "abi_revision": "0xECDB841C251A8CB9",
                "phase": "retired",
            },
            "10": {
                "abi_revision": "0xED74D73009C2B4E3",
                "phase": "sunset",
            },
            "11": {
                "abi_revision": "0xED74D73009C2B4E4",
                "phase": "supported",
            },
        },
        "special_api_levels": {
            "NEXT": {
                "abi_revision": "GENERATED_BY_BUILD",
                "as_u32": 4291821568,
            },
            "HEAD": {
                "abi_revision": "GENERATED_BY_BUILD",
                "as_u32": 4292870144,
            },
            "PLATFORM": {
                "abi_revision": "GENERATED_BY_BUILD",
                "as_u32": 4293918720,
            },
        },
    },
}


class GenerateVersionHistoryTests(unittest.TestCase):
    def test_replace_abi_revisions(self) -> None:
        version_history = copy.deepcopy(FAKE_VERSION_HISTORY)

        generate_version_history.replace_special_abi_revisions(
            version_history,
            "e821407c0ffd857326038a504d76c5e11b738608",
            datetime.datetime(2024, 7, 19, 18, 59, 39, 0, datetime.UTC),
        )

        self.assertEqual(
            version_history,
            {
                "data": {
                    "name": "Platform version map",
                    "type": "version_history",
                    "api_levels": {
                        "9": {
                            "abi_revision": "0xECDB841C251A8CB9",
                            "phase": "retired",
                        },
                        "10": {
                            "abi_revision": "0xED74D73009C2B4E3",
                            "phase": "sunset",
                        },
                        "11": {
                            "abi_revision": "0xED74D73009C2B4E4",
                            "phase": "supported",
                        },
                    },
                    "special_api_levels": {
                        "NEXT": {
                            "abi_revision": "0xFF00E821000B470F",
                            "as_u32": 4291821568,
                        },
                        "HEAD": {
                            "abi_revision": "0xFF00E821000B470F",
                            "as_u32": 4292870144,
                        },
                        "PLATFORM": {
                            "abi_revision": "0xFF01E821000B470F",
                            "as_u32": 4293918720,
                        },
                    },
                },
            },
        )

        # Double check that the day ordinal from the ABI stamp makes sense.
        self.assertEqual(
            datetime.date.fromordinal(0x000B470F), datetime.date(2024, 7, 20)
        )

    def test_add_deprecated_status_field(self) -> None:
        version_history = copy.deepcopy(FAKE_VERSION_HISTORY)

        generate_version_history.add_deprecated_status_field(version_history)

        self.assertEqual(
            version_history,
            {
                "data": {
                    "name": "Platform version map",
                    "type": "version_history",
                    "api_levels": {
                        "9": {
                            "abi_revision": "0xECDB841C251A8CB9",
                            "phase": "retired",
                            "status": "unsupported",
                        },
                        "10": {
                            "abi_revision": "0xED74D73009C2B4E3",
                            "phase": "sunset",
                            "status": "unsupported",
                        },
                        "11": {
                            "abi_revision": "0xED74D73009C2B4E4",
                            "phase": "supported",
                            "status": "supported",
                        },
                    },
                    "special_api_levels": {
                        "NEXT": {
                            "abi_revision": "GENERATED_BY_BUILD",
                            "as_u32": 4291821568,
                            "status": "in-development",
                        },
                        "HEAD": {
                            "abi_revision": "GENERATED_BY_BUILD",
                            "as_u32": 4292870144,
                            "status": "in-development",
                        },
                        "PLATFORM": {
                            "abi_revision": "GENERATED_BY_BUILD",
                            "as_u32": 4293918720,
                            "status": "in-development",
                        },
                    },
                },
            },
        )


if __name__ == "__main__":
    unittest.main()

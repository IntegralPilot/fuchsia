#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import sys
import unittest
from pathlib import Path

_SCRIPT_DIR = Path(__file__).parent
sys.path.insert(0, str(_SCRIPT_DIR))

from bazel_build_flags import DefaultBuildFlagsMap, DefaultBuildFlagsSet


class DefaultBuildFlagsTest(unittest.TestCase):
    def test_new_from_gn_config(self) -> None:
        build_dir = Path("/mock/build_dir")
        build_flags_map = DefaultBuildFlagsMap.new_from_gn_config(build_dir)

        self.assertIn("fuchsia", build_flags_map)
        self.assertIn("host", build_flags_map)

        fuchsia_set = build_flags_map["fuchsia"]
        self.assertEqual(fuchsia_set.cxx_common_build_flags, [])
        self.assertEqual(fuchsia_set.cxx_executable_build_flags, [])
        self.assertEqual(fuchsia_set.cxx_shared_library_build_flags, [])
        self.assertEqual(fuchsia_set.rust_common_build_flags, [])
        self.assertEqual(fuchsia_set.rust_executable_build_flags, [])
        self.assertEqual(fuchsia_set.rust_shared_library_build_flags, [])
        self.assertEqual(
            fuchsia_set.target_compatible_with, '["@platforms//os:fuchsia"]'
        )

        host_set = build_flags_map["host"]
        self.assertEqual(host_set.cxx_common_build_flags, [])
        self.assertEqual(host_set.cxx_executable_build_flags, [])
        self.assertEqual(host_set.cxx_shared_library_build_flags, [])
        self.assertEqual(host_set.rust_common_build_flags, [])
        self.assertEqual(host_set.rust_executable_build_flags, [])
        self.assertEqual(host_set.rust_shared_library_build_flags, [])
        self.assertEqual(host_set.target_compatible_with, "HOST_OS_CONSTRAINTS")

    def test_generate_bazel_toolchain_definitions(self) -> None:
        custom_map = DefaultBuildFlagsMap(
            {
                "target_a": DefaultBuildFlagsSet(
                    cxx_common_build_flags=["//build/flags:cxx_common1"],
                    cxx_executable_build_flags=["//build/flags:cxx_exec1"],
                    cxx_shared_library_build_flags=["//build/flags:cxx_shlib1"],
                    rust_common_build_flags=["//build/flags:rust_common1"],
                    rust_executable_build_flags=["//build/flags:rust_exec1"],
                    rust_shared_library_build_flags=[
                        "//build/flags:rust_shlib1"
                    ],
                    target_compatible_with='["@platforms//os:target_a_os"]',
                ),
                "target_b": DefaultBuildFlagsSet(
                    cxx_common_build_flags=[],
                    cxx_executable_build_flags=[],
                    cxx_shared_library_build_flags=[],
                    rust_common_build_flags=[],
                    rust_executable_build_flags=[],
                    rust_shared_library_build_flags=[],
                    target_compatible_with='["@platforms//os:target_b_os"]',
                ),
            }
        )

        generated_content = custom_map.generate_bazel_toolchain_definitions()

        # Check imports and headers
        self.assertIn(
            'load("@fuchsia_rules_common//build_flags:toolchain.bzl", "build_flags_toolchain_instance")',
            generated_content,
        )

        # Check target_a definitions
        self.assertIn(
            'name = "target_a_default_build_flags"', generated_content
        )
        self.assertIn(
            'cxx_common_build_flags = ["//build/flags:cxx_common1"]',
            generated_content,
        )
        self.assertIn(
            'cxx_executable_build_flags = ["//build/flags:cxx_exec1"]',
            generated_content,
        )
        self.assertIn(
            'cxx_shared_library_build_flags = ["//build/flags:cxx_shlib1"]',
            generated_content,
        )
        self.assertIn(
            'rust_common_build_flags = ["//build/flags:rust_common1"]',
            generated_content,
        )
        self.assertIn(
            'rust_executable_build_flags = ["//build/flags:rust_exec1"]',
            generated_content,
        )
        self.assertIn(
            'rust_shared_library_build_flags = ["//build/flags:rust_shlib1"]',
            generated_content,
        )
        self.assertIn('name = "target_a_toolchain"', generated_content)
        self.assertIn(
            'target_compatible_with = ["@platforms//os:target_a_os"]',
            generated_content,
        )
        self.assertIn(
            'toolchain = ":target_a_default_build_flags"', generated_content
        )

        # Check target_b definitions
        self.assertIn(
            'name = "target_b_default_build_flags"', generated_content
        )
        self.assertIn("cxx_common_build_flags = []", generated_content)
        self.assertIn("cxx_executable_build_flags = []", generated_content)
        self.assertIn("cxx_shared_library_build_flags = []", generated_content)
        self.assertIn("rust_common_build_flags = []", generated_content)
        self.assertIn("rust_executable_build_flags = []", generated_content)
        self.assertIn("rust_shared_library_build_flags = []", generated_content)
        self.assertIn('name = "target_b_toolchain"', generated_content)
        self.assertIn(
            'target_compatible_with = ["@platforms//os:target_b_os"]',
            generated_content,
        )
        self.assertIn(
            'toolchain = ":target_b_default_build_flags"', generated_content
        )


if __name__ == "__main__":
    unittest.main()

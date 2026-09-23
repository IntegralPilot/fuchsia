# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
import json
import os
import sys
import tempfile
import unittest
from typing import Any

import convert_size_limits
from parameterized import param, parameterized


class ConvertTest(unittest.TestCase):
    @parameterized.expand(
        [
            param(
                name="general_case",
                size_limits=dict(
                    core_limit=21,
                    core_creep_limit=22,
                    distributed_shlibs=["lib_a", "lib_b"],
                    distributed_shlibs_limit=31,
                    distributed_shlibs_creep_limit=32,
                    icu_data=["icu"],
                    icu_data_limit=41,
                    icu_data_creep_limit=42,
                    components=[
                        dict(
                            component="b1_2_comp",
                            creep_limit=12,
                            limit=20,
                            pkgs=["b1_c1", "b1_c2", "b2_c1"],
                        ),
                        dict(
                            component="b_comp",
                            creep_limit=16,
                            limit=22,
                            pkgs=["b_c2", "b_c1"],
                        ),
                        dict(
                            component="/system (drivers and early boot)",
                            creep_limit=52,
                            limit=53,
                            pkgs=[],
                        ),
                    ],
                ),
                image_assembly_config=dict(
                    base=[
                        "obj/a/b1_c1/package_manifest.json",
                        "obj/a/b_c2/package_manifest.json",
                        "obj/is_not_matched_1/package_manifest.json",
                    ],
                    cache=[
                        "obj/a/b_c1/package_manifest.json",
                        "obj/a/b2_c1/package_manifest.json",
                        "obj/a/b1_c2_expanded/rebased_package_manifest.json",
                        "obj/is_not_matched_2/package_manifest.json",
                    ],
                    system=["obj/system/package_manifest.json"],
                ),
                expected_output=dict(
                    resource_budgets=[
                        dict(
                            name="Distributed shared libraries",
                            paths=["lib_a", "lib_b"],
                            budget_bytes=31,
                            creep_budget_bytes=32,
                        ),
                        dict(
                            name="ICU Data",
                            paths=["icu"],
                            budget_bytes=41,
                            creep_budget_bytes=42,
                        ),
                    ],
                    package_set_budgets=[
                        dict(
                            name="/system (drivers and early boot)",
                            budget_bytes=53,
                            creep_budget_bytes=52,
                            merge=True,
                            packages=["obj/system/package_manifest.json"],
                        ),
                        dict(
                            name="Core system+services",
                            budget_bytes=21,
                            creep_budget_bytes=22,
                            merge=False,
                            packages=[
                                "obj/is_not_matched_1/package_manifest.json",
                                "obj/is_not_matched_2/package_manifest.json",
                            ],
                        ),
                        dict(
                            name="b1_2_comp",
                            budget_bytes=20,
                            creep_budget_bytes=12,
                            merge=False,
                            packages=[
                                "obj/a/b1_c1/package_manifest.json",
                                "obj/a/b1_c2_expanded/rebased_package_manifest.json",
                                "obj/a/b2_c1/package_manifest.json",
                            ],
                        ),
                        dict(
                            name="b_comp",
                            budget_bytes=22,
                            creep_budget_bytes=16,
                            merge=False,
                            packages=[
                                "obj/a/b_c1/package_manifest.json",
                                "obj/a/b_c2/package_manifest.json",
                            ],
                        ),
                    ],
                    total_budget_bytes=999,
                ),
                max_blob_contents_size=999,
                return_value=0,
            ),
            param(
                name="failure_when_manifest_is_matched_twice",
                size_limits=dict(
                    core_limit=21,
                    core_creep_limit=22,
                    distributed_shlibs=[],
                    distributed_shlibs_limit=31,
                    distributed_shlibs_creep_limit=32,
                    icu_data=[],
                    icu_data_limit=41,
                    icu_data_creep_limit=42,
                    components=[
                        dict(
                            component="comp1",
                            creep_limit=12,
                            limit=20,
                            pkgs=["b_c2"],
                        ),
                        dict(
                            component="comp2",
                            creep_limit=16,
                            limit=22,
                            pkgs=["b_c2"],
                        ),
                    ],
                ),
                image_assembly_config=dict(
                    base=["obj/a/b_c2/package_manifest.json"]
                ),
                expected_output=None,
                max_blob_contents_size=100,
                return_value=1,
            ),
            param(
                name="success_when_size_limit_and_image_assembly_config_is_empty",
                size_limits=dict(),
                image_assembly_config=dict(),
                expected_output=dict(
                    resource_budgets=[],
                    package_set_budgets=[],
                    total_budget_bytes=200,
                ),
                max_blob_contents_size=200,
                return_value=0,
            ),
        ]
    )
    def test_run_main(
        self,
        name: str,
        size_limits: dict[str, Any],
        image_assembly_config: dict[str, Any],
        expected_output: dict[str, Any],
        max_blob_contents_size: int,
        return_value: int,
    ) -> None:
        self.maxDiff = None  # Do not truncate the diff result.
        with tempfile.TemporaryDirectory() as tmpdir:
            platform_aibs_path = os.path.join(tmpdir, "platform_aibs.json")
            with open(platform_aibs_path, "w") as file:
                json.dump([], file)

            size_limits_path = os.path.join(tmpdir, "size_limits.json")
            with open(size_limits_path, "w") as file:
                json.dump(size_limits, file)

            image_assembly_config_path = os.path.join(
                tmpdir, "image_assembly_config.json"
            )
            with open(image_assembly_config_path, "w") as file:
                json.dump(image_assembly_config, file)

            output_path = os.path.join(tmpdir, "output.json")
            non_blobfs_output_path = os.path.join(
                tmpdir, "non_blobfs_output.json"
            )
            # The first argument of a command line is the path to the program.
            # It is unused and left empty.
            sys.argv = [
                "",
                "--size-limits",
                size_limits_path,
                "--image-assembly-config",
                image_assembly_config_path,
                "--output",
                output_path,
                "--max-blob-contents-size",
                str(max_blob_contents_size),
            ]

            self.assertEqual(convert_size_limits.main(), return_value)

            if expected_output is not None:
                with open(output_path, "r") as file:
                    self.assertEqual(expected_output, json.load(file))

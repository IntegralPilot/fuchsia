#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import sys
import typing as T
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import build_api_filter


class BuildApiModulesFilterTest(unittest.TestCase):
    def _setup_build_dir(
        self, build_dir: Path, api_files: dict[str, T.Any]
    ) -> None:
        api_list = sorted(api_files.keys())
        with (build_dir / "api.json").open("w") as f:
            json.dump(api_list, f, indent=2)

        for api_module, json_value in api_files.items():
            (build_dir / f"{api_module}.json").write_text(
                json.dumps(json_value, indent=2)
            )

    def test_filter(self) -> None:
        ninja_artifacts = [
            "obj/foo",
            "gen/bar",
            "obj/build/images/fuchsia/product_bundle/product_bundle.json",
            "obj/some_prebuilt_package/qux.debug_symbols.json",
        ]
        ninja_sources = [
            "../../src/foo.cc",
            "../../prebuilt/third_party/zoom/bin",
        ]
        module_filter = build_api_filter.BuildApiFilter(
            ninja_artifacts, ninja_sources
        )

        _TEST_CASES = [
            {
                "assembly_input_archives": (
                    [
                        {
                            "label": "//bundles/assembly:zircon.tgz(//build/toolchain/fuchsia:x64)",
                            "path": "obj/bundles/assembly/zircon.tgz",
                        },
                        {
                            "label": "//bundles/assembly:embeddable.tgz(//build/toolchain/fuchsia:x64)",
                            "path": "obj/foo",
                        },
                        {
                            "label": "//bundles/assembly:bootstrap.tgz(//build/toolchain/fuchsia:x64)",
                            "path": "obj/bundles/assembly/bootstrap.tgz",
                        },
                    ],
                    [
                        {
                            "label": "//bundles/assembly:embeddable.tgz(//build/toolchain/fuchsia:x64)",
                            "path": "obj/foo",
                        },
                    ],
                ),
                "product_bundles": (  # See https://fxbug.dev/365039385
                    [
                        {
                            "cpu": "x64",
                            "json": "obj/build/images/fuchsia/product_bundle/product_bundle.json",
                            "label": "//build/images/fuchsia:product_bundle(//build/toolchain/fuchsia:x64)",
                            "name": "terminal.x64",
                            "path": "obj/build/images/fuchsia/product_bundle",
                            "product_version": "24.99991231.0.1",
                            "transfer_manifest_path": "obj/build/images/fuchsia/terminal.x64.transfer.json",
                            "transfer_manifest_url": "file://obj/build/images/fuchsia/terminal.x64.transfer.json",
                        },
                        {
                            "cpu": "x64",
                            "json": "obj/src/bringup/lib/mexec/tests/mexec-entropy-test.product_bundle/product_bundle.json",
                            "label": "//src/bringup/lib/mexec/tests:mexec-entropy-test.product_bundle(//build/toolchain/fuchsia:x64)",
                            "name": "mexec-entropy-test",
                            "path": "obj/src/bringup/lib/mexec/tests/mexec-entropy-test.product_bundle",
                            "product_version": "24.99991231.0.1",
                            "transfer_manifest_path": "obj/src/bringup/lib/mexec/tests/mexec-entropy-test.transfer.json",
                            "transfer_manifest_url": "file://obj/src/bringup/lib/mexec/tests/mexec-entropy-test.transfer.json",
                        },
                    ],
                    [
                        {
                            "cpu": "x64",
                            "json": "obj/build/images/fuchsia/product_bundle/product_bundle.json",
                            "label": "//build/images/fuchsia:product_bundle(//build/toolchain/fuchsia:x64)",
                            "name": "terminal.x64",
                            "path": "obj/build/images/fuchsia/product_bundle",
                            "product_version": "24.99991231.0.1",
                            "transfer_manifest_path": "obj/build/images/fuchsia/terminal.x64.transfer.json",
                            "transfer_manifest_url": "file://obj/build/images/fuchsia/terminal.x64.transfer.json",
                        },
                    ],
                ),
                "debug_symbols": (
                    [
                        {
                            "debug": "obj/foo",
                            "elf_build_id": "0123456789abcdef",
                            "cpu": "x64",
                            "os": "fuchsia",
                            "label": "//src/foo:bin(//build/toolchain/fuchsia:x64)",
                        },
                        {
                            "manifest": "obj/some_prebuilt_package/zoo.debug_symbols.json",
                            "label": "//src/subsystem/zoo:zoo.prebuilt(//build/toolchain/fuchsia:x64)",
                        },
                        {
                            "manifest": "obj/some_prebuilt_package/qux.debug_symbols.json",
                            "label": "//src/subsystem/qux:qux.prebuilt(//build/toolchain/fuchsia:x64)",
                        },
                        {
                            "debug": "obj/exe.unstripped/cranberry",
                            "elf_build_id": "fedcba987654321",
                            "cpu": "x64",
                            "os": "fuchsia",
                            "label": "//src/subsystem/cranberry:bin(//build/toolchain/fuchsia:x64)",
                        },
                        {
                            "debug": "../../prebuilt/third_party/zoom/bin",
                            "os": "linux",
                            "cpu": "x64",
                            "label": "//prebuilt/third_party/zoom(//build/toolchain:host_x64)",
                        },
                    ],
                    [
                        {
                            "cpu": "x64",
                            "debug": "obj/foo",
                            "elf_build_id": "0123456789abcdef",
                            "label": "//src/foo:bin(//build/toolchain/fuchsia:x64)",
                            "os": "fuchsia",
                        },
                        {
                            "label": "//src/subsystem/qux:qux.prebuilt(//build/toolchain/fuchsia:x64)",
                            "manifest": "obj/some_prebuilt_package/qux.debug_symbols.json",
                        },
                        {
                            "cpu": "x64",
                            "debug": "../../prebuilt/third_party/zoom/bin",
                            "label": "//prebuilt/third_party/zoom(//build/toolchain:host_x64)",
                            "os": "linux",
                        },
                    ],
                ),
            }
        ]

        for test_case in _TEST_CASES:
            for api_module, input_expected_pair in test_case.items():
                input_value, expected_value = input_expected_pair
                input_content = json.dumps(input_value, indent=2)
                expected_content = json.dumps(expected_value, indent=2)
                output_content = module_filter.filter_api_file(
                    api_module, input_content
                )
                self.assertEqual(output_content, expected_content)


if __name__ == "__main__":
    unittest.main()

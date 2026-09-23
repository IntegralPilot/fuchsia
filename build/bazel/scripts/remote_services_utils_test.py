#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit-tests for remote_services_utils.py functions."""

import os
import sys
import tempfile
import typing as T
import unittest
from pathlib import Path

# Import regenerator.py as a module.
_SCRIPT_DIR = os.path.dirname(__file__)
sys.path.insert(0, _SCRIPT_DIR)
import remote_services_utils


class RemoteServicesUtilsTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self._dir = Path(self._td.name)
        self.source_dir = self._dir / "source"
        self.source_dir.mkdir()

        self.cfg_file1 = (
            self.source_dir / remote_services_utils.RBE_CONFIG_FILES[0]
        )
        self.cfg_file2 = (
            self.source_dir / remote_services_utils.RBE_CONFIG_FILES[1]
        )

        self.cfg_file1.parent.mkdir(parents=True)
        self.cfg_file1.write_text(
            "\n" + "# foo=bar\n"  # empty line  # comment line should be ignored
            "instance=projects/test_project_name/instances/test_instance_name\n"
        )

        self.cfg_file2.parent.mkdir(exist_ok=True)
        self.cfg_file2.write_text(
            "platform=container-image=docker://gcr.io/test_project/debian11,something_else\n"
        )

    def tearDown(self) -> None:
        self._td.cleanup()

    def test_generate_rbe_config(self) -> None:
        config_dict, input_files = remote_services_utils.generate_rbe_config(
            self.source_dir
        )
        self.assertDictEqual(
            config_dict,
            {
                "instance": "projects/test_project_name/instances/test_instance_name",
                "platform": "container-image=docker://gcr.io/test_project/debian11,something_else",
            },
        )
        self.assertSetEqual(input_files, set([self.cfg_file1, self.cfg_file2]))

    def test_generate_rbe_template_substitutions(self) -> None:
        _TEST_CASES: list[dict[str, T.Any]] = [
            {
                "name": "simple_project",
                "config_dict": {
                    "instance": "projects/test_project1/instances/1",
                    "platform": "foo=bar,container-image=docker://something/else,zoo=qux",
                },
                "remote_download_outputs": "all",
                "expected": {
                    "remote_download_outputs": "all",
                    "remote_instance_name": "projects/test_project1/instances/1",
                    "rbe_project": "test_project1",
                    "container_image": "docker://something/else",
                },
            },
        ]
        for test_case in _TEST_CASES:
            msg = f'For {test_case["name"]} case'
            result = remote_services_utils.generate_rbe_template_substitutions(
                test_case["config_dict"], test_case["remote_download_outputs"]
            )
            self.assertDictEqual(result, test_case["expected"], msg=msg)

    def test_generate_remote_services_bazelrc(self) -> None:
        output_file = self._dir / "remote_services.bazelrc"

        template_file = (
            self.source_dir / remote_services_utils.RBE_TEMPLATE_FILE
        )
        template_file.parent.mkdir(parents=True)
        template_file.write_text(
            r"""# AUTO-GENERATED
my_download_outputs={remote_download_outputs}
my_remote_instance_name={remote_instance_name}
my_rbe_project={rbe_project}
my_container_image={container_image}
"""
        )

        input_files = remote_services_utils.generate_remote_services_bazelrc(
            fuchsia_dir=self.source_dir,
            output_path=output_file,
            download_outputs="find-me",
        )

        self.assertEqual(
            output_file.read_text(),
            """# AUTO-GENERATED
my_download_outputs=find-me
my_remote_instance_name=projects/test_project_name/instances/test_instance_name
my_rbe_project=test_project_name
my_container_image=docker://gcr.io/test_project/debian11
""",
        )

        self.assertSetEqual(
            input_files, {template_file, self.cfg_file1, self.cfg_file2}
        )


if __name__ == "__main__":
    unittest.main()

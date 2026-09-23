# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import unittest
from pathlib import Path
from sys import argv

assert (
    len(argv) > 0
), "host_test.py expects to be passed path to generated + linked docs"
_rustdoc_actions = Path(argv.pop())


class Test(unittest.TestCase):
    _rustdoc_actions: Path

    @classmethod
    def setUpClass(cls) -> None:
        cls._rustdoc_actions = _rustdoc_actions

    def test_dry_run_output(self) -> None:
        found_actions_string = self._rustdoc_actions.read_text()
        found_actions_json = json.loads(found_actions_string)

        # assertEquals is fine here because all arrays above have length one,
        # and python checks objects for equality. We should be strict with
        # asserting exact equality here. This test helps ensure that changes
        # to rustdoc-link.py are reflected to the infra builder recipe.

        # With that being said, if you have to change the above, you should
        # make a corresponding change in infra!

        print(found_actions_json["host_action"])

        self.assertEquals(
            found_actions_json["host_action"],
            {
                "build_action": None,
                "rustdoc_action": {
                    "argfile": "host_x64/gen/build/rust/tests/rustdoc-link/basic-dry-run/output_base/argfiles/host.args"
                },
                "copy_action": {
                    "srcs": [
                        "host_x64/gen/build/rust/tests/rustdoc-link/basic-dry-run/bar.aux.doc/."
                    ],
                    "dst": "host_x64/gen/build/rust/tests/rustdoc-link/basic-dry-run/output_base/doc/host",
                },
            },
        )

        self.assertEquals(
            found_actions_json["fuchsia_action"],
            {
                "build_action": None,
                "rustdoc_action": {
                    "argfile": "host_x64/gen/build/rust/tests/rustdoc-link/basic-dry-run/output_base/argfiles/fuchsia.args"
                },
                "copy_action": {
                    "srcs": [
                        "gen/build/rust/tests/rustdoc-link/basic-dry-run/foo.aux.doc/."
                    ],
                    "dst": "host_x64/gen/build/rust/tests/rustdoc-link/basic-dry-run/output_base/doc",
                },
            },
        )

        self.assertEquals(found_actions_json["zip_action"], None)

        self.assertIn("executable", found_actions_json["verify_action"])
        self.assertIn(
            "python", found_actions_json["verify_action"]["executable"]
        )

        self.assertEquals(
            found_actions_json["verify_action"]["args"],
            [
                "../../tools/devshell/contrib/lib/rust/rustdoc_link_verify.py",
                "host_x64/gen/build/rust/tests/rustdoc-link/basic-dry-run/output_base/doc",
            ],
        )

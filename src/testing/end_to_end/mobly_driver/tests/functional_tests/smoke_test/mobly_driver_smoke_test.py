# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly Driver smoke test.

This E2E smoke test exercises the Mobly Driver's ability to conduct the following:

  - Register a trivial controller
  - Device config generation.
  - Mobly test execution.

Furthermore, this test can be used as a canary to verify Fuchsia infra
correctly parses Mobly test results and uploads artifacts from the test.
"""

import os

import trivial_controller
from mobly import asserts, base_test, test_runner
from mobly.expects import asserts


class MoblyDriverSmokeTest(base_test.BaseTestClass):
    """Mobly Driver smoke tests."""

    def setup_class(self) -> None:
        super().setup_class()
        # Infra testbed config ignores the local config and only contains real
        # devices. Inject fake configs here to test controller registration.
        self.controller_configs["TrivialController"] = [{"name": "fake_name"}]

    def test_mobly_controller_init(self) -> None:
        """Asserts a trivial controller can be registered."""
        trivial_controllers = self.register_controller(trivial_controller)
        assert len(trivial_controllers) > 0

    def test_params_exist(self) -> None:
        """Asserts user_params exactly match params.yaml."""
        assert self.user_params is not None, "Test params are missing."
        actual_keys = set(self.user_params.keys())
        # ffx-subtools-search-path is injected by the mobly driver (though not
        # guaranteed to be present in all execution environments) and is not
        # present in params.yaml. Discard it to verify that params.yaml
        # contents match exactly.
        actual_keys.discard("ffx-subtools-search-path")
        asserts.assert_equal(
            actual_keys,
            {"bool_param", "str_param", "dict_param", "list_param"},
        )
        asserts.assert_true(
            self.user_params["bool_param"],
            "bool_param does not match params.yaml",
        )
        asserts.assert_equal(
            self.user_params["str_param"],
            "some_string",
            "str_param does not match params.yaml",
        )
        asserts.assert_equal(
            self.user_params["dict_param"],
            {"fld_1": "val_1", "fld_2": "val_2"},
            "dict_param does not match params.yaml",
        )
        asserts.assert_equal(
            self.user_params["list_param"],
            [1, 2, 3, 4],
            "list_param does not match params.yaml",
        )

    def test_output_dir(self) -> None:
        """Asserts log_path is writable."""
        test_artifact_path = os.path.join(self.log_path, "artifact.txt")
        with open(test_artifact_path, "w+", encoding="utf8") as file_handle:
            file_handle.write("data")


if __name__ == "__main__":
    test_runner.main()

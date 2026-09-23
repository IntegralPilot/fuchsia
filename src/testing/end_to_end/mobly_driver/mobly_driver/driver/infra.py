# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Implements BaseDriver for the lab infra execution environment."""

import os
from typing import Any, Optional

import yaml
from mobly_driver.api import api_infra, api_mobly
from mobly_driver.driver import base, common


class InfraDriver(base.BaseDriver):
    """Infrastructure Mobly test driver.

    This driver is used when executing Mobly tests in the infra environment.
    Due to Swarming's dimension-based test allocation system, the testbed used
    for running a test is not known until at test-run-time (after Swarming bot
    has been allocated).

    InfraDriver handles this by dynamically generating a Mobly test config file
    based on the Swarming bot's advertised devices. Besides config generation,
    InfraDriver also handles the infra-specific integration necessary for that
    Mobly test results to be plumbed to Fuchsia's result storage backend.
    """

    _TESTBED_NAME = "InfraTestbed"

    def __init__(
        self,
        tb_json_path: str,
        honeydew_config: dict[str, Any],
        output_path: Optional[str] = None,
        params_path: Optional[str] = None,
        ssh_path: Optional[str] = None,
    ) -> None:
        """Initializes the instance.

        Args:
          tb_json_path: absolute path to the testbed definition JSON file.
          honeydew_config: Honeydew configuration.
          output_path: absolute path to directory for storing Mobly test output.
          params_path: absolute path to the Mobly testbed params file.

        Raises:
          KeyError if required environment variables not found.
        """
        super().__init__(
            honeydew_config=honeydew_config,
            output_path=output_path,
            params_path=params_path,
        )
        self._tb_json_path = tb_json_path
        self._ssh_path = ssh_path

    def generate_test_config(self) -> str:
        """Returns a Mobly test config in YAML format.

        The Mobly test config is a required input file of any Mobly tests.
        It includes information on the DUT(s) and specifies test parameters.

        Example output:
        ---
        TestBeds:
        - Name: SomeName
          Controllers:
            FuchsiaDevice:
            - name: fuchsia-1234-5678-90ab
          TestParams:
            param_1: "val_1"
            param_2: "val_2"

        If |params_path| is specified in InfraDriver(), then its content is
        added to the Mobly test config; otherwise, the Mobly test config will
        not include any test params.

        Returns:
          A YAML string that represents a Mobly test config.

        Raises:
          common.InvalidFormatException if the test params or tb config files
            are not valid YAML/JSON documents.
          common.DriverException if any file IO exceptions occur while reading
            user provided files.
        """
        try:
            tb_config = common.read_json_from_file(self._tb_json_path)
            for controller in tb_config:
                if controller.get("type") in (
                    api_infra.ACCESS_POINT,
                    api_infra.OPENWRT_AP,
                ):
                    controller[api_mobly.ALLOW_REGDB_BYPASS] = True

            test_params = {}
            if self._params_path:
                test_params = common.read_yaml_from_file(self._params_path)
            if "transports" in self._honeydew_config:
                ffx_config = self._honeydew_config["transports"].get("ffx", {})
                if "subtools_search_path" in ffx_config:
                    test_params["ffx-subtools-search-path"] = ffx_config[
                        "subtools_search_path"
                    ]
            botanist_honeydew_translation_map: dict[str, str] = {
                "nodename": "name",
            }
            config = api_mobly.new_testbed_config(
                testbed_name=self._TESTBED_NAME,
                output_path=self._output_path,
                honeydew_config=self._honeydew_config,
                mobly_controllers=tb_config,
                test_params_dict=test_params,
                botanist_honeydew_map=botanist_honeydew_translation_map,
                ssh_path=self._ssh_path,
            )
            return yaml.dump(config)
        except (IOError, OSError):
            raise common.DriverException("Failed to open file: %")

    def teardown(self, *args: Any) -> None:
        """Performs any required clean up upon Mobly test completion."""
        results_path = api_mobly.get_result_path(
            self._output_path, self._TESTBED_NAME
        )
        try:
            with open(results_path, "r") as f:
                # Write test result YAML file to stdout so that Mobly output
                # integrates with with `testparser`.
                print(api_infra.TESTPARSER_RESULT_HEADER)
                print(f.read())
        except OSError:
            # It's possible for the Mobly result file to not exist (e.g. if the
            # test crashed). In such cases, don't print anything.
            pass

        # Remove the symlink named `latest`; otherwise infra recipe's artifact
        # upload step fails. This is a workaround for a known artifact upload
        # bug which can be removed once the following pull request is fixed:
        # https://github.com/bazelbuild/remote-apis-sdks/pull/422
        symlink_path = api_mobly.get_latest_test_output_dir_symlink_path(
            self._output_path, self._TESTBED_NAME
        )
        try:
            os.remove(symlink_path)
        except OSError:
            # No-op if the symlink does not exist (e.g. if the test crashed).
            pass

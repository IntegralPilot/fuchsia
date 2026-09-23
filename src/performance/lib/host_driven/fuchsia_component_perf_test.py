# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Provides the implementation for simple performance tests which run a test
component that publishes a fuchsiaperf.json file.
"""

import os

import fuchsia_base_test

# LINT.IfChange
import test_data

# LINT.ThenChange(//build/testing/perf/test.gni)
from host_driven import run_test_component
from mobly import test_runner
from perf_publish import publish


class FuchsiaComponentPerfTest(fuchsia_base_test.FuchsiaBaseTest):
    """
    Mobly test class allowing to run a test component and publish its fuchsiaperf data.

    Required Mobly Test Params:
        expected_metric_names_filepath (str): Name of the file with the metric allowlist.
        ffx_test_url (str): Test URL to execute via `ffx test run`.

    Optional Mobly Test Params:
        ffx_test_args(list[str]): Test options to supply to `ffx test run`
            Default: []
        test_component_args (list[str]): Options to supply to the test component.
            Default: []
        results_path_test_arg (str): The option to be used by the test for the results path.
            Important: this is just the option (ex: "--out"). The value will be passed by the test.
            Default: None
        use_component_builtin_args (bool): Setting this parameter to true causes that no arguments
            are passed to the component using ffx test, causing it to use its built-in argument list
            (as defined in the component's CML manifest file, for ELF components). Note that when
            this parameter is set to true, test_component_args and results_path_test_arg will be
            ignored.
            Default: False
        process_runs (int): Number of times to run the test component.
            Default: 1
        snapshot_on (str): One of "teardown_class", "teardown_class_on_fail",
            "teardown_test", "teardown_test_on_fail", "never".
            Default: "never"
    """

    def test_fuchsia_component(self) -> None:
        """Run a test component in the device and publish its perf data.

        This function launches a test component in the device, collects its
        fuchsiaperf data and then publishes it ensuring that the expected
        metrics are present.
        """
        ffx_test_args: list[str] = self.user_params["ffx_test_args"]
        ffx_test_url: str = self.user_params["ffx_test_url"]
        expected_metric_names_filepath: str = self.user_params[
            "expected_metric_names_filepath"
        ]
        process_runs = self.user_params.get("process_runs", 1)
        results_path_test_arg = self.user_params.get("results_path_test_arg")
        use_component_builtin_args = self.user_params.get(
            "use_component_builtin_args", False
        )
        test_component_args = []
        if not use_component_builtin_args:
            test_component_args = self.user_params.get(
                "test_component_args", []
            )

            results_file_path: str = (
                run_test_component.DEFAULT_TARGET_RESULTS_PATH
            )
            if results_path_test_arg:
                if results_path_test_arg.endswith("="):
                    test_component_args.append(
                        f"{results_path_test_arg}{results_file_path}"
                    )
                else:
                    test_component_args += [
                        results_path_test_arg,
                        results_file_path,
                    ]
            else:
                test_component_args.append(results_file_path)

        result_files: list[str] = run_test_component.repeatedly(
            self.dut.ffx,
            ffx_test_url,
            self.test_case_path,
            ffx_test_args=ffx_test_args,
            test_component_args=test_component_args,
            process_runs=process_runs,
        )

        publish.publish_fuchsiaperf(
            result_files,
            os.path.basename(expected_metric_names_filepath),
            test_data_module=test_data,
        )


if __name__ == "__main__":
    test_runner.main()

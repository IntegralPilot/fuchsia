# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Provides utilities for Python performance tests defined with python_perf_test.gni.
"""

import os
import pathlib
from typing import Any

DEFAULT_TARGET_RESULTS_FILE: str = "results.fuchsiaperf.json"
DEFAULT_TARGET_RESULTS_PATH: str = (
    f"/custom_artifacts/{DEFAULT_TARGET_RESULTS_FILE}"
)
DEFAULT_HOST_RESULTS_FILE: str = "results.fuchsiaperf_full.json"


def repeatedly(
    ffx: Any,
    test_url: str,
    host_output_path: str,
    ffx_test_args: list[str] | None = None,
    test_component_args: list[str] | None = None,
    process_runs: int = 1,
) -> list[str]:
    """Runs a test component and collects the output fuchsiaperf files.

    Args:
      ffx: The ffx Honeydew affordance that allows to run test components on the
        target.
      test_url: The component URL of the test.
      host_output_path: Directory where the test outputs will be placed in the
        host.
      ffx_test_args: Arguments passed to `ffx test`.
      test_component_args: Arguments passed to the test component launched using
        the given `test_url`.
      process_runs: Number of times the test component will be run.

    Returns: The fuchsiaperf files that were in the outputs of each test run.
    """
    result_files: list[str] = []
    for i in range(process_runs):
        test_dir = os.path.join(host_output_path, f"ffx_test_{i}")
        result_files.append(
            once(
                ffx,
                test_url,
                test_dir,
                ffx_test_args=ffx_test_args,
                test_component_args=test_component_args,
                host_results_file=f"results_process{i}.fuchsiaperf_full.json",
            )
        )
    return result_files


def once(
    ffx: Any,
    test_url: str,
    host_output_path: str,
    ffx_test_args: list[str] | None = None,
    test_component_args: list[str] | None = None,
    host_results_file: str = DEFAULT_HOST_RESULTS_FILE,
) -> str:
    """Runs a test component and collects the output fuchsiaperf files.

    Args:
      ffx: The ffx Honeydew affordance that allows to run test components on the
        target.
      test_url: The component URL of the test.
      host_output_path: Directory where the test outputs will be placed in the
        host.
      ffx_test_args: Arguments passed to `ffx test`.
      test_component_args: Arguments passed to the test component launched using
        the given `test_url`.
      host_results_file: The name of the file in the host where the results will
        be placed.

    Returns: The path to the resulting fuchsiaperf file.
    """
    if ffx_test_args is None:
        ffx_test_args = []
    if test_component_args is None:
        test_component_args = []

    ffx.run_test_component(
        test_url,
        ffx_test_args=ffx_test_args
        + [
            "--output-directory",
            host_output_path,
        ],
        test_component_args=test_component_args,
        capture_output=False,
    )
    test_result_files = list(
        pathlib.Path(host_output_path).rglob("*.fuchsiaperf.json")
    )
    if len(test_result_files) != 1:
        raise ValueError(
            f"Expected a single result file. Got: {len(test_result_files)}"
        )

    dest_file = os.path.join(host_output_path, host_results_file)
    os.rename(test_result_files[0], dest_file)

    return dest_file

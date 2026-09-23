#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""
Returns power metrics in a JSON file.
"""

import argparse
import pathlib

from reporting import metrics
from trace_processing import trace_importing, trace_model
from trace_processing.metrics import power


def main() -> None:
    """
    Takes in a trace file in JSON format and writes metrics in fuchsiaperf.json format to
    `output_path`.
    """
    parser = argparse.ArgumentParser()
    parser.add_argument("path_to_trace_json", type=str)
    parser.add_argument("output_path", type=str)
    args = parser.parse_args()

    model: trace_model.Model = trace_importing.create_model_from_file_path(
        args.path_to_trace_json
    )
    trace_results = power.PowerMetricsProcessor().process_metrics(model)

    metrics.TestCaseResult.write_fuchsiaperf_json(
        results=trace_results,
        test_suite="Manual",
        output_path=pathlib.Path(args.output_path),
    )


if __name__ == "__main__":
    main()

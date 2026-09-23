#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""
Returns suspend metrics in a JSON file.
"""

import argparse
import pathlib
import sys

from reporting import metrics
from trace_processing import trace_importing, trace_model
from trace_processing.metrics import suspend


def main() -> None:
    """
    Takes in a trace file in JSON format and writes metrics in fuchsiaperf.json format to
    `output_path`.
    """
    parser = argparse.ArgumentParser()
    parser.add_argument("path_to_trace", type=str)
    parser.add_argument("output_path", type=str)
    args = parser.parse_args()

    if args.path_to_trace.endswith(".json"):
        path_to_trace_json = args.path_to_trace
    elif args.path_to_trace.endswith(".fxt"):
        trace2json_path = pathlib.Path(sys.argv[0]).parent / "trace2json"
        path_to_trace_json = trace_importing.convert_trace_file_to_json(
            trace_path=args.path_to_trace,
            trace2json_path=trace2json_path,
        )
    else:
        raise Exception("trace must be in either fxt or json format")

    model: trace_model.Model = trace_importing.create_model_from_file_path(
        path_to_trace_json
    )
    trace_results = suspend.SuspendMetricsProcessor().process_metrics(model)

    metrics.TestCaseResult.write_fuchsiaperf_json(
        results=trace_results,
        test_suite="Manual",
        output_path=pathlib.Path(args.output_path),
    )


if __name__ == "__main__":
    main()

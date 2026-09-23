#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Returns wakeup metrics in a JSON file.
"""
import argparse
import json
import logging
import pathlib
import sys

from reporting import metrics
from trace_processing import trace_importing, trace_model
from trace_processing.metrics import wakeup

logging.basicConfig(level=logging.INFO)


def main() -> None:
    """
    Takes in a trace file (in either .FXT or JSON format) and writes metrics in
    fuchsiaperf.json format to `output_path`.
    """
    parser = argparse.ArgumentParser()
    parser.add_argument("path_to_config", type=str)
    parser.add_argument("path_to_trace", type=str)
    parser.add_argument("output_path", type=str)
    args = parser.parse_args()

    with open(args.path_to_config, "r") as file:
        try:
            config = json.load(file)
            label: str = config["label"]
            event_names: list[str] = [e["name"] for e in config["events"]]
        except Exception as e:
            raise Exception(f"Problem parsing {args.path_to_config}", e)

    if not event_names:
        raise Exception("can't parse trace without a list of events")

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

    if not args.output_path.endswith(".fuchsiaperf.json"):
        raise Exception("output path must use .fuchsiaperf.json extension")
    model: trace_model.Model = trace_importing.create_model_from_file_path(
        path_to_trace_json
    )
    trace_results = wakeup.WakeupMetricsProcessor(
        label, event_names
    ).process_metrics(model)
    metrics.TestCaseResult.write_fuchsiaperf_json(
        results=trace_results,
        test_suite="Manual",
        output_path=pathlib.Path(args.output_path),
    )


if __name__ == "__main__":
    main()

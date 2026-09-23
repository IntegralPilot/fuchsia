#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""
Returns CPU breakdown in a JSON file.
"""

import argparse
import json
import pathlib
import sys

from trace_processing import hardware_configs, trace_importing, trace_model
from trace_processing.metrics import agg_cpu_breakdown, cpu

# Default cut-off for the percentage CPU. Any process that has CPU below this
# won't be listed in the results. User can pass in a cutoff.
DEFAULT_PERCENT_CUTOFF = 0.0


def main() -> None:
    argv = sys.argv[1:]
    parser = argparse.ArgumentParser(
        description="Breaks down CPU usage by threads that use the highest"
        " percentage of the CPU over the runtime of the trace.",
    )
    """
    Takes in a trace file in JSON format and outputs
    a free-form JSON with breakdown metrics to `output_path`.
    """

    subparsers = parser.add_subparsers(
        title="Analysis Options",
        description="Ways to break down the data provided in the trace",
        required=True,
    )

    cpu_subparser = subparsers.add_parser(
        "by_cpu",
        help="Analyzes the trace file data and reports the threads that"
        " take the most CPU time over the duration of the trace, organized"
        " by CPU.",
    )
    cpu_subparser.add_argument(
        "path_to_trace",
        type=str,
        help="Path to trace file, either .fxt or .json",
    )
    cpu_subparser.add_argument(
        "output_path", type=str, help="Output path for CPU breakdown, as .json"
    )
    cpu_subparser.add_argument(
        "--percent_cutoff",
        type=float,
        default=DEFAULT_PERCENT_CUTOFF,
        help="percent cutoff (CPU usage less than this percent will not be logged)",
    )
    parser.add_argument(
        "--group_processes",
        type=bool,
        default=False,
        help="Saves CPU breakdown by processes only (not thread-level granularity)",
    )

    agg_subparser = subparsers.add_parser(
        "by_freq",
        help="Analyzes the trace file data and reports the threads that"
        " take the most CPU time over the duration of the trace, aggregated"
        " over each unique CPU frequency present in the hardware profile.",
    )
    agg_subparser.add_argument(
        "path_to_trace",
        type=str,
        help="Path to trace file, either .fxt or .json",
    )
    agg_subparser.add_argument(
        "output_path", type=str, help="Output path for CPU breakdown, as .json"
    )
    agg_subparser.add_argument(
        "--hardware_profile",
        type=str,
        default="vim3",
        help="The hardware configuration profile to use for CPU frequencies",
    )
    agg_subparser.add_argument(
        "--percent_cutoff",
        type=float,
        default=DEFAULT_PERCENT_CUTOFF,
        help="percent cutoff (CPU usage less than this percent will not be logged)",
    )
    cpu_subparser.set_defaults(
        func=lambda args: RunCpuBreakdown(args, trace_path_json)
    )
    agg_subparser.set_defaults(
        func=lambda args: RunAggBreakdown(args, trace_path_json)
    )

    args: argparse.Namespace = parser.parse_args(argv)
    trace_path_json: str = args.path_to_trace

    if args.path_to_trace.endswith(".fxt"):
        trace2json_path = pathlib.Path(sys.argv[0]).parent / "trace2json"
        trace_path_json = trace_importing.convert_trace_file_to_json(
            trace_path=args.path_to_trace,
            trace2json_path=trace2json_path,
        )
    elif not args.path_to_trace.endswith(".json"):
        parser.print_help()
        return
    args.func(args)


# Gets the CPU breakdown and outputs it as a freeform JSON file.
def RunCpuBreakdown(args: argparse.Namespace, trace_path_json: str) -> None:
    model: trace_model.Model = trace_importing.create_model_from_file_path(
        trace_path_json
    )
    processor = cpu.CpuMetricsProcessor(percent_cutoff=args.percent_cutoff)

    _, breakdown = processor.process_freeform_metrics(model)

    if args.group_processes:
        breakdown = cpu.group_by_process_name(breakdown)

    with open(args.output_path, "w") as json_file:
        json.dump(breakdown, json_file, indent=4)


# Gets the CPU breakdown and aggregates it over the CPU frequencies,
# outputting it as a freeform JSON file.
def RunAggBreakdown(args: argparse.Namespace, trace_path_json: str) -> None:
    model: trace_model.Model = trace_importing.create_model_from_file_path(
        trace_path_json
    )
    (breakdown, total_time) = cpu.CpuMetricsProcessor(
        percent_cutoff=args.percent_cutoff
    ).process_metrics_and_get_total_time(model)
    hardware_profile = hardware_configs.configs[args.hardware_profile]
    agg_breakdown = agg_cpu_breakdown.AggCpuBreakdownMetricsProcessor(
        hardware_profile, total_time, args.percent_cutoff
    ).aggregate_metrics(breakdown)

    with open(args.output_path, "w") as json_file:
        json.dump(agg_breakdown, json_file)


if __name__ == "__main__":
    print(sys.argv[:1])

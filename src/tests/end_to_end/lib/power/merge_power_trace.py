#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""A small wrapper executable around the trace merging library"""

import argparse
import enum
import pathlib
import shutil
import sys

from power import powerserver
from trace_processing import trace_importing, trace_model


class PowerFileFormat(enum.Enum):
    # Use lowercase names here to avoid confusion in the command line flag.

    monsoon = "monsoon"


def merge_trace(
    trace: str = "",
    power: str = "",
    power_format: PowerFileFormat = PowerFileFormat.monsoon,
    output: str = "",
    gpio: bool = True,
) -> None:
    trace_json_path: str = trace_importing.convert_trace_file_to_json(
        # trace2json will be placed in the same output directory as this tool
        trace2json_path=pathlib.Path(sys.argv[0]).parent / "trace2json",
        trace_path=trace,
    )

    model: trace_model.Model = trace_importing.create_model_from_file_path(
        trace_json_path
    )

    shutil.copy(trace, output)
    samples = powerserver.read_power_samples(power)
    powerserver.merge_power_data(model, samples, output)


def main() -> None:
    parser = argparse.ArgumentParser(
        "fx merge_power_trace",
        description="Align and merge together a power.csv and a trace file",
        exit_on_error=False,
    )
    parser.add_argument(
        "--trace",
        type=str,
        default="",
        required=True,
        help=".fxt file to read trace data from",
    )
    parser.add_argument(
        "--power",
        type=str,
        default="",
        required=True,
        help=".csv file to read power data from",
    )
    parser.add_argument(
        "--power_format",
        type=PowerFileFormat,
        choices=list(PowerFileFormat),
        default=PowerFileFormat.monsoon,
        help="format of the power .csv file",
    )
    parser.add_argument(
        "--output",
        type=str,
        default="trace_with_power.fxt",
        help=".fxt file to output to",
    )
    parser.add_argument(
        "--gpio",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Synchronize based on GPIO toggle signals",
    )
    args = parser.parse_args()
    if not args:
        parser.print_usage()
    merge_trace(**vars(args))


if __name__ == "__main__":
    main()

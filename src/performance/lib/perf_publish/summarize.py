# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Provides utilities for summarizing fuchsiaperf json files and writing files in
a concise and human-readable format.
"""

import json
import os
import statistics
from typing import Any, Iterable, TextIO


def write_fuchsiaperf_json(
    destination_file: TextIO, json_list: list[Any]
) -> None:
    """Writes a human readable json file in a more concise way than pretty json."""
    destination_file.write("[")
    is_first: bool = True
    for entry in json_list:
        if not is_first:
            destination_file.write(",\n")
        json.dump(entry, destination_file)
        is_first = False
    destination_file.write("]\n")


def mean_excluding_warm_up(values: list[int | float]) -> float:
    """Returns the mean of the given list of values excluding the first one."""
    if len(values) == 0:
        raise ValueError("Cannot calculate mean of empty list")
    if len(values) == 1:
        return values[0]
    return statistics.mean(values[1:])


def summarize_perf_files(
    json_files: Iterable[str | os.PathLike[str]],
) -> list[dict[str, Any]]:
    """
    This function takes a set of "raw data" fuchsiaperf files as input
    and produces the contents of a "summary" fuchsiaperf file as output.
    The output contains just the average values for each test case.

    See `//docs/development/performance/metric_name_expectations.md` for a
    description of how the summarization is done and what benefits it provides.
    """
    output_by_name: dict[tuple[str, str], dict[str, Any]] = {}
    result: list[dict[str, Any]] = []
    for json_file in json_files:
        with open(json_file, "r") as f:
            json_data = json.load(f)
        if not isinstance(json_data, list):
            raise ValueError("Top level fuchsiaperf node must be a list")
        for entry in json_data:
            full_name = (entry["test_suite"], entry["label"])
            output_entry = output_by_name.get(full_name)
            if output_entry is None:
                output_entry = {
                    "label": entry["label"],
                    "test_suite": entry["test_suite"],
                    "unit": entry["unit"],
                    "values": [],
                }
                output_by_name[full_name] = output_entry
                result.append(output_entry)
            elif entry["unit"] != output_entry["unit"]:
                raise ValueError(
                    f"Inconsistent units in fuchsiaperf results: "
                    f'"{entry["unit"]}" and {output_entry["unit"]}'
                    f'for test "{entry["test_suite"]}", "{entry["label"]}"'
                )
            output_entry["values"].append(
                mean_excluding_warm_up(entry["values"])
            )
    for entry in result:
        entry["values"] = [statistics.mean(entry["values"])]
    return result

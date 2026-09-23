#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Display a post-build summary of RBE metrics.
"""

import argparse
import collections
import dataclasses
import json
import os
import sys
from collections.abc import Callable, Iterable, Sequence
from pathlib import Path
from typing import Any

import tablefmt

# Rather than depend on the proto (from reclient source),
import textpb

_SCRIPT = Path(__file__)


def _main_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Display RBE metrics from a build.",
        argument_default=None,
    )

    parser.add_argument(
        "--format",
        type=str,
        help="Style of output.  {table: human-readable text, json: JSON}",
        default="table",
        choices=["table", "json"],
        metavar="STYLE",
    )

    # Positional args
    parser.add_argument(
        "reproxy_logdir",
        type=Path,
        help="The reproxy log dir of the build to summarize",
        metavar="DIR",
    )

    return parser


_MAIN_ARG_PARSER = _main_arg_parser()


def labels_to_dict(labels: str) -> dict[str, str]:
    result = {}
    for label in labels.split(","):
        k, _, v = label.partition("=")
        result[k] = v
    return result


def get_action_category_from_labels(labels: str) -> str:
    labels_dict = labels_to_dict(labels)
    action_lang = labels_dict.get("lang", "")
    action_type = labels_dict.get("type", "")
    action_tool = labels_dict.get("tool", "")
    action_toolname = labels_dict.get("toolname", "")
    if action_lang == "cpp" and action_type == "compile":
        return "cxx"
    if action_tool == "clang" and action_type == "link":
        return "link"
    if action_toolname == "rustc":
        return "rust"

    return action_toolname or "other"


def get_action_category_and_metric(text: str) -> tuple[str | None, str]:
    if not text.startswith("["):
        return None, text
    if "]." not in text:
        return None, text
    labels, _, metric = text.removeprefix("[").partition("].")
    if labels:
        return get_action_category_from_labels(labels), metric
    return None, metric


def _get_stat_name(stat: dict[str, Any]) -> str:
    # based on textpb structure and stats.Stat_pb2 proto
    return stat["name"][0].text.strip('"')


def _get_stat_count(stat: dict[str, Any]) -> int:
    return int(stat["count"][0].text)


def _counts_by_value_to_dict(fields: Iterable[Any]) -> dict[str, int]:
    # based on the structure returned by textpb.parse()
    return {_get_stat_name(entry): _get_stat_count(entry) for entry in fields}


def build_metric_table(
    name: str,
    data_source: dict[str, dict[str, int]],  # [action_category][value]: count
    column_headers: Sequence[str],
    include_header: bool = True,
    with_totals: bool = False,
) -> Sequence[Sequence[str | int]]:
    """Construct a numeric table of data (2D)."""
    rows: set[str] = set()
    for v in data_source.values():
        rows.update(v.keys())

    ordered_rows = sorted(rows)

    table = tablefmt.create_table(len(rows), len(column_headers) + 1)
    totals_row: list[str | int] = ["total"] + [
        0 for _ in range(len(column_headers))
    ]
    for r, row in enumerate(ordered_rows):
        table[r][0] = "  " + row  # visual hang-indentation
        for c, col in enumerate(column_headers):
            count = data_source[col].get(row, 0)
            table[r][c + 1] = count
            totals_row[c + 1] = (
                int(totals_row[c + 1]) + count
            )  # for the "total" row

    # Assemble the table with optional header/totals.
    final_table: list[Sequence[Any]] = []
    if include_header:
        final_table.append(tablefmt.make_table_header(column_headers, name))
    else:
        final_table.append(
            tablefmt.make_separator_row(len(column_headers), name)
        )

    final_table.extend(table)

    if with_totals:
        final_table.append(totals_row)

    return final_table


def build_metric_row(
    name: str,
    data_source: dict[str, int],  # [action_category]: number
    column_headers: Sequence[str],
    include_header: bool = True,
    formatter: Callable[[int], str] | None = None,
) -> Sequence[Sequence[str | int]]:
    """Construct one 1xN array (row) of data."""
    row = tablefmt.create_row(len(column_headers) + 1)
    row[0] = name
    for c, col in enumerate(column_headers):
        value = data_source.get(col, 0)
        if formatter is not None:
            row[c + 1] = formatter(value)
        else:
            row[c + 1] = value

    if include_header:
        return [tablefmt.make_table_header(column_headers), row]

    return [row]


@dataclasses.dataclass
class RbeMetrics(object):
    status_metrics: dict[str, dict[str, Any]]
    bandwidth_metrics: dict[str, dict[str, Any]]


def load_rbe_metrics(data: dict[str, Any]) -> RbeMetrics:
    """Extracts RBE metrics into a structure of tables."""
    # Construct a table by action type
    status_metrics: dict[str, dict[str, Any]] = collections.defaultdict(dict)
    bandwidth_metrics: dict[str, dict[str, Any]] = collections.defaultdict(dict)

    if "stats" not in data:
        return RbeMetrics(
            status_metrics=status_metrics,
            bandwidth_metrics=bandwidth_metrics,
        )

    stats = {_get_stat_name(stat): stat for stat in data["stats"]}

    for name, fields in stats.items():
        # Extract labels from name (if applicable)
        action_category, metric_name = get_action_category_and_metric(name)
        action_category = action_category or "all"

        # Pick some interesting metrics to display
        if "Status" in metric_name:  # e.g. "Result.Status", "CompletionStatus"
            counts_by_value = _counts_by_value_to_dict(
                fields["counts_by_value"]
            )
            status_metrics[metric_name][action_category] = counts_by_value

        if (
            "Downloaded" in metric_name
            or "Uploaded" in metric_name
            or "TotalOutputBytes" in metric_name
        ):
            bandwidth_metrics[metric_name][action_category] = _get_stat_count(
                fields
            )

    return RbeMetrics(
        status_metrics=status_metrics,
        bandwidth_metrics=bandwidth_metrics,
    )


def build_summary_lines(
    rbe_data: RbeMetrics, reproxy_logdir: str
) -> Iterable[str]:
    joint_table = prepare_summary_table(rbe_data)

    # Render multi-table.
    script_rel = os.path.relpath(str(_SCRIPT), start=os.curdir)
    yield f"=== Remote build summary (from: {script_rel} {reproxy_logdir})"
    yield from tablefmt.format_numeric_table(joint_table)


def _format_num_bytes(x: int) -> str:
    """Format readable numbers, e.g. "6.4 MiB"."""
    return tablefmt.human_readable_size(x, "B", 1)


def prepare_summary_table(
    rbe_data: RbeMetrics,
) -> Sequence[Sequence[str | int]]:
    # Prepare table columns, by action categories.
    action_categories = {
        k for v in rbe_data.status_metrics.values() for k in v.keys()
    }
    if "all" in action_categories:
        # "all" is special, always in last position
        action_categories.remove("all")

    ordered_action_categories = sorted(action_categories) + ["all"]

    shared_header = tablefmt.make_table_header(
        ordered_action_categories, "[by action type]"
    )
    blank_row = tablefmt.make_separator_row(len(ordered_action_categories))

    # Align cells across multiple tables by viewing as a single table.
    # All cell values in these tables are numeric, and thus, right-aligned.
    joint_table = [
        shared_header,
        *build_metric_table(
            "CompletionStatus",
            rbe_data.status_metrics["CompletionStatus"],
            ordered_action_categories,
            include_header=False,
            with_totals=True,
        ),
        blank_row,
        *build_metric_row(
            "OutputBytes",
            rbe_data.bandwidth_metrics["RemoteMetadata.TotalOutputBytes"],
            ordered_action_categories,
            include_header=False,
            formatter=_format_num_bytes,
        ),
        *build_metric_row(
            "BytesDownloaded",
            rbe_data.bandwidth_metrics["RemoteMetadata.RealBytesDownloaded"],
            ordered_action_categories,
            include_header=False,
            formatter=_format_num_bytes,
        ),
        *build_metric_row(
            "BytesUploaded",
            rbe_data.bandwidth_metrics["RemoteMetadata.RealBytesUploaded"],
            ordered_action_categories,
            include_header=False,
            formatter=_format_num_bytes,
        ),
    ]
    return joint_table


def arrange_metrics_json(rbe_metrics: RbeMetrics) -> dict[str, Any]:
    return {
        "execution_statuses": rbe_metrics.status_metrics["CompletionStatus"],
        "data_sizes_bytes": {
            k.removeprefix("RemoteMetadata."): v
            for k, v in rbe_metrics.bandwidth_metrics.items()
        },
    }


def main(argv: Sequence[str]) -> int:
    args = _MAIN_ARG_PARSER.parse_args(argv)

    rbe_metrics_txt = args.reproxy_logdir / "rbe_metrics.txt"
    if not rbe_metrics_txt.exists():
        print("No RBE metrics found.")
        return 0

    with open(rbe_metrics_txt) as f:
        data = textpb.parse(f)

    rbe_data = load_rbe_metrics(data)

    def text_table_formatter(rbe_metrics: RbeMetrics) -> Iterable[str]:
        yield from build_summary_lines(rbe_metrics, args.reproxy_logdir)

    def json_lines_formatter(rbe_metrics: RbeMetrics) -> Iterable[str]:
        yield from json.dumps(
            arrange_metrics_json(rbe_metrics), indent="  "
        ).splitlines()

    output_formatters = {
        "table": text_table_formatter,
        "json": json_lines_formatter,
    }
    for line in output_formatters[args.format](rbe_data):
        print(line)

    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

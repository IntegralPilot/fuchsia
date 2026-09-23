# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import asyncio
import json
import sys
import typing
from collections import OrderedDict

import collectors
import data


class Options(argparse.Namespace):
    def __init__(self) -> None:
        self.format: str | None = "text"


def main(arg_override: typing.Sequence[str] | None = None) -> None:
    parser = argparse.ArgumentParser(
        description="Print status of your fx environment"
    )
    parser.add_argument(
        "-f",
        "--format",
        choices=["text", "json"],
        help="Format of the output",
    )

    args = parser.parse_args(args=arg_override, namespace=Options())

    asyncio.run(async_main(args))


async def async_main(args: Options) -> None:
    output_data = data.Data()

    futures = [
        collectors.environment_collector(),
        collectors.git_collector(),
        collectors.jiri_collector(),
        collectors.args_gn_collector(),
    ]

    # Wrap futures in tasks so they execute asynchronously.
    tasks = list(map(lambda x: asyncio.create_task(x), futures))

    # Iterate over tasks in order, waiting for each to end.
    for task in tasks:
        results = await task
        output_data.add_results(results)

    if args.format == "text" or args.format is None:
        print_text(output_data)
    elif args.format == "json":
        print_json(output_data)


def print_text(output_data: data.Data) -> None:
    for category, items in output_data.items.items():
        print(f"{category.pretty_str()}:")

        for item in items:
            if isinstance(item.value, list) and not item.value:
                # Skip empty lists
                continue
            notes = f" ({item.notes})" if item.notes else ""
            print(f"  {item.title}: {item.normalized_value()}{notes}")


def print_json(output_data: data.Data) -> None:
    result = OrderedDict()

    for key, items in output_data.items.items():
        item_dict = OrderedDict()
        for item in items:
            if isinstance(item.value, list) and len(item.value) == 0:
                continue
            item_dict[item.key] = item.to_dict()

        result[key] = OrderedDict(name=key.pretty_str(), items=item_dict)

    json.dump(result, sys.stdout)
    print("")

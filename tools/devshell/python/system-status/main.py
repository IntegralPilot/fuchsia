# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import typing

import ffx_cmd
import statusinfo
import termout


class Options(argparse.Namespace):
    def __init__(self) -> None:
        self.style: bool | None = None


async def main(arg_override: typing.Sequence[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description="Check whole system status")
    parser.add_argument(
        "--style",
        action=argparse.BooleanOptionalAction,
        help="If set, style output regardless of terminal type.",
        default=None,
    )

    args = parser.parse_args(args=arg_override, namespace=Options())

    style = args.style if args.style is not None else termout.is_valid()

    output = await ffx_cmd.inspect("**:root/fuchsia.inspect.Health:*").sync()

    if not output.data:
        print(statusinfo.warning("No component health info found", style=style))

    for out in output.data:
        if out.payload is None:
            continue

        status = out.payload["root"]["fuchsia.inspect.Health"]["status"]
        start_time = out.payload["root"]["fuchsia.inspect.Health"].get(
            "start_timestamp_nanos"
        )
        if start_time is not None:
            uptime = out.metadata.timestamp.nanoseconds() - start_time
        else:
            uptime = 0
        uptime = statusinfo.format_duration(float(uptime) / 1e9)

        highlighter: typing.Callable[[str], str]
        if status == "OK":
            highlighter = lambda s: statusinfo.green_highlight(s, style=style)
        elif status == "STARTING_UP":
            highlighter = lambda s: statusinfo.warning(s, style=style)
        else:
            highlighter = lambda s: statusinfo.error_highlight(s, style=style)

        print(highlighter(out.moniker))
        print(f"  Status: {highlighter(status)}")
        print(f"  Uptime: {uptime}")

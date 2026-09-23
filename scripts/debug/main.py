# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import asyncio
import sys

import package_server


async def main(args: list[str] | None = None) -> int:
    args = sys.argv[1:] if args is None else args
    parser = argparse.ArgumentParser(
        description="Fuchsia Debugging Wrapper", add_help=False
    )
    parser.add_argument(
        "command", nargs="?", help="Command to run (e.g., 'cli')"
    )

    parsed_args, fallback_args = parser.parse_known_args(args)

    if parsed_args.command == "cli":
        from cli.cli import main as cli_main

        return await cli_main(fallback_args)

    if args and args[0] in ("-h", "--help"):
        print("Fuchsia Debugging Wrapper")
        print("Usage:")
        print("  fx debug cli [command]  : Use the machine-friendly CLI")
        print(
            "  fx debug [args...]      : Start interactive zxdb session with args"
            " (see ffx debug connect --help"
        )
        return 0

    async with package_server.ensure_running():
        # TODO(https://fxbug.dev/464692993): Refactor `//scripts/fxtest/python/debugger.py` into a
        # shared library and use it here.
        ffx_cmd = await asyncio.subprocess.create_subprocess_exec(
            "ffx", "debug", "connect", *args, stdin=sys.stdin
        )

        try:
            return await ffx_cmd.wait()
        except asyncio.CancelledError:
            # This block triggers if the python script receives SIGINT (Ctrl+C)
            # or SIGTERM, and zxdb hasn't already exited.

            # Gracefully shutdown zxdb.
            ffx_cmd.terminate()

            # Give zxdb a few seconds to clean up internal state
            await asyncio.wait_for(ffx_cmd.wait(), timeout=3.0)

            # Forcefully kill zxdb if it hasn't stopped yet.
            if ffx_cmd.returncode is None:
                ffx_cmd.kill()
                await ffx_cmd.wait()
                return -9

            return ffx_cmd.returncode


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))

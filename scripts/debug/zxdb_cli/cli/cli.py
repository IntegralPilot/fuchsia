# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import asyncio
import json
import sys
from typing import Any, Final

from cli.commands import (
    async_backtrace,
    attach,
    break_cmd,
    continue_cmd,
    detach,
    evaluate,
    finish,
    get_state,
    next_cmd,
    pause,
    schema,
    stack_trace,
    start,
    step_in,
    stop,
    threads,
    variables,
    wait_for_event,
)
from cli.commands.base import BaseCommand
from daemon_manager.manager import UDS_PATH
from pydantic import ValidationError
from shared.protocol import (
    BaseRequest,
    deserialize_response,
    make_request,
    serialize,
)

# The maximum number of seconds that we will wait for the daemon to start.
# In particular, this is how long we will wait for the daemon to write into
# the pipe FD that we pass to it when we start the new process.
DAEMON_STARTUP_TIMEOUT_SECS: Final[float] = 10.0


def request_to_namespace(req: BaseRequest[Any]) -> argparse.Namespace:
    """Converts a BaseRequest to argparse.Namespace for command executors."""
    return argparse.Namespace(**req.model_dump())


async def main(args: list[str]) -> int:
    parser = argparse.ArgumentParser(description="fx debug cli")
    parser.add_argument("--json", help="JSON request string")
    parser.add_argument(
        "--ack-seq",
        type=int,
        help="Acknowledge events up to this sequence number",
    )
    subparsers = parser.add_subparsers(dest="command", required=False)

    # Statically register commands.
    commands: dict[str, type[BaseCommand]] = {}
    command_classes = [
        # keep-sorted start
        async_backtrace.Command,
        attach.Command,
        break_cmd.Command,
        continue_cmd.Command,
        detach.Command,
        evaluate.Command,
        finish.Command,
        get_state.Command,
        next_cmd.Command,
        pause.Command,
        schema.Command,
        stack_trace.Command,
        start.Command,
        step_in.Command,
        stop.Command,
        threads.Command,
        variables.Command,
        wait_for_event.Command,
        # keep-sorted end
    ]
    for cmd_class in command_classes:
        existing_choices = set(subparsers.choices.keys())
        cmd_class.register_cli(subparsers)
        new_choices = set(subparsers.choices.keys()) - existing_choices
        for choice in new_choices:
            commands[choice] = cmd_class

    alias_map: dict[str, str] = {
        alias: cmd_class.COMMAND_NAME
        for cmd_class in command_classes
        for alias in getattr(cmd_class, "ALIASES", [])
        if cmd_class.COMMAND_NAME
    }

    parsed_args = parser.parse_args(args)

    if parsed_args.json and parsed_args.command:
        print(
            "Error: --json and command are mutually exclusive", file=sys.stderr
        )
        return 1

    if not parsed_args.json and not parsed_args.command:
        print(
            "Error: Either --json or a command must be provided",
            file=sys.stderr,
        )
        return 1

    req: BaseRequest[Any] | None = None
    if parsed_args.json:
        try:
            data = json.loads(parsed_args.json)
            if not isinstance(data, dict):
                raise ValueError("JSON payload must be an object")
            if "command" in data and data["command"] in alias_map:
                data["command"] = alias_map[data["command"]]
            req = make_request(data)
            cmd_cls: type[BaseCommand] | None = commands.get(req.command)
            if cmd_cls is not None:
                exit_code = await cmd_cls.execute(request_to_namespace(req))
                if exit_code is not None:
                    return exit_code
        except (ValueError, ValidationError) as e:
            print(f"Error: {e}", file=sys.stderr)
            return 1
    elif parsed_args.command:
        cmd_cls = commands.get(parsed_args.command)
        if cmd_cls is not None:
            exit_code = await cmd_cls.execute(parsed_args)
            if exit_code is not None:
                return exit_code
        try:
            args_dict = vars(parsed_args)
            if cmd_cls is not None and cmd_cls.COMMAND_NAME:
                args_dict["command"] = cmd_cls.COMMAND_NAME
            req = make_request(args_dict)
        except (ValueError, ValidationError) as e:
            print(f"Error: {e}", file=sys.stderr)
            return 1

    assert req is not None
    assert isinstance(req, BaseRequest)
    if parsed_args.ack_seq is not None:
        req.ack_seq = parsed_args.ack_seq
    return await send_command(req)


async def send_command(req: BaseRequest[Any]) -> int:
    if not UDS_PATH.exists():
        print(
            f"Daemon socket not found at {UDS_PATH}.\n\n"
            "To start the daemon, run:\n"
            "  fx debug cli start\n\n"
            "Or run your tests with debugging enabled:\n"
            "  fx test <test_name> --agent-debugging-mode",
            file=sys.stderr,
        )
        return 1

    try:
        reader, writer = await asyncio.open_unix_connection(UDS_PATH)
    except Exception as e:
        print(f"Error communicating with daemon: {e}", file=sys.stderr)
        return 1

    try:
        writer.write(serialize(req).encode("utf-8"))
        await writer.drain()

        try:
            response_line = await asyncio.wait_for(
                reader.readline(), timeout=5.0
            )
        except asyncio.TimeoutError:
            print(
                "Timed out waiting for response from daemon.", file=sys.stderr
            )
            return 1

        if response_line:
            try:
                resp = deserialize_response(response_line.decode("utf-8"), req)
                print(serialize(resp).strip())
            except ValidationError as e:
                print(
                    f"Error: Invalid response from daemon: {e}",
                    file=sys.stderr,
                )
                return 1
        else:
            print("No response received from daemon.", file=sys.stderr)
            return 1
        return 0
    except Exception as e:
        print(f"Error communicating with daemon: {e}", file=sys.stderr)
        return 1
    finally:
        writer.close()
        try:
            await writer.wait_closed()
        except Exception:
            pass


if __name__ == "__main__":
    sys.exit(asyncio.run(main(sys.argv[1:])))

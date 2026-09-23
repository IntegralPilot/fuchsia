#!/usr/bin/env fuchsia-vendored-python

# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Generates a bash completion script for `ffx` tool.

Usage to generate from ffx output:
  source <(python3 scripts/ffx_complete/ffx_gen_complete.py --ffx_help_json_file_path=<(fx ffx --machine json --help))

Usage to generate from ffx golden files:
  source <(python3 scripts/ffx_complete/ffx_gen_complete.py --ffx_golden_dir_path=src/developer/ffx/tests/cli-goldens/goldens)
"""
import argparse
import dataclasses
import json
import os
import pathlib
import sys
import textwrap
import typing
from typing import Any, Iterable, Sequence, TypeAlias

JSONObject: TypeAlias = dict[str, "JSONValue"]
JSONValue: TypeAlias = JSONObject | str | int | float | list["JSONValue"]

try:
    import dataclasses_json_lite
except ImportError:
    # Make possible to run out of `fx` command.
    sys.path.insert(0, os.path.join(os.path.dirname(__file__), "../memory"))
    import dataclasses_json_lite

# Generate completion from a command flag.
# Key is (command_name, flag_name).
# Value is a bash expression enumerating value separated by spaces.
ARG_NAME_TO_EXPR = {
    ("Ffx", "target"): "$(ffx target list -f n)",
    ("Ffx", "machine"): "json json-pretty raw",
    ("Ffx", "log-level"): "Info Error Warn Trace",
}


@dataclasses_json_lite.dataclass_json()
@dataclasses.dataclass
class Option:
    arg_name: str


@dataclasses_json_lite.dataclass_json()
@dataclasses.dataclass
class Kind:
    option: Option | None = dataclasses.field(
        metadata=dataclasses_json_lite.config(field_name="Option"), default=None
    )
    text: str | None = None


def kind_or_string(json_data: dict[str, Any] | str) -> Kind | str:
    if isinstance(json_data, dict):
        return Kind.from_dict(json_data)  # type: ignore[attr-defined]
    elif isinstance(json_data, str):
        return Kind(text=json_data)
    else:
        raise ValueError(
            f"Kind should be a string or a dict but was {json_data!r}"
        )


@dataclasses_json_lite.dataclass_json()
@dataclasses.dataclass
class Flag:
    kind: Kind | str = dataclasses.field(
        metadata=dataclasses_json_lite.config(decoder=kind_or_string)
    )
    optionality: str
    long: str
    short: str | None
    description: str
    hidden: bool


@dataclasses_json_lite.dataclass_json()
@dataclasses.dataclass
class Positional:
    name: str
    description: str
    optionality: str
    hidden: bool


@dataclasses_json_lite.dataclass_json()
@dataclasses.dataclass
class ErrorCode:
    code: int
    description: str


@dataclasses_json_lite.dataclass_json()
@dataclasses.dataclass
class Command:
    name: str
    description: str
    examples: Sequence[str]
    flags: Sequence[Flag]
    commands: Sequence["SubCommand"]
    positionals: Sequence[Positional]
    notes: Sequence[str] | None
    error_codes: Sequence[ErrorCode] | None


@dataclasses_json_lite.dataclass_json()
@dataclasses.dataclass
class SubCommand:
    name: str
    command: Command


def print_flag(
    command: Command, flag: Flag, indent: int, file: typing.TextIO
) -> None:
    def out(text: str) -> None:
        print(textwrap.indent(text, " " * indent), file=file)

    flag_names = [flag.long]
    if flag.short:
        flag_names.append(f"-{flag.short}")

    for flag_name in flag_names:
        out(f"{flag_name})")
        if isinstance(flag.kind, Kind) and flag.kind.option:
            key = (command.name, flag.kind.option.arg_name)
            if key in ARG_NAME_TO_EXPR:
                out(f"  if (( word_index + 1 == COMP_CWORD)); then")
                out(
                    f"""    COMPREPLY=($(compgen -W "{ARG_NAME_TO_EXPR[key]}" -- "${{COMP_WORDS[$COMP_CWORD]}}"))"""
                )
                out(f"    return")
                out(f"  fi")

        out(f"  ((word_index+=2))")
        out(f"  ;;")


def print_command(command: Command, indent: int, file: typing.TextIO) -> None:
    def out(text: str) -> None:
        print(textwrap.indent(text, " " * indent), file=file)

    out("""while [ "$word_index" -lt "$COMP_CWORD" ]; do""")
    out("""  case "${COMP_WORDS[$word_index]}" in""")
    for flag in command.flags:
        print_flag(command, flag, indent + 4, file)

    for sub_cmd in command.commands:
        out(f"    {sub_cmd.name})")
        out(f"      ((word_index++))")
        print_command(sub_cmd.command, indent + 6, file)
        out(f"      ;;")

    out("    *)")
    out("      ((word_index++))")
    out("    esac")
    out("  done")
    words = []
    words += [flag.long for flag in command.flags]
    words += [f"-{flag.short}" for flag in command.flags if flag.short]
    words += [f"{sub_cmd.name}" for sub_cmd in command.commands]
    out(
        f"""COMPREPLY=($(compgen -W "{' '.join(words)}" -- "${{COMP_WORDS[$COMP_CWORD]}}"))"""
    )
    out("""return""")


def print_script(root_command: Command, file: typing.TextIO) -> None:
    print("#/usr/bin/env bash", file=file)
    print("_ffx_completions() {", file=file)
    print("word_index=1", file=file)
    print_command(root_command, 2, file)
    print("""}""", file=file)
    print("""complete -F _ffx_completions ffx""", file=file)


def find_sub_command_list(
    sub_commands_root: list[JSONObject], path_elements: Iterable[str]
) -> list[JSONObject]:
    current_sub_commands = sub_commands_root
    for element in path_elements:
        for command in current_sub_commands:
            if command["name"] == element:
                assert isinstance(command["command"], dict)
                assert isinstance(command["command"]["commands"], list)
                current_sub_commands = command["command"]["commands"]  # type: ignore
                break
        else:
            raise ValueError(
                f"No command {element} found as part of {path_elements} integration."
            )
    return current_sub_commands


def load_help_json_from_golden(golden_dir_root: pathlib.Path) -> JSONObject:
    """Build the command hierarchy from the .golden JSON files.

    Golden files contains a single command, and are laid out in
    a directory structure matching the command hierarchy."""
    command_jsons: list[JSONObject] = []
    for golden_file in golden_dir_root.rglob("*.golden"):
        sub_command_list = find_sub_command_list(
            command_jsons, golden_file.parent.relative_to(golden_dir_root).parts
        )
        json_data = json.load(open(golden_file, "r"))
        if golden_file.stem != json_data["name"].lower():
            raise ValueError(
                f"file stem is {golden_file.stem} and command name is {json_data['name'].lower()}"
            )
        sub_command_list.append(
            {"name": golden_file.stem, "command": json_data}
        )

    if len(command_jsons) != 1:
        raise ValueError(
            f"more than one root command found which is unexpected."
        )
    result = command_jsons[0]["command"]
    assert isinstance(result, dict)
    return result


def main() -> None:
    parser = argparse.ArgumentParser(
        prog="ffx_gen_complete",
        description="Generate a bash complete script for ffx tool",
        epilog="Try: source <(python3 scripts/ffx_complete/ffx_gen_complete.py --ffx_help_json_file_path=<(ffx --machine json --help))",
    )
    parser.add_argument(
        "--ffx_golden_dir_path",
        default=os.environ.get("FUCHSIA_DIR", ".")
        + "/src/developer/ffx/tests/cli-goldens/goldens",
        type=pathlib.Path,
        help="path to ffx/tests/cli-golden files",
        metavar="DIR",
    )
    parser.add_argument(
        "--ffx_help_json_file_path",
        type=argparse.FileType("r", encoding="UTF-8"),
        help="path to JSON machine output of ffx help",
        metavar="FILE",
    )
    args = parser.parse_args()

    if args.ffx_help_json_file_path:
        command_json = json.load(args.ffx_help_json_file_path)
    else:
        command_json = load_help_json_from_golden(args.ffx_golden_dir_path)

    root_command = Command.from_dict(command_json)  # type: ignore[attr-defined]
    print_script(root_command, file=sys.stdout)


if __name__ == "__main__":
    main()

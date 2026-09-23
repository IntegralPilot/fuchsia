# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This tool is to be invoked by the CTF trybot.  It is intended for
# comparing the FIDL methods that were exercised in CTF tests
# to the FIDL methods that exist in the SDK.

import argparse
import json
import os
import sys
from dataclasses import dataclass, fields
from io import TextIOWrapper
from pathlib import Path
from typing import Any, Dict, List

# TODO(b/379766709) (comment 21): Rewrite this in Go.

# The dataclasses form the output schemas of the script.
#
# The "expanded" schema has full info about endpoints on every call,
# and its schema is a list of `ExpandedMessage`.
#
# The "condensed" schema stores a dict of interned endpoints that calls refer into,
# and its schema is `CondensedSchema`.
#
# The "summary" schema is a dict from "fuchsia.library/Protocol.Message" to a dict with
# keys: "incoming", "outgoing", "intra"; values: int number of calls, and "unstable": bool.
#
# The "flat" schema is an array of dicts with keys "incoming", "outgoing", "intra" (int),
# "message" with value "fuchsia.library/Protocol.Message", and "unstable": bool. Same
# information as "summary" but it's all in the values, which is easier for SQL to unpack.


@dataclass(frozen=True)
class Endpoint:
    name: str
    url: str = ""
    moniker: str = ""


@dataclass
class CondensedCall:
    sender: int
    receiver: int
    direction: str


@dataclass
class CondensedMessage:
    name: str
    ordinal: str
    unstable: bool
    calls: List[CondensedCall]


@dataclass
class CondensedSchema:
    endpoints: Dict[int, Endpoint]
    messages: List[CondensedMessage]


@dataclass
class ExpandedCall:
    sender: Endpoint
    receiver: Endpoint
    direction: str


@dataclass
class ExpandedMessage:
    name: str
    ordinal: str
    unstable: bool
    calls: List[ExpandedCall]


@dataclass
class CallsSummary:
    unstable: bool
    incoming: int
    outgoing: int
    intra: int


@dataclass
class CallsFlat:
    message: str
    unstable: bool
    incoming: int
    outgoing: int
    intra: int


# Maintain an interned set of `Endpoint`s and return a different key for
# each unique `Endpoint`.
class Endpoints:
    def __init__(self) -> None:
        # This will be written to the condensed JSON file.
        self.endpoints_by_key: Dict[int, Endpoint] = {}
        # This interns endpoints we've seen before.
        self.keys_by_endpoint: Dict[Endpoint, int] = {}
        self.next_key = 101

    def get_key(self, endpoint: Endpoint) -> int:
        if endpoint in self.keys_by_endpoint:
            return self.keys_by_endpoint[endpoint]
        key = self.next_key
        self.next_key += 1
        self.keys_by_endpoint[endpoint] = key
        self.endpoints_by_key[key] = endpoint
        return key

    def data_to_write(self) -> Dict[int, Endpoint]:
        return self.endpoints_by_key


# The one-and-only intern-er for `Endpoint`s
ENDPOINTS = Endpoints()


# The endpoint info from the snoop files has fields we don't care about.
# ENDPOINT_FIELDS helps `cleanup_endpoint()` scrub the unwanted fields.
ENDPOINT_FIELDS = [f.name for f in fields(Endpoint)]


def cleanup_endpoint(endpoint: Dict[str, Any]) -> Dict[str, Any]:
    for key in list(endpoint.keys()):
        if key not in ENDPOINT_FIELDS:
            del endpoint[key]
    return endpoint


# Stores a single FIDL message. `name` is the name of the message, as
# `fuchsia.library/Protocol.Message` - unless the name is not in the CTF ordinal<>message
# map, in which case `name` is the same as `ordinal`. Accumulates both "expanded" and "condensed"
# versions of the call info for its message, and can supply output-schema classes for either.
class Message:
    def __init__(self, name: str, ordinal: str, unstable: bool) -> None:
        self.name = name
        self.ordinal = ordinal
        self.unstable = unstable
        self.condensed_calls: List[CondensedCall] = []
        self.expanded_calls: List[ExpandedCall] = []

    def add_calls(
        self, calls: List[Dict[str, Any]], test_path: Path, direction: str
    ) -> None:
        for call in calls:
            sender = Endpoint(**cleanup_endpoint(call["sender"]))
            receiver = Endpoint(**cleanup_endpoint(call["receiver"]))
            self.expanded_calls.append(
                ExpandedCall(
                    sender=sender,
                    receiver=call["receiver"],
                    direction=direction,
                )
            )
            self.condensed_calls.append(
                CondensedCall(
                    sender=ENDPOINTS.get_key(sender),
                    receiver=ENDPOINTS.get_key(receiver),
                    direction=direction,
                )
            )

    def call_data(self) -> Dict[str, Any]:
        data: Dict[str, Any] = {
            "incoming": 0,
            "outgoing": 0,
            "intra": 0,
            "unstable": self.unstable,
        }
        for call in self.condensed_calls:
            data[call.direction] += 1
        return data

    def summary(self) -> CallsSummary:
        data = self.call_data()
        return CallsSummary(**data)

    def flat(self) -> CallsFlat:
        data: Dict[str, Any] = self.call_data()
        data["message"] = self.name
        return CallsFlat(**data)

    def condensed_version(self) -> CondensedMessage:
        return CondensedMessage(
            name=self.name,
            ordinal=self.ordinal,
            unstable=self.unstable,
            calls=self.condensed_calls,
        )

    def expanded_version(self) -> ExpandedMessage:
        return ExpandedMessage(
            name=self.name,
            ordinal=self.ordinal,
            unstable=self.unstable,
            calls=self.expanded_calls,
        )


# Tells json how to write classes. Use by passing `default=vars_or_obj` to json.dump().
def vars_or_obj(obj: Any) -> Any:
    """Extracts fields from classes, or passes through built-in data types."""
    try:
        return vars(obj)
    except:
        return obj


# Main data structure of the program. Gathers data from all input files and outputs it
# to JSON in the correct schema - a list of Message with an item for every entry in the
# SDK API name<>ordinal map file (whether or not it has calls) and for every event in
# every scanned FIDL-snoop file (whether or not it's in the API).
class Messages:
    def __init__(self, message_file: TextIOWrapper) -> None:
        self.setup_data_structures(message_file)

    def setup_data_structures(self, message_file: TextIOWrapper) -> None:
        self.raw_methods = json.load(message_file)
        self.name_lookup: Dict[str, Message] = {}
        self.ordinal_lookup: Dict[str, Message] = {}
        # Schema is [{name: "fuchsia.library",
        #             methods: [{name: "fuchsia.library/Protocol.Message", ordinal: 1234}, ...]}, ...]
        for entry in self.raw_methods:
            for method in entry["methods"]:
                message = Message(
                    name=method["name"],
                    ordinal=method["ordinal"],
                    unstable=method.get("unstable", False),
                )
                self.name_lookup[method["name"]] = message
                self.ordinal_lookup[method["ordinal"]] = message

    def process_calls(self, path: Path, direction: str) -> None:
        with open(path) as f:
            calls = json.load(f)
            # Schema is { message: [call, call...], message: [call, call...]...}
            #   message is either an ordinal (as a string) or "fuchsia.library/Protocol/Message"
            #   call is a dict: {"sender": process, "receiver": process}
            #     process is a dict with keys: name, is_test, pid, url, moniker
            for message, call_list in calls.items():
                if message in self.ordinal_lookup:
                    self.ordinal_lookup[message].add_calls(
                        calls=call_list, test_path=path, direction=direction
                    )
                else:
                    if message not in self.name_lookup:
                        self.name_lookup[message] = Message(
                            name=message, ordinal=message, unstable=False
                        )
                    self.name_lookup[message].add_calls(
                        calls=call_list, test_path=path, direction=direction
                    )

    def data_to_write(self) -> list[Message]:
        return list(self.name_lookup.values())

    def write_expanded_to_json(self, path: Path) -> None:
        """Writes the desired data to the path in JSON format."""

        with open(path, "w") as f:
            json.dump(
                [
                    message.expanded_version()
                    for message in self.data_to_write()
                ],
                f,
                default=vars_or_obj,
                indent=2,
            )

    def write_condensed_to_json(self, path: Path) -> None:
        data = CondensedSchema(
            endpoints=ENDPOINTS.data_to_write(),
            messages=[
                message.condensed_version() for message in self.data_to_write()
            ],
        )
        with open(path, "w") as f:
            json.dump(data, f, default=vars_or_obj, indent=2)

    def write_summary_to_json(self, path: Path) -> None:
        data = dict(
            [
                (message.name, message.summary())
                for message in self.data_to_write()
            ]
        )
        with open(path, "w") as f:
            json.dump(data, f, default=vars_or_obj, indent=2)

    def write_flat_to_json(self, path: Path) -> None:
        data = [message.flat() for message in self.data_to_write()]
        with open(path, "w") as f:
            json.dump(data, f, default=vars_or_obj, indent=1)


# The core of this script. Scans for FIDL-snoop files, accumulates calls from them, and writes
# the accumulated information to specified files in the correct schema.
def check_coverage(args: argparse.Namespace) -> None:
    api_path = Path(args.api_file)
    with open(api_path) as f:
        messages = Messages(f)
    for root_dir in args.results_dir:
        for dir_path, _, file_names in os.walk(root_dir):
            for name in file_names:
                if name.endswith("intra_calls.freeform.json"):
                    messages.process_calls(Path(dir_path) / name, "intra")
                elif name.endswith("incoming_calls.freeform.json"):
                    messages.process_calls(Path(dir_path) / name, "incoming")
                elif name.endswith("outgoing_calls.freeform.json"):
                    messages.process_calls(Path(dir_path) / name, "outgoing")
    if args.json_output:
        messages.write_flat_to_json(Path(args.json_output))
    if args.expanded_json_output:
        messages.write_expanded_to_json(Path(args.expanded_json_output))
    if args.condensed_json_output:
        messages.write_condensed_to_json(Path(args.condensed_json_output))
    if args.summary_json_output:
        messages.write_summary_to_json(Path(args.summary_json_output))


def main(argv: List[str]) -> None:
    """This script takes an API file summarizing SDK FIDL methods (name <-> ordinal) and one or
    more directories containing test outputs, which should include FIDL-snoop files named
    `intra_calls.freeform.json`. It can write either of two schemas. (For soft-migration reasons,
    `--json_output` is the same as `--expanded_json_output`.)"""
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(required=True)

    subparser = subparsers.add_parser(
        "check_coverage",
        help="Summarize FIDL coverage from an API file and test result directories.",
    )
    subparser.add_argument(
        "--api_file",
        required=True,
        help="File summarizing FIDL API, produced by summarize_fidl_methods.py.",
    )
    subparser.add_argument(
        "--json_output",
        help="Path to write a flat (array of struct) FIDL coverage summary to.",
    )
    subparser.add_argument(
        "--expanded_json_output",
        help="Path to write expanded FIDL coverage summary to. Deprecated; use condensed.",
    )
    subparser.add_argument(
        "--condensed_json_output",
        help="Path to write condensed FIDL coverage summary to, with full call info.",
    )
    subparser.add_argument(
        "--summary_json_output",
        help="Path to write a brief (dict by message name) FIDL coverage summary to.",
    )
    subparser.add_argument(
        "--results_dir",
        nargs="+",
        help="Root directories of test outputs",
        required=True,
    )
    subparser.set_defaults(func=check_coverage)

    args = parser.parse_args(argv)
    args.func(args)


if __name__ == "__main__":
    main(sys.argv[1:])

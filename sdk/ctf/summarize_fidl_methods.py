# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import sys
from dataclasses import dataclass
from typing import TextIO

import depfile
from json_get import Any, JsonGet

# This script outputs a summary file of SDK FIDL methods (at version HEAD).
#
# Inputs:
#  --sdk-fidl-json   : path to sdk_fidl_json.json which lists all SDK FIDL libraries
#
# Outputs:
#  --method-summary  : path to output a JSON file which summarizes library/method in the SDK
#  --depfile         : path to output a list of files this script reads


@dataclass
class Method:
    name: str
    ordinal: str


@dataclass
class MethodWithUnstable:
    name: str
    ordinal: str
    unstable: bool

    @staticmethod
    def from_method(m: Method) -> "MethodWithUnstable":
        return MethodWithUnstable(name=m.name, ordinal=m.ordinal, unstable=True)


class ApiFromSummary:
    def __init__(self, name: str, api_file: TextIO) -> None:
        self.methods = []
        self.name = name
        raw_json_text = api_file.read()
        if not raw_json_text:
            # Some files are empty, and json won't parse that.
            api_data = json.loads("[]")
        else:
            api_data = json.loads(raw_json_text)
        for entry in api_data:
            if entry["kind"] == "protocol/member":
                self.methods.append(
                    Method(name=entry["name"], ordinal=entry["ordinal"])
                )
        self.methods.sort(key=lambda m: m.name)


class ApiFromFidlJson:
    def __init__(self, name: str, api_file: TextIO) -> None:
        self.methods: list[Method] = []
        self.name = name
        raw_json_text = api_file.read()
        if not raw_json_text:
            # Some files are empty, and json won't parse that.
            json_api_data = JsonGet("[]")
        else:
            json_api_data = JsonGet(raw_json_text)
        json_api_data.match(
            {"protocol_declarations": [Any]}, self.protocol_declarations
        )
        self.methods.sort(key=lambda m: m.name)

    def protocol_declarations(self, info: Any) -> None:
        for p in info.protocol_declarations:
            protocol = JsonGet(value=p)
            protocol.match(
                {"name": Any, "methods": [Any]}, self.method_declarations
            )

    def method_declarations(self, name_and_methods: Any) -> None:
        protocol_name = name_and_methods.name
        for m in name_and_methods.methods:
            method = JsonGet(value=m)
            name_and_ordinal = method.match({"name": Any, "ordinal": Any})
            self.methods.append(
                Method(
                    name=f"{protocol_name}.{name_and_ordinal.name}",
                    ordinal=str(name_and_ordinal.ordinal),
                )
            )


class ApiWithUnstable:
    def __init__(
        self, next_api: ApiFromFidlJson, head_api: ApiFromFidlJson
    ) -> None:
        stable_names = set([m.name for m in next_api.methods])
        self.name = head_api.name
        self.methods: list[Method | MethodWithUnstable] = []
        for m in head_api.methods:
            if m.name in stable_names:
                self.methods.append(m)
            else:
                self.methods.append(MethodWithUnstable.from_method(m))


class App:
    def __init__(self, argv: list[str]) -> None:
        self.parse_args(argv)
        self.dep_paths: list[str] = []
        self.apis: list[ApiFromSummary] | list[ApiWithUnstable] = []
        self.fidl_files = JsonGet(self.args.sdk_fidl_json.read()).match(
            [{"ir": Any, "name": Any}]
        )

    def parse_args(self, argv: list[str]) -> None:
        parser = argparse.ArgumentParser()
        parser.add_argument(
            "--sdk-fidl-json",
            help="Path to //out/*/sdk_fidl_json.json",
            required=True,
            type=argparse.FileType("r"),
        )
        parser.add_argument(
            "--method-summary",
            help="Path to write the method summary",
            required=True,
            type=argparse.FileType("w"),
        )
        parser.add_argument(
            "--depfile",
            help="Path to write the depfile listing files this script has read",
            required=True,
            type=argparse.FileType("w"),
        )
        self.args = parser.parse_args(argv)

    def run(self) -> int:
        self.dep_paths.append(self.args.sdk_fidl_json.name)
        apis: list[ApiWithUnstable] = []
        for entry in self.fidl_files:
            library_name = entry.name
            library_directory = os.path.dirname(entry.ir)

            def load_api(dir_path: str) -> ApiFromFidlJson:
                file_path = os.path.join(dir_path, f"{library_name}.fidl.json")
                self.dep_paths.append(file_path)
                with open(file_path) as api_file:
                    return ApiFromFidlJson(name=library_name, api_file=api_file)

            api_at_next = load_api(os.path.join(library_directory, "NEXT"))
            api_at_head = load_api(library_directory)
            apis.append(
                ApiWithUnstable(next_api=api_at_next, head_api=api_at_head)
            )
        self.apis = apis
        self.write_all()
        return 0

    def write_all(self) -> None:
        with self.args.method_summary as summary_file:
            json.dump(self.apis, summary_file, default=vars, indent=2)
        with self.args.depfile as d:
            depfile.DepFile.from_deps(
                "ctf_fidl_api_method_summary", self.dep_paths
            ).write_to(d)


def main() -> None:
    sys.exit(App(sys.argv[1:]).run())


if __name__ == "__main__":
    sys.exit(App(sys.argv[1:]).run())

#!/usr/bin/env fuchsia-vendored-python

# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import os
import re
import subprocess
from collections.abc import Sequence
from pathlib import Path
from typing import Any

_CPP_EXTENSIONS = [".cc", ".c", ".cpp", ".cxx", ".S", ".s"]

_FUCHSIA_CPU_MAP = {"aarch64": "arm64", "x86_64": "x64"}


def _map_fuchsia_cpu(cpu: str) -> str | None:
    """Converts a bazel cpu to a fuchsia cpu"""
    return _FUCHSIA_CPU_MAP.get(cpu, cpu)


# These regex patterns are a tuple of compiled regex's to lambdas that will be
# invoked with the match object if there is on. These regexes are usually used
# to transform a bazel path to one that is in GN.
_REGEX_PATH_PATTERNS = [
    # Fidl libraries defined in GN in the SDK
    (
        re.compile(
            ".*bazel-out.*fuchsia_sdk\/fidl\/.*\/_virtual_includes\/(?P<name>.*)_cpp"
        ),
        lambda m: "-Ifidling/gen/sdk/fidl/{fidl_lib}/{fidl_lib}/cpp".format(
            fidl_lib=m["name"]
        ),
    ),
    # Fidl libraries defined in Bazel in the SDK
    (
        re.compile(
            ".*bazel-out.*\/bin\/sdk\/fidl\/.*\/_virtual_includes\/(?P<name>.*(?<!_bindlib))_cpp"
        ),
        lambda m: "-Ifidling/gen/sdk/fidl/{fidl_lib}/{fidl_lib}/cpp".format(
            fidl_lib=m["name"]
        ),
    ),
    # Fidl libraries defined in Bazel in vendor repos.
    (
        re.compile(
            ".*bazel-out.*\/bin\/vendor\/(?P<path>.*)\/fidl\/.*\/_virtual_includes\/(?P<name>.*(?<!_bindlib))_cpp"
        ),
        lambda m: "-Ifidling/gen/vendor/{vendor_path}/fidl/{fidl_lib}/{fidl_lib}/cpp".format(
            vendor_path=m["path"], fidl_lib=m["name"]
        ),
    ),
    # Fidl bind libraries defined in Bazel in the SDK
    (
        re.compile(
            ".*bazel-out.*\/bin\/sdk\/fidl\/.*\/_virtual_includes\/(?P<name>.*)_bindlib_cpp"
        ),
        lambda m: "-Igen/sdk/fidl/{fidl_lib}/{fidl_lib}_bindlib/bind_cpp".format(
            fidl_lib=m["name"]
        ),
    ),
    # Fidl bind libraries defined in Bazel in vendor repos.
    (
        re.compile(
            ".*bazel-out.*\/bin\/vendor\/(?P<path>.*)\/fidl\/.*\/_virtual_includes\/(?P<name>.*)_bindlib_cpp"
        ),
        lambda m: "-Igen/vendor/{vendor_path}/fidl/{fidl_lib}/{fidl_lib}_bindlib/bind_cpp".format(
            vendor_path=m["path"], fidl_lib=m["name"]
        ),
    ),
    # bind libraries defined in tree under //src/devices/bind
    (
        re.compile(
            ".*bazel-out.*\/(?P<arch>[a-zA-Z0-9]+)-.*\/bin\/src\/devices\/bind\/(?P<name>.*)\/_virtual_includes.*"
        ),
        lambda m: "-I{cpu}-shared/gen/src/devices/bind/{name}/{name}/bind_cpp".format(
            cpu=_map_fuchsia_cpu(m["arch"]),
            name=m["name"],
        ),
    ),
]


def extract_file_from_args(args: Sequence[str]) -> str:
    """Finds the file in the action's arguments

    It would be nice to be able to get the single input file from the action but
    actions are type erased when they are returned in the query so we can't
    just grab the file that is being compiled from the arguments.
    """

    def get_ext(f: str) -> str:
        p = f.rfind(".")
        if p > 0:
            return f[p:]
        else:
            return ""

    files = [arg for arg in args if get_ext(arg) in _CPP_EXTENSIONS]
    if len(files) != 1:
        raise ValueError(
            f"Should only be compiling a single file, got {files} from args {args}"
        )
    return files[0]


class Action:
    """Represents an action that comes from aquery"""

    def __init__(self, action: dict[str, Any], target: dict[str, Any]) -> None:
        self.label = target["label"]
        self.target_id = action["targetId"]
        self.action_key = action["actionKey"]
        self.arguments = action["arguments"]
        self.environment_vars = action["environmentVariables"]
        self.file = extract_file_from_args(self.arguments)

    def is_external(self) -> bool:
        return not (self.label.startswith("//") or self.label.startswith("@//"))


class CompDBFormatter:
    """A class that can convert the actions into compile_commands

    The actions that come from bazel are specific to bazel invocations and do
    not map to a command that can be passed directly to clangd. Specifically,
    the file paths are not relative to the output_root. This class will do a
    best guess on the paths to make sure they map to something that works with
    Fuchsia's out directory.
    """

    def __init__(self, build_dir: str, output_base: str, output_path: str):
        self.build_dir = build_dir
        self.output_base = output_base
        self.output_base_rel = os.path.relpath(output_base, build_dir)
        self.output_path = output_path
        self.output_path_rel = os.path.relpath(output_path, build_dir)

    def rewrite_file(self, action: Action) -> str:
        if action.is_external():
            return os.path.join(self.output_base_rel, action.file)
        else:
            return os.path.join("../..", action.file)

    def maybe_rewrite_path(self, file_path: str, action: Action) -> str:
        # Check to see if this is the file we are building. Need to take special
        # care here depending on if it is an external target or not.
        if file_path == action.file:
            return self.rewrite_file(action)

        # Bazel adds -iquote "." -iquote for files that are being compiled from
        # the internal repository. This changes those to point back to the root
        # of the fuchisa source tree.
        if file_path == ".":
            return "../../"

        # There are actions which are external that reference cc_libraries which
        # are defined as part of the main workspace, mostly @internal_sdk targets.
        # The files they reference are mainly in the //sdk, //src, //vendor and //zircon
        # directories so we need to rewrite the path and treat them as local files.
        # In the future we will likely need to do this for other cc_library targets
        # that are outside of the SDK directory and will need to find a better solution.
        if file_path.startswith(("sdk/", "src/", "vendor/", "zircon/")):
            return "../../" + file_path

        # If we are incliding a generated fidl file change it to point to the fidling
        # directory. This is needed because the fidl libraries use a _virtual_include
        # path when we run the original query which does not seem to point to a valid
        # location. Instead we can fall back to the gn generated code. This is currently
        # a best effort attempt.
        # fidl_match = _FIDL_FUCHSIA_SDK_REGEX_PATTERN.match(file_path)
        # if fidl_match:
        #     fidl_lib = fidl_match.group(1)
        #     return f"-Ifidling/gen/sdk/fidl/{fidl_lib}/{fidl_lib}/cpp"

        # Check to see if any of our regex path patterns match. These paths often
        # represent files that are generated and have _virtual_includes in the
        # path. The _virtual_includes tend to not point to files that exist when
        # working in our hybrid build system so we end up just pointing to the
        # GN paths instead.
        for pattern, replacement in _REGEX_PATH_PATTERNS:
            match_obj = pattern.match(file_path)
            if match_obj:
                return replacement(match_obj)

        # map bazel-out/ paths to that of our output_path
        if "bazel-out/" in file_path:
            return file_path.replace(
                "bazel-out/", self.output_path_rel + "/", 1
            )

        # Look for arguments to files in external/ paths. This is usually
        # the clang binary and include roots
        if "external/" in file_path:
            return file_path.replace(
                "external/",
                os.path.join(self.output_base_rel, "external") + "/",
                1,
            )

        # Just a regular argument
        return file_path

    def action_to_compile_commands(self, action: Action) -> dict[str, Any]:
        return {
            "directory": self.build_dir,
            "file": self.rewrite_file(action),
            "arguments": [
                self.maybe_rewrite_path(arg, action) for arg in action.arguments
            ],
        }


def run(*command: str) -> str:
    try:
        return subprocess.check_output(
            command,
            text=True,
        ).strip()
    except subprocess.CalledProcessError as e:
        raise e


def collect_actions(action_graph: dict[str, Any]) -> Sequence[Action]:
    if not action_graph:
        return []
    targets = {t["id"]: t for t in action_graph["targets"]}
    actions = []
    for action_dict in action_graph["actions"]:
        target: dict[str, Any] = targets[action_dict["targetId"]]
        action: Action = Action(action_dict, target)
        actions.append(action)
    return actions


def get_action_graph_from_labels(
    bazel_bin: str, bazel_config_args: list[str], labels: Sequence[str]
) -> Sequence[Action]:
    labels_set = "set({})".format(" ".join(labels))
    action_graph = json.loads(
        run(
            bazel_bin,
            "aquery",
            "mnemonic('CppCompile',deps({}))".format(labels_set),
            "--output=jsonproto",
            "--noinclude_artifacts",
            "--ui_event_filters=-info,-warning",
            "--noshow_loading_progress",
            "--noshow_progress",
            "--show_result=0",
            *bazel_config_args,
        )
    )
    return collect_actions(action_graph)


def compdb_for_labels(
    build_dir: Path,
    bazel_bin: str,
    bazel_config_args: list[str],
    labels: list[str],
) -> list[dict[str, Any]]:
    """Generate compile commands for input Bazel labels.

    Args:
        build_dir: Path to the build directory.
        bazel_bin: Path to Bazel binary.
        bazel_config_args: Bazel configuration arguments.
        labels: Bazel labels to generate compile commands for.

    Returns:
        A list of compile_commands.json entries.
    """

    actions = get_action_graph_from_labels(
        bazel_bin,
        bazel_config_args,
        labels,
    )

    # Output from the following bazel info command follows this format:
    #
    #   output_base: /path/to/output/base
    #   output_path: /path/to/output/path
    #
    bazel_info = run(bazel_bin, "info", "output_base", "output_path").split()
    output_base = bazel_info[1]
    output_path = bazel_info[3]

    formatter = CompDBFormatter(
        str(build_dir),
        output_base,
        output_path,
    )
    return [formatter.action_to_compile_commands(action) for action in actions]


def dedupe(compdb: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return list({c["file"]: c for c in compdb}.values())

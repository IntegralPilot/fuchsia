# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Core logic for expanding and resolving Bazel actions' command lines."""

import dataclasses
import json
import os
import re
import shlex
import sys
from typing import Any

# Manually modify sys.path to allow other modules to import
# this one without worrying about its dependencies.
_SCRIPT_DIR = os.path.dirname(__file__)
sys.path.insert(0, _SCRIPT_DIR)
import build_utils


@dataclasses.dataclass
class ExpandedAction:
    """Models a Bazel action with its fully expanded command-line."""

    action: str  # Action's description
    target: str  # Bazel target label
    configuration: str  # Bazel configuration mnemonic (e.g. k8-fastbuild)
    mnemonic: str  # The action's mnemonic
    args: list[str]  # The command line arguments
    env_vars: list[str] = dataclasses.field(
        default_factory=list
    )  # A list of VARNAME=value strings. This is kept as a list, instead
    # of a dictionary to be as close as possible from the original
    # command-line. This helps debugging problematic cases when multiple
    # entries assign to the same key (e.g. ["FOO=1", "FOO=2"]).
    warnings: list[str] = dataclasses.field(
        default_factory=list
    )  # Warnings encountered during expansion.

    def to_dict(self) -> dict[str, Any]:
        """Serializes the expanded action into a dictionary for JSON output."""
        res = {
            "action": self.action,
            "target": self.target,
            "configuration": self.configuration,
            "mnemonic": self.mnemonic,
            "args": self.args,
        }
        if self.env_vars:
            res["environment"] = self.env_vars
        if self.warnings:
            res["warnings"] = self.warnings
        return res


def get_bazel_expanded_actions(
    bazel_launcher: build_utils.BazelLauncher,
    bazel_execroot: str,
    bazel_target: str,
    config_args: list[str],
    filter_mnemonics: None | list[str] = None,
    read_response_files: bool = False,
) -> list[ExpandedAction]:
    """Return the expanded command-line of all actions used to build a given Bazel target.

    Args:
        bazel_launcher: A BazelLauncher used to run bazel commands.
        bazel_execroot: Absolute path to the Bazel execroot.
        bazel_target: Bazel target label to get the actions for.
        config_args: Additional configuration arguments to pass to Bazel, such as --config=NAME.
        filter_mnemonics: Optional list of mnemonics to filter by. If provided only actions
            whose mnemonic match these values will be included in the returned list.
        read_response_files: Optional flag. When True, the content of response files will be
            read directly from the Bazel execroot and no cqueries will be performed. This requires
            the target to have been built before calling this function to ensure the response
            file's content is correct.

    Returns:
        A list of ExpandedAction objects with the expanded command lines.

    Raises:
        RuntimeError if invoking Bazel failed.
    """
    # 1. Run a Bazel aquery to get the list of actions with their non-expanded command lines
    #    and map of response files.
    aquery_args = config_args + ["--output=jsonproto", bazel_target]
    ret = bazel_launcher.run_query("aquery", aquery_args, ignore_errors=False)
    if ret.returncode != 0:
        raise RuntimeError(
            f"ERROR: aquery failed with exit code {ret.returncode}\n\n{ret.stderr}\n"
        )

    aquery_result = parse_aquery_output(ret.stdout, filter_mnemonics)

    if aquery_result.uses_bazel_params_file:
        read_response_files = True

    if not read_response_files:
        # 1.1 Perform cqueries to extract the BuildFlagsInfo provider values directly,
        #     without accessing the disk, then use them to expand the response file paths
        #     in the parsed actions' command lines.
        build_args_map = query_build_flags_from_bazel(
            aquery_result.get_final_build_flags_target_labels(),
            bazel_launcher,
            config_args,
        )

        def expand_action_args(
            action: ParsedAction,
        ) -> ArgumentsExpansionResult:
            return expand_args_with_build_args_map(
                action.args,
                action.env_vars,
                aquery_result.response_files_map,
                build_args_map,
            )

    else:
        # 1.2 Read the response files directly from the Bazel execroot instead of
        # performing cqueries. This is required when Bazel decides to use a .params
        # response file to split command lines that are too long.
        execroot = os.path.realpath(bazel_execroot)

        def expand_action_args(
            action: ParsedAction,
        ) -> ArgumentsExpansionResult:
            return expand_args_from_disk(action.args, action.env_vars, execroot)

    # 4. Perform Expansion
    expanded_actions: list[ExpandedAction] = []
    for action in aquery_result.actions:
        expanded_result = expand_action_args(action)

        # Skip actions that don't have any arguments. These are not useful to inspect.
        # This corresponds to Bazel's creation of symlink trees and runfiles manifests.
        if not expanded_result.expanded_args:
            continue

        expanded_actions.append(
            ExpandedAction(
                action=action.name,
                target=action.target,
                configuration=action.config,
                mnemonic=action.mnemonic,
                args=expanded_result.expanded_args,
                env_vars=expanded_result.env_vars,
                warnings=expanded_result.warnings,
            )
        )
    return expanded_actions


##############################################################################################
##############################################################################################
#####
#####    B U I L D   F L A G S   I M P L E M E N T A T I O N   D E T A I L S
#####

# Constants used to identify the type of actions that require build_flags() response.
# LINT.IfChange(cc_action_kinds)
ACTION_KIND_CPP_COMPILE = "cpp_compile"
ACTION_KIND_C_COMPILE = "c_compile"
ACTION_KIND_CPP_LINK = "cpp_link"

CC_ACTION_KINDS = {
    ACTION_KIND_CPP_COMPILE,
    ACTION_KIND_C_COMPILE,
    ACTION_KIND_CPP_LINK,
}
# LINT.ThenChange(//build/bazel_sdk/fuchsia_rules_common/build_flags/cc.bzl:cc_action_kinds)

# LINT.IfChange(rust_action_kinds)
ACTION_KIND_RUST_COMPILE = "rust_compile"

RUST_ACTION_KINDS = {
    ACTION_KIND_RUST_COMPILE,
}
# LINT.ThenChange(//build/bazel_sdk/fuchsia_rules_common/build_flags/rust.bzl:rust_action_kinds)

ACTION_KINDS = CC_ACTION_KINDS | RUST_ACTION_KINDS

# A map from response file suffixes to their corresponding
# action kind. See _generate_response_file() in build_flags.bzl.
_BUILD_FLAGS_RESPONSE_FILE_SUFFIX_MAP = {
    f".{kind}.build_flags": kind for kind in ACTION_KINDS
} | {
    ".rustc_env_file.build_flags": ACTION_KIND_RUST_COMPILE,
}


@dataclasses.dataclass(frozen=True)
class ResolvedBuildArgsFlags:
    """Models the BuildFlagsInfo values extracted from Bazel cqueries."""

    label: str
    cflags: list[str]
    cflags_c: list[str]
    cflags_cc: list[str]
    defines: list[str]
    include_dirs: list[str]
    ldflags: list[str]
    lib_dirs: list[str]
    rustflags: list[str]
    rustenv: list[str]

    @property
    def include_flags(self) -> list[str]:
        return [f"-I{include_dir}" for include_dir in self.include_dirs]

    @property
    def define_flags(self) -> list[str]:
        return [f"-D{define}" for define in self.defines]

    def get_flags_for(self, kind: str) -> list[str]:
        """Return the list of flags for the given action kind."""
        result: list[str] = []
        if kind == ACTION_KIND_CPP_COMPILE:
            result += self.include_flags
            result += self.define_flags
            result.extend(self.cflags)
            result.extend(self.cflags_cc)
        elif kind == ACTION_KIND_C_COMPILE:
            result += self.include_flags
            result += self.define_flags
            result.extend(self.cflags)
            result.extend(self.cflags_c)
        elif kind == ACTION_KIND_RUST_COMPILE:
            result.extend(self.rustflags)
            result.extend([f"-Lnative={lib_dir}" for lib_dir in self.lib_dirs])
        elif kind == ACTION_KIND_CPP_LINK:
            result.extend([f"-Wl,-L{lib_dir}" for lib_dir in self.lib_dirs])
            result.extend(self.ldflags)
        else:
            raise AssertionError(f"Unknown action kind: {kind}")
        return result

    @staticmethod
    def from_json(data: dict[str, Any]) -> "ResolvedBuildArgsFlags":
        return ResolvedBuildArgsFlags(
            label=data["label"],
            cflags=data.get("cflags", []),
            cflags_c=data.get("cflags_c", []),
            cflags_cc=data.get("cflags_cc", []),
            defines=data.get("defines", []),
            include_dirs=data.get("include_dirs", []),
            ldflags=data.get("ldflags", []),
            lib_dirs=data.get("lib_dirs", []),
            rustflags=data.get("rustflags", []),
            rustenv=data.get("rustenv", []),
        )


class ResolvedBuildArgsMap(dict[str, ResolvedBuildArgsFlags]):
    """Map from target labels to their resolved build flags."""


##############################################################################################
##############################################################################################
#####
#####    A Q U E R Y   P A R S I N G
#####


def _normalize_path(path: str) -> str:
    """Safely strips shell pwd prefixes from Bazel paths to prevent false positives."""
    if path.startswith("${pwd}/"):
        return path[len("${pwd}/") :]
    return path


@dataclasses.dataclass(frozen=True)
class ParsedAction:
    """Models a single parsed action from aquery."""

    name: str
    target: str
    config: str
    mnemonic: str
    args: list[str]
    env_vars: dict[str, str]


# A regular expression to match paths in the Bazel execroot that contain
# bazel-out/<config_dir>/bin/
_BAZEL_OUT_BIN_RE = re.compile(r"bazel-out/[^/]+/bin/")


@dataclasses.dataclass(frozen=True)
class ResponseFileTarget:
    """Models a build_flags() response file target and its type.

    The build_flags() implementation relies on the following implementation details of
    its wrapper macros:

    - Creating a _compute_final_build_flags() target that does not generate anything
      but provides the *final* BuildFlagsInfo values to use for the wrapped target.

    - Creating one or more _generate_response_file() targets, that depend on the previously
      listed one, but *only* generating response files, whose paths are added to the wrapped
      target's final command-line.

    This type is used to convert a response file path, as it appears on the action's command
    line, into the label of the _compute_final_build_flags() target, and the kind of action
    the response file corresponds to.

    This will allow performing cqueries on the returned target label to extract the final
    command-line flags, in order to expand the response file without accessing build artifacts.
    """

    label: str  # Label of the _compute_final_build_flags() target created by a wrapper macro.
    kind: str  # Type of action, see ACTION_KINDS defined above.

    @staticmethod
    def from_execroot_path(path: str) -> "None | ResponseFileTarget":
        """Try to convert a response file path into a ResponseFileTarget instance.

        Check the path of a response file as it appears on an action's command-line.
        If it happens to be in the Bazel execroot (typically within bazel-out/<config_dir>/bin/)
        and uses one of the known file extensions implemented for response files in build_flags.bzl,
        then return a new ResponseFileTarget value.

        Args:
            path: The response file path string (e.g. a compiler argument starting with '@').

        Result:
            A ResponseFileTarget instance mapping the path to its cquery label and action kind,
            or None if the path is not a recognized generated wrapper response file.
        """
        match = _BAZEL_OUT_BIN_RE.search(path)
        if not match:
            return None

        rel_path = path[match.end() :]

        if rel_path.startswith("external/"):
            # This artifact is generated by a target defined in an external repository.
            # Its path begins with external/<repo_canonical_repo_name>/
            parts = rel_path.split("/")
            repo_name = parts[1]
            rest = parts[2:]
        else:
            # This artifact is generated by a target from the root workspace.
            repo_name = ""
            rest = rel_path.split("/")

        target_full_name = rest[-1]
        package = "/".join(rest[:-1])

        action_kind = None
        base_target_name = None

        for suffix, kind in _BUILD_FLAGS_RESPONSE_FILE_SUFFIX_MAP.items():
            if target_full_name.endswith(suffix):
                base_target_name = target_full_name.removesuffix(suffix)
                action_kind = kind
                break

        if not base_target_name:
            return None

        label = f"@@{repo_name}//{package}:{base_target_name}.final_build_flags"

        assert action_kind is not None
        return ResponseFileTarget(label, action_kind)


class ResponseFileMap(dict[str, ResponseFileTarget]):
    """A map from response file path to the corresponding ResponseFileTarget value.

    Used to record the paths of build_flags.bzl response files found in an
    action's original command-line, then extract the list of targets to use
    for cqueries later. Usage is:

    1) Create instance

    2) Call try_path() for each response file path that appears in
       the input action's command-line. If this matches the format of a response
       file generated by build_flags.bzl wrapper macros, it will be recorded.
    """

    def try_path(self, response_path: str) -> None:
        target = ResponseFileTarget.from_execroot_path(response_path)
        if target:
            self[response_path] = target


@dataclasses.dataclass(frozen=True)
class ParsedAqueryResult:
    """The result of parsing aquery outputs.

    This provides:
      - A list of ParsedAction values corresponding to all non-empty actions returned
        by the aquery command.

      - A dictionary mapping response file paths (as they appear on actions command lines)
        to the matching ResponseFileTarget. These can be used later to perform cqueries
        to extract the list of final BuildFlagsInfo values.

      - A flag that will be True if any parsed action uses a Bazel .params file.

        This happens when an action's command-line is too long and is instead written to
        a response file that is generated internally by Bazel and thus cannot be
        accessed with queries (it must be read from disk after building the target).
    """

    actions: list[ParsedAction]
    response_files_map: ResponseFileMap
    uses_bazel_params_file: bool

    @staticmethod
    def new_empty() -> "ParsedAqueryResult":
        """Create a new ParsedAqueryResult with empty values."""
        return ParsedAqueryResult([], ResponseFileMap(), False)

    def get_final_build_flags_target_labels(self) -> set[str]:
        """Return a set of target labels to use for cquery queries."""
        return {t.label for t in self.response_files_map.values()}


def parse_aquery_output(
    aquery_output: str,
    filter_mnemonics: list[str] | None = None,
) -> ParsedAqueryResult:
    """Parses raw aquery output and extracts commands and response file mappings.

    Args:
        aquery_output: The raw string output from a 'bazel aquery --output=jsonproto' command.
        filter_mnemonics: Optional list of mnemonic names to filter actions early.

    Returns:
        A ParsedAqueryResult value.
    """
    # The following parses the output of `bazel aquery --format=jsonproto` which should
    # emit a JSON object that corresponds to the ActionGraphContainer schema from
    # https://github.com/bazelbuild/bazel/blob/7278be3f9b0c26842ecb8225f0215c1e4aede5a9/src/main/protobuf/analysis_v2.proto#L189

    # Filter out the output from unexpected garbage, for example our main_build.py
    # wrapper still appends "Lock acquired, proceeding with build." to stdout when performing
    # `fx bazel aquery --config=host --output=jsonproto //build/bazel/rules/host_tests/tests/cc/... 2>/dev/null`
    start_idx = aquery_output.find("{")
    end_idx = aquery_output.rfind("}")
    if start_idx < 0 or end_idx < 0 or start_idx > end_idx:
        return ParsedAqueryResult.new_empty()

    try:
        data = json.loads(aquery_output[start_idx : end_idx + 1])

        # data["targets"] is a list of Target dictionaries. Build a map associating
        # target unique ids to their label.
        targets_map = {t["id"]: t["label"] for t in data.get("targets", [])}

        # data["configuration"] is a list of Configuration dicts. Build a map
        # association configuration unique ids to the "mnemonic" which is
        # something like "k8-fastbuild" (not related to action mnemonics).
        configs_map = {
            c["id"]: c["mnemonic"] for c in data.get("configuration", [])
        }
    except Exception as e:
        print(f"ERROR: Failed to parse aquery jsonproto: {e}", file=sys.stderr)
        return ParsedAqueryResult.new_empty()

    actions: list[ParsedAction] = []
    response_files_map = ResponseFileMap()

    uses_bazel_params_file = False

    raw_actions = data.get("actions", [])
    for action in raw_actions:
        action_mnemonic = action.get("mnemonic", "Unknown Mnemonic")
        if filter_mnemonics and action_mnemonic not in filter_mnemonics:
            continue

        target_id = action.get("targetId", 0)
        action_target = targets_map.get(target_id, "Unknown Target")

        config_id = action.get("configurationId", 0)
        action_config = configs_map.get(config_id, "Unknown Configuration")

        cmd_args = action.get("arguments", [])
        action_name = f"action '{action_mnemonic} on {action_target}'"

        env_vars: dict[str, str] = {
            item["key"]: item["value"]
            for item in action.get("environmentVariables", [])
            # Sometimes there is no value at all, ignore such entries.
            if "value" in item
        }

        for i, arg in enumerate(cmd_args):
            if arg == "--env-file" and i + 1 < len(cmd_args):
                env_path = _normalize_path(cmd_args[i + 1])
                response_files_map.try_path(env_path)
            elif arg.startswith("@"):
                path = _normalize_path(arg[1:])
                if ".params" in os.path.basename(path):
                    uses_bazel_params_file = True
                else:
                    response_files_map.try_path(path)

        actions.append(
            ParsedAction(
                name=action_name,
                target=action_target,
                config=action_config,
                mnemonic=action_mnemonic,
                args=cmd_args,
                env_vars=env_vars,
            )
        )

    return ParsedAqueryResult(
        actions=actions,
        response_files_map=response_files_map,
        uses_bazel_params_file=uses_bazel_params_file,
    )


##############################################################################################
##############################################################################################
#####
#####    A R G U M E N T   E X P A N S I O N
#####


@dataclasses.dataclass(frozen=True)
class ArgumentsExpansionResult:
    """Models the result of expanding command line arguments dynamically."""

    expanded_args: list[str]  # Expanded command-line arguments.
    env_vars: list[str]  # A list of "VARNAME=value" strings.
    warnings: list[str]  # Warnings encountered during expansion.


def expand_args_from_disk(
    args: list[str],
    env_vars: dict[str, str],
    execroot: str,
) -> ArgumentsExpansionResult:
    """Recursively expand standard response files and Rust env files from disk iteratively.

    This functions takes a list of command-line arguments and will expand response files
    that appear in the input to generate a final version. It only works correctly if
    these files are in the execroot, hence must be called after building the target
    to give correct results.

    Args:
        args: The raw command line arguments list to expand.
        env_vars: The dictionary of environment variables to use for expansion.
        execroot: Absolute path to Bazel's execution root.

    Returns:
        An ArgumentsExpansionResult dataclass containing expanded arguments, environment
        variable overrides, and warnings.
    """
    import collections

    expanded: list[str] = []
    env_list: list[str] = [f"{k}={v}" for k, v in env_vars.items()]
    warnings: list[str] = []

    # The implementation below uses a single double-ended queue to parse
    # the arguments recursively. In particular, it recognizes two special
    # types of input:
    #
    #  - '@<path>': as a response file usage. The code will read the
    #    content of <path>, shell-unquote it, and inject its content into
    #    the queue.
    #
    #  -'--rust-env', '<path>': as a Rust environment file use. The code
    #   will read <path> which should be a series of NAME=value definitions,
    #   one per line, which will be recorded in env_list in the order they
    #   appear (no deduplication is performed).
    #
    # The operation is recursive (a response file can contain other @<path>
    # arguments) and will properly flatten the final result, however, path
    # cycles can exist, and the loop below will detect them and generate
    # a warning then one is detected, and will also keep the corresponding
    # final '@<path>' argument in the output to help debugging.
    #
    # Each item in the queue is either a string, modelling an incoming
    #
    # string argument, or a tuple[str, str] which will be ('POP_MARKER', abs_path)
    # to model the end of a file scropt. These items are used to pop paths
    # from the stack used for cycle detection.
    #
    # A few example will illustrate how this works, consider this input:
    #
    #  queue = [ "foo", "@path1", "bar" ]      output = []            paths = []
    #
    # The first loop iteration see "foo" and just pulls it to send to the output.
    #
    #  queue = [ "@path1", "bar" ]             output = [ "foo" ]     paths = []
    #
    # The second loop iteration will see the "@<path>" format, and will
    # push 'path1' to the `paths` stack, read the content of that file
    # into a list of unquoted string arguments, followed by a POP_MARKER item,
    # the following assumed 'path1' contained "qux", "@path2":
    #
    #  queue  = [ "qux", "@path2", (POP_MARKER, "path1"), "bar" ]
    #  output = [ "foo" ]
    #  paths  = [ "path1" ]
    #
    # The third loop iteration moves 'qux' to the output, and the fourth one
    # will see the new "@path2" statement:
    #
    #  queue  = [ "@path2", (POP_MARKER, "path1"), "bar" ]
    #  output = [ "foo", "qux" ]
    #  paths  = [ "path1" ]
    #
    # the 'path2' will pushed to the top of the stak, its content read, unquoted
    # and prepended at the start of the queue, followed by its POP_MARKER item:
    #
    #  queue  = [ "zoo", (POP_MARKER, "path2"), (POP_MARKER, "path1"), "bar" ]
    #  output = [ "foo", "qux" ]
    #  paths  = [ "path1", "path2" ]
    #
    # From there, 'zoo' is moved to the output. The first POP_MARKER simply
    # removes 'path2' from the top of the `paths` stack, the second one
    # removed "path1", then "bar" is appended to the output:
    #
    #  queue  = []
    #  output = [ "foo", "qux", "zoo", "bar" ]
    #  paths  = []
    #
    # The look also contain special logic to handle "--env-file", "<path>",
    # even when this crosses a file scope.
    #
    work_queue: collections.deque[str | tuple[str, str]] = collections.deque(
        args
    )

    # Active circular dependency path stack (mimics the recursive call stack)
    active_path: list[str] = []

    def detect_path_cycle(item: str, abs_path: str) -> bool:
        """Return true if a path cycle is detected."""
        if abs_path not in active_path:
            return False

        cycle_idx = active_path.index(abs_path)
        cycle_path = [
            os.path.relpath(p, execroot) for p in active_path[cycle_idx:]
        ] + [os.path.relpath(abs_path, execroot)]
        cycle_str = " -> ".join(cycle_path)
        warnings.append(f"Cycle detected in response file {item}: {cycle_str}")
        return True

    while work_queue:
        item = work_queue.popleft()

        # Handle pop marker to unwind active path scope
        if isinstance(item, tuple) and item[0] == "POP_MARKER":
            pop_path = item[1]

            # Consistency checks for safety.
            assert (
                active_path
            ), f"Unexpected POP_MARKER for {item[1]} with empty stack!"
            assert (
                active_path[-1] == pop_path
            ), f"Unexpected POP_MARKER for {item[1]}, expected {active_path[-1]}"
            active_path.pop()
            continue

        assert isinstance(item, str)

        # Handle --env-file <path> for Rust actions
        if item == "--env-file":
            # If this is the final --env-file, just append it as-is to the output.
            # The command-line is likely broken, but should be reported.
            if not work_queue:
                expanded.append(item)
                continue

            env_path = work_queue.popleft()

            # If the --env-file is followed by a POP_MARKER, swap them in the
            # queue and loop again to simplify processing.
            if not isinstance(env_path, str):
                # --env-file POP_MARKER ... => POP_MARKER --env-file ...
                work_queue.appendleft(item)
                work_queue.appendleft(env_path)
                continue

            assert isinstance(env_path, str)  # for mypy
            normalized_path = _normalize_path(env_path)
            abs_path = os.path.join(execroot, normalized_path)

            # Tricky case: '@path1' where path1 contains '--env-file path1'
            # This is a path cycle, keep the --env-file <path> in the output
            # for debugging after the warning (which will include the cycle).
            if detect_path_cycle(env_path, abs_path):
                expanded += [item, env_path]
                continue

            if os.path.isfile(abs_path):
                try:
                    with open(abs_path, "r") as f:
                        envs = [line.strip() for line in f if line.strip()]
                        env_list.extend(envs)
                except Exception as e:
                    warnings.append(
                        f"Failed to read env file {env_path} from disk: {e}"
                    )
                    expanded += [
                        item,
                        env_path,
                    ]  # Keep --env-file <path> in output.
            else:
                warnings.append(
                    f"Rust environment file {env_path} was not found on disk. "
                    f"This script requires build outputs to expand arguments. "
                    f"Please build the target first (using 'fx build') and re-run this script."
                )
                expanded += [
                    item,
                    env_path,
                ]  # Keep --env-file <path> in output.
            continue

        # Handle @path response files recursively
        if item.startswith("@"):
            path = item[1:]
            normalized_path = _normalize_path(path)
            abs_path = os.path.join(execroot, normalized_path)

            # Detect path cycles, keep the last @path in the output for debugging
            if detect_path_cycle(item, abs_path):
                expanded.append(item)
                continue

            if os.path.isfile(abs_path):
                try:
                    # Read the response file, assume content is shell quoted
                    # as that's what most tools expect (though there is no
                    # standard for that), and that it can contain multiple
                    # arguments per line.
                    with open(abs_path, "r") as f:
                        nested_args = []
                        for line in f:
                            line = line.strip()
                            if line:
                                nested_args.extend(shlex.split(line))

                    # Prepend POP_MARKER and nested args in original order
                    work_queue.appendleft(("POP_MARKER", abs_path))
                    for nested_arg in reversed(nested_args):
                        work_queue.appendleft(nested_arg)

                    # Save path in stack.
                    active_path.append(abs_path)
                    continue
                except OSError as e:
                    # Something went wrong, keep @path for debugging.
                    warnings.append(
                        f"Failed to read parameter file {item} from disk: {e}"
                    )
            else:
                warnings.append(
                    f"Parameter file {item} was not found on disk. "
                    f"This script requires build outputs to expand arguments. "
                    f"Please build the target first (using 'fx build') and re-run this script."
                )
        # Either a normal item, or something went wrong in the @path
        # case, and we keep it for debugging the issue.
        expanded.append(item)

    return ArgumentsExpansionResult(
        expanded_args=expanded, env_vars=env_list, warnings=warnings
    )


##############################################################################################
##############################################################################################
#####
#####    C Q U E R I E S   F O R   B U I L D   F L A G S   I N F O
#####

# Path to the .cquery file used by query_build_flags_from_bazel.
_EXPAND_BUILD_ARGS_JSON_CQUERY_PATH = os.path.join(
    _SCRIPT_DIR, "../starlark/expand_build_args_json.cquery"
)


def query_build_flags_from_bazel(
    final_build_flags_labels: set[str],
    bazel_launcher: build_utils.BazelLauncher,
    config_args: list[str],
) -> ResolvedBuildArgsMap:
    """Runs cquery to fetch custom build flags statically.

    This takes as input a ParseAqueryOutput value and uses it to perform a number
    of Bazel cqueries to compute ResolvedBuildArgsFlags values.

    Args:
        final_build_flags_labels: The labels of the _compute_final_build_flags() target that
            will be queries for their provider information by this function. E.g. the result
            of ParsedAqueryResult.get_final_build_flags_target_labels().

        bazel_launcher: The BazelLauncher instance used to invoke bazel.
        config_args: Extra configuration-specific flags (like --config) to pass to cquery.

    Returns:
        A ResolvedBuildArgsMap value.
    Raises:
        RuntimeError if there is a problem calling bazel cquery.
    """
    flags_map = ResolvedBuildArgsMap()
    if final_build_flags_labels:
        cquery_expr = f"set({' '.join(sorted(final_build_flags_labels))})"
        cquery_file = os.path.realpath(_EXPAND_BUILD_ARGS_JSON_CQUERY_PATH)

        print(f"Running cquery for response files flags ...", file=sys.stderr)
        cquery_args = [
            "--output=starlark",
            f"--starlark:file={cquery_file}",
            "--consistent_labels",
        ]
        cquery_args.extend(config_args)
        cquery_args.append(cquery_expr)

        ret = bazel_launcher.run_query(
            "cquery", cquery_args, ignore_errors=False
        )
        if ret.returncode != 0:
            raise RuntimeError(f"Error running bazel cquery:\n\n{ret.stderr}\n")

        cquery_output = ret.stdout

        for line in cquery_output.splitlines():
            line = line.strip()
            if line.startswith("{") and line.endswith("}"):
                try:
                    data = json.loads(line)
                    if data:  # Ignore empty objects returned by the cquery
                        flags = ResolvedBuildArgsFlags.from_json(data)
                        flags_map[flags.label] = flags
                except json.JSONDecodeError as e:
                    raise RuntimeError(
                        f"Error parsing JSON line: {line}, error: {e}"
                    )
    return flags_map


def expand_args_with_build_args_map(
    args: list[str],
    env_vars: dict[str, str],
    response_files_map: ResponseFileMap,
    build_args_map: ResolvedBuildArgsMap,
) -> ArgumentsExpansionResult:
    """Expand command line arguments without touching the filesystem.

    This function is similar to expand_args_from_disk() except that it will not read any
    file from disk. Instead, it relies on a BuildArgsMap to provide the collected final
    BuildFlagsInfo provider values, and map the response files that appear in the input
    arguments to the corresponding flags.

    Args:
        args: The raw command line arguments list to expand.
        env_vars: The dictionary of environment variables to use for expansion.
        response_files_map: Pre-computed mapping of argument paths to Starlark targets and kinds.
        build_args_map: A ResolvedBuildArgsMap value.

    Result:
        A ArgumentsExpansionResult containing:
        - A new list of arguments with all custom build_flags response files expanded recursively.
        - A list of resolved environment variables (e.g., "KEY=VALUE") from env files.
        - A list of warning strings.
    """
    expanded = []
    env_list = [f"{k}={v}" for k, v in env_vars.items()]
    warnings = []
    skip_next = False
    for i, arg in enumerate(args):
        if skip_next:
            skip_next = False
            continue

        if arg == "--env-file" and i + 1 < len(args):
            env_path = _normalize_path(args[i + 1])
            res_target = response_files_map.get(env_path)
            if res_target:
                data = build_args_map.get(res_target.label)
                if data is None:
                    warnings.append(
                        f"Could not resolve Rust env vars for {env_path} via cquery."
                    )
                else:
                    env_list.extend(data.rustenv)
            skip_next = True
            continue

        if arg.startswith("@"):
            path = _normalize_path(arg[1:])
            res_target = response_files_map.get(path)
            if not res_target:
                expanded.append(arg)
                warnings.append(
                    f"Standard parameter file {arg} cannot be expanded with queries. "
                )
                continue

            data = build_args_map.get(res_target.label)
            if data:
                flags = data.get_flags_for(res_target.kind)
                expanded.extend(flags)
            else:
                expanded.append(arg)
                warnings.append(
                    f"Could not resolve flags for custom response file {arg} via cquery."
                )
        else:
            expanded.append(arg)

    return ArgumentsExpansionResult(
        expanded_args=expanded, env_vars=env_list, warnings=warnings
    )

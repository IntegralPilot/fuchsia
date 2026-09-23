# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import concurrent.futures
import pathlib
import shlex
import sys
import typing as T
from itertools import zip_longest

# Root directory of the Fuchsia source tree.
_FUCHSIA_DIR = pathlib.Path(__file__).parent.parent.parent.parent

sys.path.insert(0, str(_FUCHSIA_DIR / "build/api"))
import gn_ninja_outputs

sys.path.insert(0, str(_FUCHSIA_DIR / "build/bazel/scripts"))
import bazel_build_args
from build_utils import BazelLauncher, NinjaRunner

_DEBUG = False


def set_debug(enabled: bool) -> None:
    global _DEBUG
    _DEBUG = enabled


def debug(s: T.Any) -> None:
    if _DEBUG:
        print(f"DEBUG: {s}", file=sys.stderr)


class GnCommandMap(dict[str, str]):
    """A type mapping GN labels to the command generating their outputs."""


class BazelCommandMap(dict[str, str]):
    """A typre mapping Bazel labels to the command generating their outputs."""


def query_ninja_commands(
    ninja_runner: NinjaRunner,
    gn_labels: list[str],
) -> GnCommandMap:
    """
    Fetch the ninja build commands for the given GN labels.

    Args:
        ninja_runner: The NinjaRunner instance to use.
        gn_labels: The GN labels to fetch the command line for.

    Returns:
        A dictionary mapping GN labels to their corresponding Ninja build commands.
    Raises:
        ValueError if one of gn_labels is not in the GN graph, or no output is found
          for it in the Ninja build plan.
    """
    debug(f"Querying Ninja commands for GN labels {gn_labels}...")

    if not gn_labels:
        return GnCommandMap()

    ninja_outputs = gn_ninja_outputs.load_from_build_dir(ninja_runner.build_dir)
    if not ninja_outputs:
        raise ValueError(f"Could not find Ninja outputs database")

    all_outputs = []
    for gn_label in gn_labels:
        outputs = ninja_outputs.gn_label_to_paths(gn_label)
        if not outputs:
            raise ValueError(
                f"Could not find outputs for label {gn_label} in Ninja outputs database"
            )
        all_outputs.append((gn_label, outputs))
    debug(f"Found Ninja outputs: {all_outputs}")

    outputs_for_query = [
        output for _, outputs in all_outputs for output in outputs
    ]
    ninja_cmd = ["-t", "commands", "-s"] + outputs_for_query
    debug(f"Running ninja command with args: {ninja_cmd}")

    ninja_cmd_output = ninja_runner.run_and_extract_output(ninja_cmd)
    commands = ninja_cmd_output.strip().splitlines()

    # On successful runs of `ninja -t commands -s`, each GN label should have a single Ninja command
    # associated with it.
    #
    # We check that this is true by checking that each command contains the expected output.
    results = GnCommandMap()

    for (gn_label, outputs), cmd in zip_longest(all_outputs, commands):
        # This happens if the ninja query returned a shorter output than expected.
        if not cmd:
            raise ValueError(
                f"Could not find command for label: {gn_label}, no command returned from Ninja query"
            )

        # Confirm that the command is associated with the expected output.
        for output in outputs:
            if output in cmd:
                results[gn_label] = cmd
                break

        if gn_label not in results:
            raise ValueError(
                f"Could not find command for label: {gn_label}, no matching output found in Ninja command"
            )

    return results


def query_ninja_command(
    ninja_runner: NinjaRunner,
    gn_label: str,
) -> str:
    """
    Fetch the Ninja build command for the given GN label.

    Args:
        ninja_runner: The NinjaRunner instance to use.
        gn_label: The GN label to fetch the command line for.

    Returns:
        A string representing the Ninja build command for the given GN label.
    """
    debug(f"Fetching Ninja command for GN label {gn_label}...")
    return query_ninja_commands(ninja_runner, [gn_label])[gn_label]


def query_bazel_commands(
    bazel_launcher: BazelLauncher,
    bazel_execroot: str | pathlib.Path,
    bazel_labels: list[str],
    read_response_files: bool = False,
) -> BazelCommandMap:
    """
    Query Bazel for the command lines of the rustc commands for the given Bazel labels.

    Args:
        bazel_launcher: The BazelLauncher instance to use.
        bazel_execroot: Path to the Bazel execroot directory.
        bazel_labels: The Bazel labels to fetch the command lines for.
        read_response_files: Whether to read response files directly from disk instead of using queries.

    Returns:
        A dictionary mapping Bazel labels to their corresponding rustc command lines.
    """
    if not bazel_labels:
        return BazelCommandMap()

    def normalize_label(label: str) -> str:
        """Remove optional @@ and @ prefix for root workspace labels."""
        if label.startswith("@@//"):
            return label[2:]
        if label.startswith("@//"):
            return label[1:]
        return label

    bazel_target = 'mnemonic("Rustc", {})'.format(
        " + ".join(normalize_label(l) for l in bazel_labels)
    )
    config_args = [
        "--config=host",
        "--config=quiet",
        # Ensure that the labels returned by get_bazel_expanded_actions()
        # have a canonical label, which means a @@// prefix for root workspace files.
        "--consistent_labels",
    ]
    debug(
        f"Fetching expanded Bazel commands for {bazel_labels} using get_bazel_expanded_actions with target: {bazel_target}"
    )
    try:
        expanded_actions = bazel_build_args.get_bazel_expanded_actions(
            bazel_launcher=bazel_launcher,
            bazel_execroot=str(bazel_execroot),
            bazel_target=bazel_target,
            config_args=config_args,
            filter_mnemonics=["Rustc"],
            read_response_files=read_response_files,
        )
    except Exception as e:
        raise ValueError(
            f"Failed to run bazel action expansion for labels: {e}"
        ) from e

    # Maps { normalized_label -> command-line string }
    commands_map: dict[str, str] = {}

    for action in expanded_actions:
        full_args = list(action.env_vars) + action.args
        if not full_args:
            continue
        cmd_str = shlex.join(full_args)
        commands_map.setdefault(normalize_label(action.target), cmd_str)

    result = BazelCommandMap()
    missing_labels = []
    for label in bazel_labels:
        command = commands_map.get(normalize_label(label), "")
        if command:
            result[label] = command
        else:
            missing_labels.append(label)

    if missing_labels:
        raise ValueError(f"Could not find command for labels: {missing_labels}")

    return result


def query_bazel_command(
    bazel_launcher: BazelLauncher,
    bazel_execroot: str | pathlib.Path,
    bazel_label: str,
    read_response_files: bool = False,
) -> str:
    """
    Query Bazel for the command line of the rustc command for the given Bazel
    label.

    Args:
        bazel_launcher: The BazelLauncher instance to use.
        bazel_execroot: Path to the Bazel execroot directory.
        bazel_label: The Bazel label to fetch the command line for.
        read_response_files: Whether to read response files directly from disk instead of using queries.

    Returns:
        A string representing the command line of the rustc command.
    """
    debug(f"Fetching Bazel command for Bazel label {bazel_label}...")
    return query_bazel_commands(
        bazel_launcher,
        bazel_execroot,
        [bazel_label],
        read_response_files=read_response_files,
    )[bazel_label]


def query_ninja_and_bazel_commands(
    gn_labels: list[str],
    bazel_labels: list[str],
    ninja_runner: NinjaRunner,
    bazel_launcher: BazelLauncher,
    bazel_execroot: str | pathlib.Path,
    read_response_files: bool = False,
) -> tuple[GnCommandMap, BazelCommandMap]:
    """Query both GN and Bazel commands in parallel.

    This invokes query_ninja_commands() and query_bazel_commands() in
    parallel for getting the results faster.

    Args:
        gn_labels: A list of GN target labels.
        bazel_labels: A list of Bazel target labels.
        ninja_runner: A NinjaRunner used to perform Ninja tool calls.
        bazel_launcher; A BazelLauncher used to perform Bazel queries.
        bazel_execroot: Path to the Bazel execroot directory.
        read_response_files: Optional flag. Set to True to read response files
            directly from disk instead of using queries. Only impacts Bazel
            queries.
    Returns:
        A (GnCommandMap, BazelCommandMap) pair.
    Raises:
        ValueError in case of error.
    """
    # Query GN and Bazel commands in parallel using batched queries
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as executor:
        ninja_future = executor.submit(
            query_ninja_commands,
            ninja_runner,
            gn_labels,
        )
        bazel_future = executor.submit(
            query_bazel_commands,
            bazel_launcher,
            bazel_execroot,
            bazel_labels,
            read_response_files=read_response_files,
        )
        gn_cmds: GnCommandMap = ninja_future.result()
        bazel_cmds: BazelCommandMap = bazel_future.result()

    return gn_cmds, bazel_cmds

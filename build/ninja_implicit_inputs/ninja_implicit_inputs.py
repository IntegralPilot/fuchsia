# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Manage Ninja implicit inputs, see README.md file for details."""

import collections
import dataclasses
import json
import os
import sys
import tempfile
import typing as T
from pathlib import Path

_SCRIPT_DIR = os.path.dirname(__file__)
sys.path.insert(0, os.path.join(_SCRIPT_DIR, "../api"))
from gn_labels import GnLabelQualifier
from gn_ninja_outputs import NinjaOutputsBase, NinjaOutputsJSON

sys.path.insert(0, os.path.join(_SCRIPT_DIR, "../bazel/scripts"))
from build_utils import NinjaRunner

# A type representing a dictionary mapping Ninja target paths to a set of related path strings.
# (which can represent inputs or outputs computed by walking the Ninja graph).
# All paths are relative to the Ninja build directory and stored as simple strings.
NinjaTargetPaths: T.TypeAlias = dict[str, set[str]]

# A type for a dictionary mapping GN labels to the set of implicit and unknown
# source paths they depend on. The paths will be relative to the Fuchsia source
# directory.
GnSourcePaths: T.TypeAlias = dict[str, set[str]]


def run_ninja_tool(
    tool_name: str,
    target_paths: T.Iterable[str],
    ninja_runner: NinjaRunner,
    with_depfile: bool,
) -> NinjaTargetPaths:
    """run the ninja 'affected' or 'multi-inputs' tools and return its output as a map.

    these tools takes one or more Ninja target paths as input (which
    must be relative to the ninja build dir), and will print lines
    with the following format:

    <target> <tab> <path>

    Where <path> is an output file path for the 'affected' tool, and a input
    file path for the 'multi-inputs' tool.

    args:
        tool_name: Must be either "affected" or "multi-inputs"
        target_paths: a sequence of Ninja target paths, relative to
            the ninja build directory.

        ninja_runner: A NinjaRunner instance.

        with_depfile: set to True to return results that include the
            implicit inputs recorded in the depfile, false otherwise.

    returns:
        a NinjaTargetsPath instance corresponding to the result, mapping each input
        target path value to a set of related Ninja paths.
    """
    result: NinjaTargetPaths = collections.defaultdict(set)

    # Create a temporary directory to list all root targets, then use
    # --target-list=FILE in the tool query to avoid command-line length limits
    with tempfile.NamedTemporaryFile("w+") as tmp_list:
        tmp_list.write("\n".join(target_paths))
        tmp_list.flush()  # Important to ensure file is written to disk.

        cmd_args = ["-t", tool_name]
        if tool_name == "affected":
            cmd_args += [
                "--depth=1",
                "--partition",
            ]
        elif tool_name == "multi-inputs":
            pass
        else:
            assert (
                False
            ), f"Unknown tool_name {tool_name}, must be one of: affected, multi-inputs"

        if with_depfile:
            cmd_args += ["--depfile"]

        cmd_args += ["--target-list={}".format(tmp_list.name)]
        output = ninja_runner.run_and_extract_output(cmd_args)

    for line in output.splitlines():
        target_path, tab, output_path = line.partition("\t")
        assert (
            tab == "\t"
        ), f"unexpected '-t affected' output line (expected <source><tab><output>): [{line}]"
        result[target_path].add(output_path)

    return result


@dataclasses.dataclass
class ImplicitInputsEntry:
    """Model the known implicit input file and directory paths for a given GN target.

    gn_label is a fully-qualified GN label, and all file paths are relative to the
    Ninja build directory.
    """

    gn_label: str = ""
    files: None | set[str] = None
    directories: None | set[str] = None


class ImplicitInputs:
    """The set of known implicit inputs from the //build/ninja_implicit_inputs:manifest."""

    def __init__(
        self, manifest_map: dict[str, list[ImplicitInputsEntry]]
    ) -> None:
        """Create instance. Use create_from_build_dir() instead."""
        self._map = manifest_map
        self._entries: None | list[ImplicitInputsEntry] = None
        self._known_files: None | set[str] = None
        self._known_dirs: None | set[str] = None

    @property
    def all_known_files(self) -> set[str]:
        """Return the set of all known files from all root targets."""
        if self._known_files is None:
            self._known_files = set()
            for entries in self._map.values():
                for entry in entries:
                    if entry.files:
                        self._known_files.update(entry.files)
        return self._known_files

    @property
    def all_known_dirs(self) -> set[str]:
        """Return the set of all known directories from all root targets."""
        if self._known_dirs is None:
            self._known_dirs = set()
            for entries in self._map.values():
                for entry in entries:
                    if entry.directories:
                        self._known_dirs.update(entry.directories)
        return self._known_dirs

    @property
    def all_entries(self) -> list[ImplicitInputsEntry]:
        """Return the list of all entries from all root targets, sorted by GN label."""
        if self._entries is None:
            self._entries = []
            entries_map: dict[str, ImplicitInputsEntry] = {}
            for entries in self._map.values():
                for entry in entries:
                    cur_value = entries_map.setdefault(entry.gn_label, entry)
                    if cur_value != entry:
                        continue
            self._entries = sorted(
                entries_map.values(), key=lambda x: x.gn_label
            )
        return self._entries

    @property
    def map(self) -> dict[str, list[ImplicitInputsEntry]]:
        """Return the map from Ninja root targets to implicit inputs."""
        return self._map

    def check_for_missing_files(
        self, fuchsia_dir: str | Path, build_dir: str | Path
    ) -> GnSourcePaths:
        """Check for missing files or directories.

        This is useful to verify that implicit file or directory declarations
        (e.g. C++ headers) actually exist.

        Args:
            fuchsia_dir: Fuchsia source directory.
            build_dir: Ninja build directory.
        Returns:
            A dictionary mapping GN labels to the set of missing file or
            directory paths found by the function. This will be empty if
            all files are found. All listed paths are relative to build_dir.
        """
        source_prefix = os.path.relpath(fuchsia_dir, build_dir) + "/"
        implicit_entries = self.all_entries
        missing_paths_map: dict[str, set[str]] = collections.defaultdict(set)
        for entry in implicit_entries:
            for file in entry.files or []:
                if not file.startswith(source_prefix):
                    continue  # Ignore build artifacts
                filepath = os.path.normpath(os.path.join(build_dir, file))
                if not os.path.isfile(filepath):
                    missing_paths_map[entry.gn_label].add(file)
            for directory in entry.directories or []:
                if not directory.startswith(source_prefix):
                    continue  # Ignore build artifacts
                dirpath = os.path.normpath(os.path.join(build_dir, directory))
                if not os.path.isdir(dirpath):
                    missing_paths_map[entry.gn_label].add(directory)

        return missing_paths_map

    @staticmethod
    def create_from_build_dir(build_dir: str | Path) -> "ImplicitInputs":
        """Load the GN-generated manifest of known implicit inputs.

        Args:
            build_dir: Path to the Ninja build directory.
        Returns:
            An ImplicitInputs instance.
        """
        manifest_path = os.path.join(
            build_dir, "gen/build/ninja_implicit_inputs/manifest.json"
        )
        assert os.path.exists(
            manifest_path
        ), f"Missing input manifest: {manifest_path}"
        with open(manifest_path) as f:
            manifest = json.load(f)

        known_implicit_files: set[str] = set()
        known_implicit_dirs: set[str] = set()

        manifest_map: dict[str, list[ImplicitInputsEntry]] = {}
        for manifest_entry in manifest:
            # LINT.IfChange(manifest_schema)
            assert (
                "gn_label" in manifest_entry
            ), f"Invalid entry, missing 'gn_label' key: {manifest_entry}"
            gn_label = manifest_entry["gn_label"]

            assert (
                "manifest_path" in manifest_entry
            ), f"Invalid entry, missing 'manifest_path' key: {manifest_entry}"
            submanifest_path = os.path.join(
                build_dir, manifest_entry["manifest_path"]
            )
            with open(submanifest_path) as f:
                submanifest = json.load(f)

            entries: list[ImplicitInputsEntry] = []
            for submanifest_entry in submanifest:
                entry = ImplicitInputsEntry(
                    gn_label=submanifest_entry["gn_label"],
                    files=submanifest_entry.get("files"),
                    directories=submanifest_entry.get("directories"),
                )
                entries.append(entry)

            # LINT.ThenChange(//build/ninja_implicit_inputs/BUILD.gn:manifest_schema)
            manifest_map[gn_label] = entries

        return ImplicitInputs(manifest_map)


def find_ninja_source_inputs(
    build_targets: list[str],
    ninja_runner: NinjaRunner,
    with_depfile: bool,
) -> NinjaTargetPaths:
    """Find the set of explicit source inputs from the Ninja build graph.

    Args:
        build_targets: A set of root Ninja target paths.
        ninja_runner: A NinjaRunner instance.
        with_depfile: Set to True to include results from the Ninja deps
            log. Note that this will only be accurate after a build
            that generated the build_targets artifacts.
    Returns:
       A NinjaTargetPaths instance mapping each build target to the set
       of input source files it depends on, not that the source paths
       are relative to the Ninja build directory, and will always start
       with "../".
    """
    all_inputs = run_ninja_tool(
        "multi-inputs", build_targets, ninja_runner, with_depfile=with_depfile
    )
    return {
        build_target: {path for path in paths if path.startswith("../")}
        for build_target, paths in all_inputs.items()
    }


def find_unknown_implicit_source_inputs(
    build_targets: list[str],
    implicit_inputs: ImplicitInputs,
    ninja_runner: NinjaRunner,
) -> NinjaTargetPaths:
    """Parse the Ninja deps log to find unknown implicit inputs there.

    Args:
       build_targets: A sequence of Ninja build target paths. Their
           recursive inputs will be collected.

       implicit_inputs: An ImplicitInputs instance modeling the known
           inputs from the //build/ninja_implicit_inputs:manifest.

        ninja_runner: A NinjaRunner instance.
    Returns:
        A NinjaTargetPaths instance mapping each build target to the set
        of unknown implicit source inputs for them.
    """
    result: dict[str, set[str]] = collections.defaultdict(set)

    inputs_sans_depfile = find_ninja_source_inputs(
        build_targets, ninja_runner, with_depfile=False
    )

    inputs_with_depfile = find_ninja_source_inputs(
        build_targets, ninja_runner, with_depfile=True
    )

    known_files = implicit_inputs.all_known_files
    known_dir_prefixes = [
        f"{known_dir}/" for known_dir in implicit_inputs.all_known_dirs
    ]

    def is_in_known_dir(path: str) -> bool:
        for known_dir_prefix in known_dir_prefixes:
            if path.startswith(known_dir_prefix):
                return True
        return False

    for build_target, with_depfile_inputs in inputs_with_depfile.items():
        unknown_inputs = with_depfile_inputs - inputs_sans_depfile.get(
            build_target, set()
        )
        for input_path in unknown_inputs:
            # Ignore source input paths that are in known_implicit_files.
            if input_path in known_files or is_in_known_dir(input_path):
                continue

            result[build_target].add(input_path)

    return result


def map_implicit_source_inputs(
    implicit_sources: T.Iterable[str],
    fuchsia_dir: str | Path,
    ninja_runner: NinjaRunner,
    ninja_outputs: NinjaOutputsBase,
) -> GnSourcePaths:
    """Map a list of unknown implicit inputs to GN labels.

    The result, when not empty can be passed to print_implicit_source_inputs_error().

    Args:
        implicit_sources: A sequence of paths, relative to the Ninja build
            directory, detailing unknown implicit source inputs.
        fuchsia_dir: Path to Fuchsia source directory.
        ninja_runner: A NinjaRunner instance.
        ninja_outputs: A NinjaOutputsBase instance, used to map
            Ninja output paths to their corresponding GN target label.

    Returns:
        A GnSourcePaths instance mapping GN labels to the set of input source
        paths it depends on.
    """
    # run the ninja affected tool to know which output files are affected by a given
    # implicit input. then use that to get the corresponding gn target label.

    # all source inputs begin with a prefix like ../../../../ that points
    # to the fuchsia source directory from the ninja sub-build directory.
    source_prefix = os.path.relpath(fuchsia_dir, ninja_runner.build_dir) + "/"

    gn_label_to_sources: GnSourcePaths = collections.defaultdict(set)

    all_source_paths = list(implicit_sources)
    while all_source_paths:
        # Limit the number of source path listed on the tool to avoid "Argument list too long" errors
        # when invoking Ninja.
        count = min(500, len(all_source_paths))
        source_paths = all_source_paths[:count]
        all_source_paths = all_source_paths[count:]

        affected_with_depfile = run_ninja_tool(
            "affected", source_paths, ninja_runner, with_depfile=True
        )
        affected_sans_depfile = run_ninja_tool(
            "affected", source_paths, ninja_runner, with_depfile=False
        )
        for source_path, with_depfile_outputs in affected_with_depfile.items():
            sans_depfile_outputs: set[str] = affected_sans_depfile.get(
                source_path, set()
            )
            only_depfile_outputs = with_depfile_outputs - sans_depfile_outputs
            for output in only_depfile_outputs:
                gn_label = ninja_outputs.path_to_gn_label(output)
                # A few output paths do not have a GN label, for
                # example build.ninja.stamp, ignore them
                if gn_label:
                    gn_label_to_sources[gn_label].add(
                        source_path.removeprefix(source_prefix)
                    )

    return gn_label_to_sources


def print_missing_source_inputs_error(
    gn_missing_paths: GnSourcePaths,
    fuchsia_dir: str | Path,
    build_dir: str | Path,
    out: T.TextIO,
) -> None:
    """Print an error message detailiong all missing source inputs.

    Args:
        gn_missing_paths: A GnSourcePaths instance.
        fuchsia_dir: Path to Fuchsia directory.
        build_dir: Path to Ninja build directory.
        out: The text output stream to use.
    """
    print(
        f"ERROR: The following {len(gn_missing_paths)} GN targets have missing inputs",
        file=out,
    )
    for gn_label, sources in sorted(gn_missing_paths.items()):
        print(f"\n{gn_label}", file=out)
        for source in sorted(sources):
            path = os.path.relpath(os.path.join(build_dir, source), fuchsia_dir)
            print(f"    {path}", file=out)
    print("", file=out)
    print(
        f"""The most common reasons to see this error are the following:

- A C++ header that does not exist, for example due to a typographic error.
  These are neither detected by GN nor Ninja. Either remove them if they are
  obsolete, or fix their path in the target declaration.

- A ninja_implicit_inputs_file() target that points to a directory instead of
  a file, or a ninja_implicit_inputs_directory() that points to a file instead
  of a directory.

If this doesn't correspond to any of these cases, you might want to file a bug at
go/fuchsia-build-bug with reproduction steps.

""",
        file=out,
    )


def print_implicit_source_inputs_error(
    gn_source_paths: GnSourcePaths,
    out: T.TextIO,
) -> None:
    """Print an error message detailing all the implicit inputs.

    This will look like:

    ```
    ERROR: The following <COUNT> GN targets use implicit source inputs:

    //third_party/boringssl:crypto-static(//build/toolchain:host_x64)
        third_party/boringssl/src/crypto/fipsmodule/aes/aes.cc.inc
        third_party/boringssl/src/crypto/fipsmodule/aes/aes_nohw.cc.inc
        ...

    //src/lib/zbitl:zbitl(//build/toolchain:host_x64)
        sdk/lib/zbi-format/include/lib/zbi-format/internal/debugdata.h
        ...

    ```

    Followed by instructions on how to fix common issues.

    Args:
        gn_source_paths: A GnSourcePaths instance.
        out: The text output stream to use.
    """
    print(
        f"ERROR: The following {len(gn_source_paths)} GN targets use undeclared source inputs:",
        file=out,
    )
    for gn_label, sources in sorted(gn_source_paths.items()):
        print(f"\n{gn_label}", file=out)
        for source in sorted(sources):
            print(f"    {source}", file=out)
    print("", file=out)
    print(
        f"""The most common reasons to see this error are the following:

- A C++ header that is not properly declared in the 'public' or 'sources' argument
  of its defining GN target. Just add the header in the target definition.

- A C++ header with an incorrect path, as GN doesn't check that the declared
  headers actually exist! Just fix the header path.

- A C++ header from a target dependency, which is not correctly added to the
  dependent target's 'deps' or 'public_deps'. Add the missing dependency.

- An action() target that is missing a source file path from its 'inputs' argument.
  Fix it if possible.

If this doesn't correspond to any of these cases, you might want to add a
ninja_implicit_file_inputs() or ninja_implicit_directory_inputs() dependency
to your target to specify possible extra inputs, or contact the Fuchsia build team
after filing a bug at go/fuchsia-build-bug with reproduction steps.

""",
        file=out,
    )


def create_ninja_runner_and_outputs(
    ninja_tool: str | Path, build_dir: str | Path
) -> tuple[NinjaRunner, NinjaOutputsBase]:
    """Create a NinjaRunner and NinjaOutputsBase instance.

    This is a convenience for other modules that import this one, as they
    won't have to import gn_artifacts and ninja_artifacts themselves just
    to get these values.

    Args:
        ninja_tool: Path to the Ninja binary to use.
        build_dir: Path to the Ninja build directory.
    Returns:
        A (ninja_runner, ninja_outputs) tuple.
    """
    ninja_runner = NinjaRunner(Path(ninja_tool), Path(build_dir))

    ninja_outputs = NinjaOutputsJSON()
    ninja_outputs.load_from_file(Path(build_dir) / "ninja_outputs.json")

    return ninja_runner, ninja_outputs


def create_gn_qualifier(build_dir: str | Path) -> GnLabelQualifier:
    """Create a GnLabelQualifier from the content of the build directory.

    This is a convenience for other modules that import this one, as they
    won't have to import gn_labels themselves just to get this value.

    Args:
        build_dir: Path to the Ninja build directory.
    Returns:
        A gn_targets.GnQualifier object.
    """
    return GnLabelQualifier.create_from_build_dir(build_dir)

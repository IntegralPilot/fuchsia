#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Generate a host test wrapper and associated runtime directory and runtime deps list."""

import argparse
import dataclasses
import json
import os
import shlex
import shutil
import sys
import typing as T
from pathlib import Path

sys.path.append(str(Path(__file__).parent / "../scripts"))
import build_utils
import runfiles_utils


@dataclasses.dataclass
class FuchsiaHostTestDataInfo:
    """Represents a single FuchsiaHostTestDataInfo provider."""

    # LINT.IfChange(FuchsiaHostTestDataInfo)
    label: str
    files: dict[str, str] = dataclasses.field(default_factory=dict)
    # LINT.ThenChange(//build/bazel/rules/host_tests/host_test_data.bzl:FuchsiaHostTestDataInfo)

    @staticmethod
    def from_json(json_value: T.Any) -> "FuchsiaHostTestDataInfo":
        assert isinstance(
            json_value, dict
        ), f"Input must be dictionary, got {type(json_value)}"

        label = json_value.get("label", "")
        assert label and isinstance(
            label, str
        ), f"Input must have a non-empty string label, got {label}"

        files = json_value.get("files", {})
        assert files and isinstance(
            files, dict
        ), f"Input must have a non-empty dictionary of files, got {files}"

        return FuchsiaHostTestDataInfo(
            label=label,
            files=files,
        )


class FuchsiaHostTestDataManifest:
    """A list of FuchsiaHostTestDataInfo providers."""

    def __init__(self, infos: list[FuchsiaHostTestDataInfo]):
        self.infos = infos

    def generate_final_map(self, bazel_execroot: Path) -> dict[str, Path]:
        """Generate final { dest_path -> source_path } map.

        Returns:
           A { dest_path -> source_path } map, where keys are path strings relative
           to the test runtime directory, and source_path is a Path object pointing to the
           actual file.

        Raises:
            ValueError: If there are duplicate destination paths with different source paths.
        """
        # Check for duplicates.
        # Map dest_path to (label, source_path)
        result: dict[str, Path] = {}
        labels_map: dict[str, str] = {}  # maps dest_path -> label
        for info in self.infos:
            for dest_path, source_path in info.files.items():
                src_path = bazel_execroot / source_path
                cur_source = result.setdefault(dest_path, src_path)
                if cur_source != src_path:
                    raise ValueError(
                        f"""
Conflict for destination path with multiple sources: {dest_path}
Labels:
    {labels_map[dest_path]}
    {info.label}
Sources:
    {cur_source}
    {source_path}

"""
                    )
                labels_map[dest_path] = info.label
        return result

    @staticmethod
    def from_json(json_value: T.Any) -> "FuchsiaHostTestDataManifest":
        assert isinstance(
            json_value, list
        ), f"Input must be a list, got: {type(json_value)}"
        if json_value:
            assert isinstance(
                json_value[0], dict
            ), f"Input must be a list of dicts, got: list of {type(json_value[0])}"
            infos = [
                FuchsiaHostTestDataInfo.from_json(info_dict)
                for info_dict in json_value
            ]
        else:
            infos = []

        return FuchsiaHostTestDataManifest(infos)


def find_ninja_build_dir() -> Path:
    """Find the Ninja build directory.

    This only works if this script is invoked locally from the real Bazel execroot.
    It works by walking up the directory tree from the current working directory
    until it finds a directory containing a regenerator_outputs/ directory.

    Returns:
        The absolute path to the Ninja build directory.

    Raises:
        FileNotFoundError: If the Ninja build directory is not found.
    """
    start_path = Path.cwd()
    cur_path = start_path
    while cur_path != cur_path.parent:
        if (cur_path / "regenerator_outputs").is_dir():
            return cur_path.resolve()
        cur_path = cur_path.parent
    raise FileNotFoundError(
        f"Ninja build directory not found from: {start_path}"
    )


def remove_bazel_out_prefix(bazel_path: str) -> str:
    """Remove the bazel-out/<config_dir>/bin/ prefix from a Bazel path."""
    segments = bazel_path.split("/")
    assert (
        len(segments) > 3
        and segments[0] == "bazel-out"
        and segments[2] == "bin"
    ), f"Invalid bazel path: {bazel_path}"
    return "/".join(segments[3:])


def parse_data_runfile_path(
    runfile_path: str, fuchsia_dir: Path, bazel_execroot: Path
) -> tuple[str, Path]:
    """Parse a data runfile path into a canonical repo name and a short path.

    Args:
        runfile_path: The path to the data runfile.
        fuchsia_dir: Path to the Fuchsia source directory.
        bazel_execroot: Path to the Bazel execroot.

    Returns:
        A tuple of (rlocation, artifact_path), where rlocation is a string used both as
        the key and target in output_manifest_entries, and artifact_path is a Path value
        pointing to the actual file.
    """
    if runfile_path.startswith("bazel-out/"):
        # An artifact, the path is relative to the bazel execroot.
        rlocation = "_main/" + remove_bazel_out_prefix(runfile_path)
        artifact_path = bazel_execroot / runfile_path
    elif runfile_path.startswith("external/"):
        # An artifact path that belongs to an external repository.
        rlocation = runfile_path.removeprefix("external/")
        artifact_path = bazel_execroot / runfile_path
    else:
        # A source file, the path is relative to the workspace, which itself
        # symlinks the content of the Fuchsia source directory.
        rlocation = f"_main/{runfile_path}"
        artifact_path = fuchsia_dir / runfile_path

    return rlocation, artifact_path


def format_ld_library_path_export(
    env_vars: list[tuple[str, str]], so_dirs: set[str]
) -> tuple[list[tuple[str, str]], str]:
    """Extracts user LD_LIBRARY_PATH, filters env_vars, and constructs the shell export statement."""
    user_ld_library_path = None
    filtered_env_vars = []
    for varname, value in env_vars:
        if varname == "LD_LIBRARY_PATH":
            user_ld_library_path = value
        else:
            filtered_env_vars.append((varname, value))

    ld_paths = []
    if so_dirs:
        for d in sorted(so_dirs):
            if d == ".":
                ld_paths.append("${PWD}")
            else:
                ld_paths.append(f"${{PWD}}/{d}")
    if user_ld_library_path is not None and user_ld_library_path != "":
        ld_paths.append(user_ld_library_path)

    ld_library_path_export = ""
    if ld_paths:
        path_str = ":".join(ld_paths)
        # The `${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}` syntax safely appends the existing
        # LD_LIBRARY_PATH if set, avoiding a trailing colon which would incorrectly
        # cause the dynamic linker to search the current working directory.
        ld_library_path_export = f'export LD_LIBRARY_PATH="{path_str}${{LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}}"'

    return filtered_env_vars, ld_library_path_export


def generate_test_wrapper(
    entry_point: Path,
    entry_runfiles_manifest: Path,
    test_label: str,
    output_launcher: Path,
    output_runtime_dir: Path,
    output_test_runtime_deps_json: Path,
    host_test_data_manifest: Path | None,
    host_test_wrapper_template: Path,
    data_runfiles: list[str],
    test_args: list[str],
    bazel_execroot: Path,
    python_test_lister: str,
    python_test_interpreter: str,
    python_test_info_file: Path,
) -> int:
    """Generate a Bazel host test wrapper script and related files.

    This function generates three files of interest:

    - A shell script used to invoke the actual test in the right work directory,
      and with hard-coded test arguments.

    - A directory to hold all runtime needed by the shell script. This includes the
      actual test binary, and all of its runfiles. Note that the runfiles manifest has
      been adjusted to only use paths relative to the runtime directory itself.

    - A JSON file containing the list of all files in the runtime directory, with paths
      relative to the Ninja build directory. This will be referenced by the tests.json
      entries for the test.

    Args:
        entry_point: The path to the entry point of the actual test, relative to the
            Bazel execroot. The name of that file is recorded as the test's name.
        entry_runfiles_manifest: The path to the runfiles manifest of the actual test,
            relative to the Bazel execroot.
        test_label: The label of the wrapper test, as it will appear in tests.json.
        output_launcher: The path to the output launcher script.
        output_runtime_dir: The path to the output runtime directory.
        output_test_runtime_deps_json: The path to the output test runtime deps JSON file.
        test_args: The arguments to pass to the test.
        bazel_execroot: The path to the Bazel execroot.
        python_test_lister: Optional. The path to the Python test lister tool.
        python_test_interpreter: Optional. The path to the Python interpreter to use by the test.
        python_test_imports_file: Optional. The path to the Python test imports file.
    """
    ninja_build_dir = find_ninja_build_dir()
    fuchsia_dir = build_utils.find_fuchsia_dir(from_path=ninja_build_dir)

    # Read //build/bazel/BAZEL_RUNFILES.md to understand the layout of the runfiles directory.

    # First, locate the input runfiles manifest, load it, and clean it up a little.
    input_manifest_path = bazel_execroot / entry_runfiles_manifest
    assert (
        input_manifest_path.exists()
    ), f"Missing Bazel runfiles manifest: {input_manifest_path}"
    input_manifest = runfiles_utils.RunfilesManifest.CreateFrom(
        input_manifest_path.read_text()
    )
    input_manifest.remove_legacy_external_runfiles(workspace_name="fuchsia")

    input_runfiles_dir = bazel_execroot / f"{entry_point}.runfiles"
    assert (
        input_runfiles_dir.exists()
    ), f"Missing Bazel runfiles directory: {input_runfiles_dir}"

    # Second, locate the _repo_mapping file from it. This should be an absolute path or
    # an execroot-relative one.
    repo_mapping_str = input_manifest.lookup("_repo_mapping")
    assert (
        repo_mapping_str
    ), f"Missing _repo_mapping entry from runfiles manifest at: {input_manifest_path}"
    repo_mapping_path = Path(repo_mapping_str)
    if not repo_mapping_path.is_absolute():
        repo_mapping_path = bazel_execroot / repo_mapping_path
    assert (
        repo_mapping_path.exists()
    ), f"Missing Bazel repository mapping file: {repo_mapping_path}"

    def make_runtime_symlink(dest_path: Path, target_path: Path) -> None:
        """Creates a symlink with a relative target path calculated from target_path.

        This ensures the symlink remains valid in isolated environments (like
        Swarming bots in infra).
        """
        relative_target = os.path.relpath(target_path, dest_path.parent)
        build_utils.force_raw_symlink(dest_path, Path(relative_target))

    if output_runtime_dir.exists():
        shutil.rmtree(output_runtime_dir)
    output_runtime_dir.mkdir(parents=True, exist_ok=True)

    # For every entry in the binary's runfiles manifest, create a corresponding symlink in the
    # output runfiles directory, but only use paths relative to runtime_dir.
    output_runfiles_dir = output_runtime_dir / f"{entry_point.name}.runfiles"
    output_runfiles_dir.mkdir(parents=True, exist_ok=True)

    output_manifest_entries: dict[str, str] = {}
    runtime_deps_paths: list[str | Path] = []
    so_dirs: set[str] = set()
    for source_path, target_path_str in input_manifest.as_dict().items():
        if not target_path_str:
            # This is an empty file in the input runfiles dir, create an empty
            # one in the output runfiles dir too. These are used for things like
            # Python __init__.py files.
            dest_path = output_runfiles_dir / source_path
            manifest_path = ""
            dest_path.parent.mkdir(parents=True, exist_ok=True)
            dest_path.write_text("")
        else:
            dest_path = output_runfiles_dir / source_path

            target_path = Path(target_path_str)
            if not target_path.is_absolute():
                target_path = bazel_execroot / target_path_str

            make_runtime_symlink(dest_path, target_path)
            manifest_path = source_path

            if source_path.endswith(".so") or ".so." in source_path:
                rel_so_dir = os.path.relpath(
                    dest_path.parent, output_runtime_dir
                )
                so_dirs.add(rel_so_dir)

        output_manifest_entries[source_path] = manifest_path
        runtime_deps_paths.append(dest_path)

    # The data runfiles are not part of the binary's manifest and must be added to
    # the runtime_dir as symlinks, and to its manifest. The paths are "short" meaning
    # they are related to the bazel-bin/ directory, except those that belong in
    # external repositories, which begin with ../<canonical_repo_name>/
    for runfile_path in data_runfiles:
        rlocation, artifact_path = parse_data_runfile_path(
            runfile_path, fuchsia_dir, bazel_execroot
        )
        dest_path = output_runfiles_dir / rlocation
        make_runtime_symlink(dest_path, artifact_path)
        output_manifest_entries.setdefault(rlocation, rlocation)
        runtime_deps_paths.append(dest_path)

        if rlocation.endswith(".so") or ".so." in rlocation:
            rel_so_dir = os.path.relpath(dest_path.parent, output_runtime_dir)
            so_dirs.add(rel_so_dir)

    # Create the MANIFEST file in the destination runfiles directory.
    # Unlike the input manifest, it cannot contain absollute paths, and all paths are
    # relative to the runtime_dir directory. This ensures that the corresponding test
    # can be run in isolation on a test sharder infra bot.
    exported_manifest = runfiles_utils.RunfilesManifest(
        {
            rlocation: f"{entry_point.name}.runfiles/{target_path}"
            for rlocation, target_path in output_manifest_entries.items()
        }
    )
    output_manifest_path = output_runfiles_dir / "MANIFEST"
    output_manifest_path.write_text(exported_manifest.generate_content())
    runtime_deps_paths.append(output_manifest_path)

    # Create a symlink in foo.runtime_dir for the runfiles manifest.
    output_manifest_symlink = (
        output_runtime_dir / f"{entry_point.name}.runfiles_manifest"
    )
    make_runtime_symlink(output_manifest_symlink, output_manifest_path)
    runtime_deps_paths.append(output_manifest_symlink)

    # Create a symlink in foo.runtime_dir for the entry point.
    output_entry_point = output_runtime_dir / entry_point.name
    make_runtime_symlink(output_entry_point, bazel_execroot / entry_point)
    runtime_deps_paths.append(output_entry_point)

    # Generate the launcher script
    # First separate the environment variables from the other arguments.
    # They are encoded as arguments which look like 'env VARNAME=VALUE'
    # where the space after 'env' is intentional. They can appear in
    # any location of test_args.
    env_vars: list[tuple[str, str]] = []
    real_args: list[str] = []
    for n, arg in enumerate(test_args):
        if arg.startswith("env "):
            varname, equal, value = arg[4:].partition("=")
            assert equal == "=", f"Malformed environment argument [{arg}]"
            env_vars.append((varname, value))
        else:
            real_args.append(arg)

    env_vars, ld_library_path_export = format_ld_library_path_export(
        env_vars, so_dirs
    )

    # If there is at least one environment variable assignment
    # start the exec call with "env VAR1=VALUE1 VAR2=VALUE2 ..."
    env_vars_expr = (
        "env " + " ".join(shlex.quote(f"{k}={v}") for k, v in env_vars)
        if env_vars
        else ""
    )

    subtitutions = {
        "{{runtime_dir_location}}": os.path.relpath(
            output_runtime_dir, output_launcher.parent
        ),
        "{{test_name}}": os.path.basename(entry_point),
        "{{test_args}}": " ".join([shlex.quote(arg) for arg in real_args]),
        "{{env_vars}}": env_vars_expr,
        "{{ld_library_path_export}}": ld_library_path_export,
        "{{python_test_lister}}": (
            shlex.quote(python_test_lister) if python_test_lister else ""
        ),
        "{{python_test_interpreter}}": (
            shlex.quote(python_test_interpreter)
            if python_test_interpreter
            else ""
        ),
    }
    launcher_text = host_test_wrapper_template.read_text()
    for substitution, value in subtitutions.items():
        launcher_text = launcher_text.replace(substitution, value)

    output_launcher.parent.mkdir(parents=True, exist_ok=True)
    output_launcher.write_text(launcher_text)
    output_launcher.chmod(0o755)

    # Generate a bazel_host_py_test_imports.txt file in the runtime directory,
    # which will be used by the lister tool.
    if python_test_info_file:
        bazel_imports_path = output_runtime_dir / "bazel_host_py_test_info.json"
        bazel_imports_path.write_text(python_test_info_file.read_text())
        runtime_deps_paths.append(bazel_imports_path)

    # Add all host_test_data() runtime dependencies to the runtime directory
    # and the runtimes_deps list, but do not add them to the generated
    # Bazel runfiles manifest.
    if host_test_data_manifest:
        try:
            with host_test_data_manifest.open() as f:
                test_data_manifest = FuchsiaHostTestDataManifest.from_json(
                    json.load(f)
                )
        except Exception as e:
            print(
                f"ERROR: Failed to parse host_test_data_manifest: {e}",
                file=sys.stderr,
            )
            return 1

        host_test_data_map = test_data_manifest.generate_final_map(
            bazel_execroot
        )
        for dest, source in host_test_data_map.items():
            dest_path = output_runtime_dir / dest
            make_runtime_symlink(dest_path, source)
            runtime_deps_paths.append(dest_path)

    # Generate the test_runtime_deps.json file.
    output_test_runtime_deps_json.parent.mkdir(parents=True, exist_ok=True)
    output_test_runtime_deps_json.write_text(
        json.dumps(
            sorted(
                [
                    os.path.relpath(path, ninja_build_dir)
                    for path in runtime_deps_paths
                ]
            )
        )
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--entry-point",
        type=Path,
        required=True,
        help="The entry point to wrap.",
    )
    parser.add_argument(
        "--entry-runfiles-manifest",
        type=Path,
        required=True,
        help="The runfiles manifest of the entry point.",
    )
    parser.add_argument(
        "--test-label", type=str, required=True, help="The label of the test."
    )
    parser.add_argument(
        "--output-launcher",
        type=Path,
        required=True,
        help="The output launcher script.",
    )
    parser.add_argument(
        "--output-runtime-dir",
        type=Path,
        required=True,
        help="The output runtime directory.",
    )
    parser.add_argument(
        "--output-test-runtime-deps-json",
        type=Path,
        required=True,
        help="The output runtime_deps.json file.",
    )
    parser.add_argument(
        "--data-runfile",
        action="append",
        default=[],
        type=str,
        help="Data runfiles to include in the test's runfiles.",
    )
    parser.add_argument(
        "--host-test-data-manifest",
        type=Path,
        help="An input manifest describing host_test_data() runtime dependencies.",
    )
    parser.add_argument(
        "--host-test-wrapper-template",
        type=Path,
        required=True,
        help="Input path to test wrapper script template.",
    )
    parser.add_argument(
        "--test-arg",
        action="append",
        type=str,
        default=[],
        help="Extra arguments passed to the test entry point.",
    )
    parser.add_argument(
        "--bazel-execroot",
        type=Path,
        default=Path.cwd(),
        help="The Bazel execroot (default to current directory).",
    )
    parser.add_argument(
        "--python-test-lister",
        type=str,
        default="",
        help="For python tests, runtime path of the test lister tool",
    )
    parser.add_argument(
        "--python-test-interpreter",
        type=str,
        default="python3",
        help="For python tests, the interpreter to use by the test.",
    )
    parser.add_argument(
        "--python-test-info",
        type=Path,
        help="For python tests, an input file describing the test.",
    )
    args = parser.parse_args()

    return generate_test_wrapper(
        args.entry_point,
        args.entry_runfiles_manifest,
        args.test_label,
        args.output_launcher,
        args.output_runtime_dir,
        args.output_test_runtime_deps_json,
        args.host_test_data_manifest,
        args.host_test_wrapper_template,
        args.data_runfile,
        args.test_arg,
        args.bazel_execroot,
        args.python_test_lister,
        args.python_test_interpreter,
        args.python_test_info,
    )


if __name__ == "__main__":
    sys.exit(main())

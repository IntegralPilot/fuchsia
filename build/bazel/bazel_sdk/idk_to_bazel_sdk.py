#!/usr/bin/env python3
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""A script to convert an IDK export directory into a Fuchsia SDK directory."""

import argparse
import json
import os
import shutil
import stat
import subprocess
import sys
import typing as T
from pathlib import Path

_SCRIPT_DIR = Path(__file__).parent

# See comment in BUILD.bazel to see why changing sys.path manually
# is required here. Bytecode generation is also disallowed to avoid
# polluting the Bazel execroot with .pyc files that can end up in
# the generated TreeArtifact, resulting in issues when dependent
# actions try to read it.
sys.dont_write_bytecode = True
sys.path.insert(0, str(_SCRIPT_DIR))
from generate_sdk_build_rules import generate_sdk_repository


def _copy_file_to(src_path: Path, dst_path: Path) -> None:
    """Hard-link or copy a file

    Args:
       src_path: source path
       dst_path: destination path
    """
    try:
        dst_path.hardlink_to(src_path.resolve())
    except OSError:
        shutil.copy2(src_path, dst_path)


class TempOutputDir(object):
    """Temporary output directory, starts empty, swapped atomically
    with real output directory on success only.

    This can be used as a context manager, e.g.:

        with TempOutputDir(final_output_dir_path) as output_dir:
            ... write stuff to |output_dir| here.
            ... on exit, the content of 'output_dir' is atomically
            ... swapped with |final_output_dir_path|. If an
            ... exception occurs though.
    """

    def __init__(self, output_dir: Path):
        self._final_dir = output_dir
        self._output_dir = Path(f"{output_dir}.temp")
        self._has_output_dir = True
        if self._output_dir.exists():
            shutil.rmtree(self._output_dir)
        self._output_dir.mkdir(parents=True, exist_ok=False)

    @property
    def path(self) -> Path:
        return self._output_dir

    def commit(self) -> None:
        """Ensure the temporary output dir is committed to its final location."""
        if self._has_output_dir:
            swap_dir = None
            if self._final_dir.exists():
                swap_dir = Path(f"{self._final_dir}.atomic_swap")
                if swap_dir.exists():
                    shutil.rmtree(swap_dir)

                self._final_dir.rename(swap_dir)

            self._output_dir.rename(self._final_dir)

            if swap_dir:
                shutil.rmtree(swap_dir)

            self._has_output_dir = False

    def close(self) -> None:
        """Close the output directory, remove it if it was not committed."""
        if self._has_output_dir:
            # The output directory was not committed, something
            # wrong probably happened.
            shutil.rmtree(self._output_dir)
            self._has_output_dir = False

    def __enter__(self) -> Path:
        return self._output_dir

    def __exit__(self, exc_type: T.Any, exc_val: T.Any, exc_tb: T.Any) -> None:
        if exc_type is None:
            self.commit()
        self.close()


class StarlarkStruct(object):
    """A class used to mimic a Starlark struct() value at runtime."""

    def __init__(self, **kwargs: T.Any) -> None:
        self._args = kwargs

    def __str__(self) -> str:
        result = "struct("
        for key, value in self._args.items():
            result += f"{key} = {repr(value)},"
        result += ")"
        return result

    def __getattr__(self, name: str) -> T.Any:
        if not name in self._args:
            raise AttributeError
        return self._args[name]


class BazelPath(object):
    """A Python object that mimics a Bazel path value."""

    def __init__(self, value: T.Self | Path | str, output_dir: Path = Path()):
        if isinstance(value, BazelPath):
            self._value: Path = value._value
        elif isinstance(value, Path):
            self._value = value
        else:
            if value.startswith("/"):
                self._value = Path(value)
            else:
                self._value = output_dir / value

    def __str__(self) -> str:
        return str(self._value)

    def as_path(self) -> Path:
        return self._value

    @property
    def exists(self) -> bool:
        return self._value.exists()

    @property
    def dirname(self) -> "BazelPath":
        return BazelPath(self._value.parent)

    @property
    def is_dir(self) -> bool:
        return self._value.is_dir()

    @property
    def basename(self) -> str:
        return self._value.name

    def get_child(self, *names: str) -> "BazelPath":
        return BazelPath(self._value.joinpath(*names))

    def read(self) -> str:
        return self._value.read_text()

    def readdir(self) -> list["BazelPath"]:
        # The Bazel implementation of readdir will fail with an error like
        # "Error in readdir: can't readdir(), not a directory". This will match
        # what happens when we try to iterate over the generator returned from
        # iterdir() when it is called on a regular file.
        return [BazelPath(v) for v in self._value.iterdir()]


class BazelRepositoryAttr(object):
    """A Python object that mimics the repository_ctx.attr attribute."""

    def __init__(self, name: str) -> None:
        self._name = name

    @property
    def parent_sdk(self) -> None:
        return None

    @property
    def parent_sdk_local_paths(self) -> list[T.Any]:
        return []

    @property
    def name(self) -> str:
        return self._name

    @property
    def visibility_templates(self) -> dict[str, str]:
        return {}


class BazelRepositoryContext(object):
    """A Python object that mimics a Starlark repository_ctx at runtime."""

    def __init__(
        self,
        name: str,
        workspace_root: Path,
        output_dir: Path,
        buildifier: Path,
        copy_files: bool,
    ):
        self._workspace_root = workspace_root
        self._output_dir = output_dir
        self._attr = BazelRepositoryAttr(name)
        self._bazel_outputs: list[Path] = []
        self._buildifier = buildifier
        self._copy_files = copy_files
        self._depfile_inputs: set[str] = set()

    @property
    def workspace_root(self) -> Path:
        """Return the workspace root path."""
        return self._workspace_root

    @property
    def attr(self) -> BazelRepositoryAttr:
        """Implement the repository_ctx.attr property."""
        return self._attr

    @property
    def output_dir(self) -> Path:
        """Return output directory."""
        return self._output_dir

    def path(self, path: BazelPath | str) -> BazelPath:
        """Implement repository_ctx.path()."""
        return BazelPath(path, self._output_dir)

    def _add_depfile_input(self, path: Path) -> None:
        path = path.resolve()
        if path.is_relative_to(self._output_dir):
            return
        self._depfile_inputs.add(os.path.relpath(path))

    def get_depfile_inputs(self) -> T.Sequence[str]:
        return sorted(self._depfile_inputs)

    def read(self, path: BazelPath | str) -> str:
        """Implement repository_ctx.read()."""
        if isinstance(path, BazelPath):
            dst_path = path.as_path()
        else:
            dst_path = Path(path)
        if not dst_path.is_absolute():
            dst_path = self._output_dir / dst_path
        self._add_depfile_input(dst_path)
        return dst_path.read_text()

    def file(
        self,
        filename: BazelPath | Path | str,
        content: str,
        executable: bool = False,
    ) -> None:
        """Implement repository_ctx.file()."""
        if isinstance(filename, BazelPath):
            filename = str(filename)
        dst_path = self._output_dir / filename
        dst_path.parent.mkdir(parents=True, exist_ok=True)
        dst_path.write_text(content)
        if str(dst_path).endswith((".bzl", ".bazel")):
            self._bazel_outputs.append(dst_path)
        if executable:
            s = dst_path.stat()
            dst_path.chmod(
                s.st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
            )

    def symlink(self, target: BazelPath, link_path: str) -> None:
        """Implement repository_ctx.symlink()."""
        dst_path = self._output_dir / link_path
        src_path = target.as_path()
        self._add_depfile_input(src_path)
        if self._copy_files:
            src_path = src_path.resolve()
            if src_path.is_dir():
                shutil.copytree(src_path, dst_path)
            else:
                _copy_file_to(src_path, dst_path)
        else:
            dst_path.symlink_to(target.as_path().resolve())

    def template(
        self,
        path: BazelPath | str,
        template: BazelPath | str,
        substitutions: dict[str, str],
        executable: bool = True,
    ) -> None:
        """Implement the repository_ctx.template() function."""
        template_input = self.read(BazelPath(template, self._output_dir))
        template_output = template_input
        for key, value in substitutions.items():
            if isinstance(value, list) or isinstance(value, dict):
                expansion = str(value)[1:-1]
            else:
                expansion = str(value)

            template_output = template_output.replace(key, expansion)

        self.file(path, template_output, executable)

    def execute(self, cmd: list[str]) -> "subprocess.CompletedProcess[str]":
        return subprocess.run(cmd, text=True, capture_output=True)

    def resolve_workspace_path(self, path: str) -> str:
        """Convert a path string relative to the workspace root into a real path."""
        assert path, f"Empty file path are not allowed"
        assert not path.startswith(
            ("@", "//")
        ), f"Labels are not allowed here: {path}"

        # Absolute paths are returned as-is.
        if path.startswith("/"):
            return path

        return f"{self.workspace_root}/{path}"

    def label_to_path(self, template_label: str) -> BazelPath:
        """Convert an SDK template label into a file path."""
        if template_label.startswith("//"):
            return BazelPath(
                f"{self.workspace_root}/{template_label[2:]}".replace(":", "/")
            )

        assert False, f"Invalid template file label: {template_label}"

    def copy_files(self, files_to_copy: dict[str, list[str]]) -> None:
        """Symlink or copy files from one location to another, replicating the relative directory structure

        This replicates the logic of symlink_or_copy_files() in //fuchsia/workspace/utils.bzl

        Args:
            files_to_copy: a { str -> [str] } dictionary, where keys are source root directory paths,
               and values are lists of paths relative to the source root path, and also used as destination
               paths, relative to the repository's directory.
        """
        unique_files_to_copy = set(
            (root, file)
            for root, files in files_to_copy.items()
            for file in files
        )

        # Maps destination path to source path.
        all_copies: dict[Path, Path] = {}

        for src_root, file in unique_files_to_copy:
            dest_path = self._output_dir / file
            src_path = (Path(src_root) / file).resolve()
            self._add_depfile_input(src_path)
            cur_src_path = all_copies.setdefault(dest_path, src_path)
            assert (
                cur_src_path == src_path
            ), f"Same file destination specified in two different places: {cur_src_path} and {src_path}, file={file}"

        # Create all destination directories.
        all_dest_dirs = sorted(p.parent for p in all_copies.keys())
        for dest_dir in all_dest_dirs:
            dest_dir.mkdir(parents=True, exist_ok=True)

        for dst_path, src_path in all_copies.items():
            # Do not clobber existing files.
            if dst_path.exists():
                continue
            if self._copy_files:
                _copy_file_to(src_path, dst_path)
            else:
                dst_path.symlink_to(src_path)

    def report_progress(self, message: str) -> None:
        print(message, file=sys.stdout)

    def run_buildifier(self, args: list[str]) -> bool:
        # We are seeing errors in our infrastructure builds that are saying that
        # files have lint errors but when we look at the contents of those files
        # they seem fine. For now, we will retry buildifier if it fails to see if
        # this reduces our flakes. We only print out errors on the final attempt
        # to reduce noise for real errors.
        max_allowed_attempts = 2
        for attempt in range(max_allowed_attempts):
            if self._inner_run_buildifier(
                args, attempt == max_allowed_attempts - 1
            ):
                return True
        return False

    def _inner_run_buildifier(
        self, args: list[str], report_error: bool
    ) -> bool:
        if not self._buildifier:
            return True
        command = (
            [self._buildifier.resolve()]
            + args
            + sorted(str(p) for p in self._bazel_outputs)
        )
        try:
            subprocess.run(
                command,
                cwd=self._output_dir,
                text=True,
                capture_output=True,
                check=True,
            )
            return True
        except subprocess.CalledProcessError as e:
            # Only report errors if this is the final attempt
            if report_error:
                print(
                    f"Buildifier failed with the following error code: '{e.returncode}'"
                )
                print(f"stderr: {e.stderr}")
                print("Listing files with errors...\n")

                failed_files = set()
                for line in e.stderr.splitlines():
                    # Files have the format "some/file:1:2 # failure reason"
                    # or "some/file # failure reason". Try to grab the file
                    # path from the stderr.
                    file_path_str = line.split(maxsplit=1)[0].split(":")[0]

                    if file_path_str:
                        file_path = Path(self._output_dir) / file_path_str
                        failed_files.add(file_path)

                for file_path in sorted(failed_files):
                    try:
                        print(f"File: {file_path}")
                        print(file_path.read_text())
                    except FileNotFoundError:
                        print(f"file not found: {file_path}")
        return False


class PythonRuntime(object):
    """Implement the runtime interface expected by generate_sdk_build_rules.bzl.

    Keep this in sync with new_bazel_runtime() in that file.
    """

    def __init__(self, ctx: BazelRepositoryContext) -> None:
        self._ctx = ctx

    @property
    def ctx(self) -> BazelRepositoryContext:
        return self._ctx

    @staticmethod
    def value_is_dict(v: T.Any) -> bool:
        return isinstance(v, dict)

    @staticmethod
    def value_is_list(v: T.Any) -> bool:
        return isinstance(v, list)

    @staticmethod
    def value_is_struct(v: T.Any) -> bool:
        return isinstance(v, StarlarkStruct)

    @staticmethod
    def make_struct(**kwargs: T.Any) -> StarlarkStruct:
        return StarlarkStruct(**kwargs)

    @staticmethod
    def fail(message: str) -> None:
        print(f"ERROR: {message}", file=sys.stderr)
        sys.exit(1)

    @staticmethod
    def json_decode(input: str) -> T.Any:
        return json.loads(input)

    def label_to_path(self, label: str) -> BazelPath:
        return self._ctx.label_to_path(label)

    def file_copier(self, files_to_copy: dict[str, list[str]]) -> None:
        self._ctx.copy_files(files_to_copy)

    def workspace_path(self, path: str) -> BazelPath:
        return BazelPath(self._ctx.resolve_workspace_path(path))

    def find_repository_files_by_name(self, file_name: str) -> list[str]:
        result = []
        for subpath, subdirs, files in os.walk(self._ctx.output_dir):
            for file in files:
                if file == file_name:
                    # The starlark implementation invokes the 'find' tool which
                    # returns paths prefixes with "./" so do this here too.
                    result.append(
                        "./"
                        + os.path.relpath(
                            os.path.join(subpath, file), self._ctx.output_dir
                        )
                    )
        return result

    def run_buildifier(self, args: list[str]) -> bool:
        return self._ctx.run_buildifier(args)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--input-idk",
        metavar="INPUT_DIR",
        required=True,
        type=Path,
        help="Input IDK export directory path.",
    )
    parser.add_argument(
        "--output-sdk",
        metavar="OUTPUT_DIR",
        required=True,
        type=Path,
        help="Output Bazel SDK directory path.",
    )
    parser.add_argument(
        "--use-rules_fuchsia",
        action="store_true",
        help="Generate an SDK that depends on @rules_fuchsia to load rules.",
    )
    parser.add_argument(
        "--copy-files",
        action="store_true",
        help="Copy/hard-link files instead of symlinking them.",
    )
    parser.add_argument(
        "--buildifier",
        type=Path,
        help="Path to host buildifier tool, if available.",
    )
    parser.add_argument(
        "--bazel_rules_fuchsia",
        type=Path,
        help="Path to bazel_rules_fuchsia directory (auto-detected).",
    )
    parser.add_argument(
        "--depfile",
        type=Path,
        help="Path to output Ninja depfile.",
    )

    args = parser.parse_args()

    manifests = [
        {
            "root": str(args.input_idk.resolve()),
            "manifest": "meta/manifest.json",
        }
    ]

    if not args.bazel_rules_fuchsia:
        # Assume __file__ is //build/bazel/bazel_sdk/<name>.py
        # This script can be launched through multiple symlinks from the Bazel execroot or
        # even a build sandbox. Resolve __file__ directly to get the real location
        # of the script in the Fuchsia source checkout, then walk up parent directories
        # to get the correct directory.
        args.bazel_rules_fuchsia = (
            Path(__file__).resolve().parent.parent.parent
            / "bazel_sdk/bazel_rules_fuchsia"
        )

    workspace_root = args.bazel_rules_fuchsia

    with TempOutputDir(args.output_sdk.resolve()) as output_dir:
        bazel_repository_ctx = BazelRepositoryContext(
            "fuchsia_sdk",
            workspace_root.resolve(),
            output_dir,
            args.buildifier,
            bool(args.copy_files),
        )

        runtime = PythonRuntime(bazel_repository_ctx)

        generate_sdk_repository(
            runtime,
            manifests,
        )

        # We need to iterate over the bazel_rules_fuchsia code to
        # add all the files to the depfile to ensure that any changes
        # there are tracked.
        for file in args.bazel_rules_fuchsia.rglob("*"):
            if file.is_file():
                bazel_repository_ctx._add_depfile_input(file)

        if args.depfile:
            args.depfile.write_text(
                "%s: %s"
                % (
                    args.output_sdk / "WORKSPACE.bazel",
                    " ".join(bazel_repository_ctx.get_depfile_inputs()),
                )
            )

    return 0


if __name__ == "__main__":
    sys.exit(main())

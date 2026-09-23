# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Utilities related to managing Bazel runfiles manifests and directories."""

import os
import typing as T
from pathlib import Path

# Starting with Bazel 8, the runfiles sub-directory that matches
# the current workspace is *always* named "_main", independent of
# the workspace name provided in WORKSPACE, or the module name
# provided in MODULE.bazel.
#
# On the other hand, the _repo_mapping file will contain a line
# like this one (where the first character is a comma):
#
# ,workspace_name,_main
#
# To indicate that files from the main workspace are mapped to
# that directory.
#
_BAZEL_MAIN_WORKSPACE_NAME = "_main"

RepoMappingTree: T.TypeAlias = dict[str, dict[str, str]]


class RepoMapping(object):
    """Models the content of a repository mapping file."""

    def __init__(self, tree: RepoMappingTree) -> None:
        self._tree = tree

    @staticmethod
    def CreateFrom(content: str) -> "RepoMapping":
        tree = RepoMapping.parse_repo_mapping(content)
        return RepoMapping(tree)

    def map_apparent_name(
        self, apparent_name: str, source_name: str = ""
    ) -> str:
        """Convert a repository/module apparent name to its path in the runfiles directory.

        Args:
            apparent_name: Apparent name (workspace or repository name without a @ prefix).
            source_name: Optional canonical repository name where the apparent_name appears,
               without a @@ prefix. Default value is the empty string, corresponding to the
               main workspace.
        Returns:
            A directory name under the root runfiles directory, or an empty string
            if this apparent name is unknown.
        """
        top_map = self._tree.get(source_name, {})
        return top_map.get(apparent_name, "")

    def map_input_path(self, path: str, source_name: str = "") -> str:
        """Convert an rlocation input path to the corresponding manifest source path.

        This must replace the first path segment, and apparent workspace or repository
        name, by a canonical one. For example, assuming Bazel 8.0 + BzlMod enabled:

        my_project/src/foo -> _main/src/foo
        my_repo/data/file  ->  +custom_repo_ext+some_repo/data/file

        Args:
            path: An input runfiles path, such as <apparent_name>/something/else
            source_name: Optional canonical name of the source repository or workspace,
                without a @@ or @ prefix, where this input path is used.
                Default is empty string, which corresponds to the main workspace.
        Returns:
            The actual location relative to the runfiles directory root, e.g.
            <canonical_name>/something/else for a repository artifact, or _main/something/else
            for a workspace one.
        """
        apparent_name, sep, remainder = path.partition("/")
        if sep != "/":
            # single-segment paths can be symlinks and must not be mapped.
            # This is true for the .runfiles/_repo_mapping for example.
            return path

        mapped_dir = self.map_apparent_name(apparent_name, source_name)
        if not mapped_dir:
            # This apparent name is not in the mapping file, return path as-is.
            return path

        return f"{mapped_dir}/{remainder}"

    @staticmethod
    def parse_repo_mapping(content: str) -> RepoMappingTree:
        result: RepoMappingTree = {}
        for line in content.splitlines():
            # Each line contains three comma-separated names:
            # <source_name>,<apparent_name>,<runfile_name>
            #
            # Where <source_name> is either a canonical repository name, without a @@ prefix,
            # or the empty string for the main workspace.
            #
            # Where <apparent_name> is an apparent module/repository name, without a @ prefix,
            # that appears inside of <source_name>
            #
            # Where <runfile_name> is where the corresponding files are located in the
            # runfiles directory. It is usually a canonical name, or the special value "_main"
            # for the workspace (starting with Bazel8), or the module/workspace name (before Bazel 8)
            #
            # See runfiles_utils_test.py for concrete examples.
            #
            items = line.split(",")
            if len(items) != 3:
                raise ValueError(f"Invalid line in repo_mapping file: [{line}]")

            source_name, apparent_name, canonical_name = items

            # Save the value into a nested map {source_name -> { apparent_name -> canonical_name} }
            # and check for duplicate definitions.
            top_map = result.setdefault(source_name, {})
            cur_canonical = top_map.setdefault(apparent_name, canonical_name)
            if cur_canonical is not canonical_name:
                raise ValueError(
                    f"Duplicate canonical values for @{apparent_name} used in '{source_name}': {canonical_name} vs {cur_canonical}"
                )

        return result


class RunfilesManifest(object):
    """A class grouping method to manage Bazel runfiles manifests."""

    def __init__(self, file_map: dict[str, str]) -> None:
        self._map = file_map

    @staticmethod
    def CreateFrom(content: str) -> "RunfilesManifest":
        """Create new instance from manifest file content."""
        file_map = RunfilesManifest.parse_manifest(content)
        return RunfilesManifest(file_map)

    @staticmethod
    def CreateFromDirectory(runfiles_dir: Path) -> "RunfilesManifest":
        """Create new instance from a given directory."""
        assert (
            runfiles_dir.exists()
        ), f"Runfiles directory does not exist: {runfiles_dir}"
        # If the directory contains a manifest, use it.
        probe_manifest = runfiles_dir / "MANIFEST"
        if probe_manifest.exists():
            return RunfilesManifest.CreateFrom(probe_manifest.read_text())

        # Otherwise simple walk the directory for files and symlinks.
        file_map: dict[str, str] = {}
        for root, dirnames, filenames in os.walk(runfiles_dir):
            for filename in filenames:
                path = Path(os.path.join(root, filename))
                source_path = os.path.relpath(path, runfiles_dir)
                if path.is_symlink():
                    target = path.readlink()
                    file_map[source_path] = str(target)
                else:
                    # Empty files do not need a target path after the space separator.
                    # For non-empty files though, convert the source path to an
                    # absolute one.
                    if path.stat().st_size == 0:
                        file_map[source_path] = ""
                    else:
                        file_map[source_path] = str(path.resolve())

        return RunfilesManifest(file_map)

    def lookup(self, runfile_path: str) -> str:
        """Lookup a runfile's target location, or return an empty string on error.

        Args:
            runfile_path: A runfiles-root-relative source path.
               Note that this is *not* the path passed to an Rlocation()
               function at runtime, but can be the result of a previous
               RepoMapping.map_input_path() call.

        Returns:
            The target path recorded in the manifest. An empty string if not found.
        """
        return self._map.get(runfile_path, "")

    def find_source_path_for(self, target_path: str) -> str | None:
        """Find the source path that matches a given target path in the manifest.

        This can be used to locate a real executable from a Bazel output_base layout.
        For example, with:

        bazel-bin/src/foo
        bazel-bin/src/foo.runfiles/
            main_/
                src/
                    foo -----> absolute path of bazel-bin/src/foo

        find_source_path("<absolute_bazel-bin>/src/foo") will return
        "main_/src/foo".
        """
        for source, target in self._map.items():
            if target == target_path:
                return source

        return None

    @staticmethod
    def unescape(path: str) -> str:
        """Unescape a path that appears in a runfile manifest."""
        # Reference code from:
        # https://github.com/bazelbuild/rules_cc/blob/main/cc/runfiles/runfiles.cc#L182
        result = ""
        prev_ch = "\0"
        ch = "\0"
        for ch in path:
            result += ch
            if prev_ch == "\\":
                if ch == "s":
                    # Replace \s with a space in result.
                    result = result[:-2] + " "
                elif ch == "n":
                    # Replace \n with a newline in result.
                    result = result[:-2] + "\n"
                elif ch == "b":
                    # Replace \\ with a single backslash
                    result = result[:-1]

            prev_ch = ch
        return result

    @staticmethod
    def escape(path: str) -> str:
        """Escape a path for the runfile manifest."""
        result = ""
        for ch in path:
            if ch == "\\":
                result += "\\b"
            elif ch == " ":
                result += "\\s"
            elif ch == "\n":
                result += "\\n"
            else:
                result += ch
        return result

    @staticmethod
    def path_needs_escaping(path: str) -> bool:
        """Return True if a path needs escaping."""
        return any(ch in " \n\\" for ch in path)

    @staticmethod
    def parse_manifest(content: str) -> dict[str, str]:
        """Parse the content of a manifest file.

        Args:
            content: The file's content as a string.
        Returns:
            A dictionary mapping runfiles relative source paths
            to their target location as recorded in the manifest.
        """
        result: dict[str, str] = {}
        for line in content.splitlines():
            if not line:
                # Stop parsing on empty line. This mimics the behavior
                # of the C runfiles library parser.
                break
            if line[0] == " ":
                # A line that begins with a space contains escaped paths
                # separated by a space, e.g.:
                # <SPACE> <ESCAPED_RUNFILE_PATH> <SPACE> <ESCAPED_FILE_PATH>
                escaped_source, sep, escaped_target = line[1:].partition(" ")
                if sep != " ":
                    raise ValueError(
                        f"No space separator in manifest line: [{line}]"
                    )
                source = RunfilesManifest.unescape(escaped_source)
                target = RunfilesManifest.unescape(escaped_target)
            else:
                # Otherwise, the line contains two paths separated by spaces.
                # Note that it is possible for the target path to be empty, but the
                # space separator will always be present.
                source, sep, target = line.partition(" ")
                if sep != " ":
                    raise ValueError(
                        f"No space separator in manifest line: [{line}]"
                    )

            result[source] = target
        return result

    def remove_legacy_external_runfiles(self, workspace_name: str) -> None:
        """Remove legacy <project_name>/external/<repo_name>/..." entries.

        Starting with Bazel 8.0, --legacy_external_runfiles is off and these entries
        are no longer created, but before that it was enabled by default. These were
        already omitted when running `bazel test` since Bazel 7.1, but still kept for
        `bazel build` for legacy reasons.

        Args:
            workspace_name: Name of the workspace directory name at the root of the
                runfiles directory / collection. When BzlMod is enabled, this
                must always be `_main`. Otherwise this should match the name
                given in the workspace() directive of the project's WORKSPACE file,
                or "__main__" if none is provided.
        """
        external_prefix = f"{workspace_name}/external/"
        paths_to_remove = [
            p for p in self._map.keys() if p.startswith(external_prefix)
        ]
        for p in paths_to_remove:
            self._map.pop(p, None)

    def as_dict(self) -> dict[str, str]:
        """Return the content of this manifest as a {source_path -> target_path} dictionary."""
        return self._map.copy()

    def generate_content(self) -> str:
        """Generate a manifest file content from the current instance."""
        result = ""
        for source, target in sorted(self._map.items()):
            if self.path_needs_escaping(source) or self.path_needs_escaping(
                target
            ):
                result += f" {self.escape(source)} {self.escape(target)}\n"
            else:
                result += f"{source} {target}\n"
        return result

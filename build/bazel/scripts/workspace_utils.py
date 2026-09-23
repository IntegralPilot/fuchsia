# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Misc utility functions related to the Bazel workspace."""

import dataclasses
import json
import os
import shlex
import stat
import sys
import typing as T
from pathlib import Path
from textwrap import dedent

_SCRIPT_DIR = os.path.dirname(__file__)
sys.path.insert(0, _SCRIPT_DIR)
import build_utils
from bazel_build_flags import DefaultBuildFlagsMap

# LINT.IfChange(gn_targets_dir_symlink)
# Location of the @gn_targets redirection symlink, relative
# to the Bazel workspace.
GN_TARGETS_DIR_SYMLINK = "fuchsia_build_generated/gn_targets_dir"
# LINT.ThenChange(//build/bazel/toplevel.MODULE.bazel:gn_targets_dir)

# Separation character used by Bazel for extension-generated repo names.
#
# See https://bazel.build/external/extension#repository_names_and_visibility
_BAZEL_REPO_NAME_SEPARATOR = "+"


def workspace_should_exclude_file(path: str) -> bool:
    """Return true if a file path must be excluded from the symlink list.

    Args:
        path: File path, relative to the top-level Fuchsia directory.
    Returns:
        True if this file path should not be symlinked into the Bazel workspace.
    """
    # Never symlink to the 'out' directory.
    if path == "out":
        return True
    # Don't symlink the Jiri files, this can confuse Jiri during an 'jiri update'
    # Don't symlink the .fx directory (TODO(digit): I don't remember why?)
    # Don´t symlink the .git directory as well, since it needs to be handled separately.
    if path.startswith((".jiri", ".fx", ".git")):
        return True
    # Don't symlink the convenience symlinks from the Fuchsia source tree
    if path in ("bazel-bin", "bazel-out", "bazel-repos", "bazel-workspace"):
        return True
    return False


def make_removable(path: str) -> None:
    """Ensure the file at |path| is removable."""

    islink = os.path.islink(path)

    # Skip if the input path is a symlink, and chmod with
    # `follow_symlinks=False` is not supported. Linux is the most notable
    # platform that meets this requirement, and adding S_IWUSR is not necessary
    # for removing symlinks.
    if islink and (os.chmod not in os.supports_follow_symlinks):
        return

    info = os.stat(path, follow_symlinks=False)
    if info.st_mode & stat.S_IWUSR == 0:
        try:
            if islink:
                os.chmod(
                    path, info.st_mode | stat.S_IWUSR, follow_symlinks=False
                )
            else:
                os.chmod(path, info.st_mode | stat.S_IWUSR)
        except Exception as e:
            raise RuntimeError(
                f"Failed to chmod +w to {path}, islink: {islink}, info: {info}, error: {e}"
            )


def remove_dir(path: str) -> None:
    """Properly remove a directory."""
    # shutil.rmtree() does not work well when there are readonly symlinks to
    # directories. This results in weird NotADirectory error when trying to
    # call os.scandir() or os.rmdir() on them (which happens internally).
    #
    # Re-implement it correctly here. This is not as secure as it could
    # (see "shutil.rmtree symlink attack"), but is sufficient for the Fuchsia
    # build.
    all_files = []
    all_dirs = []
    for root, subdirs, files in os.walk(path):
        # subdirs may contain actual symlinks which should be treated as
        # files here.
        real_subdirs = []
        for subdir in subdirs:
            if os.path.islink(os.path.join(root, subdir)):
                files.append(subdir)
            else:
                real_subdirs.append(subdir)

        for file in files:
            file_path = os.path.join(root, file)
            all_files.append(file_path)
            make_removable(file_path)
        for subdir in real_subdirs:
            dir_path = os.path.join(root, subdir)
            all_dirs.append(dir_path)
            make_removable(dir_path)

    for file in reversed(all_files):
        os.remove(file)
    for dir in reversed(all_dirs):
        os.rmdir(dir)
    os.rmdir(path)


def create_clean_dir(path: str) -> None:
    """Create a clean directory."""
    if os.path.exists(path):
        remove_dir(path)
    os.makedirs(path)


# Type describing a callable that takes a file path as input and
# returns a content hash string for it as output.
FileHasherType: T.TypeAlias = T.Callable[[str | Path], str]


class GeneratedWorkspaceFiles(object):
    """Models the content of a generated Bazel workspace.

    Usage is:
      1. Create instance

      2. Optionally call set_file_hasher() if recording the content hash
         of input files is useful (see add_input_file()).

      3. Call the record_xxx() methods as many times as necessary to
         describe new workspace entries. This does not write anything
         to disk.

      4. Call to_json() to return a dictionary describing all added entries
         so far. This can be used to compare it to the result of a previous
         workspace generation.

      5. Call write() to populate the workspace with the recorded entries.
    """

    def __init__(self) -> None:
        self._files: dict[str, T.Any] = {}
        self._file_hasher: FileHasherType | None = None
        self._input_files: set[Path] = set()

    def set_file_hasher(self, file_hasher: FileHasherType) -> None:
        self._file_hasher = file_hasher

    def _check_new_path(self, path: str) -> None:
        assert path not in self._files, (
            "File entry already in generated list: " + path
        )

    @property
    def input_files(self) -> set[Path]:
        """The set of input file Paths that were read through read_text_file()."""
        return self._input_files

    def read_text_file(self, path: Path) -> str:
        """Read an input file and return its content as text.

        This also ensures that the file is tracked as an input file
        to later be returned through the self.input_files property.

        Args:
            path: Input file path.
        Returns:
            content of input path, as a string.
        """
        path = path.resolve()
        self._input_files.add(path)
        self.record_input_file_hash(path)
        return path.read_text()

    def record_symlink(self, dst_path: str, target_path: str | Path) -> None:
        """Record a new symlink entry.

        Note that the entry always generates a relative symlink target
        when writing the entry to the workspace in write(), even if
        target_path is absolute.

        Args:
           dst_path: symlink path, relative to workspace root.
           target_path: symlink target path.
        """
        self._check_new_path(dst_path)
        self._files[dst_path] = {
            "type": "symlink",
            "target": str(target_path),
        }

    def record_raw_symlink(
        self, dst_path: str, target_path: str | Path
    ) -> None:
        """Record a new symlink entry.

        Note that the entry always generates a relative symlink target
        when writing the entry to the workspace in write(), even if
        target_path is absolute.

        Args:
           dst_path: symlink path, relative to workspace root.
           target_path: symlink target path.
        """
        self._check_new_path(dst_path)
        self._files[dst_path] = {
            "type": "raw_symlink",
            "target": str(target_path),
        }

    def record_file_content(
        self, dst_path: str, content: str, executable: bool = False
    ) -> None:
        """Record a new data file entry.

        Args:
            dst_path: file path, relative to workspace root.
            content: file content as a string.
            executable: optional flag, set to True to indicate this is an
               executable file (i.e. a script).
        """
        self._check_new_path(dst_path)
        entry: dict[str, T.Any] = {
            "type": "file",
            "content": content,
        }
        if executable:
            entry["executable"] = True
        self._files[dst_path] = entry

    def record_input_file_hash(self, input_path: str | Path) -> None:
        """Record the content hash of an input file.

        If set_file_hash_callback() was called, compute the content hash of
        a given input file path and record it. Note that nothing will be written
        to the workspace. This is only useful when comparing the output of
        to_json() between different generations to detect when the input file
        has changed.
        """
        input_file = str(input_path)
        self._check_new_path(input_file)
        if self._file_hasher:
            self._files[input_file] = {
                "type": "input_file",
                "hash": self._file_hasher(input_path),
            }

    def to_json(self) -> str:
        """Convert recorded entries to JSON string."""
        return json.dumps(self._files, indent=2, sort_keys=True)

    def write(self, out_dir: str | Path) -> None:
        """Write all recorded entries to a workspace directory."""
        for path, entry in self._files.items():
            type = entry["type"]
            if type == "symlink":
                target_path = entry["target"]
                link_path = os.path.join(out_dir, path)
                build_utils.force_symlink(link_path, target_path)
            elif type == "raw_symlink":
                target_path = entry["target"]
                link_path = os.path.join(out_dir, path)
                build_utils.force_raw_symlink(link_path, target_path)
            elif type == "file":
                file_path = os.path.join(out_dir, path)
                os.makedirs(os.path.dirname(file_path), exist_ok=True)
                with open(file_path, "w") as f:
                    f.write(entry["content"])
                if entry.get("executable", False):
                    os.chmod(file_path, 0o755)
            elif type == "input_file":
                # Nothing to do here.
                pass
            else:
                assert False, "Unknown entry type: " % entry["type"]

    def update_if_needed(self, out_dir: Path, manifest_path: Path) -> bool:
        """Write all recorded entries if they differ from the content of a given manifest.

        If the manifest exists and its content matches the recorded entries,
        then this method does not do anything. Otherwise, it will overwrite
        the manifests with new values, clean the output directory, and re-populate
        it entirely with new content.

        Args:
            out_dir: Output directory to update if needed.
            manifest_path: Path to manifest file to use for comparisons.
        Returns:
            True if the output directory and manifest were updated, False otherwise.
        """
        current_manifest = self.to_json()
        if out_dir.is_dir() and manifest_path.exists():
            if manifest_path.read_text() == current_manifest:
                # Nothing to change here.
                return False

        manifest_path.write_text(current_manifest)
        create_clean_dir(str(out_dir))
        self.write(out_dir)
        return True


def record_fuchsia_workspace(
    generated: GeneratedWorkspaceFiles,
    top_dir: Path,
    fuchsia_dir: Path,
    gn_output_dir: Path,
    git_bin_path: Path,
    log: T.Callable[[str], None] | None = None,
) -> None:
    """Record generated entries for the Fuchsia workspace and helper files.

    Note that this hards-code a few paths, i.e.:

      - The generated Bazel wrapper script is written to ${top_dir}/bazel
      - Bazel output base is at ${top_dir}/output_base
      - Bazel output user root is at ${top_dir}/output_user_root
      - The workspace goes into ${top_dir}/workspace/

    The log directory is now dynamically set per fx-build invocation and
    per bazel invocation in wrapper.bazel.sh.

    Args:
        generated: A GeneratedWorkspaceFiles instance modified by this function.
        top_dir: Path to the top-level
        fuchsia_dir: Path to the Fuchsia source checkout.
        gn_output_dir: Path to the GN/Ninja output directory.
        git_bin_path: Path to the host git binary to use during the build.
        log: Optional logging callback. If not None, must take a single
            string as argument.
    """

    host_os = build_utils.get_host_platform()
    host_cpu = build_utils.get_host_arch()
    host_tag = build_utils.get_host_tag()

    templates_dir = fuchsia_dir / "build" / "bazel" / "templates"

    ninja_binary = (
        fuchsia_dir / "prebuilt" / "third_party" / "ninja" / host_tag / "ninja"
    )
    bazel_bin = (
        fuchsia_dir / "prebuilt" / "third_party" / "bazel" / host_tag / "bazel"
    )
    python_prebuilt_dir = (
        fuchsia_dir / "prebuilt" / "third_party" / "python3" / host_tag
    )
    output_base_dir = top_dir / build_utils.BazelPaths.OUTPUT_BASE_FROM_TOP_DIR
    output_user_root = (
        top_dir / build_utils.BazelPaths.OUTPUT_USER_ROOT_FROM_TOP_DIR
    )
    workspace_dir = top_dir / build_utils.BazelPaths.WORKSPACE_FROM_TOP_DIR
    bazel_launcher = (
        top_dir / build_utils.BazelPaths.LAUNCHER_FROM_TOP_DIR
    ).resolve()

    if log:
        log(
            f"""- Using directories and files:
  Fuchsia:                {fuchsia_dir}
  GN build:               {gn_output_dir}
  Ninja binary:           {ninja_binary}
  Bazel source:           {bazel_bin}
  Top dir:                {top_dir}
  Logs directory:         <per-invocation: out/_build_logs/OUTDIR_BASENAME/build.$date.*/bazel_logs/>
  Bazel workspace:        {workspace_dir}
  Bazel output_base:      {output_base_dir}
  Bazel output user root: {output_user_root}
  Bazel launcher:         {bazel_launcher}
  Git binary path:        {git_bin_path}"""
        )

    def expand_template_file(
        generated: GeneratedWorkspaceFiles, filename: str, **kwargs: T.Any
    ) -> str:
        """Expand a template file and add it to the set of tracked input files."""
        template_file = templates_dir / filename
        return generated.read_text_file(template_file).format(**kwargs)

    def record_expanded_template(
        generated: GeneratedWorkspaceFiles,
        dst_path: str,
        template_name: str,
        **kwargs: T.Any,
    ) -> None:
        """Expand a template file and record its content.

        Args:
            generated: A GeneratedWorkspaceFiles instance.
            dst_path: Destination path for the recorded expanded content.
            template_name: Name of the template file to use.
            executable: Optional flag, set to True to indicate an executable file.
            **kwargs: Template expansion arguments.
        """
        executable = kwargs.get("executable", False)
        kwargs.pop("executable", None)
        content = expand_template_file(generated, template_name, **kwargs)
        generated.record_file_content(dst_path, content, executable=executable)

    generated.record_symlink(
        "workspace/MODULE.bazel",
        fuchsia_dir / "build" / "bazel" / "toplevel.MODULE.bazel",
    )

    generated.record_symlink(
        "workspace/BUILD.bazel",
        fuchsia_dir / "build" / "bazel" / "toplevel.BUILD.bazel",
    )

    # Generate top-level symlinks
    for name in os.listdir(fuchsia_dir):
        if not workspace_should_exclude_file(name):
            generated.record_symlink(f"workspace/{name}", fuchsia_dir / name)

    # Generate a platform mapping file to ensure that using --platforms=<value>
    # also sets --cpu properly, as required by the Bazel SDK rules. See comments
    # in template file for more details.
    _BAZEL_CPU_MAP = {"x64": "k8", "arm64": "aarch64"}
    host_os = build_utils.get_host_platform()
    host_cpu = build_utils.get_host_arch()
    host_tag = build_utils.get_host_tag()

    record_expanded_template(
        generated,
        "workspace/platform_mappings",
        "template.platform_mappings",
        bazel_host_cpu=_BAZEL_CPU_MAP.get(host_cpu, host_cpu),
        host_cpu=host_cpu,
        host_os=host_os,
    )

    # Generate the content of .bazelrc
    bazelrc_content = expand_template_file(
        generated,
        "template.bazelrc",
    )

    bazelrc_generator = BazelrcFromGnConfigGenerator()
    bazelrc_content += "\n" + bazelrc_generator.generate_bazelrc(gn_output_dir)
    generated.record_file_content("workspace/.bazelrc", bazelrc_content)

    # Copy the wrapper script to topdir/bazel, and generate the configuration
    # file at topdir/bazel.sh.config that is required to run it.
    # Generate wrapper script in topdir/bazel that invokes Bazel with the right --output_base.

    generated.record_symlink(
        "bazel", os.path.join(fuchsia_dir, "build/bazel/wrapper.bazel.sh")
    )

    def config_path_value(p: str | Path) -> str:
        return shlex.quote(os.path.abspath(p))

    record_expanded_template(
        generated,
        # LINT.IfChange(bazel.sh.config)
        "bazel.sh.config",
        # LINT.ThenChange(//build/bazel/wrapper.bazel.sh)
        "template.bazel.sh.config",
        bazel_bin=config_path_value(bazel_bin),
        bazel_output_base=config_path_value(output_base_dir),
        bazel_output_user_root=config_path_value(output_user_root),
        bazel_workspace_dir=config_path_value(workspace_dir),
        ninja_build_dir=config_path_value(gn_output_dir),
        prebuilt_ninja=config_path_value(ninja_binary),
        prebuilt_python_dir=config_path_value(python_prebuilt_dir),
    )

    generated.record_symlink(
        os.path.join(
            "workspace",
            "fuchsia_build_generated",
            "assembly_developer_overrides.json",
        ),
        os.path.join(gn_output_dir, "gen", "assembly_developer_overrides.json"),
    )

    generated.record_symlink(
        os.path.join(
            "workspace",
            "fuchsia_build_generated",
            "icu_build_config.json",
        ),
        os.path.join(gn_output_dir, "icu_build_config.json"),
    )

    # LINT.IfChange
    generated.record_symlink(
        "workspace/fuchsia_build_generated/args.json",
        gn_output_dir / "args.json",
    )
    # LINT.ThenChange(//build/bazel_sdk/bazel_rules_fuchsia/common/fuchsia_platform_build.bzl)

    # Create a symlink to the git host executable to make it accessible
    # when running a Bazel action on bots where it is not installed in
    # a standard location.
    generated.record_symlink(
        "workspace/fuchsia_build_generated/git",
        git_bin_path,
    )

    # LINT.IfChange
    # .jiri_root/ is not exposed to the workspace, but //build/info/BUILD.bazel
    # needs to access .jiri_root/update_history/latest so create a symlink just
    # for this file.
    generated.record_symlink(
        "workspace/fuchsia_build_generated/jiri_snapshot.xml",
        fuchsia_dir / ".jiri_root" / "update_history" / "latest",
    )
    # LINT.ThenChange(//build/info/info.gni)

    generated.record_symlink(
        "workspace/fuchsia_build_generated/content_hashes",
        gn_output_dir / "regenerator_outputs" / "bazel_content_hashes",
    )

    # Used by @fuchsia_platform_sysroot
    generated.record_symlink(
        "workspace/fuchsia_build_generated/fuchsia_platform_sysroot",
        gn_output_dir / "regenerator_outputs" / "fuchsia_platform_sysroot",
    )

    # Used when merging the IDK sub-build directories. For other use cases,
    # prefer a symlink with a narrower scope.
    generated.record_symlink(
        # LINT.IfChange
        "workspace/fuchsia_build_generated/ninja_root_build_dir",
        # LINT.ThenChange(//build/bazel/bazel_sdk/BUILD.bazel)
        gn_output_dir,
    )

    generated.record_symlink(
        # LINT.IfChange
        "workspace/fuchsia_build_generated/fuchsia_in_tree_idk.hash",
        # LINT.ThenChange(//build/bazel/bazel_sdk/BUILD.bazel, //build/bazel/toplevel.MODULE.bazel)
        # LINT.IfChange
        gn_output_dir / "sdk/prebuild/in_tree_collection.json",
        # LINT.ThenChange(//build/regenerator.py)
    )

    generated.record_symlink(
        # LINT.IfChange
        "workspace/fuchsia_build_generated/fuchsia_internal_only_idk.hash",
        # LINT.ThenChange(//build/bazel/toplevel.WORKSPACE.bzlmod)
        # LINT.IfChange
        gn_output_dir / "obj/build/bazel/fuchsia_internal_only_idk.hash",
        # LINT.ThenChange(//build/bazel/BUILD.gn)
    )

    # The following symlinks are used only by bazel_gn_target_action.py when processing
    # the list of Bazel source inputs, the actual repository setup in
    # MODULE.bazel reuses the two symlinks above instead.
    generated.record_symlink(
        # LINT.IfChange
        "workspace/fuchsia_build_generated/fuchsia_sdk.hash",
        # LINT.ThenChange(//build/bazel/scripts/bazel_gn_target_action.py)
        # LINT.IfChange
        gn_output_dir / "sdk/prebuild/in_tree_collection.json",
        # LINT.ThenChange(//build/regenerator.py)
    )

    generated.record_symlink(
        # LINT.IfChange
        "workspace/fuchsia_build_generated/internal_sdk.hash",
        # LINT.ThenChange(//build/bazel/scripts/bazel_gn_target_action.py)
        # LINT.IfChange
        gn_output_dir / "obj/build/bazel/fuchsia_internal_only_idk.hash",
        # LINT.ThenChange(//build/bazel/BUILD.gn)
    )

    # Create a link to an empty repository. This is updated by bazel_gn_target_action.py
    # before each Bazel invocation to point to the @gn_targets content specific
    # to its parent bazel_action() target.
    generated.record_symlink(
        f"workspace/{GN_TARGETS_DIR_SYMLINK}",
        fuchsia_dir
        / "build"
        / "bazel"
        / "local_repositories"
        / "empty_gn_targets",
    )

    generated.record_symlink(
        # LINT.IfChange
        "workspace/fuchsia_build_generated/fuchsia_build_info",
        # LINT.ThenChange(//build/bazel/toplevel.MODULE.bazel)
        # LINT.IfChange
        gn_output_dir / "regenerator_outputs/fuchsia_build_info",
        # LINT.ThenChange(//build/regenerator.py)
    )

    generated.record_symlink(
        # LINT.IfChange
        "workspace/fuchsia_build_generated/fuchsia_in_tree_idk",
        # LINT.ThenChange(//build/bazel/toplevel.MODULE.bazel)
        # LINT.IfChange
        gn_output_dir / "regenerator_outputs/fuchsia_in_tree_idk",
        # LINT.ThenChange(//build/regenerator.py)
    )


def generate_fuchsia_workspace(
    fuchsia_dir: Path,
    build_dir: Path,
    log: T.Callable[[str], None] | None = None,
) -> set[Path]:
    """Generate the Fuchsia Bazel workspace and associated files.

    Args:
        fuchsia_dir: Path to the Fuchsia source checkout.
        build_dir: Path to the GN/Ninja output directory.
        log: Optional logging callback. If not None, must take a single
            string as argument.

    Returns:
       A set of input file paths that were read by this function.
    """
    # Find path of host git tool
    git_bin_path = build_utils.find_host_binary_path("git")
    assert git_bin_path, f"Could not find 'git' in current PATH!"

    # # Extract target_cpu from args.json. This file is GN-generated
    # # and doesn't need to be added to input_files.
    # args_json_path = build_dir / "args.json"
    # assert args_json_path.exists(), f"Missing GN output file: {args_json_path}"
    # with args_json_path.open("rb") as f:
    #     args_json = json.load(f)
    #
    # target_cpu = args_json.get("target_cpu")
    # assert target_cpu, f"Missing target_cpu key in {args_json_path}"

    # Find the location of the Bazel top-dir relative to the Ninja
    # build directory.
    bazel_top_dir, input_files = build_utils.get_bazel_relative_topdir(
        fuchsia_dir
    )

    # Generate the bazel launcher and Bazel workspace files.
    generated = GeneratedWorkspaceFiles()

    top_dir = build_dir / bazel_top_dir

    record_fuchsia_workspace(
        generated,
        top_dir=top_dir,
        fuchsia_dir=fuchsia_dir,
        gn_output_dir=build_dir,
        git_bin_path=git_bin_path,
        log=log,
    )

    # Remove the old workspace's content, which is just a set
    # of symlinks and some auto-generated files.
    create_clean_dir(str(top_dir / "workspace"))

    # Do not clean the output_base because it is now massive,
    # and doing this will very slow and will force a lot of
    # unnecessary rebuilds after that,
    #
    # However, do remove $OUTPUT_BASE/external/ which holds
    # the content of external repositories.
    #
    # This is a guard against repository rules that do not
    # list their input dependencies properly, and would fail
    # to re-run if one of them is updated. Sadly, this is
    # pretty common with Bazel.
    create_clean_dir(os.path.join(top_dir / "output_base" / "external"))

    generated.write(top_dir)

    return input_files | generated.input_files


class GnBuildArgs(object):
    """A class to handle `gn_build_variables_for_bazel.json` input files.

    These files are used to export the values of GN args to an `args.bzl` file
    and optionally `vendor_<name>_args.bzl` files in the @fuchsia_build_info
    Bazel repository.

    See the comments for //build/bazel:gn_build_variables_for_bazel for the
    format of the `gn_build_variables_for_bazel.json` files.

    Vendor-specific files should always be placed at
        $OUT_DIR/vendor_<name>_gn_build_variables_for_bazel.json
    and the target that generates them must always be in the dependency graph
    when building anything from the vendor repo.
    """

    @staticmethod
    def find_all_gn_build_variables_for_bazel(
        fuchsia_dir: Path, build_dir: Path
    ) -> list[str]:
        """Find `all gn_build_variables_for_bazel.json` files in the Fuchsia checkout.

        Args:
            fuchsia_dir: Path to Fuchsia source dir.
            build_dir: Path to Fuchsia build directory.

        Returns:
            A list of `gn_build_variables_for_bazel.json` file paths relative to
            the build directory root.

        Raises:
            ValueError if a required input file is missing.
        """
        args_files: list[str] = []

        base_file_name = "gn_build_variables_for_bazel.json"

        main_file_path = build_dir / base_file_name
        if not (main_file_path).exists():
            raise ValueError(
                f"Missing required build arguments file: {main_file_path}"
            )
        args_files.append(base_file_name)

        # The //vendor/ tree is optional and does not exist in open-source checkouts.
        vendor_dir = fuchsia_dir / "vendor"
        if vendor_dir.is_dir():
            for vendor_name in os.listdir(vendor_dir):
                vendor_file = f"vendor_{vendor_name}_{base_file_name}"
                if (build_dir / vendor_file).exists():
                    args_files.append(vendor_file)

        return args_files

    @staticmethod
    def generate_args_bzl(
        gn_args_to_export: list[dict[str, T.Any]], args_json_path: Path
    ) -> str:
        """Generate an `args.bzl` file defining values extracted from GN's args.

        Args:
            gn_args_to_export: A list of dictionaries describing each GN arg.
                See comments for //build/bazel:gn_build_variables_for_bazel for
                the format.
            args_json_path: Path of source file for build_args (never accessed).
        Returns:
            The content of the generated `args.bzl` file as a string.
        Raises:
            ValueError if the input content is malformed.
        """

        def fail(msg: str) -> None:
            raise ValueError(msg)

        args_contents = """# AUTO-GENERATED BY FUCHSIA BUILD - DO NOT EDIT
# Variables listed from {source_path}

\"\"\"A subset of GN args that are needed in the Bazel build.\"\"\"
""".format(
            source_path=args_json_path
        )

        for gn_arg in gn_args_to_export:
            args_contents += "\n# From {}\n".format(gn_arg["location"])
            varname = gn_arg["name"]
            vartype = gn_arg["type"]
            value = gn_arg["value"]
            if vartype in ["bool", "array_of_strings"]:
                args_contents += "{} = {}".format(varname, value)
            elif vartype in ["string", "string_or_false", "path"]:
                if vartype == "string_or_false" and not value:
                    value = ""
                elif vartype == "path":
                    if value.startswith("//"):
                        value = value[2:]
                    elif value.startswith("/"):
                        # Pass through the absolute path.
                        value = value
                    else:
                        fail(
                            "Path '{}' does not begin with '//' or '/'".format(
                                value,
                            )
                        )

                args_contents += '{} = "{}"'.format(varname, value)
            else:
                fail(
                    "Unknown type name '{}'".format(
                        vartype,
                    )
                )
            args_contents += "\n"

        return args_contents

    @staticmethod
    def record_fuchsia_build_info_dir(
        fuchsia_dir: Path,
        build_dir: Path,
        generated: GeneratedWorkspaceFiles,
    ) -> None:
        """Record the content of @fuchsia_build_info in a GeneratedWorkspaceFiles instance.

        Args:
            fuchsia_dir: Path to Fuchsia source directory.
            build_dir: Path to Fuchsia build directory.
            generated: A GeneratedWorkspaceFiles instance. Its record_file_content()
                method will be called to populate the repository.
        """
        generated.record_file_content(
            "MODULE.bazel",
            """\
module(name = "fuchsia_build_info", version = "1")

bazel_dep(name = "platforms", version = "1.0.0")
bazel_dep(name = "fuchsia_rules_common", version = "0.0")
""",
        )

        generated.record_file_content("BUILD.bazel", "")

        # Generate BUILD.bazel which contains the definitions of toolchains
        # for default build_flags().

        # TODO(digit): Generate this to reflect GN build configuration.
        default_flags_map = DefaultBuildFlagsMap.new_from_gn_config(build_dir)
        generated.record_file_content(
            "default_build_flags/BUILD.bazel",
            default_flags_map.generate_bazel_toolchain_definitions(),
        )

        # Generate the args.bzl and vendor_{name}_args.bzl files
        args_files_relative_paths = (
            GnBuildArgs.find_all_gn_build_variables_for_bazel(
                fuchsia_dir, build_dir
            )
        )

        for relative_path in sorted(args_files_relative_paths):
            file_path = build_dir / relative_path
            with (file_path).open("rb") as f:
                gn_args_json = json.load(f)
            args_bzl_content = GnBuildArgs.generate_args_bzl(
                gn_args_json, file_path
            )

            if relative_path.startswith("vendor_"):
                vendor_name = relative_path.split("_")[1]
                args_bzl_filename = f"vendor_{vendor_name}_args.bzl"
            else:
                args_bzl_filename = "args.bzl"
            generated.record_file_content(args_bzl_filename, args_bzl_content)

    @staticmethod
    def generate_fuchsia_build_info(
        fuchsia_dir: Path, build_dir: Path, repository_dir: Path
    ) -> None:
        generated = GeneratedWorkspaceFiles()
        GnBuildArgs.record_fuchsia_build_info_dir(
            fuchsia_dir, build_dir, generated
        )

        generated.update_if_needed(
            repository_dir, Path(f"{repository_dir}.generated-info.json")
        )


@dataclasses.dataclass
class GnTargetsDirectoryManifestEntry(object):
    """A single entry in the manifest describing @gn_targets,
    corresponding to a single GN bazel_input_directory() target.

    All paths are relative to the Ninja build directory.

    Fields:
       bazel_package: Bazel package name (without leading // prefix).
       bazel_name: Bazel target name.
       generator_label: GN target label that generates the file.

       license_spdx_file: Path to an SPDX JSON file describing the license for all
           outputs of the generator target. This file is generated by the
           generated_licenses_spdx() GN template.

       output_files: List of Ninja build artifacts.
           Only non-empty for bazel_input_file() GN targets.

       output_directory: Path to the directory containing Ninja output files.
           Only non-empty for bazel_input_directory() GN targets.
    """

    # LINT.IfChange(GnTargetsDirectoryManifestEntry)

    bazel_package: str
    bazel_name: str
    generator_label: str
    license_spdx_file: str
    output_files: list[str] = dataclasses.field(default_factory=list)
    output_directory: str = ""

    # LINT.ThenChange(//build/bazel/bazel_inputs.gni:GnTargetsDirectoryManifestEntry)

    @staticmethod
    def from_json_value(json_data: T.Any) -> "GnTargetsDirectoryManifestEntry":
        """Create new instance from JSON value."""
        assert isinstance(json_data, dict)
        assert "bazel_package" in json_data
        assert "bazel_name" in json_data
        assert "generator_label" in json_data
        assert "license_spdx_file" in json_data
        assert ("output_files" in json_data) != (
            "output_directory" in json_data
        ), "One, and only one, of output_files or output_directory must be defined"
        return GnTargetsDirectoryManifestEntry(
            bazel_package=json_data["bazel_package"],
            bazel_name=json_data["bazel_name"],
            generator_label=json_data["generator_label"],
            license_spdx_file=json_data["license_spdx_file"],
            output_files=json_data.get("output_files", []),
            output_directory=json_data.get("output_directory", ""),
        )


@dataclasses.dataclass
class GnTargetsDirectoryManifest(object):
    """The manifest describing @gn_targets, corresponding to a single GN bazel_action() target."""

    entries: list[GnTargetsDirectoryManifestEntry]

    @staticmethod
    def from_json_value(json_data: T.Any) -> "GnTargetsDirectoryManifest":
        """Create new instance from JSON value."""
        assert isinstance(json_data, list)
        return GnTargetsDirectoryManifest(
            entries=[
                GnTargetsDirectoryManifestEntry.from_json_value(entry)
                for entry in json_data
            ],
        )


class BazelTargetGnInputsEntriesMap(dict[str, GnTargetsDirectoryManifestEntry]):
    """Maps Bazel target names to a GnTargetsDirectoryManifestEntry instance."""


class BazelPackageAndTargetToGnInputsEntriesMap(
    dict[str, BazelTargetGnInputsEntriesMap]
):
    """Maps Bazel package names to a BazelTargetGnInputsEntriesMap instance."""


def record_gn_targets_dir_from_entries(
    generated: GeneratedWorkspaceFiles,
    build_dir: Path,
    manifest_entries_package_map: BazelPackageAndTargetToGnInputsEntriesMap,
    all_licenses_spdx_path: Path,
) -> None:
    # Ensure build_dir is absolute. Most symlink targets must be absolute for Bazel
    # to work properly.
    build_dir = build_dir.resolve()

    if not all_licenses_spdx_path.exists():
        raise ValueError(
            f"Missing licensing information file: {all_licenses_spdx_path}"
        )

    # This creates two sets of symlinks.
    #
    # The first one maps `_files/{ninja_path}` to the absolute path of the corresponding
    # Ninja artifact in the build directory, e.g.:
    #
    #  _files/obj/src/foo/foo.cc.o ----> $NINJA_BUILD_DIR/obj/src/foo/foo.cc
    #
    # There is one such symlink per Ninja output paths.
    #
    # Second, for each bazel_package value, `{bazel_package}/_files` will be a relative
    # symlink that points to the top-level `_files` directory, as in:
    #
    #  src/foo/_files ---> ../../_files
    #
    # This is used by the BUILD.bazel file generated in the same sub-directory, that can
    # reference the artifacts using labels like "_files/{ninja_path}" without having
    # to care for Bazel package boundaries, as in:
    #
    #  ```
    #  # Generated as src/foo/BUILD.bazel
    #  filegroup(
    #     name = "foo",
    #     srcs = [ "_files/obj/src/foo/foo.cc.o" ]
    #  )
    #  ```
    #
    # The reason why this double indirection exists is purely for debuggability!
    # It is easier to see all the Ninja artifacts exposed at once from the top-level
    # _files/ directory when verifying correctness.
    #
    # It is perfectly possible to only place absollute symlinks under
    # {bazel_package}/_files/... but doing this leads to repositories that are
    # harder to inspect in practice due to the extra long paths it creates.

    # The top-level directory that will contain symlinks to all Ninja output
    # files, using . For example _files/obj/src/foo/foo.cc.o
    files_subdir = "_files"

    def record_file_symlink(file: str) -> str:
        """Record a symlink to a file in the files directory.

        Args:
            file: A file path, relative to the Ninja build directory.
        Returns:
            The symlink's path, relative to the generated output directory.
        """
        # Create //_files/{ninja_path} as a symlink to the Ninja output location.
        target_file = build_dir / file
        if not target_file.exists():
            # If the target does not exist, create an empty file with a timestamp of 0
            # This ensures Bazel queries will not error, and that the next Ninja
            # invocation will regenerate the file.
            target_file.parent.mkdir(parents=True, exist_ok=True)
            target_file.write_text("")
            os.utime(target_file, ns=(0, 0))

        link_file = f"{files_subdir}/{file}"
        generated.record_raw_symlink(
            link_file,
            target_file,
        )
        return link_file

    all_license_files: list[str] = []

    # Create the ///{gn_dir}/BUILD.bazel file for each GN directory.
    # Every target defined in {gn_dir}/BUILD.gn that is part of the manifest
    # will have its own filegroup() entry with the corresponding target name.
    for bazel_package, name_map in manifest_entries_package_map.items():
        content = dedent(
            """\
            # AUTO-GENERATED - DO NOT EDIT

            load("@rules_license//rules:license.bzl", "license")

            package(default_visibility = ["//visibility:public"])

            """
        )

        for bazel_name, entry in name_map.items():
            # Create the license() target for this entry, pointing to the SPDX
            # file that will be generated by Ninja.
            license_symlink = record_file_symlink(entry.license_spdx_file)
            license_target_name = f"{bazel_name}.license"
            content += dedent(
                """\
                # From GN target: {label}
                license(
                    name = "{license_target_name}",
                    package_name = "Legacy Ninja Build Outputs",
                    license_text = "{license_symlink}",
                )
                """
            ).format(
                label=entry.generator_label,
                license_target_name=license_target_name,
                license_symlink=license_symlink,
            )
            all_license_files.append(license_symlink)

            file_links = entry.output_files
            if file_links:
                for file in file_links:
                    record_file_symlink(file)

                content += dedent(
                    """\
                    filegroup(
                        name = "{name}",
                        applicable_licenses = [":{license_target_name}"],
                        srcs = """
                ).format(
                    name=bazel_name,
                    license_target_name=license_target_name,
                )
                if len(file_links) == 1:
                    content += '["_files/%s"],\n' % file_links[0]
                else:
                    content += "[\n"
                    for file in file_links:
                        content += '        "_files/%s",\n' % file
                    content += "    ],\n"
                content += ")\n"

            dir_link = entry.output_directory
            if dir_link:
                # Create //_files/{ninja_path} as a symlink to the real path.
                target_dir = build_dir / dir_link
                generated.record_raw_symlink(
                    f"{files_subdir}/{dir_link}", target_dir
                )

                # NOTE: allow_empty is necessary when performing queries
                # before building anything. The glob() might return an empty
                # list in this case since there is nothing built from Ninja yet.
                content += dedent(
                    """\
                    filegroup(
                        name = "{name}",
                        applicable_licenses = [":{license_target_name}"],
                        srcs = glob(["{ninja_path}/**"], exclude_directories=1, allow_empty=True),
                    )
                    alias(
                        name = "{name}.directory",
                        actual = "{ninja_path}",
                    )
                    """
                ).format(
                    name=bazel_name,
                    license_target_name=license_target_name,
                    ninja_path=f"{files_subdir}/{dir_link}",
                )

        generated.record_file_content(f"{bazel_package}/BUILD.bazel", content)

        # Because {bazel_package}/BUILD.bazel contains label references
        #
        # such as "_files/obj/src/microfuchsia/tee/ta/noop/ta-noop.far", create
        # {bazel_package}/_files as a symlink to the top-level _files directory.
        #
        # A relative path target is required, as the final output directory path is
        # not known yet. This much walk back the bazel_package path fragments.
        generated.record_raw_symlink(
            f"{bazel_package}/{files_subdir}",
            ("../" * len(bazel_package.split("/"))) + files_subdir,
        )

    # The symlink for the special all_licenses_spdx.json file.
    # IMPORTANT: This must end in `.spdx.json` for license classification to work correctly!
    # LINT.IfChange(all_licenses_spdx_path)
    generated.record_symlink(
        "all_licenses.spdx.json", all_licenses_spdx_path.resolve()
    )
    # LINT.ThenChange(//build/bazel/bazel_action_utils.py:all_licenses_spdx_path)

    # LINT.IfChange(all_license_files)
    # The list of all license file paths. This is later used to check that they were
    # properly updated by Ninja
    generated.record_file_content(
        "all_license_files.txt", "\n".join(sorted(all_license_files))
    )
    # LINT.ThenChange(//build/bazel/scripts/bazel_action_utils.py:all_license_files)

    # The content of BUILD.bazel
    build_content = dedent(
        """\
        # AUTO-GENERATED - DO NOT EDIT
        load("@rules_license//rules:license.bzl", "license")

        # This contains information about all the licenses of all
        # Ninja outputs exposed in this repository.
        # IMPORTANT: package_name *must* be "Legacy Ninja Build Outputs"
        # as several license pipeline exception files hard-code this under //vendor/...
        license(
            name = "all_licenses_spdx_json",
            package_name = "Legacy Ninja Build Outputs",
            license_text = "all_licenses.spdx.json",
            visibility = ["//visibility:public"]
        )
        """
    )

    generated.record_file_content("BUILD.bazel", build_content)
    generated.record_file_content(
        "MODULE.bazel",
        dedent(
            """\
            # AUTO-GENERATED - DO NOT EDIT

            module(name = "gn_targets", version = "1")

            bazel_dep(name = "rules_license", version = "1.0.0")"""
        ),
    )


class BazelrcFromGnConfigGenerator(object):
    """Generate a .bazelrc file fragment that contains --config=<name> definitions."""

    _DEFAULT_PLATFORMS = (
        "host",
        "linux_x64",
        "linux_arm64",
        "fuchsia",
        "fuchsia_sdk",
        "fuchsia_sdk_x64",
        "fuchsia_sdk_arm64",
        "fuchsia_sdk_riscv64",
        "fuchsia_platform",
        "fuchsia_platform_x64",
        "fuchsia_platform_arm64",
        "fuchsia_platform_riscv64",
    )

    def __init__(
        self, platform_names: T.Iterable[str] = _DEFAULT_PLATFORMS
    ) -> None:
        self._platform_names = platform_names

    def generate_bazelrc(self, build_dir: Path) -> str:
        """Generate a .bazelrc file that contains platform-specific definitions.

        This takes as input the GN-generated files from //build/bazel:bazel_args_json
        and produces a .bazelrc file that contains definitions for the following:

        --config=host: Matching the current host platform.

        --config=fuchsia_sdk_x64, --config=fuchsia_sdk_arm64, --config=fuchsia_sdk_riscv64
        --config=fuchsia_platform_x64, --config=fuchsia_platform_arm64, --config=fuchsia_platform_riscv64
            Matching different Fuchsia platforms.

        Args:
            build_dir: The Ninja build directory.

        Returns:
            The content of the generated .bazelrc file fragment.
        """
        # For each config_name value, create a .bazelrc fragment that will
        # define Bazel flags to use by default with --config=${config_name}
        output = (
            "# Auto-generated lists of --config=<name> settings, do not edit!\n"
        )

        def to_string(l: T.Any) -> str:
            assert isinstance(
                l, list
            ), f"Expected list of strings, got {type(l)} instead!"
            return " ".join(l)

        config_names = ("host", "fuchsia")
        config_values: dict[str, dict[str, str]] = {}
        for config_name in config_names:
            config_file = build_dir / f"bazel_args/{config_name}.json"
            assert (
                config_file.exists()
            ), f"Missing GN-generated input file: {config_file}"
            with config_file.open() as f:
                config = json.load(f)

            common_args = to_string(config["common"])
            build_args = to_string(config["build"])
            output += f"common:{config_name}_config_args {common_args}\n"
            output += f"build:{config_name}_config_args {build_args}\n\n"
            config_values[config_name] = config

        for platform in self._platform_names:
            config_args_name = (
                "fuchsia_config_args"
                if platform.startswith("fuchsia")
                else "host_config_args"
            )

            if platform in ("fuchsia_sdk", "fuchsia_platform", "fuchsia"):
                # As a special case, make "--config=fuchsia_sdk" and
                # "--config=fuchsia_platform" the same as
                # "--config=fuchsia_sdk_${target_cpu}" and
                # "--config=fuchsia_platform_${target_cpu}", respectively.
                platform_base = "fuchsia"
                values = config_values[platform_base]
                current_cpu = values["current_cpu"]
                platform_name = f"{platform}_{current_cpu}"

                # As a very special case, allow the legacy config name "fuchsia"
                # to be used as an alias for "fuchsia_sdk" until we have
                # migrated people off of it. Override `platform` since there are
                # no "fuchsia_$CPU" platforms.
                if platform == "fuchsia":
                    platform_name = f"fuchsia_sdk_{current_cpu}"
            else:
                platform_name = platform
            output += f"common:{platform} --config={config_args_name} --platforms=//build/bazel/platforms:{platform_name}\n"

        return output


def check_regenerator_inputs_updates(
    build_dir: Path,
    inputs_file: str = "regenerator_outputs/regenerator_inputs.txt",
) -> set[str]:
    """Check whether any regenerator input has changed.

    Args:
        build_dir: Ninja build directory path.
        inputs_file: Optional path string to the file listing all inputs,
           relative to build_dir. Default value is
           regenerator_outputs/regenerator_inputs.txt.

    Returns:
        A set of file paths, relative to build_dir, whose timestamp is
        newer than that of the inputs_file file itself.
    """
    inputs_path = build_dir / inputs_file
    if not inputs_path.exists():
        # If the file does not exist, a regeneration is required.
        return {inputs_file}

    inputs_timestamp = inputs_path.stat().st_mtime
    changed_inputs: set[str] = set()
    for dep in inputs_path.read_text().splitlines():
        dep_path = build_dir / dep
        if not dep_path.exists():
            changed_inputs.add(dep)
            continue

        dep_timestamp = dep_path.stat().st_mtime
        if dep_timestamp > inputs_timestamp:
            changed_inputs.add(dep)

    return changed_inputs


def _extract_variant_and_subpath(dest: str, prefix: str) -> tuple[str, str]:
    """Extracts (variant, subpath) from a destination path.

    E.g. "dist/lib/ld.so.1"      -> ("", "ld.so.1")
         "dist/lib/asan/ld.so.1" -> ("asan", "asan/ld.so.1")
    """
    subpath = dest.removeprefix(prefix)
    variant = subpath.split("/")[0] if "/" in subpath else ""
    return variant, subpath


def _generate_sysroot_dist_targets(
    dist_files: dict[str, tuple[str, str]],
    debug_files: dict[str, str],
    bazel_arch: str,
) -> list[str]:
    """Generates target declarations for sysroot dist files.

    Args:
        dist_files: A dict mapping from variant name to a tuple
            (stripped_file, subpath), where stripped_file is the path to the
            stripped file relative to the sysroot directory, and subpath is the
            path under `lib` to install the stripped file.
        debug_files: A dict mapping from variant name to the path to the
            unstripped file relative to the sysroot directory.
        bazel_arch: The target CPU architecture (in Bazel's cpu representation, e.g. "x86_64")

    Returns:
        A list of target declarations of fuchsia_unstripped_binary targets.
    """
    dist_targets: list[str] = []
    for variant in sorted(dist_files.keys()):
        if variant not in debug_files:
            continue
        target_name = "sysroot_library_dist" + (
            f".{variant}" if variant else ""
        )
        stripped_file, subpath = dist_files[variant]
        unstripped_file = debug_files[variant]
        dist_targets.append(
            dedent(
                f"""\
                fuchsia_unstripped_binary(
                    name = "{target_name}",
                    dest = "lib/{subpath}",
                    stripped_file = "{stripped_file}",
                    unstripped_file = "{unstripped_file}",
                    target_compatible_with = [
                        "@platforms//os:fuchsia",
                        "@platforms//cpu:{bazel_arch}",
                    ],
                    visibility = ["//visibility:public"],
                )"""
            )
        )
    return dist_targets


def generate_fuchsia_platform_sysroot_repository(
    repository_dir: Path,
    repository_name: str,
    sysroot_json: Path,
    gn_target_cpu: str,
    build_dir: Path | None = None,
) -> None:
    """Generate a Bazel repository that contains a sysroot for the Fuchsia platform artifacts.

    Args:
        repository_dir: Output directory for the generated Bazel repository.
        repository_name: Name of the generated repository.
        sysroot_json: Input sysroot JSON file, such as the one generated by
          //zircon/public/sysroot_sdk:sysroot_for_fuchsia_platform.
        gn_target_cpu: The GN target CPU architecture.
        build_dir: Optional path to the Ninja build directory. Defaults to the parent
          directory of --sysroot_json.
    """
    sysroot_json = sysroot_json.resolve()
    repository_dir = repository_dir.resolve()

    if build_dir is None:
        build_dir = sysroot_json.parent

    build_dir = build_dir.resolve()

    header_files: list[str] = []
    lib_files: list[str] = []
    # Paths to symlinked stripped files and debug files, relative to the repository root.
    # These files are later used as sources for fuchsia_unstripped_binary targets.
    dist_files: dict[str, tuple[str, str]] = {}
    debug_files: dict[str, str] = {}
    with sysroot_json.open("r") as f:
        sysroot_json_data = json.load(f)

    dest_prefix = "sysroot/"

    for entry in sysroot_json_data:
        source_path = build_dir / entry["source"]
        symlink_dest = entry["dest"]
        symlink_dest_path = dest_prefix + symlink_dest
        build_utils.force_symlink_to_ninja_artifact(
            repository_dir / symlink_dest_path, source_path
        )

        if symlink_dest.startswith("include/"):
            header_files.append(symlink_dest_path)
        elif symlink_dest.startswith("lib/"):
            lib_files.append(symlink_dest_path)
        elif symlink_dest.startswith("dist/lib/"):
            assert symlink_dest.endswith(
                "ld.so.1"
            ), f"Unexpected file in dist/lib/: {symlink_dest}"
            variant, subpath = _extract_variant_and_subpath(
                symlink_dest, "dist/lib/"
            )
            dist_files[variant] = (symlink_dest_path, subpath)
        elif symlink_dest.startswith("debug/"):
            assert symlink_dest.endswith(
                "libc.so"
            ), f"Unexpected file in debug/: {symlink_dest}"
            variant, _ = _extract_variant_and_subpath(symlink_dest, "debug/")
            debug_files[variant] = symlink_dest_path

    # As Bazel doesn't support labels pointing to directories, this empty file can
    # be referenced as @<repo_name>//:sysroot/empty instead.
    repository_dir.mkdir(parents=True, exist_ok=True)
    (repository_dir / "sysroot/empty").write_text("")

    dist_targets = _generate_sysroot_dist_targets(
        dist_files,
        debug_files,
        build_utils.gn_arch_to_bazel(gn_target_cpu),
    )

    fuchsia_dir = build_utils.find_fuchsia_dir(build_dir)
    # Symlink the root LICENSE file so the sysroot package can reference it.
    build_utils.force_symlink(
        repository_dir / "LICENSE", fuchsia_dir / "LICENSE"
    )
    module_content = dedent(
        f"""\
        module(name = "{repository_name}")

        bazel_dep(name = "fuchsia_rules_common", version = "")
        local_path_override(
            module_name = "fuchsia_rules_common",
            path = "{fuchsia_dir}/build/bazel_sdk/fuchsia_rules_common",
        )
        bazel_dep(name = "platforms", version = "1.1.0")
        bazel_dep(name = "rules_license", version = "1.0.0")
        """
    )
    load_block = dedent(
        """\
        load(
            "@fuchsia_rules_common//debug_symbols:debug_symbols.bzl",
            "fuchsia_unstripped_binary",
        )
        load("@rules_license//rules:license.bzl", "license")"""
    )
    dist_targets_str = "\n\n".join(dist_targets)

    (repository_dir / "MODULE.bazel").write_text(module_content)

    header_files_str = "".join(f'        "{f}",\n' for f in header_files)
    lib_files_str = "".join(f'        "{f}",\n' for f in lib_files)

    (repository_dir / "BUILD.bazel").write_text(
        f"""# AUTO-GENERATED - DO NOT EDIT

{load_block}

package(default_applicable_licenses = [":license"])

license(
    name = "license",
    license_kinds = ["@rules_license//licenses/spdx:BSD-2-Clause"],
    license_text = "LICENSE",
)

exports_files(["sysroot/empty"])

filegroup(
    name = "sysroot_header_files",
    srcs = [
{header_files_str}    ],
    visibility = ["//visibility:public"]
)

filegroup(
    name = "sysroot_library_files",
    srcs = [
{lib_files_str}    ],
    visibility = ["//visibility:public"]
)

{dist_targets_str}
"""
    )


def repository_name(label: str) -> str:
    """Returns repository name of the input label.

    Supports both canonical repository names (starts with @@) and apparent
    repository names (starts with @).
    """
    repository, sep, _ = label.partition("//")
    assert sep == "//", f"Missing // in label: {label}"
    return repository.removeprefix("@@").removeprefix("@")


def innermost_repository_name(label: str) -> str:
    """Returns the innermost repository names.

    * For top-level repositories, this is their canonical repository name.

    * For repos generated by extensions, this is the repo_name used their
      corresponding extensions' repo namespaces. Repo generated by extensions
      have the canonical names in the form of
      `module_repo_canonical_name+extension_name+repo_name`, this function
      returns `repo_name` from it.

      See https://bazel.build/external/extension#repository_names_and_visibility

      NOTE: It's clearly stated in the doc that this naming convention is NOT an
      API. However, in our use cases, we'd either do rely on this or hardcode
      canonical names for extension generated repos everywhere. And the latter
      is even less reliable.
    """
    canonical_repo_name = repository_name(label)
    if _BAZEL_REPO_NAME_SEPARATOR not in canonical_repo_name:
        return canonical_repo_name
    elements = canonical_repo_name.split(_BAZEL_REPO_NAME_SEPARATOR)
    if len(elements) < 3:
        # This is a repo_name+version canonical name, return as-is.
        return canonical_repo_name
    return elements[-1]

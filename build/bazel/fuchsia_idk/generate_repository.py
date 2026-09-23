#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Generate a Bazel repository for a given Fuchsia IDK.

This rewrite the metadata files to use Bazel target labels for
any file that is a Ninja artifact, instead of a source file,
e.g. "arch/x64/lib/libfoo.so" -> "@fuchsia_idk//arch/x64:lib/libfoo.so"

For now, these targets only map to filegroup() targets that wrap
a symlink to the actual Ninja artifact. Later, these will point to
actual Bazel actions to build the corresponding file on demand
directly in the Bazel graph.
"""

import argparse
import json
import os
import sys
import typing as T
from pathlib import Path

# Location of IDK top-level manifest listing all IDK atoms.
_IDK_MANIFEST_RELPATH = "meta/manifest.json"

# Glean the list of libc's generated headers from its checked-in JSON data.
_SYSROOT_GENERATED_HEADERS = sorted(
    [
        header["name"]
        for header in sum(
            (
                json.load(
                    (
                        Path(__file__).parents[3]
                        / "sdk"
                        / "lib"
                        / "c"
                        / "include"
                        / file
                    ).open()
                )
                for file in ["generated.json", "llvm-libc-generated.json"]
            ),
            [],
        )
    ]
)


def _write_file(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.is_symlink():
        path.unlink()
    path.write_text(content)


def _create_symlink(dst_path: Path, target_path: Path) -> None:
    """Create symlink at |dst_path| pointing to |target_path|."""
    dst_dir = dst_path.parent
    dst_dir.mkdir(parents=True, exist_ok=True)
    if not target_path.is_absolute():
        target_path = target_path.resolve()
    target_path = Path(os.path.relpath(target_path, dst_dir))
    if dst_path.is_symlink() or dst_path.exists():
        dst_path.unlink()
    dst_path.symlink_to(target_path)


def _to_starlark_string_list(items: list[str], indent: int = 4) -> str:
    if len(items) == 0:
        return "[]"
    if len(items) == 1:
        return f'["{items.pop()}"]'
    result = "[\n"
    for item in items:
        result += " " * indent + f'    "{item}",\n'
    result += " " * indent + "]"
    return result


def _to_starlark_string_dict(mapping: dict[str, str], indent: int = 4) -> str:
    if not mapping:
        return "{}"
    if len(mapping) == 1:
        key, value = mapping.popitem()
        return f'{{ "{key}": "{value}" }}'
    items = sorted(mapping.items())
    result = "{\n"
    for key, value in items:
        result += " " * indent + f'    "{key}": "{value}",\n'
    result += " " * indent + "}"
    return result


def split_path_to_package_name(path: str) -> tuple[str, str]:
    """Convert IDK-relative path to Bazel (package path, target/file name) pair."""
    # Use a single .build-id/BUILD.bazel file for all .build-id files.
    if path.startswith(".build-id/"):
        return (".build-id", path[len(".build-id/") :])

    # Use a single package per arch/{cpu}/ directory as well.
    # Special case arch/{cpu}/sysroot/ too.
    if path.startswith("arch/"):
        path_components = path.split("/")
        assert len(path_components) >= 3, f"Unexpected arch-related path {path}"
        if path_components[2] == "sysroot":
            package_name, rest = (
                f"arch/{path_components[1]}/sysroot",
                path_components[3:],
            )
        else:
            package_name, rest = (
                f"arch/{path_components[1]}",
                path_components[2:],
            )
        return (package_name, "/".join(rest))

    # Use a single packages/<name>/BUILD.bazel for prebuilt packages.
    # Note that this includes packages/blobs/BUILD.bazel.
    if path.startswith("packages/"):
        path_components = path.split("/")
        assert (
            len(path_components) > 2
        ), f"Unexpected package-related path: {path}"
        package_name, rest = path_components[1], path_components[2:]
        return (f"packages/{package_name}", "/".join(rest))

    # Otherwise, use <dirname>, <basename>, with <dirname> set to the
    # empty string instead of "." for the current directory.
    p = Path(path)
    package = str(p.parent)
    if package == ".":
        package = ""
    return (package, p.name)


class OutputPackageInfo(object):
    """Information about a given Bazel package in the output IDK directory."""

    def __init__(self) -> None:
        # The list of files exported by this package.
        self._exports: set[str] = set()
        # The list of filesgroups in this package.
        self._filegroups: dict[str, list[str]] = {}
        # The list of aliases in this package.
        self._aliases: dict[str, str] = {}
        self._target_names: set[str] = set()

    def add_export(self, name: str) -> None:
        assert (
            name not in self._target_names
        ), f"Cannot export known target name: {name}"
        self._exports.add(name)

    def add_filegroup(self, name: str, srcs: T.Sequence[str]) -> None:
        assert name not in self._target_names, f"Duplicate target name: {name}"
        self._target_names.add(name)
        self._filegroups[name] = list(srcs)

    def add_alias(self, name: str, actual: str) -> None:
        assert name not in self._target_names, f"Duplicate target name: {name}"
        self._target_names.add(name)
        self._aliases[name] = actual

    def generate_build_bazel(self) -> str:
        result = "# AUTO-GENERATED - DO NOT EDIT !\n\n"
        result += 'package(default_visibility = ["//visibility:public"])\n'

        if self._exports:
            result += """
# The following are direct symlinks that can point into the Ninja output directory or the Fuchsia source dir.
exports_files({exports})
""".format(
                exports=_to_starlark_string_list(
                    sorted(self._exports), indent=0
                )
            )

        for name, srcs in self._filegroups.items():
            srcs_list = _to_starlark_string_list(sorted(srcs))
            result += f"""
filegroup(
    name = "{name}",
    srcs = {srcs_list},
)
"""

        for name, actual in self._aliases.items():
            result += f"""
alias(
    name = "{name}",
    actual = "{actual}",
)
"""
        return result


class OutputIdk(object):
    """Information about a given output IDK directory."""

    def __init__(self, output_dir: Path):
        self._output_dir = output_dir
        self._packages: dict[str, OutputPackageInfo] = {}
        self._symlinks: dict[str, Path] = {}
        self._files: dict[str, str] = {}

        # A map from Bazel target labels to where to copy their
        # artifact when generating a final IDK directory.
        self._final_files: dict[str, str] = {}
        self._final_metas: dict[str, str] = {}

    def add_symlink(self, link_relpath: str, target_path: Path) -> None:
        cur_path = self._symlinks.setdefault(link_relpath, target_path)
        assert (
            cur_path == target_path
        ), f"Duplicate symlinks at {link_relpath}: [{cur_path}] vs [{target_path}]"
        assert (
            link_relpath not in self._files
        ), f"Cannot add symlink {link_relpath} over existing file"

    def add_file(
        self, relpath: str, content: str, is_meta: bool = False
    ) -> None:
        cur_content = self._files.setdefault(relpath, content)
        assert (
            cur_content == content
        ), f"Duplicate file content at {relpath}:\n<<<<<<\n{cur_content}\n=====\n{content}\n>>>>"
        assert (
            relpath not in self._symlinks
        ), f"Cannot add file {relpath} over existing symlink"
        self.add_final_file(relpath, is_meta)

    def add_json_file(
        self, relpath: str, json_content: T.Any, is_meta: bool = False
    ) -> None:
        self.add_file(
            relpath,
            json.dumps(json_content, indent=2, separators=(",", ": ")),
            is_meta=is_meta,
        )

    def get_package_info_for(self, package_path: str) -> OutputPackageInfo:
        result = self._packages.get(package_path)
        if result is None:
            result = self._packages.setdefault(
                package_path, OutputPackageInfo()
            )
        return result

    def add_final_file(self, dst_path: str, is_meta: bool = False) -> None:
        dst_path = str(
            dst_path
        )  # TODO(digit): Add assert, debug origin of Path value!
        package, name = split_path_to_package_name(dst_path)
        package_info = self.get_package_info_for(package)
        package_info.add_export(name)
        if is_meta:
            self._final_metas[f"//%s:%s" % (package, name)] = dst_path
        else:
            self._final_files[f"//%s:%s" % (package, name)] = dst_path

    def add_artifact_file(self, dst_path: str, target_label: str) -> None:
        self._final_files[target_label] = dst_path

    def write_all(self) -> None:
        # Ensure there is an OutputPackageInfo for the top-level package.
        self.get_package_info_for("")

        # Write all package BUILD.bazel files.
        for package_dir, package_info in self._packages.items():
            package_path = self._output_dir / package_dir / "BUILD.bazel"
            package_path.parent.mkdir(parents=True, exist_ok=True)
            package_path.write_text(package_info.generate_build_bazel())

        # Create symlinks.
        for link_relpath, target_path in self._symlinks.items():
            link_path = self._output_dir / link_relpath
            _create_symlink(link_path, target_path)

        # Write files.
        for file_relpath, content in self._files.items():
            file_path = self._output_dir / file_relpath
            file_path.parent.mkdir(parents=True, exist_ok=True)
            file_path.write_text(content)


class PathRewriter(object):
    """Rewrite paths belonging to a metadata JSON file."""

    def __init__(
        self,
        canonical_repo_name: str,
        output_idk: OutputIdk,
        input_dir: Path,
        ninja_build_dir: Path,
        exceptions: T.Sequence[str] = [],
    ):
        """Create new instance

        Args:
            repo_name: Repository name (e.g. "fuchsia_idk").
            output_idk: An OutputIdk instance.
            input_dir: Path to the input export IDK directory.
            ninja_build_dir: Path to the Ninja build directory.
            exceptions: optional list of file basenames that will
                be preserved as symlinks, even if they are generated
                Ninja artifacts.
        """
        self._canonical_repo_name = canonical_repo_name
        self._output_idk = output_idk
        self._input_dir = input_dir
        self._ninja_build_dir = ninja_build_dir
        self._exceptions = exceptions

    def clone_with_exceptions(
        self, exceptions: T.Sequence[str]
    ) -> "PathRewriter":
        """Create new instance with a different list of exceptions."""
        return PathRewriter(
            self._canonical_repo_name,
            self._output_idk,
            self._input_dir,
            self._ninja_build_dir,
            exceptions,
        )

    def path(self, path: str, relative_from: None | Path = None) -> str:
        """Rewrite a single path, potentially creating a symlink for it."""
        package, name = split_path_to_package_name(path)
        if relative_from:
            assert not path.startswith(
                ("/", "@")
            ), f"Expected relative file: {path}"
            src_path = (relative_from / path).resolve()
        else:
            src_path = (self._input_dir / path).resolve()

        # Create a symlink, and ensure the file is exported
        # by its Bazel package.
        self._output_idk.add_symlink(path, src_path)
        pkg_info = self._output_idk.get_package_info_for(package)
        pkg_info.add_export(name)

        if not src_path.is_relative_to(self._ninja_build_dir):
            # This is a source file, return the path unmodified.
            self._output_idk.add_final_file(path)
            return path
        elif any(path.endswith("/" + tail) for tail in self._exceptions):
            # This file must be kept as a direct symlink even
            # though it is generated by Ninja. This is currently
            # required for sysroot files because the C++ toolchain
            # configuration cannot currently support a generated
            # sysroot directory.
            self._output_idk.add_final_file(path)
            return path
        else:
            # This is a Ninja generated file, return a label to
            # its Bazel package.
            label = f"@@{self._canonical_repo_name}//{package}:{name}"
            self._output_idk.add_artifact_file(path, label)
            return label

    def path_list(
        self, paths: list[str], relative_from: None | Path = None
    ) -> list[str]:
        """Rewrite all paths in a list, return new list."""
        return [self.path(p, relative_from) for p in paths]

    def property_inplace(self, obj: dict[str, str], prop: str) -> None:
        """Rewrite obj[prop] as path in-place if possible."""
        value: str | None = obj.get(prop, None)
        if value is not None:
            obj[prop] = self.path(value)

    def properties_inplace(
        self, obj: dict[str, str], props: T.Sequence[str]
    ) -> None:
        """Applies property_inplace() for a sequence of property names."""
        for prop in props:
            self.property_inplace(obj, prop)

    def path_list_inplace(self, paths: list[str]) -> None:
        """Rewrite all paths in a list, modify in-place."""
        # Subtle: using `path_list[:] = new_value` changes the content
        # of path_list in-place, instead of assigning a new value to
        # the local variable by that name.
        paths[:] = self.path_list(paths)

    def rewrite_metadata(self, meta: T.Any, meta_dir: str) -> None:
        """Rewrite metadata JSON value in-place.

        Args:
            meta: The meta.json file as a JSON dict.
            meta_dir: The directory where the file lives in.
        """
        atom_type = meta.get("type")
        # version_history.json places the type under data.type
        if atom_type is None and "data" in meta:
            atom_type = meta["data"].get("type")

        if atom_type == "bind_library":
            self.path_list_inplace(meta["sources"])
            return

        if atom_type == "cc_prebuilt_library":
            self.path_list_inplace(meta["headers"])
            binaries = meta.get("binaries", {})
            for arch, binary_group in binaries.items():
                self.properties_inplace(binary_group, ("debug", "dist", "link"))
            for variant in meta.get("variants", []):
                values = variant["values"]
                self.properties_inplace(
                    values, ("debug", "dist_lib", "link_lib")
                )
            # Handle the symlink for the ifs file, which is listed by
            # name only, instead of an SDK-relative file path.
            # E.g. 'foo.ifs' instead of 'pkg/foo/foo.ifs'
            meta_ifs = meta.get("ifs")
            if meta_ifs:
                self.path(f"{meta_dir}/{meta_ifs}")
            return

        if atom_type == "cc_source_library":
            self.path_list_inplace(meta["headers"])
            self.path_list_inplace(meta["sources"])
            return

        if atom_type == "companion_host_tool":
            if "files" in meta:
                self.path_list_inplace(meta["files"])
            target_files = meta.get("target_files", {})
            for arch, filegroup in target_files.items():
                self.path_list_inplace(filegroup)
            return

        if atom_type == "dart_library":
            self.path_list_inplace(meta["sources"])
            return

        if atom_type in ("component_manifest", "config", "license"):
            self.path_list_inplace(meta["data"])
            return

        if atom_type == "documentation":
            self.path_list_inplace(meta["docs"])
            return

        if atom_type == "emu_manifest":
            self.path_list_inplace(meta["disk_images"])
            self.property_inplace(meta, "initial_ramdisk")
            self.property_inplace(meta, "kernel")
            return

        if atom_type == "experimental_python_e2e_test":
            self.path_list_inplace(meta["files"])
            return

        if atom_type == "ffx_tool":
            toolfiles = meta.get("files", {})
            self.property_inplace(toolfiles, "executable")
            self.property_inplace(toolfiles, "executable_metadata")
            for arch, toolfiles in meta["target_files"].items():
                self.property_inplace(toolfiles, "executable")
                self.property_inplace(toolfiles, "executable_metadata")
            return

        if atom_type == "fidl_library":
            self.path_list_inplace(meta["sources"])
            return

        if atom_type == "host_tool":
            self.path_list_inplace(meta.get("files", []))
            for arch, filegroup in meta.get("target_files", {}).items():
                self.path_list_inplace(filegroup)
            return

        if atom_type == "loadable_module":
            for arch, filegroup in meta["binaries"].items():
                self.path_list_inplace(filegroup)
            self.path_list_inplace(meta["resources"])
            return

        if atom_type == "package":
            for variant in meta["variants"]:
                self.property_inplace(variant, "manifest_file")
                self.path_list_inplace(variant["files"])
            return

        if atom_type == "sysroot":
            # The following files are generated by Ninja.
            # For now, keep them as direct symlinks until
            # the Bazel C++ toolchain definition can support
            # sysroot generated by Bazel actions.
            sysroot_rewriter = self.clone_with_exceptions(
                _SYSROOT_GENERATED_HEADERS
                + [
                    # Generated syscall cdecl source files.
                    # LINT.IfChange(sysroot_generated_cdecl_sources)
                    "cdecls.inc",
                    "cdecls-next.inc",
                    # LINT.ThenChange(//zircon/tools/zither/zither_library.gni)
                    # Prebuilt libraries.
                    "libc.so",
                    "libzircon.so",
                    "Scrt1.o",
                ]
            )
            for variant in meta.get("variants", []):
                values = variant["values"]
                sysroot_rewriter.path_list_inplace(values["debug_libs"])
                sysroot_rewriter.path_list_inplace(values["dist_libs"])
                sysroot_rewriter.path_list_inplace(values["headers"])
                sysroot_rewriter.path_list_inplace(values["link_libs"])

            for arch, version in meta.get("versions", {}).items():
                sysroot_rewriter.path_list_inplace(version["debug_libs"])
                sysroot_rewriter.path_list_inplace(version["dist_libs"])
                sysroot_rewriter.path_list_inplace(version["headers"])
                sysroot_rewriter.path_list_inplace(version["link_libs"])

            for ifs in meta.get("ifs_files", []):
                sysroot_rewriter.path(f"{meta_dir}/{ifs}")
            return

        if atom_type == "version_history":
            # No paths to rewrite here.
            return

        if atom_type == "virtual_device":
            # No paths to rewrite here.
            return

        raise Exception(f"Unknown atom type {atom_type} in {meta}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--repository-name", required=True, help="IDK repository name."
    )
    parser.add_argument(
        "--output-dir", type=Path, required=True, help="Output directory path."
    )
    parser.add_argument(
        "--input-dir",
        type=Path,
        required=True,
        help="Input IDK export directory path.",
    )
    parser.add_argument(
        "--fuchsia-dir",
        type=Path,
        # Assume this script is under //build/bazel/scripts/
        default=Path(__file__).parent.parent.parent.parent,
        help="Specify Fuchsia source directory (auto-detected)",
    )
    parser.add_argument(
        "--ninja-build-dir",
        type=Path,
        help="Specify Ninja output directory (auto-detected)",
    )
    args = parser.parse_args()

    if not args.ninja_build_dir:
        # args.fuchsia_dir points to the Bazel workspace, which contains symlinks
        # to the real files in the actual top-level source directory. Resolve
        # one of them to find the real location.
        #
        #  out/default/gen/build/bazel/workspace/README.md -->
        #     ../../../../../../README.md
        #
        args.fuchsia_dir = (args.fuchsia_dir / "README.md").resolve().parent
        if not (args.fuchsia_dir / ".jiri_manifest").exists():
            parser.error(f"Invalid Fuchsia directory path: {args.fuchsia_dir}")

        fx_build_dir = args.fuchsia_dir / ".fx-build-dir"
        if not fx_build_dir.exists():
            parser.error(
                f"Cannot detect build directory, use 'fx set' in: {args.fuchsia_dir}"
            )
        build_subdir = fx_build_dir.read_text().strip()
        if not build_subdir:
            parser.error(
                f"Cannot detect build directory, use --ninja-output-dir=DIR"
            )

        args.ninja_build_dir = args.fuchsia_dir / build_subdir

    if not args.ninja_build_dir.exists():
        parser.error(
            f"Ninja build directory does not exist: {args.ninja_build_dir}"
        )

    GenerateIdkRepository(
        args.repository_name,
        args.output_dir,
        args.input_dir,
        args.ninja_build_dir,
    )
    return 0


def GenerateIdkRepository(
    name: str, output_dir: Path, input_dir: Path, ninja_build_dir: Path
) -> None:
    """Generate a Bazel repository from the specified Fuchsia IDK directory.

    Args:
        name: Repository name.
        output_dir: Path for the IDK repository directory.
        input_dir: Path to the IDK directory from which to create the repository.
        ninja_build_dir: Path to the Ninja build directory.
    """
    input_dir = input_dir.resolve()
    ninja_build_dir = ninja_build_dir.resolve()

    output_idk = OutputIdk(output_dir)

    # Symlink then source meta/manifest.json
    input_manifest_path = input_dir / _IDK_MANIFEST_RELPATH
    input_manifest = json.load(input_manifest_path.open())

    output_idk.add_symlink(_IDK_MANIFEST_RELPATH, input_manifest_path)

    rewriter = PathRewriter(name, output_idk, input_dir, ninja_build_dir)

    # Handle each part separately.
    for part in input_manifest["parts"]:
        part_meta_path = part["meta"]

        src_meta_path = input_dir / part_meta_path
        try:
            meta = json.load(src_meta_path.open())
        except:
            # In case of malformed input JSON file, print its path
            # to ease debugging this script.
            print(
                f"INVALID METADATA FILE PATH {src_meta_path}", file=sys.stderr
            )
            raise

        meta_dir = os.path.dirname(part_meta_path)
        rewriter.rewrite_metadata(meta, meta_dir)

        output_idk.add_json_file(part_meta_path, meta, is_meta=True)

    output_idk.write_all()


if __name__ == "__main__":
    sys.exit(main())

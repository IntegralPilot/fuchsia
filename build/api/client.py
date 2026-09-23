#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""A tool providing information about the Fuchsia build graph(s).

This is not intended to be called directly by developers, but by
specialized tools and scripts like `fx`, `ffx` and others.

See https://fxbug.dev/42084664 for context.
"""

# TECHNICAL NOTE: Reduce imports to a strict minimum to keep startup time
# of this script as low a possible. You can always perform an import lazily
# only when you need it (e.g. see how json and difflib are imported below).
import argparse
import functools
import os
import sys
import typing as T
from pathlib import Path

_SCRIPT_FILE = Path(__file__)
_SCRIPT_DIR = _SCRIPT_FILE.parent
sys.path.insert(0, str(_SCRIPT_DIR))
from script_commands import ScriptCommandBase, ScriptCommandList

sys.path.insert(0, str(_SCRIPT_DIR / "../bazel/scripts"))

if T.TYPE_CHECKING:
    import gn_labels
    import gn_ninja_outputs

_FUCHSIA_DIR = (_SCRIPT_DIR / ".." / "..").resolve()


def _get_host_platform() -> str:
    """Return host platform name, following Fuchsia conventions."""
    if sys.platform == "linux":
        return "linux"
    elif sys.platform == "darwin":
        return "mac"
    else:
        return os.uname().sysname


def _get_host_arch() -> str:
    """Return host CPU architecture, following Fuchsia conventions."""
    host_arch = os.uname().machine
    if host_arch == "x86_64":
        return "x64"
    elif host_arch.startswith(("armv8", "aarch64")):
        return "arm64"
    else:
        return host_arch


def _get_host_tag() -> str:
    """Return host tag, following Fuchsia conventions."""
    return "%s-%s" % (_get_host_platform(), _get_host_arch())


def _get_target_cpu(build_dir: Path) -> str:
    """Return target_cpu value from current GN build configuration."""
    args_json_path = build_dir / "args.json"
    if not args_json_path.exists():
        return "unknown_cpu"
    import json

    args_json = json.load(args_json_path.open())
    if not isinstance(args_json, dict):
        return "unknown_cpu"
    return args_json.get("target_cpu", "unknown_cpu")


def get_build_dir(fuchsia_dir: Path) -> Path:
    """Get current Ninja build directory."""
    # Use $FUCHSIA_DIR/.fx-build-dir if present. This is only useful
    # when invoking the script directly from the command-line, i.e.
    # during build system development.
    #
    # `fx` scripts should use the `fx-build-api-client` function which
    # always sets --build-dir to the appropriate value instead
    # (https://fxbug.dev/336720162).
    file = fuchsia_dir / ".fx-build-dir"
    if not file.exists():
        return Path()
    return fuchsia_dir / file.read_text().strip()


def get_ninja_path(fuchsia_dir: Path, host_tag: str) -> Path:
    """Return Path of Ninja executable."""
    return fuchsia_dir / f"prebuilt/third_party/ninja/{host_tag}/ninja"


def _warning(msg: str) -> None:
    """Print a warning message to stderr."""
    if sys.stderr.isatty():
        print(f"\033[1;33mWARNING:\033[0m {msg}", file=sys.stderr)
    else:
        print(f"WARNING: {msg}", file=sys.stderr)


def _error(msg: str) -> None:
    """Print an error message to stderr."""
    if sys.stderr.isatty():
        print(f"\033[1;31mERROR:\033[0m {msg}", file=sys.stderr)
    else:
        print(f"ERROR: {msg}", file=sys.stderr)


def _printerr(msg: str) -> int:
    """Like _error() but returns 1."""
    _error(msg)
    return 1


# NOTE: Do not use dataclasses because its import adds 20ms of startup time
# which is _massive_ here.
class BuildApiModule:
    """Simple dataclass-like type describing a given build API module."""

    def __init__(self, name: str, file_path: Path):
        self.name = name
        self.path = file_path

    def get_content(self) -> str:
        """Return content as string."""
        return self.path.read_text()

    def get_content_as_json(self) -> object:
        """Return content as a JSON object + lazy-loads the 'json' module."""
        import json

        return json.load(self.path.open())


class BuildApiModuleList(object):
    """Models the list of all build API module files."""

    def __init__(self, build_dir: Path):
        self._modules: list[BuildApiModule] = []
        self.list_path = build_dir / "build_api_client_info"
        if not self.list_path.exists():
            return

        for line in self.list_path.read_text().splitlines():
            name, equal, file_path = line.partition("=")
            assert (
                equal == "="
            ), f"Invalid format for input file: {self.list_path}"
            self._modules.append(BuildApiModule(name, build_dir / file_path))

        self._modules.sort(key=lambda x: x.name)  # Sort by name.
        self._map = {m.name: m for m in self._modules}

    def empty(self) -> bool:
        """Return True if modules list is empty."""
        return len(self._modules) == 0

    def modules(self) -> T.Sequence[BuildApiModule]:
        """Return the sequence of BuildApiModule instances, sorted by name."""
        return self._modules

    def find(self, name: str) -> BuildApiModule | None:
        """Find a BuildApiModule by name, return None if not found."""
        return self._map.get(name)

    def names(self) -> T.Sequence[str]:
        """Return the sorted list of build API module names."""
        return [m.name for m in self._modules]


def load_ninja_outputs_database(build_dir: Path) -> None | T.Any:
    """Load the Ninja outputs database, return None on error."""
    try:
        import gn_ninja_outputs

        return gn_ninja_outputs.load_from_build_dir(build_dir)
    except RuntimeError as e:
        print(f"ERROR: Could not load outputs database: {e}", file=sys.stderr)
        return None


class LastBuildApiFilter(object):
    """Filter one or more build API modules based on last build artifacts."""

    @staticmethod
    def add_parser_arguments(parser: argparse.ArgumentParser) -> None:
        """Add parser arguments related to filtering the output."""
        parser.add_argument(
            "--last-build-only",
            action="store_true",
            help="Only include values matching the targets from the last build invocation.",
        )
        parser.add_argument(
            "--no-last-build-check",
            action="store_true",
            help="When --last-build-only is used, ignore last build success check.",
        )

    @staticmethod
    def has_filter_flag(args: argparse.Namespace) -> bool:
        """Returns True if |args| contains a filtering flag."""
        return bool(args.last_build_only)

    def __init__(self, args: argparse.Namespace):
        self._error = ""

        # Verify that the last build was successful, and set error otherwise.
        if not args.no_last_build_check:
            if not (args.build_dir / "last_ninja_build_success.stamp").exists():
                self._error = "Last build did not complete successfully (use --ignore-last-build-check to ignore)."
                return

        self._ninja = get_ninja_path(args.fuchsia_dir, args.host_tag)
        self._filter = self.generate_filter(self._ninja, args.build_dir)

    @property
    def error(self) -> str:
        return self._error

    def filter_json(self, api_name: str, json_value: T.Any) -> T.Any:
        assert not self._error, "Cannot call filter_json() on error"
        return self._filter.filter_api_json(api_name, json_value)

    @staticmethod
    def generate_filter(ninja: Path, build_dir: Path) -> T.Any:
        import build_utils
        import ninja_artifacts
        from build_api_filter import BuildApiFilter

        ninja_runner = build_utils.NinjaRunner(ninja, build_dir)
        last_build_artifacts = ninja_artifacts.get_last_build_artifacts(
            ninja_runner
        )
        last_build_sources = ninja_artifacts.get_last_build_sources(
            ninja_runner
        )
        return BuildApiFilter(last_build_artifacts, last_build_sources)


###########################################################################################
###########################################################################################
#####
#####   COMMAND: list
#####


class ListCommand(ScriptCommandBase):
    """List all available build API module names."""

    @staticmethod
    def run(args: argparse.Namespace) -> int:
        """Implement the `list` command."""
        for name in args.modules.names():
            print(name)
        return 0


###########################################################################################
###########################################################################################
#####
#####   COMMAND: print
#####


class PrintCommand(ScriptCommandBase):
    """Print build API module content."""

    DESCRIPTION = """
Print the content of a given build API module, given its name.
Use the 'list' command to print the list of all available names.
"""

    @staticmethod
    def add_arguments(parser: argparse.ArgumentParser) -> None:
        LastBuildApiFilter.add_parser_arguments(parser)
        parser.add_argument("api_name", help="Name of build API module.")

    @staticmethod
    def run(args: argparse.Namespace) -> int:
        module = args.modules.find(args.api_name)
        if not module:
            return _printerr(
                f"Unknown build API module name {args.api_name}, must be one of:\n\n %s\n"
                % "\n ".join(args.modules.names())
            )

        if not module.path.exists():
            return _printerr(
                f"Missing input file, please use `fx set` or `fx gen` command: {module.path}"
            )

        content = module.get_content()
        if LastBuildApiFilter.has_filter_flag(args):
            import json

            api_filter = LastBuildApiFilter(args)
            if api_filter.error:
                print(f"ERROR: {api_filter.error}", file=sys.stderr)
                return 1

            json_content = api_filter.filter_json(
                args.api_name, json.loads(content)
            )
            content = json.dumps(json_content, indent=2, separators=(",", ": "))

        print(content)
        return 0


###########################################################################################
###########################################################################################
#####
#####   COMMAND: print_all
#####


class PrintAllCommand(ScriptCommandBase):
    """Print single JSON containing the content of all build API modules."""

    @staticmethod
    def add_arguments(parser: argparse.ArgumentParser) -> None:
        parser.add_argument(
            "--pretty",
            action="store_true",
            help="Pretty print the JSON output.",
        )
        LastBuildApiFilter.add_parser_arguments(parser)

    @staticmethod
    def run(args: argparse.Namespace) -> int:
        """Implement the `print_all` command."""
        result = {}
        for module in args.modules.modules():
            if module.name != "api":
                result[module.name] = {
                    "file": os.path.relpath(module.path, args.build_dir),
                    "json": module.get_content_as_json(),
                }

        import json

        if LastBuildApiFilter.has_filter_flag(args):
            api_filter = LastBuildApiFilter(args)
            if api_filter.error:
                print(f"ERROR: {api_filter.error}", file=sys.stderr)
                return 1

            for name, v in result.items():
                v["json"] = api_filter.filter_json(name, v["json"])

        if args.pretty:
            print(
                json.dumps(
                    result, sort_keys=True, indent=2, separators=(",", ": ")
                )
            )
        else:
            print(json.dumps(result, sort_keys=True))
        return 0


###########################################################################################
###########################################################################################
#####
#####   COMMAND: print_debug_symbols
#####   COMMAND: export_last_build_debug_symbols
#####


class DebugSymbolCommandState(object):
    """Common state for both print_debug_symbols and export_last_build_debug_symbols."""

    def __init__(
        self,
        modules: BuildApiModuleList,
        ninja: Path,
        build_dir: Path,
        resolve_build_ids: bool = True,
        keep_duplicates: bool = False,
        test_mode: bool = False,
    ) -> None:
        import debug_symbols

        self._modules = modules
        self._ninja = ninja
        self._build_dir = build_dir
        self._debug_parser = debug_symbols.DebugSymbolsManifestParser(build_dir)
        self._merge_duplicates = not keep_duplicates

        if resolve_build_ids:
            self._debug_parser.enable_build_id_resolution()
        if test_mode:
            # --test-mode is used during regression testing to avoid
            # using a fake ELF input file. Simply return the file name
            # as the build-id value for now.
            def get_build_id(path: Path | str) -> str:
                return Path(path).name

            self._debug_parser.set_build_id_callback_for_test(get_build_id)

    def parse_manifest(self, last_build_only: bool) -> int:
        module = self._modules.find("debug_symbols")
        assert module
        if not module.path.exists():
            return _printerr(
                f"Missing input file, please use `fx set` or `fx gen` command: {module.path}"
            )

        import json

        with module.path.open("rt") as f:
            manifest_json = json.load(f)

        if last_build_only:
            # Only filter the top-level entries, as Bazel-generated artifacts
            # have a "debug" path that points directly in the Bazel output_base
            # and are not known as Ninja artifacts.
            api_filter = LastBuildApiFilter.generate_filter(
                self._ninja, self._build_dir
            )
            manifest_json = api_filter.filter_api_json(
                "debug_symbols", manifest_json
            )

        try:
            self._debug_parser.parse_manifest_json(manifest_json, module.path)
            if self._merge_duplicates:
                self._debug_parser.deduplicate_entries()
        except ValueError as e:
            return _printerr(str(e))
        return 0

    @property
    def debug_symbol_entries(self) -> list[dict[str, T.Any]]:
        return self._debug_parser.entries


class PrintDebugSymbolsCommand(ScriptCommandBase):
    """Print flattened debug symbol entries."""

    DESCRIPTION = "Print the content of debug_symbols.json and all the files it includes as a single JSON list of entries."

    @staticmethod
    def add_arguments(parser: argparse.ArgumentParser) -> None:
        parser.add_argument(
            "--pretty",
            action="store_true",
            help="Pretty print the JSON output.",
        )
        parser.add_argument(
            "--resolve-build-ids",
            action="store_true",
            help="Force resolution of build-id values.",
        )
        parser.add_argument(
            "--keep-duplicates",
            action="store_true",
            help="Keep duplicate entries, only used for debugging.",
        )
        parser.add_argument(
            "--test-mode",
            action="store_true",
            help="Enable test mode for debugging.",
        )
        LastBuildApiFilter.add_parser_arguments(parser)

    @staticmethod
    def run(args: argparse.Namespace) -> int:
        state = DebugSymbolCommandState(
            args.modules,
            get_ninja_path(args.fuchsia_dir, args.host_tag),
            args.build_dir,
            resolve_build_ids=args.resolve_build_ids,
            keep_duplicates=args.keep_duplicates,
            test_mode=args.test_mode,
        )

        status = state.parse_manifest(bool(args.last_build_only))
        if status != 0:
            return status

        result = state.debug_symbol_entries

        import json

        if args.pretty:
            print(
                json.dumps(
                    result, sort_keys=True, indent=2, separators=(",", ": ")
                )
            )
        else:
            print(json.dumps(result, sort_keys=True))
        return 0


class ExportLastBuildDebugSymbolsCommand(ScriptCommandBase):
    """Export debug symbols from last build."""

    DESCRIPTION = "Export debug symbols from last build to a directory."

    @staticmethod
    def add_arguments(parser: argparse.ArgumentParser) -> None:
        parser.add_argument(
            "--output-dir",
            type=Path,
            required=True,
            help="Output directory for exported symbols.",
        )
        parser.add_argument(
            "--with-breakpad-symbols",
            action="store_true",
            help="Generate breakpad symbols.",
        )
        parser.add_argument(
            "--dump_syms",
            type=Path,
            help="Path to dump_syms tool for --with-breakpad-symbols (auto-detected).",
        )
        parser.add_argument(
            "--with-gsym-symbols",
            action="store_true",
            help="Generate GSYM symbols.",
        )
        parser.add_argument(
            "--gsymutil",
            type=Path,
            help="Path to llvm-gsymutil tool for --with-gsym-symbols (auto-detected).",
        )
        parser.add_argument(
            "-j",
            "--jobs",
            type=int,
            default=os.cpu_count(),
            help="Number of parallel jobs (defaults to number of cores).",
        )
        parser.add_argument(
            "--quiet",
            action="store_true",
            help="Do not print progress messages.",
        )
        parser.add_argument(
            "--keep-duplicates",
            action="store_true",
            help="Keep duplicate entries, only used for debugging.",
        )
        parser.add_argument(
            "--test-mode",
            action="store_true",
            help="Enable test mode for debugging.",
        )

    @staticmethod
    def run(args: argparse.Namespace) -> int:
        import debug_symbols

        state = DebugSymbolCommandState(
            args.modules,
            get_ninja_path(args.fuchsia_dir, args.host_tag),
            args.build_dir,
            resolve_build_ids=True,  # Always resolve the .build-id value
            keep_duplicates=args.keep_duplicates,
            test_mode=args.test_mode,
        )

        state.parse_manifest(last_build_only=True)

        dump_syms_tool = None
        gsymutil_tool = None

        if args.with_breakpad_symbols:
            dump_syms_tool = args.dump_syms
            if not dump_syms_tool:
                dump_syms_tool = (
                    args.fuchsia_dir
                    / f"prebuilt/third_party/breakpad/{args.host_tag}/dump_syms/dump_syms"
                )
                if not dump_syms_tool.exists():
                    if args.host_tag != "linux_x64":
                        print(
                            f"WARNING: Ignoring breakpad symbol generation (https://fxbug.dev/447331878).",
                            file=sys.stderr,
                        )
                        dump_syms_tool = None
                    else:
                        print(
                            f"ERROR: Missing breakpad tool, use --dump_syms=TOOL: {dump_syms_tool}",
                            file=sys.stderr,
                        )
                        return 1

        if args.with_gsym_symbols:
            gsymutil_tool = args.gsymutil
            if not gsymutil_tool:
                gsymutil_tool = (
                    args.fuchsia_dir
                    / f"prebuilt/third_party/clang/{args.host_tag}/bin/llvm-gsymutil"
                )
                if not gsymutil_tool.exists():
                    print(
                        f"ERROR: Missing gsymutil tool, use --gsymutil=TOOL: {gsymutil_tool}",
                        file=sys.stderr,
                    )
                    return 1

        def log_error(error: str) -> None:
            print(f"ERROR: {error}", file=sys.stderr)

        def log(msg: str) -> None:
            if not args.quiet:
                print(msg)

        exporter = debug_symbols.DebugSymbolExporter(
            args.build_dir,
            dump_syms_tool=dump_syms_tool,
            gsymutil_tool=gsymutil_tool,
            log=log,
            log_error=log_error,
        )
        exporter.parse_debug_symbols(state.debug_symbol_entries)

        if not exporter.export_debug_symbols(args.output_dir, depth=args.jobs):
            return 1

        return 0


###########################################################################################
###########################################################################################
#####
#####   COMMAND: last_ninja_artifacts
#####


class LastNinjaArtifactsCommand(ScriptCommandBase):
    """Print the list of Ninja artifacts matching the last build invocation."""

    @staticmethod
    def run(args: argparse.Namespace) -> int:
        import build_utils
        import ninja_artifacts

        ninja = get_ninja_path(args.fuchsia_dir, args.host_tag)
        ninja_runner = build_utils.NinjaRunner(ninja, args.build_dir)

        last_artifacts = ninja_artifacts.get_last_build_artifacts(ninja_runner)

        print("\n".join(last_artifacts))
        return 0


###########################################################################################
###########################################################################################
#####
#####   COMMAND: ninja_path_to_gn_label
#####


class NinjaPathToGnLabelCommand(ScriptCommandBase):
    """Print the GN label of a each input Ninja output path."""

    @staticmethod
    def add_arguments(parser: argparse.ArgumentParser) -> None:
        parser.add_argument("paths", nargs="*", help="Ninja output paths.")
        parser.add_argument(
            "--allow-unknown",
            action="store_true",
            help="Allow unknown paths.",
        )

    @staticmethod
    def run(args: argparse.Namespace) -> int:
        outputs = load_ninja_outputs_database(args.build_dir)
        if not outputs:
            return 1

        import gn_ninja_outputs

        assert isinstance(outputs, gn_ninja_outputs.NinjaOutputsBase)

        failure = False
        labels = set()
        for path in args.paths:
            label = outputs.path_to_gn_label(path)
            if label:
                labels.add(label)
                continue

            if args.allow_unknown and not path.startswith("/"):
                labels.add(path)
                continue

            print(
                f"ERROR: Unknown Ninja target path: {path}",
                file=sys.stderr,
            )
            failure = True

        if failure:
            return 1

        print("\n".join(sorted(labels)))
        return 0


###########################################################################################
###########################################################################################
#####
#####   COMMAND: gn_label_to_ninja_paths
#####


@functools.lru_cache
def _load_json_list(file_path: Path) -> list[dict[str, T.Any]]:
    """Safely load a list JSON file, returning [] on missing/corrupt."""
    import json

    try:
        data = json.loads(file_path.read_text())
        return data if isinstance(data, list) else []
    except (json.JSONDecodeError, OSError):
        return []


def _normalize_label(label: str) -> str:
    """Normalize a GN or Bazel label for comparison (strip @, toolchain, qualify)."""
    from gn_labels import qualify_gn_target_name

    target = label.split("(", 1)[0].removeprefix("@")
    return qualify_gn_target_name(target) if target.startswith("//") else target


def _find_bazel_wrapper_target(
    label: str,
    build_dir: Path,
    outputs: "gn_ninja_outputs.NinjaOutputsBase",
    qualifier: "gn_labels.GnLabelQualifier",
) -> tuple[str, list[str], str]:
    """Find the GN wrapper target or direct Bazel command for a Bazel target.

    Args:
        label: The target label to look up (e.g. "//tools/foo:bar").
        build_dir: Path to Ninja build directory.
        outputs: Ninja outputs database for mapping stamp files to GN labels.
        qualifier: Helper to qualify GN labels and format build args.

    Returns:
        A tuple of (gn_target, gn_args, bazel_target) where:
          - gn_target: The matching GN wrapper target label, or "" if not found.
          - gn_args: A list of build arguments for the GN target, or [].
          - bazel_target: A direct Bazel command hint if applicable, or "".
        If no matching wrapper is found, returns ("", [], "").
    """
    target_label = _normalize_label(label)

    # 1. Check bazel_root_targets.json (host tools)
    for item in _load_json_list(build_dir / "bazel_root_targets.json"):
        if _normalize_label(item.get("bazel_label", "")) == target_label:
            gn_target = item.get("host_bin_label") or item.get(
                "gn_subtarget_label"
            )
            if gn_target:
                bazel_arg = f"@{target_label}"
                return (
                    gn_target,
                    qualifier.label_to_build_args(gn_target),
                    f"fx build --host {bazel_arg}",
                )

    # 2. Check bazel_target_infos.json (platform & general bazel actions)
    for item in _load_json_list(build_dir / "bazel_target_infos.json"):
        if _normalize_label(item.get("bazel_target", "")) == target_label:
            stamp = item.get("stamp_path")
            if stamp and (gn_target := outputs.path_to_gn_label(stamp)):
                return (
                    gn_target,
                    qualifier.label_to_build_args(gn_target),
                    "",
                )

    return ("", [], "")


def resolve_gn_labels_to_ninja_paths(
    labels: list[str],
    build_dir: Path,
    host_tag: str,
    allow_unknown: bool = False,
) -> tuple[str, list[str]]:
    """Convert a list of GN labels to the corresponding Ninja output paths.

    Args:
        labels: List of input GN labels.
        build_dir: Path to Ninja build directory.
        host_tag: Host tag value (e.g. "linux-x64").
        allow_unknown: Optional flag, set it to True to allow non-GN labels to
            be passed as input, and returned as-is in the output.
    Returns:
        On success, return ("", ninja_path), where ninja_paths is a list of
        corresponding Ninja target paths.

        On failure, return (error_message, []) where error_message indicates
        and error.
    """
    outputs = load_ninja_outputs_database(build_dir)
    if not outputs:
        return ("Could not load Ninja outputs", [])

    import gn_ninja_outputs

    assert isinstance(outputs, gn_ninja_outputs.NinjaOutputsBase)

    from gn_labels import GnLabelQualifier

    host_cpu = host_tag.split("-")[1]
    target_cpu = _get_target_cpu(build_dir)
    qualifier = GnLabelQualifier(host_cpu, target_cpu)

    all_paths = []
    for label in labels:
        if label.startswith("//"):
            qualified_label = qualifier.qualify_label(label)
            paths = outputs.gn_label_to_paths(qualified_label)
            if paths:
                all_paths.extend(paths)
                continue
            gn_target, gn_args, direct_bazel = _find_bazel_wrapper_target(
                qualified_label, build_dir, outputs, qualifier
            )
            if gn_target:
                qualified_gn = qualifier.qualify_label(gn_target)
                wrapper_paths = outputs.gn_label_to_paths(qualified_gn)
                if wrapper_paths:
                    all_paths.extend(wrapper_paths)
                    gn_cmd = f"fx build {' '.join(gn_args)}"
                    print(
                        f"NOTE: Auto-mapped Bazel target '{label}' to GN wrapper '{gn_cmd}'.",
                        file=sys.stderr,
                    )
                    continue
                gn_cmd = f"fx build {' '.join(gn_args)}"
                msg = (
                    f"Unknown GN label (not in the configured graph): {label}\n"
                )
                msg += f"NOTE: '{label}' is a Bazel target wrapped by GN.\n"
                msg += f"      Did you mean: {gn_cmd}"
                if direct_bazel:
                    msg += f"\n      Or for direct Bazel: {direct_bazel}"
                return (msg, [])
            return (
                f"Unknown GN label (not in the configured graph): {label}",
                [],
            )
        elif label.startswith("/"):
            return (
                f"Absolute path is not a valid GN label or Ninja path: {label}",
                [],
            )
        elif allow_unknown:
            # Assume this is a Ninja path.
            all_paths.append(label)
        else:
            return (f"Not a proper GN label (must start with //): {label}", [])

    return ("", all_paths)


class GnLabelToNinjaPathsCommand(ScriptCommandBase):
    """Print the Ninja output paths of one or more GN labels."""

    DESCRIPTION = "Print the Ninja output paths of one or more GN labels."

    @staticmethod
    def add_arguments(parser: argparse.ArgumentParser) -> None:
        parser.add_argument("labels", nargs="*", help="GN labels.")
        parser.add_argument(
            "--allow-unknown",
            action="store_true",
            help="Allow unknown labels.",
        )

    @staticmethod
    def run(args: argparse.Namespace) -> int:
        error_message, all_paths = resolve_gn_labels_to_ninja_paths(
            args.labels, args.build_dir, args.host_tag, args.allow_unknown
        )
        if error_message:
            _error(error_message)
            return 1

        for path in sorted(all_paths):
            print(path)
        return 0


###########################################################################################
###########################################################################################
#####
#####   COMMAND: fx_build_args_to_labels
#####


class FxBuildArgsToLabelsCommand(ScriptCommandBase):
    """Convert fx build arguments to fully-qualified GN labels."""

    @staticmethod
    def add_arguments(parser: argparse.ArgumentParser) -> None:
        parser.add_argument(
            "--allow-targets",
            action="store_true",
            help="Allow Ninja target names in arguments.",
        )
        parser.add_argument("--args", required=True, nargs=argparse.REMAINDER)

    @staticmethod
    def run(args: argparse.Namespace) -> int:
        outputs = load_ninja_outputs_database(args.build_dir)
        if not outputs:
            return 1

        import gn_ninja_outputs

        assert isinstance(outputs, gn_ninja_outputs.NinjaOutputsBase)

        from gn_labels import GnLabelQualifier

        host_cpu = args.host_tag.split("-")[1]
        target_cpu = _get_target_cpu(args.build_dir)
        qualifier = GnLabelQualifier(host_cpu, target_cpu)

        failure = False

        # The following is used by `fx build` to print a warning when a Ninja
        # path is passed to it, instead of a GN label. The warning will print
        # the correct GN label. The function also errors if the path does not
        # correspong to anything in the Ninja build plan.
        def ninja_path_to_gn_label(path: str) -> str:
            label = outputs.path_to_gn_label(path)
            if label:
                label_args = qualifier.label_to_build_args(label)
                _warning(
                    f"Use '{' '.join(label_args)}' instead of Ninja path '{path}'"
                )
                return label

            error_msg = f"Unknown Ninja path: {path}"

            if args.allow_targets and outputs.is_valid_target_name(path):
                target_labels = outputs.target_name_to_gn_labels(path)
                if len(target_labels) == 1:
                    label_args = qualifier.label_to_build_args(target_labels[0])
                    _warning(
                        f"Use '{' '.join(label_args)}' instead of Ninja target '{path}'"
                    )
                    return target_labels[0]

                if len(target_labels) > 1:
                    error_msg = (
                        f"Ambiguous Ninja target name '{path}' matches several GN labels:\n"
                        + "\n".join(target_labels)
                    )
                else:
                    error_msg = f"Unknown Ninja target: {path}"

            # Try to fall back to a GN label by prepending //
            fallback_label = "//" + path
            fallback_label_qualified = qualifier.qualify_label(fallback_label)
            if outputs.gn_label_to_paths(fallback_label_qualified):
                label_args = qualifier.label_to_build_args(
                    fallback_label_qualified
                )
                _warning(
                    f"Use '{' '.join(label_args)}' instead of '{path}' for GN targets"
                )
                return fallback_label_qualified

            _error(error_msg)
            nonlocal failure
            failure = True
            return ""

        qualifier.set_ninja_path_to_gn_label(ninja_path_to_gn_label)

        labels = qualifier.build_args_to_labels(args.args)
        if failure:
            return 1

        for label in labels:
            print(label)

        return 0


###########################################################################################
###########################################################################################
#####
#####   COMMAND: should_file_changes_trigger_build
#####


class ShouldFileChangesTriggerBuildCommand(ScriptCommandBase):
    """Detect whether a list of changed files should require a new build."""

    DESCRIPTION_RAW = """
Take as input a list of paths to source files that have changed since the last build, to determine
if they should require re-running the build (based on the current build configuration).

If no change is necessary, return 0 after printing 'NO' to stdout.
If a change is necessary, return 0 after printing 'YES: <reason>' to stdout. "
An error status indicates a problem when running the tool.

Note that results will corresponds to the top-level targets of the previous build,
and their transitive dependencies. Running this command in a clean checkout will not
return correct results, as depfile dependencies will be missing.
"""

    @staticmethod
    def add_arguments(parser: argparse.ArgumentParser) -> None:
        parser.add_argument(
            "file_path",
            type=Path,
            nargs="*",
            default=[],
            help="Path to changed source file, relative to the Fuchsia source directory.",
        )
        parser.add_argument(
            "--files-list",
            type=Path,
            help="Path to an input text file that contains one source file path per line. All paths should be relative to the Fuchsia source directory",
        )
        parser.add_argument(
            "--root-target",
            nargs="*",
            default=[],
            help="By default, only considers changes that will affect the targets of the last build. "
            + "Use this flag to specify a different set of target GN labels. Can be used multiple times.",
        )

    @staticmethod
    def run(args: argparse.Namespace) -> int:
        changed_files = args.file_path
        if args.files_list:
            changed_files += args.files_list.read_text().splitlines()

        import build_utils
        import ninja_artifacts

        root_targets: list[str] = []
        for target in args.root_target:
            if target.startswith("//"):
                # A GN target label - convert its first Ninja output path.
                error_message, ninja_paths = resolve_gn_labels_to_ninja_paths(
                    [target], args.build_dir, args.host_tag
                )
                if error_message:
                    _error(error_message)
                    return 1
                root_targets.extend(ninja_paths[:1])
            elif target.startswith("@"):
                # A Bazel target label - not supported at the moment.
                _error(
                    f"ERROR: --root-target does not support Bazel labels for now!: {target}"
                )
                return 1
            else:
                # Assume a Ninja target
                root_targets.append(target)

        ninja_path = get_ninja_path(args.fuchsia_dir, args.host_tag)
        ninja_runner = build_utils.NinjaRunner(ninja_path, args.build_dir)
        result, reason = ninja_artifacts.should_file_changes_trigger_build(
            changed_files,
            args.fuchsia_dir,
            ninja_runner,
            root_targets=root_targets if root_targets else None,
        )
        if result:
            print(f"YES: {reason}")
        else:
            print("NO")
        return 0


###########################################################################################
###########################################################################################
#####
#####   COMMAND: affected_tests
#####


class AffectedTestsCommand(ScriptCommandBase):
    """Compute the list of tests affected by a set of changed files."""

    DESCRIPTION = """
Determine the set of tests that should be run after the last build,
based on a list of changed source file paths. On success, print a list
where each line is in the format <test_label>,<test_env>, where
<test_label> is either a GN or Bazel target label, and <test_env> is either
"host" or "device".
"""

    @staticmethod
    def add_arguments(parser: argparse.ArgumentParser) -> None:
        parser.add_argument(
            "--files-list",
            type=Path,
            required=True,
            help="Path to an input text file that contains one source file path per line. All paths should be relative to the Fuchsia source directory",
        )

    @staticmethod
    def run(args: argparse.Namespace) -> int:
        changed_files = args.files_list.read_text().splitlines()
        # Lazy imports, see technical note at the top of this file.
        import affected_tests

        sys.path.insert(0, f"{_SCRIPT_DIR}/../bazel/scripts")
        import build_utils

        ninja_path = get_ninja_path(args.fuchsia_dir, args.host_tag)
        ninja_runner = build_utils.NinjaRunner(ninja_path, args.build_dir)

        bazel_paths = build_utils.BazelPaths(args.fuchsia_dir, args.build_dir)
        bazel_launcher = build_utils.BazelLauncher(bazel_paths.launcher)

        test_targets = affected_tests.find_tests_affected_by_changed_files(
            changed_files, args.fuchsia_dir, ninja_runner, bazel_launcher
        )
        for target in sorted(test_targets, key=lambda x: x.label):
            env = "device" if target.os_name == "fuchsia" else "host"
            print(f"{target.label},{env}")
        return 0


class FileToTestPackageCache(object):
    """Caches the mapping from source files to the test packages that depend on them."""

    def __init__(self, build_dir: Path) -> None:
        self.cache_path = build_dir / "file_to_test_package_cache.json"
        self.build_dir = build_dir
        self.cache: dict[str, list[str]] = {}
        self.dirty = False
        self._load()

    def _load(self) -> None:
        if not self.cache_path.exists():
            return

        # Check if cache is stale
        cache_mtime = self.cache_path.stat().st_mtime
        for name in [
            "tests.json",
            "rust-project.json",
            "compile_commands.json",
            "args.gn",
        ]:
            p = self.build_dir / name
            if p.exists() and p.stat().st_mtime > cache_mtime:
                # Cache is stale
                return

        try:
            import json

            self.cache = json.loads(self.cache_path.read_text())
        except Exception as e:
            print(
                f"ERROR: Failed to load cache from {self.cache_path}: {e}",
                file=sys.stderr,
            )

    def get(self, source_path: str) -> list[str] | None:
        """Retrieves the list of test packages associated with a source file.

        Args:
            source_path: The path to the source file.

        Returns:
            A list of test package names if found, otherwise None.
        """
        return self.cache.get(source_path)

    def set(self, source_path: str, packages: list[str]) -> None:
        """Sets the list of test packages associated with a source file.

        Args:
            source_path: The path to the source file.
            packages: The list of test package names.
        """
        if self.cache.get(source_path) != packages:
            self.cache[source_path] = packages
            self.dirty = True

    def save(self) -> None:
        """Saves the cache to disk."""
        if self.dirty:
            import json

            self.cache_path.write_text(json.dumps(self.cache))
            self.dirty = False


class FileToTestPackageCommand(ScriptCommandBase):
    """Find the fuchsia_test_packages(s) that depend on a given source file."""

    @staticmethod
    def add_arguments(parser: argparse.ArgumentParser) -> None:
        parser.add_argument(
            "--source-path",
            required=True,
            help="The source file path.",
        )

    def run(self, args: argparse.Namespace) -> int:
        import json
        import time

        def _log(msg: str) -> None:
            if not args.quiet:
                print(f"[{time.strftime('%H:%M:%S')}] {msg}", file=sys.stderr)

        cache = FileToTestPackageCache(args.build_dir)
        cached_result = cache.get(args.source_path)
        if cached_result is not None:
            print(json.dumps(cached_result))
            return 0

        test_packages = set()

        outputs = load_ninja_outputs_database(args.build_dir)
        if not outputs:
            return 1

        import gn_ninja_outputs

        assert isinstance(outputs, gn_ninja_outputs.NinjaOutputsBase)

        from file_to_test_package import FileToTestPackageFinder

        finder = FileToTestPackageFinder(
            args.build_dir,
            args.fuchsia_dir,
            outputs,
            _log,
            host_tag=args.host_tag,
        )

        try:
            fast_packages = finder.find_test_packages_fast(args.source_path)
            if fast_packages:
                test_packages.update(fast_packages)
                _log(f"Fast path found {len(test_packages)} packages.")
        except Exception as e:
            _log(f"Fast path failed: {e}")

        if not test_packages:
            _log(
                f"No test packages found for {args.source_path}.",
            )
            _log(
                "If this file contains a test, make sure it is included in your build configuration.",
            )
            _log(
                "You can add it using `fx set ... --with //path/to:your-test-package`",
            )
            return 1

        result_list = sorted(list(test_packages))
        print(json.dumps(result_list))

        cache.set(args.source_path, result_list)
        cache.save()

        return 0


###########################################################################################
###########################################################################################
#####
#####   COMMAND: target_metadata
#####


class TargetMetadataCommand(ScriptCommandBase):
    """Collects target metadata for the build.

    The schema of the output is defined in target_metadata.schema.json.
    """

    DESCRIPTION = """
Collects deps, sources, and inputs for all targets from project.json,
and merges them into a JSON file along with their source directories.
"""

    TARGET_METADATA_VERSION = 1

    @staticmethod
    def add_arguments(parser: argparse.ArgumentParser) -> None:
        parser.add_argument(
            "--output",
            type=Path,
            required=True,
            help="The output JSON file path.",
        )

    def run(self, args: argparse.Namespace) -> int:
        import json
        import time

        def _log(msg: str) -> None:
            if not args.quiet:
                print(f"[{time.strftime('%H:%M:%S')}] {msg}", file=sys.stderr)

        project_json_path = args.build_dir / "project.json"
        if not project_json_path.exists():
            _error(f"project.json file not found at: {project_json_path}")
            return 1

        _log(f"Loading {project_json_path}...")
        try:
            with project_json_path.open("r") as f:
                project_data = json.load(f)
        except Exception as e:
            _error(f"Failed to read project.json at {project_json_path}: {e}")
            return 1

        targets_data = project_data.get("targets", {})

        _log("Processing target metadata...")
        merged_data: dict[str, dict[str, T.Any]] = {}

        def strip_label_to_file_or_dir_name(labels: list[str]) -> list[str]:
            """Strips a target label to a file or directory name.

            This strips toolchains, target names, and preceding label identifiers to get
            a relative file path.

            //foo/bar:baz($host_toolchain) -> foo/bar
            //foo/bar/host_file.py -> foo/bar/host_file.py
            """
            return [
                label.split(":")[0].lstrip("@").lstrip("//") for label in labels
            ]

        for target, target_entry in targets_data.items():
            target_info: dict[str, T.Any] = {}
            # Target is a target label in either GN or Bazel format, and corresponds to
            # the same label in the keys of the output dictionary.
            if "deps" in target_entry and target_entry["deps"] is not None:
                target_info["deps"] = target_entry["deps"]
            # Sources are paths from the root of the source tree to files.
            if (
                "sources" in target_entry
                and target_entry["sources"] is not None
            ):
                target_info["sources"] = strip_label_to_file_or_dir_name(
                    target_entry["sources"]
                )
            # Inputs are paths from the root of the source tree to files.
            if "inputs" in target_entry and target_entry["inputs"] is not None:
                target_info["inputs"] = strip_label_to_file_or_dir_name(
                    target_entry["inputs"]
                )

            # Extract source_dir from the GN label: //foo/bar:baz(...) -> foo/bar
            target_info["source_dir"] = strip_label_to_file_or_dir_name(
                [target]
            )[0]

            merged_data[target] = target_info

        _log(f"Writing output to {args.output}...")
        output_dict = {
            "$schema": "target_metadata.schema.json",
            "version": self.TARGET_METADATA_VERSION,
            "targets": merged_data,
        }
        try:
            with args.output.open("w") as f:
                json.dump(output_dict, f, indent=2)
            _log("Done.")
            return 0
        except Exception as e:
            _log(f"Failed to write output to {args.output}: {e}")
            return 1


###########################################################################################
###########################################################################################
#####
#####   Main program
#####


def main(main_args: T.Sequence[str]) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawTextHelpFormatter
    )
    parser.add_argument(
        "--fuchsia-dir",
        default=_FUCHSIA_DIR,
        type=Path,
        help="Specify Fuchsia source directory.",
    )
    parser.add_argument(
        "--build-dir",
        type=Path,
        help="Specify Ninja build directory.",
    )
    parser.add_argument(
        "--host-tag",
        help="Host platform tag, using Fuchsia conventions (auto-detected).",
        # NOTE: Do not set a default with _get_host_tag() here for faster startup,
        # since the //build/api/client wrapper script will always set this option.
    )
    parser.add_argument(
        "--quiet",
        help="If True, suppress informational output.",
        default=False,
        action="store_true",
    )

    commands = ScriptCommandList(parser)
    commands.add_command(ListCommand())
    commands.add_command(PrintCommand())
    commands.add_command(PrintAllCommand())
    commands.add_command(PrintDebugSymbolsCommand())
    commands.add_command(ExportLastBuildDebugSymbolsCommand())
    commands.add_command(LastNinjaArtifactsCommand())
    commands.add_command(NinjaPathToGnLabelCommand())
    commands.add_command(GnLabelToNinjaPathsCommand())
    commands.add_command(FxBuildArgsToLabelsCommand())
    commands.add_command(ShouldFileChangesTriggerBuildCommand())
    commands.add_command(AffectedTestsCommand())
    commands.add_command(FileToTestPackageCommand())
    commands.add_command(TargetMetadataCommand())

    args = parser.parse_args(main_args)

    if not args.build_dir:
        args.build_dir = get_build_dir(args.fuchsia_dir)

    if not args.build_dir.exists():
        return _printerr(
            "Could not locate build directory, please use `fx set` command or use --build-dir=DIR.",
        )

    if not args.host_tag:
        args.host_tag = _get_host_tag()

    args.modules = BuildApiModuleList(args.build_dir)
    if args.modules.empty():
        return _printerr(
            f"Missing input file, did you run `fx gen` or `fx set`?: {args.modules.list_path}"
        )

    return commands.run(args, keep_exception=True)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

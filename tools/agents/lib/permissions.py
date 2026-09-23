# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Permission definitions, profile resolution, command list reading, and regex expansion."""

from __future__ import annotations

import dataclasses
import pathlib
import re
import shlex
from collections.abc import Sequence

from agents.lib import paths

_ARG_VALUE_PATTERN = r"""(?:[^\s"']*(?:"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')+[^\s"']*|"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|\S+)"""

ENV_VARS_PREFIX_PATTERN = rf"([A-Za-z_][A-Za-z0-9_]*={_ARG_VALUE_PATTERN}\s+)*"
GIT_GLOBAL_FLAGS_PATTERN = (
    rf"(\s+("
    rf"(-C|--git-dir|--work-tree|-c)(\s+|=){_ARG_VALUE_PATTERN}"
    rf"|--namespace(\s+|=){_ARG_VALUE_PATTERN}"
    rf"|--no-pager|--no-color|--literal-pathspecs|--no-optional-locks|--bare|-p|--paginate"
    r"))*"
)
FX_GLOBAL_FLAGS_PATTERN = (
    rf"(\s+("
    rf"(-t|--target|--dir|--enable|--disable|--invoker)(\s+|=){_ARG_VALUE_PATTERN}"
    r"|-xx|-x|-i|--"
    r"))*"
)
FFX_GLOBAL_FLAGS_PATTERN = (
    rf"(\s+("
    rf"(-t|--target|--machine|-c|--config|--env-root|-e|--env|--isolate-dir|--stamp|--timeout|-l|--log-level|-o|--log-output)(\s+|=){_ARG_VALUE_PATTERN}"
    r"|--schema|-v|--verbose|--no-environment|--strict|-d|--direct"
    r"))*"
)
JIRI_GLOBAL_FLAGS_PATTERN = (
    rf"(\s+("
    rf"(-j|--j|-color|--color|-progress-window|--progress-window|-root|--root|-time-log-threshold|--time-log-threshold|-timefile|--timefile)(\s+|=){_ARG_VALUE_PATTERN}"
    rf"|(-show-progress|--show-progress)(={_ARG_VALUE_PATTERN})?"
    r"|-quiet|-q|--quiet|-time|--time|-vv|-v|--vv|--v"
    r"))*"
)

PYTHON_COMMAND_NAMES: set[str] = {
    "python",
    "python3",
    "fuchsia-vendored-python",
}

PYTHON_TOOL_PREFIXES: tuple[str, ...] = (
    "python",
    "python3",
    "fuchsia-vendored-python",
    "scripts/fuchsia-vendored-python",
    "./scripts/fuchsia-vendored-python",
    ".jiri_root/bin/fuchsia-vendored-python",
)

TRUSTED_HELP_TOOLS: set[str] = {
    "fx",
    "ffx",
    "jiri",
    "git",
    "jj",
    "bb",
    "cipd",
}


def _is_sed_inplace(arguments: str) -> bool:
    """Check if sed arguments include an in-place modification flag."""
    try:
        tokens = shlex.split(arguments)
    except ValueError:
        tokens = arguments.split()

    for token in tokens:
        if token == "--in-place" or token.startswith("--in-place="):
            return True
        if token.startswith("-") and not token.startswith("--"):
            # Matches short option bundle containing 'i' (e.g. -i, -i.bak, -Ei, -in)
            if re.match(r"^-[a-zA-Z]*i", token):
                return True
    return False


def _is_git_clean_force(arguments: str) -> bool:
    """Check if git clean arguments include a force flag."""
    try:
        tokens = shlex.split(arguments)
    except ValueError:
        tokens = arguments.split()

    if not tokens or tokens[0] != "clean":
        return False

    for token in tokens[1:]:
        if token == "--force" or token.startswith("--force="):
            return True
        if token.startswith("-") and not token.startswith("--"):
            # Matches short option bundle containing 'f' (e.g. -f, -df, -fd, -fdx, -fxd)
            if "f" in token:
                return True
    return False


@dataclasses.dataclass(frozen=True)
class ProfileDefinition:
    """Definition of a permission profile flavor."""

    description: str
    allow: Sequence[str] = ()
    deny: Sequence[str] = ()
    ask: Sequence[str] = ()


@dataclasses.dataclass(frozen=True)
class PermissionGrants:
    """Resolved collection of permission grants across allow, deny, and ask."""

    allow: Sequence[str] = ()
    deny: Sequence[str] = ()
    ask: Sequence[str] = ()


PROFILE_DEFINITIONS: dict[str, ProfileDefinition] = {
    "read-only": ProfileDefinition(
        description="Harmless inspection and build/test commands. Workspace edits, external ops, cache, and device ops prompt.",
        allow=("read_only.txt",),
        deny=("never_allow.txt",),
        ask=(
            "external_changes.txt",
            "device_ops.txt",
            "cache_destruction.txt",
            "batch_execution.txt",
        ),
    ),
    "local-changes": ProfileDefinition(
        description="Workspace edits, formatting, local commits, emulators, and device ops. Batch, external & cache ops prompt.",
        allow=(
            "read_only.txt",
            "local_changes.txt",
            "device_ops.txt",
        ),
        deny=("never_allow.txt",),
        ask=(
            "batch_execution.txt",
            "cache_destruction.txt",
            "external_changes.txt",
        ),
    ),
    "external-changes": ProfileDefinition(
        description="Local & device changes plus remote reviews, git push, and remote infra. Batch & cache ops prompt.",
        allow=(
            "read_only.txt",
            "local_changes.txt",
            "external_changes.txt",
            "device_ops.txt",
        ),
        deny=("never_allow.txt",),
        ask=(
            "batch_execution.txt",
            "cache_destruction.txt",
        ),
    ),
    "full-access": ProfileDefinition(
        description="Broad unprompted developer access across all local, external, device, and cache tools.",
        allow=(
            "read_only.txt",
            "local_changes.txt",
            "external_changes.txt",
            "device_ops.txt",
            "cache_destruction.txt",
            "batch_execution.txt",
        ),
        deny=("never_allow.txt",),
        ask=(),
    ),
}


@dataclasses.dataclass(frozen=True)
class ToolSpec:
    """Specification for building anchored tool command regex grants."""

    binary_prefix: str
    global_flags_pattern: str = ""
    allow_flags_anywhere: bool = False

    @property
    def prefix(self) -> str:
        """Anchored prefix matching environment variables, binary path, and global flags."""
        return (
            f"{ENV_VARS_PREFIX_PATTERN}"
            f"{self.binary_prefix}\\b"
            f"{self.global_flags_pattern}"
        )

    def expand(self, arguments: str) -> list[str]:
        """Expand tool arguments into anchored regex pattern grants."""
        prefix = self.prefix
        if not arguments:
            return [f"command(regex:{prefix}(\\s+.*)?)"]

        if self.allow_flags_anywhere:
            try:
                tokens = shlex.split(arguments)
            except ValueError:
                tokens = arguments.split()

            if tokens:
                subcmd = tokens[0]
                flags = [t for t in tokens[1:] if t.startswith("-")]
                positional = [t for t in tokens[1:] if not t.startswith("-")]

                if flags and not positional:
                    # Note: When multiple flags are defined in the template,
                    # they are matched in the sequence specified.
                    flag_patterns: list[str] = []
                    for f in flags:
                        escaped = re.escape(f)
                        if f.startswith("--") and "=" not in f:
                            flag_patterns.append(
                                rf"{escaped}(?:={_ARG_VALUE_PATTERN})?"
                            )
                        else:
                            flag_patterns.append(escaped)
                    flag_pattern = "\\s+".join(flag_patterns)
                    escaped_subcmd = re.escape(subcmd)
                    return [
                        f"command(regex:{prefix}\\s+{escaped_subcmd}"
                        f"(?:\\s+{_ARG_VALUE_PATTERN})*\\s+{flag_pattern}(?:\\s+.*)?)"
                    ]

        escaped_args = re.escape(arguments)
        return [f"command(regex:{prefix}\\s+{escaped_args}(\\s+.*)?)"]


TOOL_SPECS: dict[str, ToolSpec] = {
    "fx": ToolSpec(
        binary_prefix=r"(\S+/)?fx",
        global_flags_pattern=FX_GLOBAL_FLAGS_PATTERN,
    ),
    "ffx": ToolSpec(
        binary_prefix=rf"(?:(\S+/)?fx{FX_GLOBAL_FLAGS_PATTERN}\s+)?(\S+/)?ffx",
        global_flags_pattern=FFX_GLOBAL_FLAGS_PATTERN,
    ),
    "jiri": ToolSpec(
        binary_prefix=r"(\S+/)?jiri",
        global_flags_pattern=JIRI_GLOBAL_FLAGS_PATTERN,
    ),
    "git": ToolSpec(
        binary_prefix=r"(\S+/)?git",
        global_flags_pattern=GIT_GLOBAL_FLAGS_PATTERN,
        allow_flags_anywhere=True,
    ),
}


def _tool_prefix(base_name: str) -> str:
    """Return anchored regex prefix for a tool, reusing ToolSpec when available."""
    if base_name in TOOL_SPECS:
        return TOOL_SPECS[base_name].prefix
    escaped_base = re.escape(base_name)
    return f"{ENV_VARS_PREFIX_PATTERN}(\\S+/)?{escaped_base}\\b"


def _expand_help_variants(base_name: str) -> list[str]:
    """Expand trusted tool help commands into subcommands and --help flag."""
    prefix = _tool_prefix(base_name)
    return [
        f"command(regex:{prefix}(?:\\s+help(?:\\s+.*)?|(?:\\s+{_ARG_VALUE_PATTERN})*\\s+--help))"
    ]


def _expand_python_variants(arguments: str) -> list[str]:
    """Expand python command variants across interpreter paths."""
    results: list[str] = []
    for prefix in PYTHON_TOOL_PREFIXES:
        candidate = (
            f"command({prefix} {arguments})"
            if arguments
            else f"command({prefix})"
        )
        if candidate not in results:
            results.append(candidate)
    return results


def _expand_git_clean_force_variants() -> list[str]:
    """Expand git clean force variants into an anchored regex matching any force flag."""
    prefix = (
        f"{ENV_VARS_PREFIX_PATTERN}"
        f"{TOOL_SPECS['git'].binary_prefix}\\b"
        f"{TOOL_SPECS['git'].global_flags_pattern}"
    )
    return [
        f"command(regex:{prefix}\\s+clean"
        f"(?:\\s+{_ARG_VALUE_PATTERN})*\\s+(-[a-zA-Z]*f\\S*|--force(?:={_ARG_VALUE_PATTERN})?)(?:\\s+.*)?)"
    ]


def _expand_sed_variants(base_command: str, arguments: str) -> list[str]:
    """Expand sed command variants, providing dedicated regex matching for in-place flags."""
    if _is_sed_inplace(arguments):
        return [
            f"command(regex:{ENV_VARS_PREFIX_PATTERN}(\\S+/)?sed\\b"
            f"(?:\\s+{_ARG_VALUE_PATTERN})*\\s+(-[a-zA-Z]*i\\S*|--in-place(\\S*)?)(?:\\s+.*)?)"
        ]
    return _expand_binary_variants(base_command, arguments)


def _expand_binary_variants(base_command: str, arguments: str) -> list[str]:
    """Expand general system binaries into anchored regex pattern grants."""
    base_name = pathlib.Path(base_command).name
    escaped_base = re.escape(base_name)
    prefix = f"{ENV_VARS_PREFIX_PATTERN}(\\S+/)?"

    if not arguments:
        return [f"command(regex:{prefix}{escaped_base}\\b(?:\\s+.*)?)"]

    if arguments.endswith("/"):
        escaped_args = re.escape(arguments.rstrip("/"))
        arg_suffix = "/+(?:\\s+.*)?"
    elif arguments.endswith("~"):
        escaped_args = re.escape(arguments)
        arg_suffix = "/*(?:\\s+.*)?"
    else:
        escaped_args = re.escape(arguments)
        arg_suffix = "(?:\\s+.*)?"

    return [
        f"command(regex:{prefix}{escaped_base}\\b\\s+{escaped_args}{arg_suffix})"
    ]


def _clean_command_line(raw_line: str) -> str:
    """Clean a command line, stripping comments while preserving quoted '#' characters."""
    line = raw_line.strip()
    if not line or line.startswith("#"):
        return ""
    try:
        tokens = shlex.split(line, comments=True, posix=False)
        return " ".join(tokens)
    except ValueError:
        return re.sub(r"\s+#.*$", "", line).strip()


def expand_command_variants(raw_line: str) -> list[str]:
    """Expand a human-friendly command line into agent grant variants."""
    line = _clean_command_line(raw_line)
    if not line:
        return []

    if re.match(r"^[a-zA-Z_]+\(.*\)$", line):
        return [line]

    try:
        tokens = shlex.split(line, posix=False)
    except ValueError:
        tokens = line.split()

    if not tokens:
        return []

    raw_base = tokens[0]
    base_command = raw_base.strip("\"'")
    arguments = line[len(raw_base) :].strip()

    if not base_command:
        return []

    base_name = pathlib.Path(base_command).name
    if arguments == "help" and base_name in TRUSTED_HELP_TOOLS:
        return _expand_help_variants(base_name)

    results: list[str] = []
    if base_name in TOOL_SPECS:
        if base_name == "git" and _is_git_clean_force(arguments):
            results.extend(_expand_git_clean_force_variants())
        else:
            results.extend(TOOL_SPECS[base_name].expand(arguments))
    elif base_name in PYTHON_COMMAND_NAMES:
        results.extend(_expand_python_variants(arguments))
    elif base_name == "sed":
        results.extend(_expand_sed_variants(base_command, arguments))
    else:
        results.extend(_expand_binary_variants(base_command, arguments))

    return results


def read_command_list_file(file_path: pathlib.Path) -> list[str]:
    """Read a permission list file and return expanded grant variants."""
    if not file_path.is_file():
        return []

    grants: list[str] = []
    with file_path.open("r", encoding="utf-8") as file_handle:
        for line in file_handle:
            grants.extend(expand_command_variants(line))
    return grants


def load_profile_grants(
    fuchsia_dir: pathlib.Path, profile_name: str
) -> PermissionGrants:
    """Load and aggregate permission grants for a given profile."""
    if profile_name not in PROFILE_DEFINITIONS:
        raise ValueError(
            f"Unknown profile '{profile_name}'. Valid: {list(PROFILE_DEFINITIONS.keys())}"
        )

    profile = PROFILE_DEFINITIONS[profile_name]
    permission_dirs = paths.find_permission_dirs(fuchsia_dir)

    def _collect_rules(category_files: Sequence[str]) -> list[str]:
        rules: list[str] = []
        for filename in category_files:
            for perm_dir in permission_dirs:
                file_path = perm_dir / filename
                rules.extend(read_command_list_file(file_path))
        return list(dict.fromkeys(rules))

    return PermissionGrants(
        allow=_collect_rules(profile.allow),
        deny=_collect_rules(profile.deny),
        ask=_collect_rules(profile.ask),
    )

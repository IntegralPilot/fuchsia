# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Process GN labels for `fx build` and other tools."""

import json
import os
import typing as T

# A dictionary of convenience Fuchsia toolchain aliases.
# Keep these in sync with the rest of the Fuchsia build definitions.
_TOOLCHAIN_ALIASES = {
    "host": "//build/toolchain:host_{host_cpu}",
    "default": "//build/toolchain/fuchsia:{target_cpu}",
    "fuchsia": "//build/toolchain/fuchsia:{target_cpu}",
    "fidl": "//build/fidl:fidling",
}


def split_gn_target(target: str) -> tuple[str, str]:
    dir, colon, name = target.partition(":")
    if colon == ":":
        return (dir, name)

    pos = target.rfind("/")
    assert pos >= 0
    return (target[:pos], target[pos + 1 :])


def split_gn_label(label: str) -> tuple[str, str]:
    """Split a label into a (target, toolchain) pair."""
    assert label.startswith("//"), f"GN Label must start with //: {label}"
    tc_pos = label.find("(", 2)
    if tc_pos < 0:
        assert label.find("//", 2) < 0, f"GN label cannot use // after start."
        return (label, "")

    assert (
        label[-1] == ")"
    ), f"GN label missing toolchain closing paren: {label}"
    assert (
        label.find("//", tc_pos + 3) < 0
    ), f"Malformed GN label toolchain: {label}"
    return (label[0:tc_pos], label[tc_pos + 1 : -1])


def qualify_gn_target_name(target: str) -> str:
    """Qualify GN target with a proper name (e.g. //foo -> //foo:foo)."""
    assert target.startswith("//"), f"GN Label must start with //: {target}"
    if target.find(":") >= 0:
        return target

    pos = target.rfind("/")
    assert pos >= 1
    return f"{target}:{target[pos + 1:]}"


class GnLabelQualifier(object):
    def __init__(self, host_cpu: str, target_cpu: str) -> None:
        self._host_cpu = host_cpu
        self._target_cpu = target_cpu
        alias_dict = {
            "host_cpu": host_cpu,
            "target_cpu": target_cpu,
        }
        self._aliases = {
            alias: label.format(**alias_dict)
            for alias, label in _TOOLCHAIN_ALIASES.items()
        }
        self._default_toolchain = self._aliases["default"]
        self._ninja_path_to_gn_label: T.Callable[[str], str] = lambda x: x

    @staticmethod
    def create_from_build_dir(build_dir: str) -> "GnLabelQualifier":
        """Create new instance from the build directory content.

        A convenience method that automatically detects the host CPU
        and extracts the target CPU value from args.json.

        Args:
            build_dir: Path to the Ninja build directory (after `gn gen`).
        Returns:
            A new GnLabelQualifier instance configured for the host cpu
            and the target cpu configured through `args.gn`.
        """
        host_cpu = os.uname().machine
        if host_cpu == "x86_64":
            host_cpu = "x64"
        elif host_cpu.startswith(("armv8", "aarch64")):
            host_cpu = "arm64"

        with open(os.path.join(build_dir, "args.json")) as f:
            target_cpu = json.load(f)["target_cpu"]

        return GnLabelQualifier(host_cpu, target_cpu)

    def set_ninja_path_to_gn_label(self, func: T.Callable[[str], str]) -> None:
        self._ninja_path_to_gn_label = func

    def qualify_toolchain(self, toolchain: str) -> str:
        if toolchain.startswith("//"):
            assert (
                toolchain.find("(") < 0
            ), f"GN toolchain cannot have toolchain suffix: {toolchain}"
            return qualify_gn_target_name(toolchain)

        alias = self._aliases.get(toolchain)
        assert alias is not None, f"Unknown GN toolchain alias: {toolchain}"
        return alias

    def qualify_label(self, label: str, fallback_toolchain: str = "") -> str:
        target, toolchain = split_gn_label(label)
        target = qualify_gn_target_name(target)
        if toolchain:
            toolchain = self.qualify_toolchain(toolchain)
            if toolchain == self._default_toolchain:
                toolchain = ""
        else:
            toolchain = fallback_toolchain

        if toolchain:
            return f"{target}({toolchain})"
        else:
            return target

    def label_to_build_args(self, label: str) -> list[str]:
        target, toolchain = split_gn_label(label)
        target_dir, target_name = split_gn_target(target)
        if target_name == os.path.basename(target_dir):
            target = target_dir

        if toolchain == self._default_toolchain:
            toolchain = ""

        if not toolchain:
            return [target]

        for alias, alias_toolchain in self._aliases.items():
            if alias_toolchain == toolchain:
                return [f"--{alias}", target]

        return [f"--toolchain={toolchain}", target]

    def build_args_to_labels(self, args: list[str]) -> list[str]:
        result = []
        cur_toolchain = ""
        for arg in args:
            if arg.startswith("--toolchain="):
                cur_toolchain = self.qualify_toolchain(
                    arg[len("--toolchain=") :]
                )
                continue
            if arg.startswith("--"):
                alias = self._aliases.get(arg[2:])
                assert alias is not None, f"Invalid toolchain alias {arg}"
                cur_toolchain = alias
                if cur_toolchain == self._default_toolchain:
                    cur_toolchain = ""
                continue

            if arg.startswith("//"):
                result.append(self.qualify_label(arg, cur_toolchain))
                continue

            # A Ninja target path maybe? To be handled by the caller.
            label = self._ninja_path_to_gn_label(arg)
            if label:
                result.append(label)

        return result

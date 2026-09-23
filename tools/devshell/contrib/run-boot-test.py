#!/usr/bin/env fuchsia-vendored-python
# Copyright 2020 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
from __future__ import annotations

import argparse
import difflib
import json
import os
import platform
import shlex
import subprocess
import sys
from dataclasses import dataclass
from enum import Enum

SCRIPT_DIR = os.path.dirname(__file__)
FUCHSIA_DIR = os.path.join(SCRIPT_DIR, "..", "..", "..")
PREBUILT_3P_DIR = os.path.join(FUCHSIA_DIR, "prebuilt", "third_party")
HOST_OS = {"Linux": "linux", "Darwin": "mac"}[platform.system()]
HOST_CPU = {"x86_64": "x64", "arm64": "arm64"}[platform.machine()]
HOST_PLATFORM = f"{HOST_OS}-{HOST_CPU}"


# Special values identifying a PE (i.e., Portable Executable) image, of which
# UEFI executables are examples.
#
# In identifying a UEFI executable, it is insufficient to match based on magic
# alone; ARM Linux boot firmware (e.g., our arm64 boot shim) uses that same
# value.
#
# See https://learn.microsoft.com/en-us/windows/win32/debug/pe-format.
PE_MAGIC = b"MZ"
PE_SIGNATURE = b"PE\0\0"


def is_pe(filepath):
    if not os.path.exists(filepath):
        return False
    with open(filepath, "rb") as f:
        if f.read(2) != PE_MAGIC:
            return False
        f.seek(0x3C)
        signature_offset = int.from_bytes(f.read(4), byteorder="little")
        f.seek(signature_offset)
        return f.read(4) == PE_SIGNATURE


def error(str):
    RED = "\033[91m"
    END = "\033[0m"
    print(RED + "ERROR: " + str + END)


def warning(str):
    YELLOW = "\033[93m"
    END = "\033[0m"
    print(YELLOW + "WARNING: " + str + END)


def info(str):
    BLUE = "\033[94m"
    END = "\033[0m"
    print(BLUE + "INFO: " + str + END)


class BootTest:
    # The state of the associated product bundle.
    class ProductBundle(Enum):
        # The product bundle name given in the test metadata is not recognized.
        UNKNOWN = 0

        # The product bundle is not yet built.
        NOT_BUILT = 1

        # The product bundle is built.
        BUILT = 2

    def __init__(self, product_bundles, test_json, build_dir):
        self.build_dir = build_dir

        test = test_json["test"]
        self.name = test["name"]
        self.label = test["label"]

        self.zbi = None
        self.qemu_kernel = None
        self.efi_disk = None

        if test_json["product_bundle"] in product_bundles:
            self._product_bundle = product_bundles[test_json["product_bundle"]]
            if os.path.exists(
                os.path.join(build_dir, self._product_bundle["json"]),
            ):
                self._state = self.ProductBundle.BUILT
                self._set_images_if_built()
            else:
                self._state = self.ProductBundle.NOT_BUILT

        else:
            self._product_bundle = {
                "cpu": None,
                "json": None,
                "name": test_json["product_bundle"],
                "path": None,
            }
            self._state = self.ProductBundle.UNKNOWN

    def _set_images_if_built(self):
        assert self._state == self.ProductBundle.BUILT
        with open(
            os.path.join(self.build_dir, self._product_bundle["json"]),
        ) as f:
            manifest = json.load(f)

        product_bundle_dir = self._product_bundle["path"]

        # Check the bootloader partitions for a `efi_shell` type.
        bootloader_partitions = manifest["partitions"]["bootloader_partitions"]

        # There is UEFI disk, and we should ignore `system_a``.
        if (
            len(bootloader_partitions) == 1
            and bootloader_partitions[0]["type"] == "efi-shell"
        ):
            self.efi_disk = os.path.join(
                product_bundle_dir,
                bootloader_partitions[0]["image"],
            )
            return

        # Normal QEMU Kernel handling a ZBI.
        for image in manifest["system_a"]:
            path = os.path.join(product_bundle_dir, image["path"])
            # A test ramdisk may or may not be a ZBI, strictly speaking, but convenient
            # enough to present it as such, especially as we must do so to the tooling.
            if image["type"] in ["zbi", "test-ramdisk"]:
                self.zbi = path
            elif image["type"] == "kernel":
                self.qemu_kernel = path

    # Enables sorting by name.
    def __lt__(self, other):
        return self.name < other.name

    @staticmethod
    def is_boot_test(test_json):
        return bool(test_json.get("is_boot_test"))

    def is_uefi_boot(self):
        if self.efi_disk:
            return True
        # The QEMU kernel might be a UEFI executable. Look for PE magic.
        if not self.qemu_kernel:
            return False
        kernel = os.path.join(self.build_dir, self.qemu_kernel)
        return is_pe(kernel)

    def arch(self):
        return self._product_bundle["cpu"]

    def product_bundle_name(self):
        return self._product_bundle["name"]

    # Attempts to ensure that the associated product bundle is built, returning
    # a boolean indicating success.
    def ensure_product_bundle(self, rebuild):
        assert (
            self._state == self.ProductBundle.BUILT
            or self._state == self.ProductBundle.NOT_BUILT
        ), f"unexpected state {self._state}"
        if self._state != self.ProductBundle.NOT_BUILT and not rebuild:
            return self._state == self.ProductBundle.BUILT
        build_command = ["fx", "build", self.label]
        info("Rebuilding: " + " ".join(build_command))
        if (
            subprocess.run(
                build_command,
                check=False,
                cwd=self.build_dir,
            ).returncode
            == 0
        ):
            self._state = self.ProductBundle.BUILT
            self._set_images_if_built()
            return True
        return False

    def print(self, command=None):
        kinds = []
        if self._state == self.ProductBundle.UNKNOWN:
            kinds = ["unknown: product bundle not recognized"]
        elif self._state == self.ProductBundle.NOT_BUILT:
            kinds = ["unknown: not yet built"]
        else:
            if self.is_uefi_boot():
                kinds.append("UEFI")
            if self.qemu_kernel:
                kinds.append("QEMU kernel")
            if self.zbi:
                kinds.append("ZBI")
            if self.efi_disk:
                kinds.append("EFI disk")
        print("* %s (%s)" % (self.name, ", ".join(kinds)))
        print("    label: %s" % self.label)
        print("    cpu: %s" % (self.arch() or "Unknown!"))
        if self.qemu_kernel:
            print("    qemu kernel: %s" % self.qemu_kernel)
        if self.zbi:
            print("    zbi: %s" % self.zbi)
        if self.efi_disk:
            print("    efi disk: %s" % self.efi_disk)
        if command:
            print("    command: %s" % " ".join(map(shlex.quote, command)))


# Basic options for running the test under emulation, as gleaned from test
# metadata.
@dataclass
class EmulatorProfile:
    # Base kernel arguments that must be passed.
    kernel_args: list[str]

    # Whether to enable hardware acceleration.
    accel: bool

    # Constructs the default profile, used when there is no explicit mention in
    # the test metadata.
    @classmethod
    def default(cls) -> EmulatorProfile:
        return cls(kernel_args=[], accel=True)

    # Attempts to construct an EmulatorProfile from a tests.json entry,
    # returning EmulatorProfile.default() if there was no indication.
    @classmethod
    def from_test_json(cls, test_json: dict) -> EmulatorProfile:
        envs = test_json["environments"]
        for env in envs:
            emulator_spec = env.get("emulator")
            if not emulator_spec:
                continue
            return cls(
                kernel_args=emulator_spec.get("kernel_args", []),
                accel=emulator_spec.get("accel", True),
            )
        return EmulatorProfile.default()


def find_bootserver(build_dir):
    with open(os.path.join(build_dir, "tool_paths.json")) as file:
        tool_paths = json.load(file)
    bootservers = [
        os.path.join(build_dir, tool["path"])
        for tool in tool_paths
        if (
            tool["name"] == "bootserver"
            and tool["cpu"] == HOST_CPU
            and tool["os"] == HOST_OS
        )
    ]
    if bootservers:
        return bootservers[0]
    print("Cannot find bootserver for %s-%s" % (HOST_OS, HOST_CPU))
    sys.exit(1)


EPILOG = """
In order to use this tool, please ensure that your boot test (usually defined
by one of zbi_test(), qemu_kernel_test(), or efi_test()) is in your GN graph. A
way to do this is to add //bundles/boot_tests to your `fx set` invocation.
"""


def main():
    build_dir = os.path.relpath(os.getenv("FUCHSIA_BUILD_DIR"))
    if build_dir is None:
        print("FUCHSIA_BUILD_DIR not set")
        return 1

    parser = argparse.ArgumentParser(
        prog="fx run-boot-test",
        description="Run a boot test.",
        epilog=EPILOG,
    )
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument(
        "--boot",
        "-b",
        action="store_true",
        help="Run via bootserver",
    )
    modes.add_argument(
        "--fastboot",
        "-f",
        action="store_true",
        help="Run via fastboot boot",
    )
    parser.add_argument(
        "--args",
        "-a",
        metavar="RUNNER-ARG",
        action="append",
        default=[],
        help="Pass RUNNER-ARG to bootserver / fx qemu / crosvm run",
    )
    parser.add_argument(
        "--cmdline",
        "-c",
        metavar="KERNEL-ARGS",
        action="append",
        default=[],
        help="Add kernel command-line arguments.",
    )
    parser.add_argument(
        "--test-filter",
        metavar="TEST-NAMES",
        action="append",
        default=[],
        help="Shorthand for `--cmdline=--gtest_filter=TEST-NAMES`.",
    )
    parser.add_argument(
        "--count",
        metavar="COUNT",
        action="append",
        default=[],
        help="Shorthand for `--cmdline=--gtest_repeat=COUNT`.",
    )
    parser.add_argument(
        "--kernel-shell-script",
        metavar="LINE",
        action="append",
        default=[],
        help="Shorthand for `--cmdline=kernel.shell.script=...` where each"
        " LINE has whitespace converted to `+`, all concatenated with `;`.",
    )
    parser.add_argument(
        "--kernel-unittest",
        metavar="TEST",
        action="append",
        default=[],
        help="Shorthand for a --kernel-shell-script sequence of `ut TEST`"
        " for each switch, plus some boilerplate.",
    )
    modes.add_argument("--crosvm", action="store_true", help="Run via crosvm")
    parser.add_argument(
        "--crosvm-args",
        metavar="ARG",
        action="append",
        default=[],
        help="Pass ARG to crosvm before `run` subcommand (--args go after)",
    )
    parser.add_argument(
        "--crosvm-path",
        metavar="PATH",
        type=str,
        default=os.path.relpath(
            os.path.join(PREBUILT_3P_DIR, "crosvm", HOST_PLATFORM, "crosvm"),
            build_dir,
        ),
        help="Use crosvm binary at PATH",
    )
    parser.add_argument(
        "name",
        help="Name of the boot test (target) to run",
        nargs="?",
    )
    parser.add_argument(
        "--arch",
        help="CPU architecture to run",
        metavar="ARCH",
        default=os.getenv("FUCHSIA_ARCH"),
    )
    parser.add_argument(
        "--build",
        action=argparse.BooleanOptionalAction,
        help="Use `fx build` to update images before running them (default).",
        default=True,
    )
    parser.add_argument(
        "--symbolize",
        action=argparse.BooleanOptionalAction,
        help="Pipe output through `ffx debug symbolize --no-prettify`.",
        default=False,
    )
    args = parser.parse_args()

    if args.arch is None:
        print("FUCHSIA_ARCH not set")
        return 1

    if args.kernel_unittest:
        if args.kernel_shell_script:
            error(
                "--kernel-unittest overrides --kernel-shell-script;"
                "they cannot be used together",
            )
            return 1
        args.kernel_shell_script = [
            f"ut {test}" for test in args.kernel_unittest
        ] + [
            "boot-test-success",
            "graceful-shutdown",
        ]
    if args.kernel_shell_script:
        args.cmdline.append(
            "kernel.shell.script="
            + ";".join(
                "+".join(line.split()) for line in args.kernel_shell_script
            ),
        )

    # Construct a map of product bundles by name. Boot test metadata points to
    # a product bundle in this way.
    with open(os.path.join(build_dir, "product_bundles.json")) as file:
        product_bundles = {}
        for bundle in json.load(file):
            product_bundles[bundle["name"]] = bundle

    # There can be multiple versions of the same boot test for different host
    # architectures. These will otherwise only differ in metadata name, a
    # difference that `BootTest()` normalizes away.
    with open(os.path.join(build_dir, "tests.json")) as file:
        boot_tests = {}
        emulator_profiles = {}
        for test in json.load(file):
            if not BootTest.is_boot_test(test):
                continue
            boot_test = BootTest(product_bundles, test, build_dir)
            if boot_test.arch() != args.arch:
                continue
            boot_tests[boot_test.name] = boot_test

            # All explicitly specified emulator metadata should coincide, so we
            # freely overwrite the profile for a given boot test in that case.
            emulator_profile = EmulatorProfile.from_test_json(test)
            if (
                boot_test.name not in emulator_profiles
                or emulator_profile != EmulatorProfile.default()
            ):
                emulator_profiles[boot_test.name] = emulator_profile

    if not boot_tests:
        warning(
            "no boot tests found. Is //bundles/boot_tests in your GN graph?",
        )
        return 0

    if not args.name:
        for test in sorted(boot_tests.values()):
            test.print()
        return 0

    names = [test.name for test in boot_tests.values()]
    # A cut-off of 0.8 was determined to be good enough in experimenting
    # with input names against "core-tests".
    matching_names = difflib.get_close_matches(args.name, names, cutoff=0.8)
    matches = [boot_tests[name] for name in matching_names]
    if len(matches) == 0:
        error("no boot tests closely matching a name of '%s' found" % args.name)
        return 1
    # The returned matches will be ordered by similarlity; if we have an exact
    # match, always go with that.
    if len(matches) > 1 and matches[0].name != args.name:
        error(
            "no boot tests closely matching a name of '%s' found. Closest matches:"
            % args.name,
        )
        for test in matches:
            test.print()
        return 1

    test = matches[0]
    assert test.ensure_product_bundle(args.build), (
        "unable to build product bundle %s" % test.product_bundle_name()
    )
    emulator_profile = emulator_profiles[test.name]

    if args.boot:
        if test.qemu_kernel:
            print(error("cannot use --boot with QEMU-only test %s" % test.name))
            return 1
        assert test.zbi
        bootserver = find_bootserver(build_dir)
        cmd = [bootserver, "--boot", test.zbi] + args.args
    elif args.fastboot:
        assert test.zbi
        cmd = ["fastboot", "boot", test.zbi] + args.args
    elif args.crosvm:
        if not test.qemu_kernel:
            print(
                error("cannot use --crosvm with no-kernel test %s" % test.name),
            )
            return 1
        if not emulator_profile.accel:
            print(
                error(
                    "cannot use --crosvm with unaccelerated test %s"
                    % test.name,
                ),
            )
            return 1
        cmd = [args.crosvm_path] + args.crosvm_args + ["run"] + args.args
        if test.zbi:
            cmd += ["--initrd", test.zbi]
        cmd.append(test.qemu_kernel)
        args.cmdline.extend(emulator_profile.kernel_args)
    else:
        cmd = [
            "fx",
            "qemu",
            "--arch",
            args.arch,
            "--no-authorized-keys",
        ] + args.args

        if not emulator_profile.accel:
            cmd += ["--no-kvm"]
        if test.is_uefi_boot():
            cmd += ["--uefi"]
        if test.qemu_kernel:
            cmd += ["-t", test.qemu_kernel]
        if test.zbi:
            cmd += ["-z", test.zbi]
        if test.efi_disk:
            cmd += ["-D", test.efi_disk]
        args.cmdline.extend(emulator_profile.kernel_args)

    for arg in (
        args.cmdline
        + [f"--gtest_filter={t}" for t in args.test_filter]
        + [f"--gtest_repeat={count}" for count in args.count]
    ):
        cmd += [
            "-p" if args.crosvm else ("--cmdline" if args.fastboot else "-c"),
            arg,
        ]

    if not args.boot and not args.fastboot and not args.crosvm:
        # Prevents QEMU from boot-looping, as most boot tests do not have a
        # means of gracefully shutting down.
        cmd += ["--", "-no-reboot"]

    test.print(command=cmd)
    if not args.symbolize:
        # Just run the emulator synchronously.
        return subprocess.run(cmd, check=False, cwd=build_dir).returncode

    # Start the emulator running asynchronously.
    emulator = subprocess.Popen(
        cmd,
        cwd=build_dir,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    # Run the symbolizer synchronously.  When the emulator finishes,
    # the symbolizer will see EOF on its stdin and exit too.
    symbolizer_status = subprocess.run(
        ["fx", "ffx", "debug", "symbolize", "--no-prettify"],
        check=False,
        cwd=build_dir,
        stdin=emulator.stdout,
    ).returncode

    # The emulator probably already exited or else the symbolizer wouldn't have
    # exited.  But still wait for the emulator in case e.g. the symbolizer
    # crashed.  And even when it's already finished, collect its exit status.
    emulator_status = emulator.wait()

    # The emulator's exit code is most important.  But also exit nonzero if the
    # symbolizer did so even though the emulator exited with success.
    return emulator_status if emulator_status != 0 else symbolizer_status


if __name__ == "__main__":
    sys.exit(main())

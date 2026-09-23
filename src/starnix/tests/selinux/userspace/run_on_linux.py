#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Runs SEStarnix userspace tests on Linux in qemu."""

import argparse
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from typing import Any, Callable, Dict, List

SUCCESS_RE = re.compile("TEST SUCCESS$", re.MULTILINE)


def build_initrd(
    work_dir: pathlib.Path,
    files: Dict[str, pathlib.Path],
) -> pathlib.Path:
    """Builds an initrd containing the provided files.

    Args:
      work_dir: the temporary dir we are working in.
      files: mapping of destination path in initrd to source path on host.

    Returns:
      The path to the initrd.
    """
    initrd_dir = work_dir / "initrd"
    initrd_dir.mkdir()

    for package_path, output_path in files.items():
        if package_path == "data/bin/init_for_linux":
            dest_path = initrd_dir / "init"
        elif package_path.startswith("data/lib/"):
            dest_path = initrd_dir / package_path.removeprefix("data/")
        else:
            dest_path = initrd_dir / package_path
        dest_path.parent.mkdir(exist_ok=True, parents=True)
        # Verify that the binary is built for x86_64 if it is an ELF.
        if output_path.is_file():
            file_out = subprocess.check_output(
                ["file", "-b", str(output_path)], text=True
            )
            if "ELF" in file_out and "x86-64" not in file_out:
                raise ValueError(
                    f"Binary {output_path} ({package_path}) is not built for x86_64: {file_out.strip()}"
                )

        shutil.copy(output_path, initrd_dir / dest_path)

    subprocess.run(
        "find . | cpio --quiet -R +0:+0 -H newc -o | gzip -9 -n > ../initrd.img",
        shell=True,
        check=True,
        cwd=initrd_dir,
    )
    return work_dir / "initrd.img"


def get_fuchsia_dir() -> pathlib.Path:
    """Returns fuchsia_dir."""
    assert "FUCHSIA_DIR" in os.environ
    return pathlib.Path(os.environ["FUCHSIA_DIR"])


def get_output_dir() -> pathlib.Path:
    """Returns the directory where the built artifacts are located."""
    return pathlib.Path.cwd()


def get_test_names() -> List[str]:
    """Returns the list of test names."""
    test_list_path = (
        get_output_dir()
        / "host_x64/obj/src/starnix/tests/selinux/userspace/test_list.json"
    )
    return sorted(json.loads(test_list_path.read_text()))


def collect_initrd_mapping(
    output_dir: pathlib.Path,
) -> Dict[str, pathlib.Path]:
    """Returns the mapping of files to include in the distribution."""
    files_map_path = (
        output_dir
        / "host_x64/obj/src/starnix/tests/selinux/userspace/files_map.json"
    )

    filename_to_dest = {}
    data = json.loads(files_map_path.read_text())

    for entry in data:
        src = entry.get("source")
        dest = entry.get("destination")
        if src and dest:
            filename_to_dest[pathlib.Path(src).name] = dest

    target_out_dir = (
        output_dir / "host_x64/obj/src/starnix/tests/selinux/userspace"
    )
    dirs_to_scan = [
        "libs",
        "expectations",
        "policies",
        "linux_x64",
        "audit_expectations",
    ]

    files = {}
    for d in dirs_to_scan:
        dir_path = target_out_dir / d
        for file_path in dir_path.iterdir():
            if file_path.is_file():
                filename = file_path.name
                dest = filename_to_dest.get(filename)
                if dest:
                    files[dest] = file_path

    return files


def parse_audit_expectations_from_output(stdout: str) -> list[dict[str, Any]]:
    """Parses audit expectation JSON blobs from the test runner stdout."""
    audit_expectations = []
    json_buffer = ""
    for line in stdout.splitlines():
        stripped_line = line.strip()
        if not stripped_line:
            continue

        if stripped_line.startswith("{"):
            json_buffer = stripped_line
        elif json_buffer:
            json_buffer += stripped_line

        if json_buffer and stripped_line.endswith("}"):
            try:
                data = json.loads(json_buffer)
                audit_expectations.append(data)
            except json.JSONDecodeError as e:
                print(f"Failed to load JSON object: {e}")
            finally:
                json_buffer = ""
    return audit_expectations


def sanitize_audit_log(log: str) -> str:
    """Removes volatile fields from an audit log string for comparison."""
    # Remove audit(timestamp:serial):
    sanitized = re.sub(r"audit\([^)]*\):\s*", "", log)
    # Remove pid=...
    sanitized = re.sub(r"\s+pid=\S+", "", sanitized)
    # Remove comm="..."
    sanitized = re.sub(r'\s+comm="[^"]*"', "", sanitized)
    return sanitized.strip()


def write_updated_expectations(
    all_new_expectations: list[dict[str, Any]],
) -> None:
    """Updates the `audit_success.json` file with new results, if they are meaningfully different."""
    if not all_new_expectations:
        print("No new audit expectations found to update.")
        return

    fuchsia_dir = get_fuchsia_dir()
    expectations_file = (
        fuchsia_dir
        / "src/starnix/tests/selinux/userspace/expectations/audit_success.json"
    )
    tests_array_name = "audit_success"

    try:
        with open(expectations_file, "r") as f:
            existing_data = json.load(f)
    except (FileNotFoundError, json.JSONDecodeError) as e:
        print(f"Failed to parse expectations file: {e}")
        return

    existing_tests_map = {
        test["name"]: test for test in existing_data.get(tests_array_name, [])
    }

    updated_count = 0
    for new_exp in all_new_expectations:
        test_name = new_exp["name"]
        existing_exp = existing_tests_map.get(test_name)

        # Sanitize the new expectations before comparison and potential saving.
        new_exp["audit_expectations"] = [
            sanitize_audit_log(log)
            for log in new_exp.get("audit_expectations", [])
        ]

        if not existing_exp:
            print(f"Adding new expectations for {test_name}")
            existing_tests_map[test_name] = new_exp
            updated_count += 1
            continue

        # Compare existing and new expectations by sanitizing audit log strings.
        old_audits = existing_exp.get("audit_expectations", [])
        new_audits = new_exp.get("audit_expectations", [])

        # Simple length check first.
        if len(old_audits) != len(new_audits):
            print(
                f"Updating expectations for {test_name} (audit count changed)"
            )
            existing_tests_map[test_name] = new_exp
            updated_count += 1
            continue

        # Compare sanitized audit strings.
        are_different = False
        for old_audit, new_audit in zip(old_audits, new_audits):
            if old_audit != new_audit:
                are_different = True
                break

        if are_different:
            print(
                f"Updating expectations for {test_name} (audit content changed)"
            )
            existing_tests_map[test_name] = new_exp
            updated_count += 1
        else:
            print(f"No meaningful changes for {test_name}, skipping update.")

    if updated_count == 0:
        print("No meaningful expectation changes detected. File not updated.")
        return

    # Convert back to a list and sort by name for consistent output.
    updated_tests = sorted(existing_tests_map.values(), key=lambda x: x["name"])
    final_data = {tests_array_name: updated_tests}
    with open(expectations_file, "w") as f:
        json.dump(final_data, f, indent=4)
        f.write("\n")
    print(
        f"Successfully updated {expectations_file} with {updated_count} changes."
    )


class TestSestarnixUserspaceOnLinux(unittest.TestCase):
    work_dir: pathlib.Path
    output_dir: pathlib.Path
    kernel_path: pathlib.Path
    initrd_path: pathlib.Path
    args = argparse.Namespace(
        all_output=False,
        preserve_work_dir=False,
        json=False,
        update_audit_expectations=False,
        skip_audit=False,
        kernel=None,
    )
    new_audit_expectations: list[dict[str, Any]] = []

    @classmethod
    def setUpClass(cls) -> None:
        cls.work_dir = pathlib.Path(tempfile.mkdtemp())
        cls.output_dir = get_output_dir()

        gki_dir = (
            cls.output_dir
            / "host_x64/obj/src/starnix/tests/selinux/userspace/gki"
        )

        if cls.args.kernel:
            if cls.args.kernel.is_absolute():
                cls.kernel_path = cls.args.kernel
            else:
                cls.kernel_path = pathlib.Path.cwd() / cls.args.kernel
        else:
            cls.kernel_path = gki_dir / "bzImage"

        print(f"DEBUG: Using kernel_path: {cls.kernel_path}", flush=True)

        if not cls.kernel_path.is_file():
            raise RuntimeError(f"Kernel not found at {cls.kernel_path}")

        try:
            files = collect_initrd_mapping(cls.output_dir)
            # Only add kernel modules to the initrd if we are using the default kernel.
            if not cls.args.kernel:
                for item in gki_dir.iterdir():
                    if item.is_file() and item.suffix == ".ko":
                        files[f"lib/modules/{item.name}"] = item
            cls.initrd_path = build_initrd(cls.work_dir, files)
        except Exception as e:
            raise RuntimeError(f"Failed to build initrd: {e}")

    @classmethod
    def tearDownClass(cls) -> None:
        if cls.args.update_audit_expectations:
            write_updated_expectations(cls.new_audit_expectations)
        if cls.args.preserve_work_dir:
            print(f"Workdir preserved at {cls.work_dir}")
        else:
            shutil.rmtree(cls.work_dir, ignore_errors=True)

    def run_specific_test(self, test_name: str) -> None:
        qemu_path = (
            self.output_dir
            / "host_x64/obj/src/starnix/tests/selinux/userspace/qemu/qemu-system-x86_64"
        )
        qemu_bios_path = (
            self.output_dir
            / "host_x64/obj/src/starnix/tests/selinux/userspace/qemu"
        )

        append_args = f"console=ttyS0 security=selinux debug=all audit=1 audit_backlog_limit=0 panic=-1 -- data/tests/{test_name}_bin"
        if self.args.json or self.args.update_audit_expectations:
            append_args += " --json"
        if self.args.skip_audit:
            append_args += " --skip-audit"

        qemu_args = [
            str(qemu_path),
            "-L",
            str(qemu_bios_path),
            "-kernel",
            str(self.kernel_path),
            "-initrd",
            str(self.initrd_path),
            "-no-reboot",
            "-display",
            "none",
            "-vga",
            "none",
            "-chardev",
            "stdio,id=char0",
            "-device",
            "virtio-serial-pci",
            "-device",
            "virtconsole,chardev=char0",
            "-m",
            "1G",
            "-enable-kvm",
            "-append",
            append_args,
        ]
        print(f"Running QEMU command: {' '.join(qemu_args)}", flush=True)

        result = subprocess.run(
            qemu_args,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        passed = SUCCESS_RE.search(result.stdout) is not None
        if self.args.all_output:
            print(result.stdout)
        elif not passed:
            print("Failure output:")
            result_lines: list[str] = []
            record_lines: bool = False
            tests_were_run = False
            for line in result.stdout.splitlines():
                if "[ RUN      ]" in line:
                    record_lines = True
                    tests_were_run = True
                if not record_lines:
                    continue
                result_lines.append(line)
                if "[       OK ]" in line:
                    print(line)
                    result_lines = []
                elif "[  FAILED  ]" in line:
                    print(*result_lines, sep="\n")
                    result_lines = []
            if not tests_were_run:
                print(result.stdout)
                print("Failed to run any tests! See all preceding output.")
            print(f"End of output ({test_name})")

        if self.args.update_audit_expectations:
            self.new_audit_expectations.extend(
                parse_audit_expectations_from_output(result.stdout)
            )

        self.assertTrue(passed, f"Test {test_name} failed")


def populate_dynamic_tests() -> None:
    """Generates test methods on the class."""
    try:
        tests = get_test_names()
    except Exception as e:
        err_msg = str(e)

        def test_discovery_failed(self: Any) -> None:
            raise RuntimeError(f"Failed to discover tests: {err_msg}")

        setattr(
            TestSestarnixUserspaceOnLinux,
            "test_discovery_failed",
            test_discovery_failed,
        )
        return

    for test_name in tests:
        method_name = "test_" + test_name

        def make_test_method(name: str) -> Callable[[Any], None]:
            return lambda self: self.run_specific_test(name)

        # Attach the method to the class
        setattr(
            TestSestarnixUserspaceOnLinux,
            method_name,
            make_test_method(test_name),
        )


# When this script is started from `fx test`, `__name__` is not "__main__"
if __name__ != "__main__":
    populate_dynamic_tests()
else:
    # The arguments are parsed twice: once here to get the custom arguments, and once by unittest, to get the unittest arguments.
    parser = argparse.ArgumentParser(
        add_help=False,
        description="Run SEStarnix userspace tests on Linux via QEMU.",
    )

    parser.add_argument(
        "--preserve-work-dir",
        help="Keep the work directory on exit.",
        action="store_true",
    )
    parser.add_argument(
        "--kernel",
        help="Path to the linux kernel to use (optional, defaults to prebuilt GKI kernel).",
        type=pathlib.Path,
    )
    parser.add_argument(
        "--all-output",
        help="Emit all output from tests directly, with no pretty-filtering.",
        action="store_true",
    )
    parser.add_argument(
        "--json",
        help="Generate audit JSON objects for expectations.",
        action="store_true",
    )
    parser.add_argument(
        "--update-audit-expectations",
        help="Update the audit expectation JSON file with the results from this run.",
        action="store_true",
    )
    parser.add_argument(
        "--skip-audit",
        help="Skip audit log checks in the test.",
        action="store_true",
    )

    args, remaining_args = parser.parse_known_args()
    if args.json:
        args.all_output = True

    if {"-h", "--help", "--h"}.intersection(sys.argv):
        print("Custom Test Runner Help:")
        parser.print_help()
        print("\nStandard Unittest Help:")
    else:
        fuchsia_dir = get_fuchsia_dir()
        output_dir_str = subprocess.check_output(
            ["scripts/fx", "get-build-dir"], cwd=fuchsia_dir, text=True
        ).strip()
        output_dir = pathlib.Path(output_dir_str)
        if not output_dir.is_absolute():
            output_dir = fuchsia_dir / output_dir

        # Run `fx gn desc` to see if the target exists. If not, exit early.
        target = "//src/starnix/tests/selinux/userspace:tests"
        gn_desc = subprocess.run(
            ["scripts/fx", "gn", "desc", str(output_dir), target],
            cwd=fuchsia_dir,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        if gn_desc.returncode != 0:
            print(f"ERROR: {target} is not part of the build graph.")
            sys.exit(1)

        print(f"Re-building {target}...")
        subprocess.run(
            [
                "scripts/fx",
                "build",
                target,
            ],
            check=True,
            cwd=fuchsia_dir,
        )
        # To match the behavior of when the script is invoked by `fx test`,
        # change the CWD to the output directory.
        os.chdir(output_dir)

        populate_dynamic_tests()

    TestSestarnixUserspaceOnLinux.args = args
    unittest.main(argv=[sys.argv[0]] + remaining_args, verbosity=2)

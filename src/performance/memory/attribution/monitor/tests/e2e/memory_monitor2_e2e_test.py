# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tests `ffx profile memory component` integration with `memory_monitor2` and attribution
principals. Also verifies other protocol exposed by `memory_monitor2`

The test it verifies the features are availability, but does not verify the data.
"""
import json
import re
from pathlib import Path
from typing import Any

import fuchsia_base_test
from honeydew.transports.ffx import errors as ffx_errors
from honeydew.transports.ffx import types as ffx_types
from mobly import asserts, test_runner


def assertContainsRegex(reg_str: str, content: str) -> None:
    asserts.assert_true(
        re.search(reg_str, content),
        msg=f"The text content (len={len(content)}) does not contain any occurrence of regex: {reg_str}\n"
        f"content[:256] = {content[:256]}",
    )


class MemoryMonitor2EndToEndTest(fuchsia_base_test.FuchsiaBaseTest):
    async def setup_class(self) -> None:
        """setup_class is called once before running tests."""
        await super().setup_class()

    def write_output(self, cmd_output: str, filename: str) -> None:
        """Writes the command output to a dedicated file for investigation."""
        with open(
            Path(self.test_case_path) / filename,
            "wt",
        ) as out:
            out.write(cmd_output)

    def test_memory_monitor2_does_not_log_error(self) -> None:
        errors = self.dut.ffx.run(
            [
                "log",
                "--symbolize",
                "off",
                "--severity",
                "error",
                "--component",
                "memory_monitor2",
                "dump",
            ],
            machine=ffx_types.MachineFormat.RAW,
        )
        asserts.assert_equal("", errors.strip())

    def test_ffx_profile_memory_component_without_args(self) -> None:
        # This test parses the "raw" output of the ffx command. It probably
        # should be ported to use the JSON output.
        profile = self.dut.ffx.run(
            [
                "-c",
                "ffx_profile_memory_components=true",
                "profile",
                "memory",
                "components",
            ],
            log_output=False,
            machine=ffx_types.MachineFormat.RAW,
        )
        self.write_output(profile, "profile_memory_components.txt")

        # Verifies that some data is produced.
        assertContainsRegex(r"(?m)^Total memory: \d+\.\d+ MiB$", profile)
        assertContainsRegex(r"(?m)^Kernel: +\d+\.\d+ MiB$", profile)
        assertContainsRegex(
            r"(?m)^\s*Processes:\s*memory_monitor2\.cm \(\d+\)\s*$", profile
        )
        assertContainsRegex(
            r"(?m)^\s*Memory stalls \(full\): \d+(\.\d+)? .?s\s*$", profile
        )
        assertContainsRegex(r"(?m)^\s*Page refaults: \d+(\.\d+)?\s*$", profile)

    def test_ffx_profile_memory_component_stdin_cycle(self) -> None:
        debug_json = self.dut.ffx.run(
            [
                "-c",
                "ffx_profile_memory_components=true",
                "profile",
                "memory",
                "components",
                "--debug-json",
            ],
            log_output=False,
        )
        import subprocess

        process = self.dut.ffx.popen(
            [
                "-c",
                "ffx_profile_memory_components=true",
                "profile",
                "memory",
                "components",
                "--stdin-input",
                "--debug-json",
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        stdout_data, stderr_data = process.communicate(input=debug_json)
        asserts.assert_equal(debug_json.strip(), stdout_data.strip())
        asserts.assert_equal("", stderr_data.strip())

    def test_ffx_profile_memory_component_with_machine_json_output(
        self,
    ) -> None:
        cmd_output = self.dut.ffx.run(
            [
                "-c",
                "ffx_profile_memory_components=true",
                "--machine",
                "json-pretty",
                "profile",
                "memory",
                "components",
            ],
            log_output=False,
        )
        self.write_output(
            cmd_output, "profile_memory_components_machine_json.json"
        )

        profile = json.loads(cmd_output)
        asserts.assert_in("Summary", set(profile))

        (mm2,) = [
            p
            for p in profile["Summary"]["principals"]
            if p["name"] == "core/memory_monitor2"
        ]
        asserts.assert_in("processes", mm2)
        asserts.assert_in("vmos", mm2)

    def test_memory_monitor_unnamed_vmos_should_not_increase(self) -> None:
        # The number of unnamed VMOs should not increase.
        # If this test breaks because there are more unnamed VMOs, you should fix your code to
        # always name your VMOs, as this greatly helps analyzing memory in general.
        # If this test breaks because there are less unnamed VMOs, this is great! Change the test to
        # check the new, lower number and send us the change for review.

        cmd_output = self.dut.ffx.run(
            [
                "-c",
                "ffx_profile_memory_components=true",
                "--machine",
                "json-pretty",
                "profile",
                "memory",
                "components",
            ],
            log_output=False,
        )

        profile = json.loads(cmd_output)
        (mm2,) = [
            p
            for p in profile["Summary"]["principals"]
            if p["name"] == "core/memory_monitor2"
        ]

        asserts.assert_in("vmos", mm2)

        # 2 is the current number of unnamed VMOs in memory_monitor2.
        asserts.assert_in("[unnamed]", mm2["vmos"])
        asserts.assert_in("count", mm2["vmos"]["[unnamed]"])
        asserts.assert_equal(2, mm2["vmos"]["[unnamed]"]["count"])

    def test_ffx_profile_memory_component_with_debug_json_output(self) -> None:
        cmd_output = self.dut.ffx.run(
            [
                "-c",
                "ffx_profile_memory_components=true",
                "profile",
                "memory",
                "components",
                "--debug-json",
            ],
            log_output=False,
        )
        self.write_output(
            cmd_output, "profile_memory_components_debug_json.json"
        )

        profile = json.loads(cmd_output)
        asserts.assert_in("bucket_definitions", set(profile.keys()))
        asserts.assert_greater(len(profile["bucket_definitions"]), 0)

    def test_memory_monitor2_inspect(self) -> None:
        inspect_json = self.dut.ffx.run(
            [
                "--machine",
                "json-pretty",
                "inspect",
                "show",
                "core/memory_monitor2",
            ]
        )
        self.write_output(inspect_json, "inspect_show.json")

        inspect_list = json.loads(inspect_json)
        (only_entry,) = inspect_list

        # Verifies that some data is produced.
        # There should be at least one assertion per lazy node to detect failures and timeouts.
        asserts.assert_equal(only_entry["moniker"], "core/memory_monitor2")

        self.assert_inspect_payload_is_valid(only_entry["payload"])

    def test_memory_monitor2_inspect2(self) -> None:
        inspect_col = self.dut.get_inspect_data(
            monikers=["core/memory_monitor2"]
        )
        (only_entry,) = inspect_col.data
        # Verifies that some data is produced.
        # There should be at least one assertion per lazy node to detect failures and timeouts.
        asserts.assert_equal(only_entry.moniker, "core/memory_monitor2")
        if only_entry.payload is None:
            raise AssertionError("Payload should not be none")

        self.write_output(
            json.dumps(only_entry.payload), "inspect_payload.json"
        )
        self.assert_inspect_payload_is_valid(only_entry.payload)

    @staticmethod
    def assert_inspect_payload_is_valid(payload: dict[str, Any]) -> None:
        root = payload["root"]

        asserts.assert_in("kmem_stats", root)
        asserts.assert_in("total_heap_bytes", root["kmem_stats"])

        asserts.assert_in("kmem_stats_compression", root)
        asserts.assert_in(
            "compressed_fragmentation_bytes", root["kmem_stats_compression"]
        )

        asserts.assert_in("logger", root)
        # Do not test for buckets. The node is absent when no capture occurred yet.
        asserts.assert_in("measurements", root["logger"])

        asserts.assert_in("stalls", root)
        asserts.assert_in("full_ms", root["stalls"])
        asserts.assert_in("some_ms", root["stalls"])

        asserts.assert_in("task health", root)
        for k, v in root["task health"].items():
            asserts.assert_equal(v, "ok", msg=f"task health {k} is not ok")

    def test_profile_memory_with_monitor2_report(self) -> None:
        # This test parses the "raw" output of the ffx command. It probably
        # should be ported to use the JSON output.
        profile = self.dut.ffx.run(
            [
                "profile",
                "memory",
                "--backend",
                "memory_monitor_2",
            ],
            log_output=False,
            machine=ffx_types.MachineFormat.RAW,
        )
        # Verifies that the report comes from memory_monitor2.
        assertContainsRegex(r"(?m)^ Principal name:", profile)

    def test_ffx_profile_memory_with_json_output(self) -> None:
        cmd_output = self.dut.ffx.run(
            [
                "--machine",
                "json-pretty",
                "profile",
                "memory",
                "--backend",
                "memory_monitor_2",
            ],
            log_output=False,
        )
        self.write_output(cmd_output, "profile_memory.json")
        # Remove `Resource %d not found` line from the output.
        # TODO(b/409272413): simplify this code when stdio and stderr are no longer aggregated.
        cmd_output = "\n".join(
            l for l in cmd_output.split("\n") if not l.startswith("Resource ")
        )
        # Assert that this is a ComponentDigest
        profile = json.loads(cmd_output)["ComponentDigest"]
        asserts.assert_in("digest", set(profile.keys()))

        # Assert that is has a principal for memory monitor 2.
        (principal,) = [
            p
            for p in profile["principals"]
            if p["name"] == "core/memory_monitor2"
        ]
        asserts.assert_in("processes", principal)
        asserts.assert_in("vmos", principal)

    def test_profile_memory_with_monitor2_incompatible_args(self) -> None:
        INCOMPATIBLE_ARGS_LIST: list[list[str]] = [
            ["--process-koids", "123"],
            ["--process-names", "123"],
            ["--interval", "123"],
            ["--undigested"],
            ["--exact-sizes"],
        ]
        for incompatible_args in INCOMPATIBLE_ARGS_LIST:
            with asserts.assert_raises(ffx_errors.FfxCommandError):
                self.dut.ffx.run(
                    [
                        "profile",
                        "memory",
                        "--backend",
                        "memory_monitor_2",
                    ]
                    + incompatible_args,
                    log_output=False,
                )

    def test_memory_monitor_abridged_snapshot(self) -> None:
        # The fast path should produce an output similar to the normal path.
        profile = self.dut.ffx.run(
            [
                "-c",
                "ffx_profile_memory_components=true",
                "profile",
                "memory",
                "components",
                "--abridged",
            ],
            log_output=False,
            machine=ffx_types.MachineFormat.RAW,
        )

        self.write_output(profile, "profile_memory_components_fast.txt")

        # Verifies that some data is produced.
        assertContainsRegex(r"(?m)^Total memory: \d+\.\d+ MiB$", profile)
        assertContainsRegex(r"(?m)^Kernel: +\d+\.\d+ MiB$", profile)
        assertContainsRegex(
            r"(?m)^\s*Processes:\s*memory_monitor2\.cm \(\d+\)\s*$", profile
        )
        assertContainsRegex(
            r"(?m)^\s*Memory stalls \(full\): \d+(\.\d+)? .?s\s*$", profile
        )
        assertContainsRegex(r"(?m)^\s*Page refaults: \d+(\.\d+)?\s*$", profile)


if __name__ == "__main__":
    test_runner.main()

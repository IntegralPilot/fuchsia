#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""End-to-end benchmark test for ffx speedtest socket throughput."""

import json
import os
from typing import Any

import fuchsia_base_test
import perf_publish.publish as publish
import test_data
from honeydew.transports.ffx.types import MachineFormat
from mobly import asserts, test_runner


class FfxSpeedtestBenchmark(fuchsia_base_test.FuchsiaBaseTest):
    """Performance benchmark for ffx speedtest socket transport."""

    async def setup_class(self) -> None:
        await super().setup_class()
        self._fuchsiaperf_records: list[dict[str, Any]] = []

    async def teardown_class(self) -> None:
        results_file = os.path.join(self.log_path, "speedtest.fuchsiaperf.json")
        with open(results_file, "w") as f:
            json.dump(self._fuchsiaperf_records, f, indent=4)

        publish.publish_fuchsiaperf(
            [results_file],
            "fuchsia.ffx.speedtest.txt",
            test_data_module=test_data,
        )
        await super().teardown_class()

    def test_socket_throughput_pull_streaming(self) -> None:
        """Measures socket pull throughput (device to host) in streaming mode with 4096 KB buffer."""
        cmd = [
            "speedtest",
            "--repeat",
            "1",
            "socket",
            "--transfer-mb",
            "100",
            "--buffer-kb",
            "4096",
            "--rx",
        ]
        output = self.dut.ffx.run(cmd, machine=MachineFormat.JSON)
        values = self._parse_throughput_values(output)
        asserts.assert_equal(
            len(values),
            1,
            f"Expected 1 pull streaming throughput values, found {len(values)}:\n{output}",
        )

        self._fuchsiaperf_records.append(
            {
                "test_suite": "fuchsia.ffx.speedtest",
                "label": "SocketThroughput_Pull_Streaming",
                "values": values,
                "unit": "bytesPerSecond",
            }
        )

    def test_socket_throughput_pull_individual_reads(self) -> None:
        """Measures socket pull throughput (device to host) in individual reads mode with 4096 KB buffer."""
        cmd = [
            "speedtest",
            "--repeat",
            "1",
            "socket",
            "--transfer-mb",
            "100",
            "--buffer-kb",
            "4096",
            "--rx",
            "--fdomain-individual-reads",
        ]
        output = self.dut.ffx.run(cmd, machine=MachineFormat.JSON)
        values = self._parse_throughput_values(output)
        asserts.assert_equal(
            len(values),
            1,
            f"Expected 1 pull individual reads throughput values, found {len(values)}:\n{output}",
        )

        self._fuchsiaperf_records.append(
            {
                "test_suite": "fuchsia.ffx.speedtest",
                "label": "SocketThroughput_Pull_IndividualReads",
                "values": values,
                "unit": "bytesPerSecond",
            }
        )

    def test_socket_throughput_push_streaming(self) -> None:
        """Measures socket push throughput (host to device) in streaming mode with 4096 KB buffer."""
        cmd = [
            "speedtest",
            "--repeat",
            "1",
            "socket",
            "--transfer-mb",
            "100",
            "--buffer-kb",
            "4096",
        ]
        output = self.dut.ffx.run(cmd, machine=MachineFormat.JSON)
        values = self._parse_throughput_values(output)
        asserts.assert_equal(
            len(values),
            1,
            f"Expected 1 push streaming throughput values, found {len(values)}:\n{output}",
        )

        self._fuchsiaperf_records.append(
            {
                "test_suite": "fuchsia.ffx.speedtest",
                "label": "SocketThroughput_Push_Streaming",
                "values": values,
                "unit": "bytesPerSecond",
            }
        )

    def test_socket_throughput_push_individual_reads(self) -> None:
        """Measures socket push throughput (host to device) in individual reads mode with 4096 KB buffer."""
        cmd = [
            "speedtest",
            "--repeat",
            "1",
            "socket",
            "--transfer-mb",
            "100",
            "--buffer-kb",
            "4096",
            "--fdomain-individual-reads",
        ]
        output = self.dut.ffx.run(cmd, machine=MachineFormat.JSON)
        values = self._parse_throughput_values(output)
        asserts.assert_equal(
            len(values),
            1,
            f"Expected 1 push individual reads throughput values, found {len(values)}:\n{output}",
        )

        self._fuchsiaperf_records.append(
            {
                "test_suite": "fuchsia.ffx.speedtest",
                "label": "SocketThroughput_Push_IndividualReads",
                "values": values,
                "unit": "bytesPerSecond",
            }
        )

    def _parse_throughput_values(self, output: str) -> list[float]:
        """Parses receiver throughput values in bytes/sec from speedtest JSON output."""
        reports = json.loads(output)
        values = []
        for report in reports:
            direction = report.get("direction")
            if direction == "tx":
                throughput_bps = report["server"]["throughput"]
            else:
                throughput_bps = report["client"]["throughput"]
            values.append(throughput_bps / 8.0)
        return values


if __name__ == "__main__":
    test_runner.main()

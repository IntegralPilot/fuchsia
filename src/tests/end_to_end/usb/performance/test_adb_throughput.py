# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""ADB Throughput Performance Test."""

import logging
import os
import pathlib
import re
import tempfile
import typing

import fuchsia_base_test
import perf_publish.publish as publish
import test_data
from mobly import asserts, signals, test_runner
from reporting import metrics
from trace_processing import trace_importing
from trace_processing.metrics import cpu

_LOGGER: logging.Logger = logging.getLogger(__name__)

# Defaults if not overridden by test params in BUILD.gn:
_DEFAULT_RAM_FILE_SIZE_BYTES: int = 50_000_000
_DEFAULT_STORAGE_FILE_SIZE_BYTES: int = 50_000_000

_DEVICE_TMP_FILE_PATH: str = "/tmp/adb_perf_test.bin"
_DEVICE_STORAGE_FILE_PATH: str = "/data/local/tmp/adb_perf_storage.bin"


class AdbThroughputTest(fuchsia_base_test.FuchsiaBaseTest):
    """Measures ADB push and pull throughput over USB using both storage-isolated and storage-backed transfers."""

    async def setup_class(self) -> None:
        """Reads build-time test parameters for transfer sizes and checks ADB support."""
        await super().setup_class()

        if not await self.dut.adb.is_supported():
            raise signals.TestAbortClass("ADB is not supported on this device")

        _LOGGER.debug(f"Device serial number: {await self.dut.serial_number()}")

        # Configurable transfer sizes from test parameters in BUILD.gn
        self._ram_file_size_bytes = int(
            self.user_params.get(
                "transfer_file_size_bytes", _DEFAULT_RAM_FILE_SIZE_BYTES
            )
        )
        self._storage_file_size_bytes = int(
            self.user_params.get(
                "storage_transfer_file_size_bytes",
                _DEFAULT_STORAGE_FILE_SIZE_BYTES,
            )
        )
        _LOGGER.debug(
            f"Configured transfer sizes: RAM={self._ram_file_size_bytes} bytes "
            f"({self._ram_file_size_bytes / (1024 * 1024):.1f} MB), "
            f"Storage={self._storage_file_size_bytes} bytes "
            f"({self._storage_file_size_bytes / (1024 * 1024):.1f} MB)"
        )

    def _create_temp_file_in_ram(
        self, size_bytes: int = _DEFAULT_RAM_FILE_SIZE_BYTES
    ) -> typing.IO[bytes]:
        """Creates a temporary file of specified size on host RAM disk (/dev/shm)."""
        ram_dir = "/dev/shm"
        if not (os.path.exists(ram_dir) and os.access(ram_dir, os.W_OK)):
            raise signals.TestError(
                f"Host RAM disk ({ram_dir}) is not available or writable. "
                "Cannot run storage-isolated benchmark."
            )
        f = tempfile.NamedTemporaryFile(dir=ram_dir)
        f.truncate(size_bytes)
        f.flush()
        f.seek(0)
        return f

    async def _run_transfer_benchmark(
        self,
        command: str,
        source_path: str,
        dest_path: str,
        metric_suite: str,
    ) -> None:
        """Executes ADB transfer command with tracing, extracts metrics, and publishes results."""
        trace_file = f"{metric_suite}_trace.fxt"
        trace_path = os.path.join(self.test_case_path, trace_file)

        async with self.dut.tracing.trace_session(
            categories=["system_metrics"],
            buffer_size=32,
            download=True,
            directory=self.test_case_path,
            trace_file=trace_file,
        ):
            output = await self.dut.adb.run(
                [command, "-Z", source_path, dest_path]
            )
            _LOGGER.info(
                f"Output from `adb {command}` ({metric_suite}): {output}"
            )

        assert output is not None, "ADB command produced no output."

        test_case_results = self._extract_metrics(output)

        processor = cpu.CpuMetricsProcessor(aggregates_only=True)
        model = trace_importing.create_model_from_trace_file_path(
            trace_path, patterns=processor.event_patterns
        )
        test_case_results.extend(processor.process_metrics(model))

        results_file = (
            pathlib.Path(self.test_case_path)
            / f"{metric_suite}.fuchsiaperf.json"
        )
        metrics.TestCaseResult.write_fuchsiaperf_json(
            results=test_case_results,
            test_suite=metric_suite,
            output_path=results_file,
        )

        publish.publish_fuchsiaperf(
            [results_file],
            f"{metric_suite}.txt",
            test_data_module=test_data,
        )

    def _extract_metrics(self, adb_output: str) -> list[metrics.TestCaseResult]:
        """Extracts wall-time and transfer rate from ADB transfer output."""
        match = re.search(r"([0-9]+) bytes in ([0-9.]+)s", adb_output)
        assert (
            match is not None
        ), f"Failed to find proper transfer output in: {adb_output}"
        bytes_transferred = int(match.group(1))
        duration_seconds = float(match.group(2))
        asserts.assert_greater(
            duration_seconds, 0, "Transfer duration must be greater than zero."
        )

        rate = bytes_transferred / duration_seconds
        wall_time_ms = duration_seconds * 1000.0

        return [
            metrics.TestCaseResult(
                "Rate",
                metrics.Unit.bytesPerSecond,
                [rate],
                "Data transfer speed",
            ),
            metrics.TestCaseResult(
                "WallTime",
                metrics.Unit.milliseconds,
                [wall_time_ms],
                f"Wallclock time to transfer {bytes_transferred} bytes",
            ),
        ]

    async def test_adb_push(self) -> None:
        """Pushes an uncompressed buffer from host RAM (/dev/shm) to /dev/null on device."""
        with self._create_temp_file_in_ram(
            self._ram_file_size_bytes
        ) as local_ram_f:
            await self._run_transfer_benchmark(
                command="push",
                source_path=local_ram_f.name,
                dest_path="/dev/null",
                metric_suite="fuchsia.usb.adb.push",
            )

    async def test_adb_pull(self) -> None:
        """Pulls an uncompressed buffer from device RAM disk (/tmp) to /dev/null on host."""
        with self._create_temp_file_in_ram(
            self._ram_file_size_bytes
        ) as local_ram_f:
            try:
                await self.dut.adb.run(
                    ["push", "-Z", local_ram_f.name, _DEVICE_TMP_FILE_PATH]
                )
                await self._run_transfer_benchmark(
                    command="pull",
                    source_path=_DEVICE_TMP_FILE_PATH,
                    dest_path="/dev/null",
                    metric_suite="fuchsia.usb.adb.pull",
                )
            finally:
                await self.dut.adb.run(
                    ["shell", "rm", "-f", _DEVICE_TMP_FILE_PATH]
                )

    async def test_adb_push_storage(self) -> None:
        """Pushes an uncompressed file from host disk to device flash storage."""
        with tempfile.NamedTemporaryFile() as local_disk_f:
            local_disk_f.truncate(self._storage_file_size_bytes)
            local_disk_f.flush()
            try:
                await self._run_transfer_benchmark(
                    command="push",
                    source_path=local_disk_f.name,
                    dest_path=_DEVICE_STORAGE_FILE_PATH,
                    metric_suite="fuchsia.usb.adb.push_storage",
                )
            finally:
                await self.dut.adb.run(
                    [
                        "shell",
                        "rm",
                        "-f",
                        _DEVICE_STORAGE_FILE_PATH,
                    ]
                )

    async def test_adb_pull_storage(self) -> None:
        """Pulls an uncompressed file from device flash storage to host disk."""
        with (
            tempfile.NamedTemporaryFile() as local_disk_f,
            tempfile.NamedTemporaryFile() as local_pull_f,
        ):
            local_disk_f.truncate(self._storage_file_size_bytes)
            local_disk_f.flush()
            try:
                await self.dut.adb.run(
                    [
                        "push",
                        "-Z",
                        local_disk_f.name,
                        _DEVICE_STORAGE_FILE_PATH,
                    ]
                )
                await self._run_transfer_benchmark(
                    command="pull",
                    source_path=_DEVICE_STORAGE_FILE_PATH,
                    dest_path=local_pull_f.name,
                    metric_suite="fuchsia.usb.adb.pull_storage",
                )
            finally:
                await self.dut.adb.run(
                    [
                        "shell",
                        "rm",
                        "-f",
                        _DEVICE_STORAGE_FILE_PATH,
                    ]
                )


if __name__ == "__main__":
    test_runner.main()

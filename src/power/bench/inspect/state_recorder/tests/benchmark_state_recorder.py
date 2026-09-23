# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import json
import logging
import os
import signal
from typing import Any

import fuchsia_base_test
import perf_publish.publish as publish
import test_data
from honeydew.transports.ffx.types import MachineFormat
from memory import profile
from mobly import test_runner

_LOGGER = logging.getLogger(__name__)
TEST_MONIKER = "fuchsia-pkg://fuchsia.com/state_recorder_bench#meta/state_recorder_bench.cm"


class StateRecorderMemoryBenchmarkTest(fuchsia_base_test.FuchsiaBaseTest):
    async def setup_class(self) -> None:
        """Initialize all DUT(s)."""
        await super().setup_class()

    async def _run_benchmark(
        self,
        capacity: int,
        entries: int,
        lazy_record: bool,
        metric_group: str,
    ) -> list[dict[str, Any]]:
        """Run the benchmark for state recorder."""
        _LOGGER.info(
            "Running state recorder memory benchmark for %s...", metric_group
        )

        cmd = self.dut.ffx.generate_ffx_cmd(
            cmd=[
                "test",
                "run",
                TEST_MONIKER,
                "--realm",
                "/core/testing/system-tests",
                "--",
                "--capacity",
                str(capacity),
                "--entries",
                str(entries),
            ]
            + (["--lazy-record"] if lazy_record else []),
            include_target=True,
            machine=MachineFormat.RAW,
        )
        _LOGGER.info("Running command: %s", " ".join(cmd))

        process = await asyncio.create_subprocess_exec(
            *cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.STDOUT,
            preexec_fn=os.setsid,
        )

        try:
            assert process.stdout is not None

            while True:
                line = await process.stdout.readline()
                if not line:
                    break
                line_str = line.decode("utf-8").strip()
                _LOGGER.info("Test output: %s", line_str)
                if "WAITING FOR MEMORY PROFILING" in line_str:
                    _LOGGER.info(
                        "Test component is waiting for memory profiling"
                    )
                    break

            report = profile.capture(
                dut=self.dut,
                buckets_metrics=".*",
            )

            freeform: Any = report.freeform
            profile_data = freeform.get("memory_profile", {})
            principals = profile_data.get("principals", [])

            target_principals = [
                p
                for p in principals
                if "state_recorder_benchmark_worker" in p.get("name", "")
            ]

            if len(target_principals) != 1:
                _LOGGER.error(
                    "Expected exactly 1 state_recorder_benchmark_worker in memory profile, found %d",
                    len(target_principals),
                )
                raise RuntimeError(
                    f"Expected exactly 1 state_recorder_benchmark_worker in memory profile, found {len(target_principals)}"
                )

            target_principal = target_principals[0]

            metrics = []
            inspect_heap_populated = 0
            inspect_heap_vmo = None
            for vmo in target_principal.get("vmos", []):
                if vmo.get("name") == "InspectHeap":
                    inspect_heap_vmo = vmo
                    break

            if inspect_heap_vmo:
                if "populated_private" not in inspect_heap_vmo:
                    raise KeyError(
                        "'populated_private' not found in InspectHeap VMO for state_recorder_benchmark_worker"
                    )
                inspect_heap_populated = inspect_heap_vmo["populated_private"]
                _LOGGER.info(
                    "%s InspectHeap Populated: %s bytes",
                    metric_group,
                    inspect_heap_populated,
                )
            else:
                _LOGGER.warning(
                    "InspectHeap VMO not found for state_recorder_benchmark_worker"
                )

            if "populated_private" not in target_principal:
                raise KeyError(
                    "'populated_private' not found in target principal for state_recorder_benchmark_worker"
                )
            total_private = target_principal["populated_private"]
            _LOGGER.info(
                "%s Total Private Memory: %s bytes",
                metric_group,
                total_private,
            )

            # Each recorded entry consists of an 8-byte timestamp and a 4-byte state value (u32),
            # totaling 12 bytes of raw data per entry.
            raw_data_size = entries * 12
            metrics = [
                {
                    "test_suite": "fuchsia.power.state_recorder",
                    "label": f"Memory/{metric_group}/{entries}Entries/InspectHeap",
                    "values": [inspect_heap_populated],
                    "unit": "bytes",
                },
                {
                    "test_suite": "fuchsia.power.state_recorder",
                    "label": f"Memory/{metric_group}/{entries}Entries/ActualDataSize",
                    "values": [raw_data_size],
                    "unit": "bytes",
                },
                {
                    "test_suite": "fuchsia.power.state_recorder",
                    "label": f"Memory/{metric_group}/{entries}Entries/TotalPrivate",
                    "values": [total_private],
                    "unit": "bytes",
                },
            ]

            return metrics
        finally:
            _LOGGER.info("Sending SIGINT to process group")
            try:
                os.killpg(process.pid, signal.SIGINT)
            except ProcessLookupError:
                pass
            await process.wait()

    async def test_memory_benchmarks(self) -> None:
        results = []

        # Run eager mode benchmarks for implementation
        for size in [100, 200, 400, 800, 1600]:
            results.extend(
                await self._run_benchmark(
                    capacity=size,
                    entries=size,
                    lazy_record=False,
                    metric_group="Eager",
                )
            )

        test_perf_file = os.path.join(self.log_path, "test.fuchsiaperf.json")
        with open(test_perf_file, "w") as f:
            json.dump(results, f, indent=4)

        _LOGGER.info(
            "Generated state recorder combined fuchsiaperf data:\n%s",
            json.dumps(results, indent=4),
        )

        publish.publish_fuchsiaperf(
            [test_perf_file],
            "fuchsia.power.state_recorder.txt",
            test_data_module=test_data,
        )


if __name__ == "__main__":
    test_runner.main()

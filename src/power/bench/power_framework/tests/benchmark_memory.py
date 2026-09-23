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
TEST_MONIKER = "fuchsia-pkg://fuchsia.com/power-framework-bench-integration-tests#meta/integration.cm"


class PowerMemoryBenchmarkTest(fuchsia_base_test.FuchsiaBaseTest):
    async def setup_class(self) -> None:
        """Initialize all DUT(s)."""
        await super().setup_class()

    async def _run_benchmark(
        self,
        test_filter: str,
        principal_name: str,
        metric_group: str,
        repeat: int,
    ) -> list[dict[str, Any]]:
        """Run the benchmark specified by `test_filter`.

        `principal_name` is the name of the principal to look for in the memory profile.
        `metric_group` is the name of the metric group for the measured component.
        """
        _LOGGER.info("Running memory benchmark for %s...", metric_group)
        cmd = self.dut.ffx.generate_ffx_cmd(
            cmd=[
                "test",
                "run",
                TEST_MONIKER,
                "--realm",
                "/core/testing/system-tests",
                "--test-filter",
                test_filter,
                "--",
                "--repeat",
                str(repeat),
                "--timeout-secs",
                "60",
                "--wait-for-memory-profiling",
            ],
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

        assert process.stdout is not None

        while True:
            line = await process.stdout.readline()
            if not line:
                break
            line_str = line.decode("utf-8").strip()
            _LOGGER.info("Test output: %s", line_str)
            if "WAITING FOR MEMORY PROFILING" in line_str:
                _LOGGER.info("Test component is waiting for memory profiling")
                break

        report = profile.capture(
            dut=self.dut,
            buckets_metrics=".*",
        )

        freeform: Any = report.freeform
        profile_data = freeform.get("memory_profile", {})
        principals = profile_data.get("principals", [])

        target_principal = None
        for p in principals:
            if principal_name in p.get("name", ""):
                target_principal = p
                break

        metrics = []
        if target_principal:
            if "populated_private" not in target_principal:
                raise KeyError(
                    f"'populated_private' not found in principal {principal_name}"
                )
            overall_private_populated = target_principal["populated_private"]
            _LOGGER.info(
                "%s Total Populated: %s bytes",
                principal_name,
                overall_private_populated,
            )

            inspect_heap_populated = 0
            inspect_heap_vmo = None
            for vmo in target_principal.get("vmos", []):
                if vmo.get("name") == "InspectHeap":
                    inspect_heap_vmo = vmo
                    break

            if inspect_heap_vmo:
                if "populated_private" not in inspect_heap_vmo:
                    raise KeyError(
                        f"'populated_private' not found in InspectHeap VMO for {principal_name}"
                    )
                inspect_heap_populated = inspect_heap_vmo["populated_private"]
                _LOGGER.info(
                    "%s InspectHeap Populated: %s bytes",
                    principal_name,
                    inspect_heap_populated,
                )
            else:
                _LOGGER.warning(
                    "InspectHeap VMO not found for %s", principal_name
                )

            metrics = [
                {
                    "test_suite": "fuchsia.power.framework",
                    "label": f"Memory/{metric_group}/PrivatePopulated",
                    "values": [overall_private_populated],
                    "unit": "bytes",
                },
                {
                    "test_suite": "fuchsia.power.framework",
                    "label": f"Memory/{metric_group}/PrivatePopulated/InspectHeap",
                    "values": [inspect_heap_populated],
                    "unit": "bytes",
                },
            ]
        else:
            _LOGGER.error("%s not found in memory profile", principal_name)
            raise RuntimeError(f"{principal_name} not found in memory profile")

        # Kill the `ffx test` invocation.
        _LOGGER.info("Sending SIGINT to process group")
        os.killpg(process.pid, signal.SIGINT)
        await process.wait()

        return metrics

    async def test_memory_benchmarks(self) -> None:
        results = []

        # Run SAG benchmark.
        results.extend(
            await self._run_benchmark(
                test_filter="*test_sag_takewakelease",
                principal_name="test-system-activity-governor",
                metric_group="SAG",
                repeat=10000,
            )
        )

        # Run Power Broker benchmark.
        results.extend(
            await self._run_benchmark(
                test_filter="*test_large_topology_lease_benchmark",
                principal_name="test-power-broker",
                metric_group="PowerBroker",
                repeat=3000,
            )
        )

        # Publish combined metrics.
        test_perf_file = os.path.join(self.log_path, "test.fuchsiaperf.json")
        with open(test_perf_file, "w") as f:
            json.dump(results, f, indent=4)

        _LOGGER.info(
            "Generated combined fuchsiaperf data:\n%s",
            json.dumps(results, indent=4),
        )

        publish.publish_fuchsiaperf(
            [test_perf_file],
            "fuchsia.power.memory.txt",
            test_data_module=test_data,
        )


if __name__ == "__main__":
    test_runner.main()

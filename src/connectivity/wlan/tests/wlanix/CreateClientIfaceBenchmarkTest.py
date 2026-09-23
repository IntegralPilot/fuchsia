# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Benchmark test that creates and destroys a wlanix client interface repeatedly
to measure the time required for interface creation.
"""
import json
import logging
import os
import statistics

import fidl_fuchsia_wlan_wlanix as fidl_wlanix
import perf_publish.publish as publish
import test_data
from mobly import asserts, test_runner
from trace_processing import trace_importing, trace_model, trace_utils
from wlanix_testing import base_test

logger = logging.getLogger(__name__)

NUM_ITERATIONS = 20


class CreateClientIfaceBenchmarkTest(base_test.WifiChipBaseTestClass):
    async def _create_iface(self) -> str:
        proxy, server = self.dut.fuchsia_controller.channel_create()
        (
            await self.wifi_chip_proxy.create_sta_iface(iface=server.take())
        ).unwrap()
        wifi_sta_iface = fidl_wlanix.WifiStaIfaceClient(proxy)

        iface_name = (await wifi_sta_iface.get_name()).unwrap().iface_name
        assert iface_name is not None, "iface_name should not be None"
        return iface_name

    async def _destroy_iface(self, iface_name: str) -> None:
        (
            await self.wifi_chip_proxy.remove_sta_iface(iface_name=iface_name)
        ).unwrap()

        iface_names = (
            (await self.wifi_chip_proxy.get_sta_iface_names())
            .unwrap()
            .iface_names
        )
        assert iface_names is not None, "iface_names should not be None"
        asserts.assert_false(
            iface_name in iface_names,
            f"Iface {iface_name} still exists after removal",
        )

    async def setup_test(self) -> None:
        await super().setup_test()
        self.trace_file = "benchmark.fxt"
        self.trace_path = os.path.join(self.log_path, self.trace_file)
        self.iface_name: str | None = None

    async def teardown_test(self) -> None:
        if self.iface_name:
            logger.warning(
                f"Cleaning up lingering iface {self.iface_name} during teardown"
            )
            await self.wifi_chip_proxy.remove_sta_iface(
                iface_name=self.iface_name
            )
            self.iface_name = None

        if os.path.exists(self.trace_path):
            logger.info(f"Trace file generated at: {self.trace_path}")
        await super().teardown_test()

    def _log_summary_stats(
        self, durations_ms: list[float], metric_name: str
    ) -> None:
        if not durations_ms:
            logger.warning(f"No timing data collected for {metric_name}!")
            return

        avg_duration = statistics.mean(durations_ms)
        min_duration = min(durations_ms)
        max_duration = max(durations_ms)
        std_dev = (
            statistics.stdev(durations_ms) if len(durations_ms) > 1 else 0.0
        )

        logger.info(f"--- {metric_name} Performance Metrics ---")
        logger.info(f"Iterations: {len(durations_ms)}")
        logger.info(f"Average:    {avg_duration:.2f} ms")
        logger.info(f"Minimum:    {min_duration:.2f} ms")
        logger.info(f"Maximum:    {max_duration:.2f} ms")
        logger.info(f"Std Dev:    {std_dev:.2f} ms")
        logger.info("----------------------------------------------")

    def _get_event_durations(
        self, model: trace_model.Model, event_name: str
    ) -> list[float]:
        events = list(
            trace_utils.filter_events(
                model.all_events(),
                category="wlan",
                name=event_name,
                type=trace_model.DurationEvent,
            )
        )
        if len(events) < NUM_ITERATIONS:
            logger.warning(
                f"Expected {NUM_ITERATIONS} '{event_name}' events but only found {len(events)}."
            )

        durations_ms = []
        for event in events:
            if event.duration is not None:
                duration_ms = event.duration.to_microseconds_f() / 1000.0
                durations_ms.append(duration_ms)
            else:
                logger.warning(f"Found '{event_name}' event with no duration")
        return durations_ms

    async def test_create_destroy_client_iface(self) -> None:
        async with self.dut.tracing.trace_session(
            categories=["wlan"],
            download=True,
            directory=self.log_path,
            trace_file=self.trace_file,
        ):
            for i in range(NUM_ITERATIONS):
                logger.info(f"Iteration {i+1}: Creating client iface")
                self.iface_name = await self._create_iface()
                logger.info(
                    f"Iteration {i+1}: Created iface {self.iface_name}. Now destroying."
                )

                await self._destroy_iface(self.iface_name)

                logger.info(
                    f"Iteration {i+1}: Successfully destroyed iface {self.iface_name}"
                )
                self.iface_name = None

        model = trace_importing.create_model_from_trace_file_path(
            self.trace_path,
            patterns=set(["create_client_iface", "destroy_client_iface"]),
        )

        creation_durations_ms = self._get_event_durations(
            model, "create_client_iface"
        )
        destruction_durations_ms = self._get_event_durations(
            model, "destroy_client_iface"
        )

        if creation_durations_ms and destruction_durations_ms:
            # Publish fuchsiaperf data extracted from our trace file.
            fuchsiaperf_data = [
                {
                    "test_suite": "fuchsia.wlan.wlanix",
                    "label": "CreateClientIface",
                    "values": creation_durations_ms,
                    "unit": "ms",
                },
                {
                    "test_suite": "fuchsia.wlan.wlanix",
                    "label": "DestroyClientIface",
                    "values": destruction_durations_ms,
                    "unit": "ms",
                },
            ]
            test_perf_file = os.path.join(
                self.log_path, "test.fuchsiaperf.json"
            )
            with open(test_perf_file, "w") as f:
                json.dump(fuchsiaperf_data, f)

            print("Publishing fuchsiaperf data to fuchsia-perf")
            publish.publish_fuchsiaperf(
                [test_perf_file],
                "fuchsia.wlan.wlanix.txt",
                test_data_module=test_data,
                runtime_deps_dir=".",
            )

            # Log summary statistics for our current run.
            self._log_summary_stats(creation_durations_ms, "Interface Creation")
            self._log_summary_stats(
                destruction_durations_ms, "Interface Destruction"
            )
        else:
            logger.error("No timing data collected from trace!")
            asserts.fail("No timing data collected")


if __name__ == "__main__":
    test_runner.main()

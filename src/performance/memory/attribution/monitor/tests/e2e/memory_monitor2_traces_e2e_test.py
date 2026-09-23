# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tests `ffx profile memory component` integration with `memory_monitor2` and attribution
principals. Also verifies other protocol exposed by `memory_monitor2`

The test it verifies the features are availability, but does not verify the data.
"""
import asyncio
import json
import re
from pathlib import Path

import fuchsia_base_test
from mobly import asserts, test_runner
from trace_processing import trace_importing, trace_model, trace_utils


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

    def test_memory_monitor2_is_in_traces_provider(self) -> None:
        json_text = self.dut.ffx.run(
            ["--machine", "json-pretty", "trace", "list-providers"]
        )
        self.write_output(json.dumps(json_text), "trace_list-providers.json")
        providers = json.loads(json_text)
        asserts.assert_in(
            "memory_monitor2.cm", [prov["name"] for prov in providers]
        )

    async def test_memory_traces_content_collect(self) -> None:
        CATEGORY = "memory:kernel"
        EXPECTED_EVENTS = {
            "kmem_stats_a",
            "kmem_stats_b",
            "kmem_stats_compression",
            "kmem_stats_compression_time",
            "memory_stall",
        }
        trace_path = Path(self.test_case_path) / "trace.fxt"
        async with self.dut.tracing.trace_session(
            categories=[CATEGORY],
            download=True,
            directory=str(trace_path.parent),
            trace_file=trace_path.name,
        ):
            # Events are logged every seconds. It is not very nice to have to wait a given amount
            # of time. If that proves brittle, we should fallback on a larger value.
            await asyncio.sleep(4)

        model = trace_importing.create_model_from_trace_file_path(
            trace_path, patterns=EXPECTED_EVENTS
        )
        events = list(
            trace_utils.filter_events(
                model.all_events(),
                category=CATEGORY,
                type=trace_model.Event,
            )
        )
        asserts.assert_greater(len(events), 0)

        event_names = {event.name for event in events}
        asserts.assert_equal(event_names, EXPECTED_EVENTS)

        for event in events:
            if event.name == "memory_stall":
                asserts.assert_in("page_refaults", event.args.keys())


if __name__ == "__main__":
    test_runner.main()

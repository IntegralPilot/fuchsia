#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""E2E test for diagnostics functionality.

Test that asserts that we can read logs and Inspect.
"""

import json
import logging
from typing import Any, Dict, List

import fuchsia_base_test
from mobly import asserts, test_runner
from perf import action_timer

_LOGGER: logging.Logger = logging.getLogger(__name__)
_TEST_SUITE = "fuchsia.test.diagnostics"


class DiagnosticsTest(fuchsia_base_test.FuchsiaBaseTest):
    async def setup_class(self) -> None:
        await super().setup_class()
        self._repeat_count: int = self.user_params["repeat_count"]

    def test_inspect(self) -> None:
        """Validates that we can snapshot Inspect from the device."""
        with action_timer.timer(
            _TEST_SUITE, "Inspect", self.test_case_path
        ) as t:
            for _ in range(self._repeat_count):
                with t.record_iteration():
                    result = self.dut.ffx.run(
                        cmd=["--machine", "json", "inspect", "show"],
                        log_output=False,
                    )
                inspect_data: list[Any] = json.loads(result)
                asserts.assert_greater(len(inspect_data), 0)
                self._check_archivist_data(inspect_data)

    def _check_archivist_data(self, inspect_data: List[Dict[str, Any]]) -> None:
        """Find the Archivist's data, and assert that it's status is 'OK'."""
        archivist_only: List[Dict[str, Any]] = [
            data
            for data in inspect_data
            if data.get("moniker") == "bootstrap/archivist"
        ]
        asserts.assert_equal(
            len(archivist_only),
            1,
            "Expected to find one Archivist in the Inspect output.",
        )
        archivist_data = archivist_only[0]

        health = archivist_data["payload"]["root"]["fuchsia.inspect.Health"]
        asserts.assert_equal(
            health["status"],
            "OK",
            "Archivist did not return OK status",
        )

    def test_logs(self) -> None:
        """Validates that we can snapshot logs from the device."""
        with action_timer.timer(_TEST_SUITE, "Logs", self.test_case_path) as t:
            for _ in range(self._repeat_count):
                with t.record_iteration():
                    logger_output = self.dut.ffx.run(
                        cmd=[
                            "--machine",
                            "json",
                            "log",
                            "--symbolize",
                            "off",
                            "dump",
                        ],
                        log_output=False,
                    )
                asserts.assert_greater(len(logger_output), 0)


if __name__ == "__main__":
    test_runner.main()

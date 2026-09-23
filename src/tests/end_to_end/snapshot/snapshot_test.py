#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""E2E test for Fuchsia snapshot functionality

Test that Fuchsia snapshots include Inspect data for Archivist, and
that Archivist is OK.
"""

import json
import logging
import os
import tempfile
import zipfile
from typing import Any, Dict, List

import fuchsia_base_test
from mobly import asserts, test_runner
from perf import action_timer

_LOGGER: logging.Logger = logging.getLogger(__name__)
_SNAPSHOT_ZIP = "snapshot_test.zip"
_TEST_SUITE = "fuchsia.test.diagnostics"


class SnapshotTest(fuchsia_base_test.FuchsiaBaseTest):
    async def setup_class(self) -> None:
        await super().setup_class()
        self._repetitions = self.user_params["repeat_count"]

    async def test_snapshot(self) -> None:
        """Get a device snapshot and extract the inspect.json file."""
        with action_timer.timer(
            _TEST_SUITE, "Snapshot", self.test_case_path
        ) as t:
            for _ in range(self._repetitions):
                with t.record_iteration():
                    directory = tempfile.TemporaryDirectory()
                    try:
                        await self.dut.snapshot(directory.name, _SNAPSHOT_ZIP)
                        final_path = os.path.join(directory.name, _SNAPSHOT_ZIP)
                        with zipfile.ZipFile(final_path) as zf:
                            self._validate_inspect(zf)
                    finally:
                        directory.cleanup()

    def _validate_inspect(self, zf: zipfile.ZipFile) -> None:
        with zf.open("inspect.json") as inspect_file:
            inspect_data = json.load(inspect_file)
        asserts.assert_greater(len(inspect_data), 0)
        self._check_archivist_data(inspect_data)

    def _check_archivist_data(self, inspect_data: List[Dict[str, Any]]) -> None:
        # Find the Archivist's data, and assert that it's status is "OK"
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


if __name__ == "__main__":
    test_runner.main()

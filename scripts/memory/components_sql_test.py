# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests `ffx profile memory component` integration with `components_sql.py`
"""

import json
import os.path
import sqlite3

import components_sql
import fuchsia_base_test
from mobly import asserts, test_runner


class ComponentsSqlEndToEndTest(fuchsia_base_test.FuchsiaBaseTest):
    async def setup_class(self) -> None:
        """setup_class is called once before running tests."""
        await super().setup_class()
        self.dut.ffx.run(
            ["config", "set", "ffx_profile_memory_components", "true"]
        )

    async def test_components_sql_creates_db(self) -> None:
        detailed_profile = self.dut.ffx.run(
            [
                "--machine",
                "json",
                "profile",
                "memory",
                "components",
                "--detailed",
            ],
            log_output=False,
        )
        components_sql.process_json_input(
            json.loads(detailed_profile),
            os.path.join(self.test_case_path, "components_sql_test.db"),
        )

        db = sqlite3.connect(
            os.path.join(self.test_case_path, "components_sql_test.db")
        )
        cursor = db.cursor()
        cursor.execute("SELECT name FROM sqlite_master WHERE type='table';")
        tables = cursor.fetchall()
        asserts.assert_count_equal(
            [
                ("kernel_stats",),
                ("principals",),
                ("vmos",),
                ("resource_names",),
                ("resources",),
                ("principals_resources",),
                ("blob_annotations",),
            ],
            tables,
        )

        cursor.execute("SELECT * FROM principals;")
        principals = cursor.fetchall()
        asserts.assert_greater(len(principals), 0)

        cursor.execute("SELECT * FROM vmos;")
        vmos = cursor.fetchall()
        asserts.assert_greater(len(vmos), 0)

        cursor.execute("SELECT * FROM kernel_stats;")
        kernel_stats = cursor.fetchall()
        asserts.assert_greater(len(kernel_stats), 0)


if __name__ == "__main__":
    test_runner.main()

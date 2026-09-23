# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Driver restart stress test runner."""

import datetime
import logging

import driver_stress_lib
import fuchsia_base_test
from mobly import test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class DriverRestartStressTest(fuchsia_base_test.FuchsiaBaseTest):
    """Driver start/stop stress test runner.

    Attributes:
        dut: FuchsiaDevice object.

    Required Mobly Test Params:
        driver_url (str): Absolute component URL of the driver.
        iterations (int): Number of times restart test need to be executed.
        mechanism (str): "host_restart", "disable_enable", or "both". Default is "both".
        devfs_paths (list[str]): Optional list of relative devfs paths to track.
    """

    async def pre_run(self) -> None:
        """Mobly method used to generate the test cases at run time."""
        iterations = int(self.user_params.get("iterations", 1))
        test_args: list[tuple[int]] = []

        for iteration in range(1, iterations + 1):
            test_args.append((iteration,))

        mechanism = self.user_params.get("mechanism", "both")
        if mechanism in ("host_restart", "both"):
            self.generate_tests(
                test_logic=self._test_logic_restart,
                name_func=self._name_func_restart,
                arg_sets=test_args,
            )
        if mechanism in ("disable_enable", "both"):
            self.generate_tests(
                test_logic=self._test_logic_disable_enable,
                name_func=self._name_func_disable_enable,
                arg_sets=test_args,
            )

    async def setup_class(self) -> None:
        """setup_class is called once before running tests."""
        await super().setup_class()
        self.driver_url: str = self.user_params["driver_url"]
        self.mechanism: str = self.user_params.get("mechanism", "both")
        self.devfs_paths: list[str] = self.user_params.get("devfs_paths", [])
        component = self.driver_url.split("#")[-1].split("/")[-1]
        self.moniker: str = component.replace(".cm", "")

    async def _test_logic_restart(self, iteration: int) -> None:
        """Test case logic orchestrating teardown via driver restart and recovery checks."""
        start_time = datetime.datetime.now(datetime.timezone.utc).isoformat()
        _LOGGER.info("Starting driver restart stress iteration# %s", iteration)

        for path in self.devfs_paths:
            await driver_stress_lib.assert_devfs_presence(
                dut=self.dut, path=path, expected=True
            )

        _LOGGER.info("Restarting driver host for '%s'...", self.driver_url)
        self.dut.ffx.run(cmd=["driver", "restart", self.driver_url])

        await driver_stress_lib.verify_driver_loaded(
            dut=self.dut, driver_url=self.driver_url
        )

        for path in self.devfs_paths:
            await driver_stress_lib.assert_devfs_presence(
                dut=self.dut, path=path, expected=True
            )

        driver_stress_lib.audit_driver_crashes(
            dut=self.dut, moniker=self.moniker, start_time=start_time
        )

        _LOGGER.info(
            "Successfully finished driver restart stress iteration# %s",
            iteration,
        )

    async def _test_logic_disable_enable(self, iteration: int) -> None:
        """Test case logic orchestrating teardown via disable/enable and recovery checks."""
        start_time = datetime.datetime.now(datetime.timezone.utc).isoformat()
        _LOGGER.info(
            "Starting driver disable/enable stress iteration# %s", iteration
        )

        for path in self.devfs_paths:
            await driver_stress_lib.assert_devfs_presence(
                dut=self.dut, path=path, expected=True
            )

        _LOGGER.info("Disabling driver '%s'...", self.driver_url)
        self.dut.ffx.run(cmd=["driver", "disable", self.driver_url])

        for path in self.devfs_paths:
            await driver_stress_lib.assert_devfs_presence(
                dut=self.dut, path=path, expected=False
            )

        _LOGGER.info(
            "Registering driver '%s' to reverse unbinding...",
            self.driver_url,
        )
        self.dut.ffx.run(cmd=["driver", "register", self.driver_url])

        await driver_stress_lib.verify_driver_loaded(
            dut=self.dut, driver_url=self.driver_url
        )

        for path in self.devfs_paths:
            await driver_stress_lib.assert_devfs_presence(
                dut=self.dut, path=path, expected=True
            )

        driver_stress_lib.audit_driver_crashes(
            dut=self.dut, moniker=self.moniker, start_time=start_time
        )

        _LOGGER.info(
            "Successfully finished driver disable/enable stress iteration# %s",
            iteration,
        )

    def _name_func_restart(self, iteration: int) -> str:
        """Generates individual iteration test case names for restart."""
        return f"test_restart_iteration_{iteration}"

    def _name_func_disable_enable(self, iteration: int) -> str:
        """Generates individual iteration test case names for disable/enable."""
        return f"test_disable_enable_iteration_{iteration}"


if __name__ == "__main__":
    test_runner.main()

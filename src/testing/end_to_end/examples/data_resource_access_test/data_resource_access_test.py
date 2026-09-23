# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Lacewing test that accesses user-specified data resources.

Demonstrates accessing custom input data as Python resource.
"""

import json
import logging
import subprocess
from importlib.resources import as_file, files

import fuchsia_base_test
import my_resources
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class DataResourceAccessTest(fuchsia_base_test.FuchsiaBaseTest):
    def test_data_resource_access_as_data(self) -> None:
        """Read data directly from file resource and log it."""
        with files(my_resources).joinpath("test_data.json").open("rb") as f:
            greeting = json.loads(f.read())["greeting"]
            for fuchsia_device in self.fuchsia_devices:
                _LOGGER.info(f"{fuchsia_device.device_name} says {greeting}!")

    def test_data_resource_access_as_file(self) -> None:
        """Pass resource as a filepath to another program."""
        with as_file(files(my_resources).joinpath("test_data.json")) as f:
            out = subprocess.check_output(["cat", f])
            json_obj = json.loads(out)
            asserts.assert_in("greeting", json_obj)


if __name__ == "__main__":
    test_runner.main()

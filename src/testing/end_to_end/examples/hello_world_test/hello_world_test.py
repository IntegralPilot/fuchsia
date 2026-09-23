# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Hello World test for Lacewing."""

import logging

import fuchsia_base_test
from mobly import test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class HelloWorldTest(fuchsia_base_test.FuchsiaBaseTest):
    def test_hello_world(self) -> None:
        for fuchsia_device in self.fuchsia_devices:
            _LOGGER.info("%s says hello!", fuchsia_device.device_name)


if __name__ == "__main__":
    test_runner.main()

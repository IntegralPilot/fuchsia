# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Example test that always fails with DeviceDidNotSuspendError for testing exonerations."""

import fuchsia_base_test
from honeydew.utils import power as honeydew_power
from mobly import test_runner


class ExonerationFailingTest(fuchsia_base_test.FuchsiaBaseTest):
    def test_always_exonerate_fail(self) -> None:
        raise honeydew_power.DeviceDidNotSuspendError(
            "Dummy exception raised to demonstrate test exoneration in tefmocheck. 8d3d922a-8c8c-44bb-bc8c-f09c7a72d733"
        )


if __name__ == "__main__":
    test_runner.main()

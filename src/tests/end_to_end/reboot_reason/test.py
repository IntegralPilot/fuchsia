# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import fuchsia_base_test
import reboot_reason_test_cases
from mobly import test_runner


class RebootReasonTest(fuchsia_base_test.FuchsiaBaseTest):
    TEST_CASES = [reboot_reason_test_cases.RebootReasonTestCases]


if __name__ == "__main__":
    test_runner.main()

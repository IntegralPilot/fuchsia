# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import fuchsia_base_test
import suspend_resume_test_cases
from mobly import test_runner


class SuspendResumeTest(fuchsia_base_test.FuchsiaBaseTest):
    TEST_CASES = [suspend_resume_test_cases.SuspendResumeTestCases]


if __name__ == "__main__":
    test_runner.main()

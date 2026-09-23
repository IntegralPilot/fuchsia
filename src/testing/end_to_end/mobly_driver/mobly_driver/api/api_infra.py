# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Contains all Fuchsia Infra APIs used in Mobly Driver."""

# Defined in https://osscs.corp.google.com/fuchsia/fuchsia/+/main:tools/botanist/constants/constants.go.
BOT_ENV_TESTBED_CONFIG = "FUCHSIA_TESTBED_CONFIG"

# Defined in https://osscs.corp.google.com/fuchsia/fuchsia/+/main:tools/botanist/targets/target.go
FUCHSIA_DEVICE = "FuchsiaDevice"

# Defined as an Auxiliary device in https://osscs.corp.google.com/fuchsia/fuchsia/+/main:tools/botanist/targets/auxiliary.go
ACCESS_POINT = "AccessPoint"
OPENWRT_AP = "OpenWrtAP"

# LINT.IfChange(mobly_test_start)
TESTPARSER_PREAMBLE = "======== Mobly config content ========"
# LINT.ThenChange(
# //src/testing/end_to_end/antlion/runner/src/driver/infra.rs:mobly_test_start,
# //tools/testing/tefmocheck/string_in_log_check.go:mobly_test_start,
# //tools/testing/testparser/testparser.go:mobly_test_start,
# )

# LINT.IfChange(mobly_test_end)
TESTPARSER_RESULT_HEADER = "[=====MOBLY RESULTS=====]"
# LINT.ThenChange(
# //src/testing/end_to_end/antlion/runner/src/driver/infra.rs:mobly_test_end,
# //tools/testing/tefmocheck/string_in_log_check.go:mobly_test_end,
# //tools/testing/testparser/moblytest.go:mobly_test_end,
# )

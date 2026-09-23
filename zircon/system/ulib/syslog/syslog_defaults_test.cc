// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/global.h>

#if FUCHSIA_API_LEVEL_LESS_THAN(29)

#include <zxtest/zxtest.h>

// This tests the default configuration of the legacy syslog library
// It needs to be in its own test package to ensure it executes independently
// of other tests that may manipulate the global state.

TEST(SyslogTests, test_log_severity_invalid) {
  fx_logger_t* logger = fx_log_get_logger();
  EXPECT_EQ(FX_LOG_INFO, fx_logger_get_min_severity(logger));
  EXPECT_STATUS(ZX_ERR_INVALID_ARGS, fx_logger_set_min_severity(logger, FX_LOG_FATAL + 1));
  EXPECT_EQ(FX_LOG_INFO, fx_logger_get_min_severity(logger));
}

#endif  // FUCHSIA_API_LEVEL_LESS_THAN(29)

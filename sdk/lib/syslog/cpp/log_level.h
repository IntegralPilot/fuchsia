// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_SYSLOG_CPP_LOG_LEVEL_H_
#define LIB_SYSLOG_CPP_LOG_LEVEL_H_

#include <cstdint>

namespace fuchsia_logging {

using RawLogSeverity = uint8_t;

enum LogSeverity : RawLogSeverity {
  Trace = 0x10,
  Debug = 0x20,
  Info = 0x30,
  Warn = 0x40,
  Error = 0x50,
  Fatal = 0x60,
};

#define FUCHSIA_LOG_SEVERITY_STEP_SIZE ((uint8_t)0x10)

constexpr LogSeverity kDefaultLogLevel = LogSeverity::Info;

// Assert that log levels are in ascending order.
// Numeric comparison is generally used to determine whether to log.
static_assert(LogSeverity::Trace < LogSeverity::Debug, "");
static_assert(LogSeverity::Debug < LogSeverity::Info, "");
static_assert(LogSeverity::Info < LogSeverity::Warn, "");
static_assert(LogSeverity::Warn < LogSeverity::Error, "");
static_assert(LogSeverity::Error < LogSeverity::Fatal, "");

}  // namespace fuchsia_logging

#endif  // LIB_SYSLOG_CPP_LOG_LEVEL_H_

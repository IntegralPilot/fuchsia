// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// This file provides logging wrappers and vDSO-next management helpers.
#ifndef SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_INTERNAL_COMMON_H_
#define SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_INTERNAL_COMMON_H_

#include <ios>  // for std::hex

#include <bringup/lib/restricted-machine/testing/needs-next.h>

// If defined, all restricted_machine code will log to the zxtest
// LogSink otherwise it uses syslog.
#if !defined(RESTRICTED_MACHINE_LOG_TO_ZXTEST)

#include <lib/syslog/cpp/macros.h>
// Log helper to make it easy to change logging tags.
#define RM_LOG(level) FX_LOGS(level)
#define RM_DLOG(level) FX_DLOGS(level)

#else

// We provide a very crude streaming logger that emits to the zxtest sink.
#include <sstream>

#include <zxtest/zxtest.h>
namespace {

static inline auto LogSink() {
  return zxtest::Runner::GetInstance()->mutable_reporter()->mutable_log_sink();
}

enum RmLogLevel {
  kRmLogLevelVoid = -1,
  kRmLogLevelDEBUG = 0,
  kRmLogLevelINFO = 1,
  kRmLogLevelWARNING = 2,
  kRmLogLevelERROR = 3,
  kRmLogLevelFATAL = 4,
};

class TestLogMessage {
 public:
  TestLogMessage(int severity, const char *severity_str, const char *file, int line_num,
                 const char *func)
      : severity_(severity),
        severity_str_(severity_str),
        file_(file),
        line_num_(line_num),
        func_(func) {}
  ~TestLogMessage() {
    // -1 reserved for NDEBUG.
    if (severity_ != kRmLogLevelVoid) {
      LogSink()->Write("[%s] %s:%d:%s(): %.*s\n", severity_str_, file_, line_num_, func_,
                       static_cast<int>(stream_.str().size()), stream_.str().c_str());
    }
    assert(severity_ != kRmLogLevelFATAL);
  }
  std::ostream &stream() { return stream_; }

 private:
  int severity_ = 0;
  const char *severity_str_{nullptr};
  const char *file_{nullptr};
  int line_num_ = 0;
  const char *func_{nullptr};
  std::ostringstream stream_{};
};

}  // namespace

#define RM_LOG(level) \
  TestLogMessage(kRmLogLevel##level, #level, __FILE__, __LINE__, __func__).stream()
#ifdef NDEBUG
#define RM_DLOG(level) \
  TestLogMessage(kRmLogLevelVoid, #level, __FILE__, __LINE__, __func__).stream()
#else
#define RM_DLOG(level) \
  TestLogMessage(kRmLogLevel##level, #level, __FILE__, __LINE__, __func__).stream()
#endif

#endif  // defined(RESTRICTED_MACHINE_LOG_TO_ZXTEST)

#endif  // SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_INTERNAL_COMMON_H_

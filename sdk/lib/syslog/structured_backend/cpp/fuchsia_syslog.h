// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_SYSLOG_STRUCTURED_BACKEND_CPP_FUCHSIA_SYSLOG_H_
#define LIB_SYSLOG_STRUCTURED_BACKEND_CPP_FUCHSIA_SYSLOG_H_

#include <zircon/availability.h>

#if FUCHSIA_API_LEVEL_LESS_THAN(29)

#include <lib/syslog/structured_backend/cpp/log_buffer.h>
#include <lib/zx/clock.h>

#include <cstdint>

namespace fuchsia_syslog {

using fuchsia_logging::FlushConfig;
using fuchsia_logging::LogBuffer;

}  // namespace fuchsia_syslog

#endif

#endif  // LIB_SYSLOG_STRUCTURED_BACKEND_CPP_FUCHSIA_SYSLOG_H_

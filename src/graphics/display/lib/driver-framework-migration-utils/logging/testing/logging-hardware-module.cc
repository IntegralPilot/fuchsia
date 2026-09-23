// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/driver-framework-migration-utils/logging/testing/logging-hardware-module.h"

#include "src/graphics/display/lib/driver-framework-migration-utils/logging/zxlogf.h"

namespace display::testing {

bool LoggingHardwareModule::LogTrace() const {
  if (zxlog_level_enabled(TRACE)) {
    zxlogf(TRACE, "trace");
    return true;
  }
  return false;
}

bool LoggingHardwareModule::LogDebug() const {
  if (zxlog_level_enabled(DEBUG)) {
    zxlogf(DEBUG, "debug");
    return true;
  }
  return false;
}

bool LoggingHardwareModule::LogInfo() const {
  if (zxlog_level_enabled(INFO)) {
    zxlogf(INFO, "info");
    return true;
  }
  return false;
}

bool LoggingHardwareModule::LogWarning() const {
  if (zxlog_level_enabled(WARNING)) {
    zxlogf(WARNING, "warning");
    return true;
  }
  return false;
}

bool LoggingHardwareModule::LogError() const {
  if (zxlog_level_enabled(ERROR)) {
    zxlogf(ERROR, "error");
    return true;
  }
  return false;
}

}  // namespace display::testing

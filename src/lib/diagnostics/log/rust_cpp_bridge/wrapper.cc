// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdk/lib/syslog/cpp/log_settings.h"

extern "C" {
void init_cpp_logging(uint8_t min_severity) {
  fuchsia_logging::LogSettingsBuilder settings;
  settings.WithMinLogSeverity(min_severity);
  settings.BuildAndInitialize();
}
}

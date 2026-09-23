// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_SYSLOG_CPP_LOG_SETTINGS_INTERNAL_H_
#define LIB_SYSLOG_CPP_LOG_SETTINGS_INTERNAL_H_

#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/logging_backend_fuchsia_globals.h>
#include <lib/syslog/structured_backend/cpp/raw_log_settings.h>

namespace fuchsia_logging::internal {

template <typename T>
auto WithRawSettings(const fuchsia_logging::LogSettings& settings, T callback) {
  std::vector<const char*> tags;
  tags.reserve(settings.tags.size());
  for (const std::string& tag : settings.tags) {
    tags.push_back(tag.c_str());
  }

  RawLogSettings raw_settings{
      .min_log_level = settings.min_log_level,
      .log_sink = settings.log_sink,
      .tags = tags.data(),
      .tags_count = settings.tags.size(),
      .dispatcher = settings.single_threaded_dispatcher,
      .severity_change_callback = settings.severity_change_callback,
      .severity_change_callback_context = settings.severity_change_callback_context,
  };
  return callback(raw_settings);
}

}  // namespace fuchsia_logging::internal

#endif  // LIB_SYSLOG_CPP_LOG_SETTINGS_INTERNAL_H_

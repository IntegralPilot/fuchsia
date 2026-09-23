// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_SYSLOG_CPP_LOG_SETTINGS_H_
#define LIB_SYSLOG_CPP_LOG_SETTINGS_H_

#include <lib/syslog/cpp/log_level.h>

#include <type_traits>
#include <vector>
#ifdef __Fuchsia__
#include <lib/async/dispatcher.h>
#include <lib/zx/channel.h>
#endif
#include <zircon/availability.h>

#include <string>

namespace fuchsia_logging {

/// Interest listener configuration
enum InterestListenerBehavior : uint8_t {
  /// Disable interest listening completely
  Disabled = 0,
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  /// Enable the interest listener, but don't wait for an initial interest.
  EnabledNonBlocking = 1,
#endif
  /// Enable the interest listener, and block logging startup on an initial interest
  Enabled = 2,
};

/// Settings which control the behavior of logging.
struct LogSettings {
  /// The minimum logging level.
  /// Anything at or above this level will be logged (if applicable).
  /// Anything below this level will be silently ignored.
  ///
  /// The log level defaults to LOG_INFO.
  ///
  /// Log messages for FX_VLOGS(x) (from macros.h) log verbosities in
  /// the range between INFO and DEBUG
  fuchsia_logging::RawLogSeverity min_log_level = fuchsia_logging::kDefaultLogLevel;
  std::vector<std::string> tags;
#ifndef __Fuchsia__
  /// The name of a file to which the log should be written.
  /// When non-empty, the previous log output is closed and logging is
  /// redirected to the specified file.  It is not possible to revert to
  /// the previous log output through this interface.
  std::string log_file;
#else
  /// A single-threaded dispatcher to use for change notifications.
  /// Must be single-threaded. Passing a dispatcher that has multiple threads
  /// will result in undefined behavior.
  /// This must be an async_dispatcher_t*.
  /// This can't be defined as async_dispatcher_t* since it is used
  /// from fxl which is a host+target library. This prevents us
  /// from adding a Fuchsia-specific dependency.
  async_dispatcher_t* single_threaded_dispatcher = nullptr;
  // Allows to define the LogSink handle to use. When no handle is provided, the default LogSink
  // in the pogram incoming namespace will be used.
  zx_handle_t log_sink = ZX_HANDLE_INVALID;
  /// Interest listener callback, or nullptr
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  void (*severity_change_callback)(fuchsia_logging::RawLogSeverity severity) = nullptr;
#else
  void (*severity_change_callback)(void*, fuchsia_logging::RawLogSeverity severity) = nullptr;
  void* severity_change_callback_context = nullptr;
#endif
  fuchsia_logging::InterestListenerBehavior interest_listener_config_ = fuchsia_logging::Enabled;
#endif
};
static_assert(std::is_copy_constructible<LogSettings>::value);

/// Builder class that allows building log settings,
/// then setting them.
class LogSettingsBuilder {
 public:
  /// Sets the default log severity. If not explicitly set,
  /// this defaults to INFO, or to the value specified by Archivist.
  LogSettingsBuilder& WithMinLogSeverity(fuchsia_logging::RawLogSeverity min_log_level);

  /// Sets the tags that are implicitly logged with every message.
  LogSettingsBuilder& WithTags(const std::initializer_list<std::string>& tags);

#ifndef __Fuchsia__
  /// Sets the log file.
  LogSettingsBuilder& WithLogFile(const std::string_view& log_file);
#else
  /// Sets the log sink handle.
  LogSettingsBuilder& WithLogSink(zx_handle_t log_sink);

  /// Sets the dispatcher to use.
  LogSettingsBuilder& WithDispatcher(async_dispatcher_t* dispatcher);

  /// Disables the interest listener.
  LogSettingsBuilder& DisableInterestListener();

#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  /// Disables waiting for the initial interest from Archivist.
  /// The level specified in SetMinLogSeverity or INFO will be used
  /// as the default.
  LogSettingsBuilder& DisableWaitForInitialInterest();
#endif

  /// Configures the interest listener settings.
  LogSettingsBuilder& WithInterestListenerConfiguration(InterestListenerBehavior config);

  /// Sets a callback that is invoked when the severity changes.
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  LogSettingsBuilder& WithSeverityChangedListener(
      void (*callback)(fuchsia_logging::RawLogSeverity severity));
#else
  LogSettingsBuilder& WithSeverityChangedListener(
      void* context, void (*callback)(void* context, fuchsia_logging::RawLogSeverity severity));
#endif
#endif

  /// Configures the log settings
  /// and initializes (or re-initializes) the LogSink connection.
  void BuildAndInitialize();

 private:
  LogSettings settings_;
};

// Moved to log_message_impl.h.
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
/// Gets the minimum log severity for the current process. Never returns a value
/// higher than LOG_FATAL.
fuchsia_logging::RawLogSeverity GetMinLogSeverity();
#endif

}  // namespace fuchsia_logging

#endif  // LIB_SYSLOG_CPP_LOG_SETTINGS_H_

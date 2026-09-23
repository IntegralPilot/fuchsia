// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_FXL_LOG_SETTINGS_COMMAND_LINE_H_
#define SRC_LIB_FXL_LOG_SETTINGS_COMMAND_LINE_H_

#include <lib/syslog/cpp/log_level.h>
#include <lib/syslog/cpp/log_settings.h>

#include <string>
#include <vector>

#include "src/lib/fxl/fxl_export.h"

namespace fxl {

class CommandLine;

struct LogSettings {
  // The minimum logging level.
  // Anything at or above this level will be logged (if applicable).
  // Anything below this level will be silently ignored.
  //
  // The log level defaults to LOG_INFO.
  //
  // Log messages for FX_VLOGS(x) (from macros.h) log verbosities in
  // the range between INFO and DEBUG
  fuchsia_logging::RawLogSeverity min_log_level = fuchsia_logging::kDefaultLogLevel;
#ifndef __Fuchsia__
  // The name of a file to which the log should be written.
  // When non-empty, the previous log output is closed and logging is
  // redirected to the specified file.  It is not possible to revert to
  // the previous log output through this interface.
  std::string log_file;
#endif
  // Set to true to disable the interest listener. Changes to interest will not be
  // applied to your log settings.
  bool disable_interest_listener = false;
#ifdef __Fuchsia__
  // A single-threaded dispatcher to use for change notifications.
  // Must be single-threaded. Passing a dispatcher that has multiple threads
  // will result in undefined behavior.
  // This must be an async_dispatcher_t*.
  // This can't be defined as async_dispatcher_t* since it is used
  // from fxl which is a host+target library. This prevents us
  // from adding a Fuchsia-specific dependency.
  async_dispatcher_t* single_threaded_dispatcher = nullptr;
  // Allows to define the LogSink handle to use. When no handle is provided, the default LogSink
  // in the pogram incoming namespace will be used.
  zx_handle_t log_sink = ZX_HANDLE_INVALID;
#endif
};
static_assert(std::is_copy_constructible<LogSettings>::value);

// Parses log settings from standard command-line options.
//
// Recognizes the following options:
//   --severity=<TRACE|DEBUG|INFO|WARNING|ERROR|FATAL> : sets |min_log_level| to indicated severity
//   --verbose         : sets |min_log_level| to (LOG_INFO - 1)
//   --verbose=<level> : sets |min_log_level| incrementally lower than INFO
//   --quiet           : sets |min_log_level| to LOG_WARNING
//   --quiet=<level>   : sets |min_log_level| incrementally higher than INFO
//   --log-file=<file> : sets |log_file| to file, uses default output if empty
//
// Quiet supersedes verbose if both are specified.
//
// Returns false and leaves |out_settings| unchanged if there was an
// error parsing the options.  Otherwise updates |out_settings| with any
// values which were overridden by the command-line.
bool ParseLogSettings(const fxl::CommandLine& command_line, fxl::LogSettings* out_settings);

// Parses and applies log settings from standard command-line options.
// Returns false and leaves the active settings unchanged if there was an
// error parsing the options.
//
// See |ParseLogSettings| for syntax.
#ifdef __Fuchsia__
bool SetLogSettingsFromCommandLine(const fxl::CommandLine& command_line,
                                   async_dispatcher_t* dispatcher = nullptr);
// Similar to the method above but uses the given list of tags instead of
// the default which is the process name. On host |tags| is ignored.
bool SetLogSettingsFromCommandLine(const fxl::CommandLine& command_line,
                                   const std::initializer_list<std::string>& tags,
                                   async_dispatcher_t* dispatcher = nullptr);
#else
bool SetLogSettingsFromCommandLine(const fxl::CommandLine& command_line);
bool SetLogSettingsFromCommandLine(const fxl::CommandLine& command_line,
                                   const std::initializer_list<std::string>& tags);
#endif

// Do the opposite of |ParseLogSettings()|: Convert |settings| to the
// command line arguments to pass to a program. The result is empty if
// |settings| is the default.
std::vector<std::string> LogSettingsToArgv(const fxl::LogSettings& settings);

}  // namespace fxl

#endif  // SRC_LIB_FXL_LOG_SETTINGS_COMMAND_LINE_H_

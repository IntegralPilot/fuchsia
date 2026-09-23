// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYSMEM_SERVER_LOGGING_H_
#define SRC_SYSMEM_SERVER_LOGGING_H_

#include <lib/fit/function.h>
#include <lib/syslog/cpp/macros.h>
#include <stdarg.h>
#include <zircon/compiler.h>

#include <string>

namespace sysmem_service {

using LogCallback = fit::function<void(::fuchsia_logging::LogSeverity severity, const char* file,
                                       int line, const char* formatted_str)>;
LogCallback& GetDefaultLogCallback();

void vLogToCallback(::fuchsia_logging::LogSeverity severity, const char* file, int line,
                    const char* prefix, const char* format, va_list args,
                    const LogCallback& log_callback);

void vLog(::fuchsia_logging::LogSeverity severity, const char* file, int line, const char* prefix,
          const char* format, va_list args);

void Log(::fuchsia_logging::LogSeverity severity, const char* file, int line, const char* prefix,
         const char* format, ...) __PRINTFLIKE(5, 6);

// Creates a unique name by concatenating prefix and a 64-bit unique number.
std::string CreateUniqueName(const char* prefix);

// Represents a source code location. Use FROM_HERE to get the current file location.
class Location {
 public:
  static Location FromHere(const char* file, int line_number) {
    return Location(file, line_number);
  }
  Location(const char* file, int line_number) : file_(file), line_(line_number) {}

  const char* file() const { return file_; }
  int line() const { return line_; }

 private:
  const char* file_{};
  int line_{};
};

#define FROM_HERE Location::FromHere(__FILE__, __LINE__)

class LoggingMixin {
 protected:
  explicit LoggingMixin(const char* logging_prefix) : logging_prefix_(logging_prefix) {}
  void LogInfo(Location location, const char* format, ...) __PRINTFLIKE(3, 4) {
    va_list args;
    va_start(args, format);
    vLog(fuchsia_logging::LogSeverity::Info, location.file(), location.line(), logging_prefix_,
         format, args);
    va_end(args);
  }
  void LogError(Location location, const char* format, ...) __PRINTFLIKE(3, 4) {
    va_list args;
    va_start(args, format);
    vLog(fuchsia_logging::LogSeverity::Info, location.file(), location.line(), logging_prefix_,
         format, args);
    va_end(args);
  }

  const char* logging_prefix() const { return logging_prefix_; }

 private:
  const char* logging_prefix_;
};

// This is used for functions where they'd spam the log in a unit test. This doesn't accept varargs,
// but the function doing the complaining can format locally as needed.
//
// FROM_HERE, severity, c string to log or mute or count or similar
using ComplainFunction = fit::function<void(Location, fuchsia_logging::LogSeverity, const char*)>;

}  // namespace sysmem_service

#endif  // SRC_SYSMEM_SERVER_LOGGING_H_

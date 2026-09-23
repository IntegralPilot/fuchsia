// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Code for listening to logger service and dumping the logs.
// This implements LogListener interface for logger fidl @ //sdk/fidl/fuchsia.logger

#ifndef RUNTESTS_UTILS_LOG_EXPORTER_H_
#define RUNTESTS_UTILS_LOG_EXPORTER_H_

#include <fidl/fuchsia.logger/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/wait.h>
#include <lib/fit/function.h>
#include <stdint.h>

#include <memory>
#include <string_view>
#include <utility>

#include <fbl/string.h>
#include <fbl/vector.h>

namespace runtests {

// Error while launching LogExporter.
enum class ExporterLaunchError {
  OPEN_FILE,
  CREATE_CHANNEL,
  FIDL_ERROR,
  CONNECT_TO_LOGGER_SERVICE,
  NO_ERROR,
};

// Emits log messages to file.
class LogExporter : public fidl::WireServer<fuchsia_logger::LogListenerSafe> {
 public:
  using ErrorHandler = fit::function<void(zx_status_t)>;
  using FileErrorHandler = fit::function<void(const char* error)>;

  // Creates object and starts listening for msgs on channel written by Log
  // interface in logger FIDL.
  //
  // |channel| channel to read log messages from.
  // |output_file| file to write logs to.
  //
  LogExporter(FILE* output_file, async_dispatcher_t* dispatcher,
              fidl::ServerEnd<fuchsia_logger::LogListenerSafe> channel, ErrorHandler error_handler,
              FileErrorHandler file_error_handler);
  ~LogExporter() override;

 private:
  // Keeps track of the count of dropped logs for a process.
  struct DroppedLogs {
    uint64_t pid;
    uint32_t dropped_logs;
  };

  void Log(LogRequestView request, LogCompleter::Sync& completer) override;
  void LogMany(LogManyRequestView request, LogManyCompleter::Sync& completer) override;
  void Done(DoneCompleter::Sync& completer) override;

  // Helper method to log |message| to file.
  ssize_t LogMessage(fuchsia_logger::wire::LogMessage message);

  // Helper method to call |file_error_handler_|.
  void NotifyFileError(const char* error);

  // Helper method to write severity string.
  int WriteSeverity(int32_t severity);

  const FileErrorHandler file_error_handler_;
  FILE* output_file_;
  fidl::ServerBinding<fuchsia_logger::LogListenerSafe> binding_;

  // Vector to keep track of dropped logs per pid
  fbl::Vector<DroppedLogs> dropped_logs_;
};

// Launches Log Exporter.
//
// |dispatcher| the dispatcher on which to run the listener.
// |syslog_path| file path where to write logs.
// |error| error to set in case of failure.
//
// Returns nullptr if it is not possible to launch Log Exporter.
std::unique_ptr<LogExporter> LaunchLogExporter(async_dispatcher_t* dispatcher,
                                               std::string_view syslog_path,
                                               ExporterLaunchError* error);

}  // namespace runtests

#endif  // RUNTESTS_UTILS_LOG_EXPORTER_H_
